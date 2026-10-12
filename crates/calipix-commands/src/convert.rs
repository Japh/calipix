//! Pixel type conversions (Image > Type).
//!
//! Converting replaces the whole image, every plane of it, and can be
//! undone. The selection is kept, as are calibration and lookup tables
//! unless the conversion makes them meaningless.

use calipix_command::{
    CommandCtx, CommandSpec, DatasetId, Effects, Error, ImageEffect, Outputs, Params, Registry,
    Result, typed,
};
use calipix_core::{Axis, AxisKind, Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta};
use calipix_ops::convert::{self, UNWEIGHTED, WEIGHTED};
use calipix_render::{ChannelMode, LevelMap};

use crate::view::{MODE_PROPERTY, channel_mode, plane_position};

/// The plane on view, whose data range stands in for a display range that
/// has not been set. Interfaces fill these in from the view.
#[derive(Params, Clone, Copy)]
struct ViewPlane {
    /// Slice on view, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "slice")]
    slice: i64,
    /// Frame on view, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "frame")]
    frame: i64,
}

#[derive(Params)]
struct ScaledParams {
    /// The image to convert.
    #[param(label = "Image")]
    input: DatasetId,
    /// Stretch the display range over the whole range of the new type.
    /// Without it values are kept, and those the new type cannot hold are
    /// limited to its range.
    #[param(label = "Scale when converting", default = true)]
    scale: bool,
    /// Turn colour into grey with the weights 0.299, 0.587 and 0.114 for
    /// red, green and blue instead of averaging them.
    #[param(label = "Weighted RGB conversion", default = false)]
    weighted: bool,
    #[param(flatten)]
    view: ViewPlane,
}

#[derive(Params)]
struct FloatParams {
    /// The image to convert.
    #[param(label = "Image")]
    input: DatasetId,
    /// Turn colour into grey with the weights 0.299, 0.587 and 0.114 for
    /// red, green and blue instead of averaging them.
    #[param(label = "Weighted RGB conversion", default = false)]
    weighted: bool,
}

#[derive(Params)]
struct RgbParams {
    /// The image to convert.
    #[param(label = "Image")]
    input: DatasetId,
    #[param(flatten)]
    view: ViewPlane,
}

#[derive(Params)]
struct TypeParams {
    /// The image to convert.
    #[param(label = "Image")]
    input: DatasetId,
    /// The pixel type to convert to: `u` and `i` are unsigned and signed
    /// integers and `f` floating point, followed by the number of bits.
    #[param(
        label = "Pixel type",
        choices("u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "f32", "f64"),
        ijm = "type"
    )]
    pixel_type: String,
}

fn spec(id: &str, title: &str, weight: f64, summary: &str) -> CommandSpec {
    CommandSpec::new(id, title)
        .summary(summary)
        .menu("Image > Type", weight)
        .effects(Effects {
            image: ImageEffect::Modifies,
            ..Effects::default()
        })
}

fn no_complex(meta: &ImageMeta) -> Result<()> {
    if meta.dtype().is_complex() {
        return Err(Error::failed(
            "complex images cannot be converted to another pixel type",
        ));
    }
    Ok(())
}

/// Metadata like `meta` for another pixel type and other axes. The colour
/// model and channel descriptions are reset; the caller sets them.
pub(crate) fn rebuilt(meta: &ImageMeta, dtype: DType, axes: Vec<(Axis, usize)>) -> Result<ImageMeta> {
    let mut out = ImageMeta::new(dtype, axes)?.with_name(meta.name.clone());
    out.properties = meta.properties.clone();
    Ok(out)
}

/// The axes of `meta` with their sizes.
pub(crate) fn axes_of(meta: &ImageMeta) -> Vec<(Axis, usize)> {
    meta.axes()
        .iter()
        .cloned()
        .zip(meta.shape().iter().copied())
        .collect()
}

/// The plane numbers of the red, green and blue planes of each picture of
/// an RGB image, pictures in the order of the image's other axes.
pub(crate) fn rgb_pictures(meta: &ImageMeta) -> Result<Vec<[usize; 3]>> {
    let axis = meta
        .axis_index(&AxisKind::Channel)
        .filter(|&axis| meta.shape()[axis] == 3)
        .ok_or_else(|| Error::failed("an RGB image needs three channels"))?;
    let mut pictures = Vec::with_capacity(meta.plane_count() / 3);
    for plane in 0..meta.plane_count() {
        let mut position = meta.plane_position(plane)?;
        if position[axis - 2] != 0 {
            continue;
        }
        let mut numbers = [plane; 3];
        for (channel, number) in numbers.iter_mut().enumerate() {
            position[axis - 2] = channel;
            *number = meta.plane_index(&position)?;
        }
        pictures.push(numbers);
    }
    Ok(pictures)
}

/// The axes of `meta` without its channel axis.
fn without_channels(meta: &ImageMeta) -> Vec<(Axis, usize)> {
    axes_of(meta)
        .into_iter()
        .filter(|(axis, _)| axis.kind != AxisKind::Channel)
        .collect()
}

/// The display range each channel is converted through: its own, or else
/// the data range of its plane on view, which is what ImageJ's display
/// range is when none has been set.
fn channel_ranges(image: &Image, view: ViewPlane) -> Result<Vec<(f64, f64)>> {
    let meta = image.meta();
    (0..meta.size(&AxisKind::Channel))
        .map(|channel| {
            let position = plane_position(meta, channel, view.slice, view.frame)?;
            let plane = meta.plane_index(&position)?;
            Ok(calipix_render::effective_display_range(image, plane)?)
        })
        .collect()
}

/// Every plane of `image` passed through `convert`, with progress and
/// cancellation.
fn each_plane(
    ctx: &CommandCtx<'_>,
    image: &Image,
    convert: impl Fn(usize, &Buffer) -> Result<Buffer>,
) -> Result<Vec<Buffer>> {
    let count = image.meta().plane_count();
    (0..count)
        .map(|plane| {
            ctx.check_cancelled()?;
            ctx.progress(plane as f64 / count as f64, "Converting");
            convert(plane, &image.read_plane(plane)?)
        })
        .collect()
}

/// An RGB image turned into one grey plane per picture by `gray`.
fn rgb_to_gray(
    ctx: &CommandCtx<'_>,
    image: &Image,
    gray: impl Fn([&Buffer; 3]) -> calipix_core::Result<Buffer>,
) -> Result<Image> {
    let meta = image.meta();
    let pictures = rgb_pictures(meta)?;
    let mut planes = Vec::with_capacity(pictures.len());
    for (step, numbers) in pictures.iter().enumerate() {
        ctx.check_cancelled()?;
        ctx.progress(step as f64 / pictures.len() as f64, "Converting");
        let [r, g, b] = [
            image.read_plane(numbers[0])?,
            image.read_plane(numbers[1])?,
            image.read_plane(numbers[2])?,
        ];
        planes.push(gray([&r, &g, &b])?);
    }
    let out = rebuilt(meta, planes[0].dtype(), without_channels(meta))?;
    Ok(Image::from_planes(out, planes)?)
}

/// `image` with other planes of another pixel type under the same axes,
/// and `edit` applied to each channel's description.
fn retyped(
    image: &Image,
    planes: Vec<Buffer>,
    edit: impl Fn(&mut ChannelMeta),
) -> Result<Image> {
    let mut meta = image.meta().with_dtype(planes[0].dtype());
    if meta.channels.is_empty() {
        meta.channels = vec![ChannelMeta::default(); meta.size(&AxisKind::Channel)];
    }
    for entry in &mut meta.channels {
        edit(entry);
    }
    Ok(Image::from_planes(meta, planes)?)
}

fn weights(weighted: bool) -> [f64; 3] {
    if weighted { WEIGHTED } else { UNWEIGHTED }
}

fn is_gray_table(lut: &calipix_core::Lut) -> bool {
    let table = &lut.table;
    (0..256).all(|i| table[i] == [i as u8; 3]) || (0..256).all(|i| table[i] == [255 - i as u8; 3])
}

/// Image > Type > 8-bit (`ImageConverter.convertToGray8`,
/// `StackConverter.convertToGray8`).
fn to_8bit(ctx: &mut CommandCtx<'_>, p: ScaledParams) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta();
    no_complex(meta)?;
    let converted = if meta.color == ColorModel::Rgb {
        rgb_to_gray(ctx, &image, |rgb| {
            convert::rgb_to_gray_u8(rgb, weights(p.weighted))
        })?
    } else if meta.dtype() == DType::U8 {
        // Already 8-bit: a colour lookup table is replaced by grey, as for
        // ImageJ's pseudocolour images. The samples stay as they are.
        if !meta
            .channels
            .iter()
            .any(|c| c.lut.as_ref().is_some_and(|lut| !is_gray_table(lut)))
        {
            return Ok(Outputs::new());
        }
        let mut meta = meta.clone();
        for entry in &mut meta.channels {
            if entry.lut.as_ref().is_some_and(|lut| !is_gray_table(lut)) {
                entry.lut = None;
            }
        }
        image.with_meta(meta)?
    } else {
        let planes = if p.scale {
            let ranges = channel_ranges(&image, p.view)?;
            each_plane(ctx, &image, |plane, samples| {
                let range = ranges[meta.plane_channel(plane)?];
                let levels = LevelMap::new(samples.dtype(), false, range)?.levels(samples)?;
                Ok(Buffer::from_vec(levels))
            })?
        } else {
            each_plane(ctx, &image, |_, samples| {
                Ok(convert::convert(samples, DType::U8)?)
            })?
        };
        retyped(&image, planes, |channel| channel.display_range = None)?
    };
    ctx.workspace_mut().set_image(p.input, converted)?;
    Ok(Outputs::new())
}

/// Image > Type > 16-bit (`ImageConverter.convertToGray16`,
/// `StackConverter.convertToGray16`).
fn to_16bit(ctx: &mut CommandCtx<'_>, p: ScaledParams) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta();
    no_complex(meta)?;
    if meta.dtype() == DType::U16 {
        return Ok(Outputs::new());
    }
    let converted = if meta.color == ColorModel::Rgb {
        rgb_to_gray(ctx, &image, |rgb| {
            let grey = convert::rgb_to_gray_u8(rgb, weights(p.weighted))?;
            convert::convert(&grey, DType::U16)
        })?
    } else if meta.dtype().is_float() && p.scale {
        let ranges = channel_ranges(&image, p.view)?;
        let planes = each_plane(ctx, &image, |plane, samples| {
            let range = ranges[meta.plane_channel(plane)?];
            Ok(convert::float_to_u16_scaled(samples, range)?)
        })?;
        // ImageJ shows a scaled stack over the full 16-bit range, and a
        // single image over its data range.
        let range = (meta.plane_count() > 1).then_some((0.0, 65535.0));
        retyped(&image, planes, |channel| channel.display_range = range)?
    } else {
        let planes = each_plane(ctx, &image, |_, samples| {
            Ok(convert::convert(samples, DType::U16)?)
        })?;
        retyped(&image, planes, |channel| channel.display_range = None)?
    };
    ctx.workspace_mut().set_image(p.input, converted)?;
    Ok(Outputs::new())
}

/// Image > Type > 32-bit (`ImageConverter.convertToGray32`,
/// `StackConverter.convertToGray32`).
fn to_32bit(ctx: &mut CommandCtx<'_>, p: FloatParams) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta();
    no_complex(meta)?;
    if meta.dtype() == DType::F32 {
        return Ok(Outputs::new());
    }
    let converted = if meta.color == ColorModel::Rgb {
        let grey = rgb_to_gray(ctx, &image, |rgb| {
            convert::rgb_to_gray_f32(rgb, weights(p.weighted))
        })?;
        // The display range of the RGB image carries over.
        let planes = (0..grey.meta().plane_count())
            .map(|plane| grey.read_plane(plane))
            .collect::<calipix_core::Result<Vec<_>>>()?;
        retyped(&grey, planes, |channel| {
            channel.display_range = Some((0.0, 255.0))
        })?
    } else {
        let byte = meta.dtype() == DType::U8;
        let planes = each_plane(ctx, &image, |_, samples| {
            Ok(convert::convert(samples, DType::F32)?)
        })?;
        // The image keeps the display range it had. For 8-bit data that is
        // 0 to 255 when none was set.
        retyped(&image, planes, |channel| {
            if byte && channel.display_range.is_none() {
                channel.display_range = Some((0.0, 255.0));
            }
        })?
    };
    ctx.workspace_mut().set_image(p.input, converted)?;
    Ok(Outputs::new())
}

const RGB_NAMES: [&str; 3] = ["Red", "Green", "Blue"];

/// True for what Image > Type > RGB Stack makes of an RGB image: three
/// 8-bit channels named Red, Green and Blue.
fn is_rgb_stack(meta: &ImageMeta) -> bool {
    meta.dtype() == DType::U8
        && meta.size(&AxisKind::Channel) == 3
        && meta.channels.len() == 3
        && meta
            .channels
            .iter()
            .zip(RGB_NAMES)
            .all(|(channel, name)| channel.name.as_deref() == Some(name) && channel.lut.is_none())
}

/// Image > Type > RGB Color (`ImageConverter.convertToRGB`,
/// `convertRGBStackToRGB`, `StackConverter.convertToRGB`,
/// `RGBStackConverter`): what the image looks like becomes its samples.
fn to_rgb(ctx: &mut CommandCtx<'_>, p: RgbParams) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta();
    no_complex(meta)?;
    if meta.color == ColorModel::Rgb {
        return Ok(Outputs::new());
    }
    let channels = meta.size(&AxisKind::Channel);
    let converted = if is_rgb_stack(meta) && meta.color == ColorModel::Grayscale {
        // The red, green and blue planes are already there.
        let mut out = meta.clone();
        out.color = ColorModel::Rgb;
        out.channels = Vec::new();
        out.properties.remove(MODE_PROPERTY);
        image.with_meta(out)?
    } else {
        if channels > 1 && channel_mode(meta) != ChannelMode::Composite {
            return Err(Error::failed(format!(
                "the {channels} channels of this image are shown one at a time, so there is \
                 no single colour picture to convert; change its channel mode to composite \
                 first (View > Set Channel Mode)"
            )));
        }
        // Fix the display ranges that are not set, so that every plane is
        // rendered with the range of the plane on view, as ImageJ does.
        let ranges = channel_ranges(&image, p.view)?;
        let mut fixed = meta.clone();
        if fixed.channels.is_empty() {
            fixed.channels = vec![ChannelMeta::default(); channels];
        }
        for (entry, range) in fixed.channels.iter_mut().zip(ranges) {
            entry.display_range = Some(range);
        }
        let shown = image.with_meta(fixed)?;

        // One picture for each position along the axes other than channel.
        let positions: Vec<Vec<usize>> = (0..meta.plane_count())
            .filter(|&plane| meta.plane_channel(plane).is_ok_and(|c| c == 0))
            .map(|plane| meta.plane_position(plane))
            .collect::<calipix_core::Result<_>>()?;
        let mut planes = Vec::with_capacity(positions.len() * 3);
        for (step, position) in positions.iter().enumerate() {
            ctx.check_cancelled()?;
            ctx.progress(step as f64 / positions.len() as f64, "Converting");
            let picture = calipix_render::render_plane(&shown, position)?;
            for c in 0..3 {
                planes.push(Buffer::from_vec(
                    picture
                        .data
                        .chunks_exact(4)
                        .map(|rgba| rgba[c])
                        .collect::<Vec<u8>>(),
                ));
            }
        }
        let mut axes = axes_of(meta)[..2].to_vec();
        axes.push((Axis::new(AxisKind::Channel), 3));
        axes.extend(without_channels(meta).into_iter().skip(2));
        let mut out = rebuilt(meta, DType::U8, axes)?;
        out.color = ColorModel::Rgb;
        out.properties.remove(MODE_PROPERTY);
        Image::from_planes(out, planes)?
    };
    ctx.workspace_mut().set_image(p.input, converted)?;
    Ok(Outputs::new())
}

/// Image > Type > RGB Stack (`ImageConverter.convertToRGBStack`): the
/// same samples as three grey channels.
fn to_rgb_stack(ctx: &mut CommandCtx<'_>, p: crate::InputOnly) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let mut meta = image.meta().clone();
    if meta.color != ColorModel::Rgb {
        return Err(Error::failed(
            "only an RGB image can be split into a red, green and blue stack",
        ));
    }
    meta.color = ColorModel::Grayscale;
    meta.channels = RGB_NAMES
        .iter()
        .map(|name| ChannelMeta {
            name: Some(name.to_string()),
            ..ChannelMeta::default()
        })
        .collect();
    meta.properties
        .insert(MODE_PROPERTY.to_string(), "grayscale".to_string());
    ctx.workspace_mut()
        .set_image(p.input, image.with_meta(meta)?)?;
    Ok(Outputs::new())
}

/// Converts to any pixel type without rescaling.
fn to_type(ctx: &mut CommandCtx<'_>, p: TypeParams) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta();
    no_complex(meta)?;
    let target = DType::from_name(&p.pixel_type).ok_or_else(|| Error::InvalidArg {
        param: "pixel_type".to_string(),
        reason: format!("`{}` is not a pixel type", p.pixel_type),
    })?;
    if meta.dtype() == target {
        return Ok(Outputs::new());
    }
    if meta.color == ColorModel::Rgb {
        return Err(Error::failed(
            "the channels of an RGB image are 8-bit by definition; turn it into grey \
             (Image > Type > 8-bit) or into three channels (Image > Type > RGB Stack) first",
        ));
    }
    let planes = each_plane(ctx, &image, |_, samples| {
        Ok(convert::convert(samples, target)?)
    })?;
    let converted = retyped(&image, planes, |_| {})?;
    ctx.workspace_mut().set_image(p.input, converted)?;
    Ok(Outputs::new())
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        spec(
            "image.to_8bit",
            "Convert to 8-bit",
            10.0,
            "Converts an image to 8-bit grey, with values from 0 to 255.",
        )
        .doc(
            "With scaling, 16-bit and float images are converted through their display \
             range: what is shown darkest becomes 0 and what is shown brightest 255. A \
             channel without a display range uses the minimum and maximum of its plane on \
             view. Without scaling values are kept and limited to 0 to 255. RGB images \
             become the average of red, green and blue, or their weighted sum.",
        )
        .ijm_name("8-bit"),
        to_8bit,
    ))?;
    registry.register(typed(
        spec(
            "image.to_16bit",
            "Convert to 16-bit",
            20.0,
            "Converts an image to unsigned 16-bit, with values from 0 to 65535.",
        )
        .doc(
            "With scaling, float images are converted through their display range: its \
             minimum becomes 0 and its maximum 65535. Without scaling, and for integer \
             images, values are kept, rounded and limited to 0 to 65535.",
        )
        .ijm_name("16-bit"),
        to_16bit,
    ))?;
    registry.register(typed(
        spec(
            "image.to_32bit",
            "Convert to 32-bit Float",
            30.0,
            "Converts an image to 32-bit floating point, keeping its values.",
        )
        .doc("RGB images become the average of red, green and blue, or their weighted sum.")
        .ijm_name("32-bit"),
        to_32bit,
    ))?;
    registry.register(typed(
        spec(
            "image.to_rgb",
            "Convert to RGB Color",
            40.0,
            "Converts an image to RGB colour as it is displayed.",
        )
        .doc(
            "The display range and lookup table are applied to the samples, and the \
             channels of a composite image are blended into one picture. A stack of 8-bit \
             channels named Red, Green and Blue becomes an RGB image with the same samples.",
        )
        .ijm_name("RGB Color"),
        to_rgb,
    ))?;
    registry.register(typed(
        spec(
            "image.to_rgb_stack",
            "Convert to RGB Stack",
            50.0,
            "Splits an RGB image into red, green and blue channels shown in grey.",
        )
        .doc("The samples are unchanged; Convert to RGB Color puts them back together.")
        .ijm_name("RGB Stack"),
        to_rgb_stack,
    ))?;
    registry.register(typed(
        spec(
            "image.convert_type",
            "Convert Pixel Type",
            60.0,
            "Converts an image to any pixel type, keeping its values.",
        )
        .doc(
            "Values are never rescaled. Those the new type cannot hold take its nearest \
             limit, so negative values become 0 in an unsigned type; fractions are rounded \
             to the nearest whole number; NaN becomes 0.",
        ),
        to_type,
    ))?;
    Ok(())
}
