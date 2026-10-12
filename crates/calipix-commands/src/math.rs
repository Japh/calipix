//! Arithmetic commands (Process > Math) and the image calculator.

use calipix_command::{
    CommandCtx, CommandSpec, DatasetId, Effects, Error, ImageEffect, Outputs, ParamType, Params,
    Registry, Result, typed,
};
use calipix_core::{
    Axis, AxisKind, Buffer, ColorModel, DType, Image, ImageMeta, Pixel, Rect, crop_buffer, dispatch,
};
use calipix_ops::convert::{self, UNWEIGHTED};
use calipix_ops::math::{self, CalcOp, MathOp};
use calipix_render::{ChannelMode, LevelMap};

use crate::process::PlaneScope;

/// A command of Process > Math that takes one number.
macro_rules! value_params {
    ($name:ident, $default:expr, $doc:expr) => {
        #[derive(Params)]
        struct $name {
            #[param(flatten)]
            scope: PlaneScope,
            #[doc = $doc]
            #[param(label = "Value", default = $default, ijm = "value")]
            value: f64,
        }
    };
}

value_params!(AddParams, 25.0, "Number to add to every sample.");
value_params!(
    SubtractParams,
    25.0,
    "Number to subtract from every sample."
);
value_params!(MultiplyParams, 1.25, "Number to multiply every sample by.");
value_params!(
    DivideParams,
    1.25,
    "Number to divide every sample by. Dividing an integer image by zero leaves it unchanged."
);
value_params!(
    MinParams,
    0.0,
    "Lowest value allowed; samples below it are raised to it."
);
value_params!(
    MaxParams,
    255.0,
    "Highest value allowed; samples above it are lowered to it."
);
#[derive(Params)]
struct GammaParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Exponent: below 1 brightens the mid-tones, above 1 darkens them.
    #[param(
        label = "Value",
        default = 0.5,
        min = 0.05,
        max = 5.0,
        step = 0.02,
        ijm = "value"
    )]
    value: f64,
}

value_params!(
    SetParams,
    25.0,
    "Value given to every sample. For an RGB image it is a colour packed as 0xRRGGBB, \
     so values up to 255 set only blue."
);

#[derive(Params)]
struct BitsParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// The other operand, written in binary, for example 11110000.
    #[param(label = "Value (binary)", default = "11110000", ijm = "value")]
    value: String,
}

#[derive(Params)]
struct ScopeOnly {
    #[param(flatten)]
    scope: PlaneScope,
}

#[derive(Params)]
struct NanParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Smallest value to keep.
    #[param(label = "Lower limit", no_persist)]
    lower: f64,
    /// Largest value to keep.
    #[param(label = "Upper limit", no_persist)]
    upper: f64,
}

#[derive(Params)]
struct CalculatorParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// What to do with each pair of samples. Image 1 is the left operand:
    /// Subtract gives image 1 minus image 2.
    #[param(
        label = "Operation",
        default = "Add",
        choices(
            "Add",
            "Subtract",
            "Multiply",
            "Divide",
            "AND",
            "OR",
            "XOR",
            "Min",
            "Max",
            "Average",
            "Difference",
            "Copy",
            "Transparent-zero"
        ),
        ijm = "operation"
    )]
    operation: String,
    /// The second operand. It is converted to the pixel type of image 1.
    #[param(label = "Image 2", ijm = "image2")]
    image2: DatasetId,
    /// Put the result in a new image instead of changing image 1.
    #[param(label = "Create new image", default = true, ijm = "create")]
    create: bool,
    /// Calculate in 32-bit float, so that nothing is clipped or rounded.
    /// Unless both images are already float this always creates a new image.
    #[param(label = "32-bit (float) result", default = false, ijm = "32-bit")]
    float_result: bool,
    /// What a float division by zero gives: `infinity` (the IEEE result,
    /// signed, with NaN for 0 / 0), `NaN`, `max` or a number.
    #[param(label = "Divide by zero value", default = "infinity", advanced)]
    divide_by_zero: String,
}

fn modifies() -> Effects {
    Effects {
        image: ImageEffect::Modifies,
        ..Effects::default()
    }
}

fn math_spec(id: &str, title: &str, weight: f64, ijm: &str, summary: &str) -> CommandSpec {
    CommandSpec::new(id, title)
        .summary(summary)
        .menu("Process > Math", weight)
        .ijm_name(ijm)
        .effects(modifies())
}

/// The channels a scope's planes belong to.
fn channels_in_scope(scope: &PlaneScope, meta: &ImageMeta) -> Result<Vec<usize>> {
    let mut channels = scope
        .planes(meta)?
        .into_iter()
        .map(|plane| Ok(meta.plane_channel(plane)?))
        .collect::<Result<Vec<_>>>()?;
    channels.sort_unstable();
    channels.dedup();
    Ok(channels)
}

/// Forgets the display range of `channels`, so that they are shown over
/// their data range again (ImageJ's `resetMinAndMax`).
pub(crate) fn reset_display_range(
    ctx: &mut CommandCtx<'_>,
    id: DatasetId,
    channels: &[usize],
) -> Result<()> {
    let image = ctx.image(id)?.clone();
    let mut meta = image.meta().clone();
    let mut changed = false;
    for &channel in channels {
        if let Some(entry) = meta.channels.get_mut(channel)
            && entry.display_range.take().is_some()
        {
            changed = true;
        }
    }
    if changed {
        ctx.workspace_mut().set_image(id, image.with_meta(meta)?)?;
    }
    Ok(())
}

/// Runs one Process > Math operation over a scope (`ImageMath.run`).
fn run_math(
    ctx: &mut CommandCtx<'_>,
    scope: &PlaneScope,
    verb: &str,
    op: MathOp,
) -> Result<Outputs> {
    let meta = ctx.image(scope.input)?.meta().clone();
    let rgb = meta.color == ColorModel::Rgb;
    let channels = channels_in_scope(scope, &meta)?;
    let outputs = scope.apply(ctx, verb, |plane, info| {
        let range = meta
            .channels
            .get(info.channel)
            .and_then(|c| c.display_range);
        let op = match op {
            MathOp::Set(value) if rgb => {
                MathOp::Set(math::rgb_set_components(value)[info.channel] as f64)
            }
            other => other,
        };
        math::apply(plane, op, range)
    })?;
    // ImageMath resets the display range after the operations that move the
    // extremes, except for 8-bit and RGB images, whose range is fixed.
    let moves_extremes = match op {
        MathOp::Min(_) | MathOp::Max(_) | MathOp::NanBackground { .. } => true,
        MathOp::Abs => meta.dtype().is_signed(),
        _ => false,
    };
    if moves_extremes && meta.dtype() != DType::U8 {
        reset_display_range(ctx, scope.input, &channels)?;
    }
    Ok(outputs)
}

fn binary(value: &str) -> Result<i32> {
    i32::from_str_radix(value.trim(), 2).map_err(|_| Error::InvalidArg {
        param: "value".to_string(),
        reason: format!("`{value}` is not a binary number of up to 31 digits, such as 11110000"),
    })
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        math_spec(
            "math.add",
            "Add",
            10.0,
            "Add...",
            "Adds a number to every sample.",
        )
        .doc(
            "Integer images take the whole part of the number and stop at the limits of \
             their pixel type.",
        ),
        |ctx, p: AddParams| run_math(ctx, &p.scope, "Adding", MathOp::Add(p.value)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.subtract",
            "Subtract",
            20.0,
            "Subtract...",
            "Subtracts a number from every sample.",
        )
        .doc(
            "Integer images take the whole part of the number and stop at the limits of \
             their pixel type.",
        ),
        |ctx, p: SubtractParams| run_math(ctx, &p.scope, "Subtracting", MathOp::Subtract(p.value)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.multiply",
            "Multiply",
            30.0,
            "Multiply...",
            "Multiplies every sample by a number.",
        )
        .doc("Integer results are rounded and stop at the limits of the pixel type."),
        |ctx, p: MultiplyParams| run_math(ctx, &p.scope, "Multiplying", MathOp::Multiply(p.value)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.divide",
            "Divide",
            40.0,
            "Divide...",
            "Divides every sample by a number.",
        )
        .doc(
            "Integer results are rounded. Dividing a float image by zero gives infinity, \
             and NaN where the sample is zero.",
        ),
        |ctx, p: DivideParams| run_math(ctx, &p.scope, "Dividing", MathOp::Divide(p.value)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.and",
            "AND",
            50.0,
            "AND...",
            "Combines every sample with a binary number, keeping the bits set in both.",
        )
        .doc("Float images are left unchanged."),
        |ctx, p: BitsParams| run_math(ctx, &p.scope, "Masking", MathOp::And(binary(&p.value)?)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.or",
            "OR",
            60.0,
            "OR...",
            "Combines every sample with a binary number, keeping the bits set in either.",
        )
        .doc("Float images are left unchanged."),
        |ctx, p: BitsParams| run_math(ctx, &p.scope, "Combining", MathOp::Or(binary(&p.value)?)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.xor",
            "XOR",
            70.0,
            "XOR...",
            "Combines every sample with a binary number, keeping the bits set in exactly one.",
        )
        .doc("Float images are left unchanged."),
        |ctx, p: BitsParams| run_math(ctx, &p.scope, "Combining", MathOp::Xor(binary(&p.value)?)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.min",
            "Min",
            80.0,
            "Min...",
            "Raises every sample below a given value to that value.",
        ),
        |ctx, p: MinParams| run_math(ctx, &p.scope, "Limiting", MathOp::Min(p.value)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.max",
            "Max",
            90.0,
            "Max...",
            "Lowers every sample above a given value to that value.",
        ),
        |ctx, p: MaxParams| run_math(ctx, &p.scope, "Limiting", MathOp::Max(p.value)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.gamma",
            "Gamma",
            100.0,
            "Gamma...",
            "Raises every sample to a power, changing the brightness of the mid-tones.",
        )
        .doc(
            "8-bit and RGB samples are scaled to 0 to 1 over 0 to 255 first; other integer \
             images over their display range (or each plane's minimum and maximum if none \
             is set). Float samples are raised to the power as they are, and those that \
             are not positive become 0.",
        ),
        |ctx, p: GammaParams| run_math(ctx, &p.scope, "Applying gamma", MathOp::Gamma(p.value)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.set",
            "Set",
            110.0,
            "Set...",
            "Gives every sample the same value.",
        ),
        |ctx, p: SetParams| run_math(ctx, &p.scope, "Setting", MathOp::Set(p.value)),
    ))?;
    registry.register(typed(
        math_spec(
            "math.log",
            "Log",
            120.0,
            "Log",
            "Replaces every sample by its natural logarithm.",
        )
        .doc(
            "Integer images are rescaled so that the result fills the same range: 8-bit \
             and RGB images over 0 to 255, others up to the maximum of their display range \
             (or of each plane if none is set). In a float image negative samples become \
             NaN and zero becomes negative infinity.",
        ),
        |ctx, p: ScopeOnly| run_math(ctx, &p.scope, "Taking logarithms", MathOp::Log),
    ))?;
    registry.register(typed(
        math_spec(
            "math.exp",
            "Exp",
            130.0,
            "Exp",
            "Replaces every sample by its exponential, the inverse of Log.",
        )
        .doc("Integer images are rescaled as for Log."),
        |ctx, p: ScopeOnly| run_math(ctx, &p.scope, "Taking exponentials", MathOp::Exp),
    ))?;
    registry.register(typed(
        math_spec(
            "math.square",
            "Square",
            140.0,
            "Square",
            "Multiplies every sample by itself.",
        ),
        |ctx, p: ScopeOnly| run_math(ctx, &p.scope, "Squaring", MathOp::Square),
    ))?;
    registry.register(typed(
        math_spec(
            "math.square_root",
            "Square Root",
            150.0,
            "Square Root",
            "Replaces every sample by its square root.",
        )
        .doc("Integer results are rounded down. Negative float samples become 0."),
        |ctx, p: ScopeOnly| run_math(ctx, &p.scope, "Taking square roots", MathOp::SquareRoot),
    ))?;
    registry.register(typed(
        math_spec(
            "math.reciprocal",
            "Reciprocal",
            160.0,
            "Reciprocal",
            "Replaces every sample of a float image by one divided by it.",
        )
        .doc("Samples that are zero become NaN."),
        |ctx, p: ScopeOnly| run_math(ctx, &p.scope, "Taking reciprocals", MathOp::Reciprocal),
    ))?;
    registry.register(typed(
        math_spec(
            "math.nan_background",
            "NaN Background",
            170.0,
            "NaN Background",
            "Sets the samples of a float image outside a range of values to NaN.",
        )
        .doc(
            "Samples that are NaN are left out of measurements, so this removes the \
             background from them.",
        ),
        |ctx, p: NanParams| {
            let op = MathOp::NanBackground {
                lower: p.lower,
                upper: p.upper,
            };
            run_math(ctx, &p.scope, "Removing background", op)
        },
    ))?;
    registry.register(typed(
        math_spec(
            "math.abs",
            "Abs",
            180.0,
            "Abs",
            "Replaces every negative sample by its positive counterpart.",
        )
        .doc("Images of an unsigned pixel type are left unchanged."),
        |ctx, p: ScopeOnly| run_math(ctx, &p.scope, "Taking absolute values", MathOp::Abs),
    ))?;

    registry.register(typed(
        CommandSpec::new("math.image_calculator", "Image Calculator")
            .summary(
                "Combines two images sample by sample, for example adding or subtracting them.",
            )
            .doc(
                "The images are aligned at their top-left corners and only where they \
                 overlap is anything calculated. Without Whole stack, the plane on view of \
                 image 1 is combined with the plane at the same position of image 2 (its \
                 first plane if the two images are organised differently). With Whole stack, \
                 image 2 must have as many planes as image 1 or just one. The selection is \
                 ignored.",
            )
            .menu("Process", 60.0)
            .ijm_name("Image Calculator...")
            .output("image", ParamType::Image)
            .effects(Effects {
                image: ImageEffect::Creates,
                ..Effects::default()
            }),
        calculator,
    ))?;

    Ok(())
}

/// One picture of an image: a plane, or the three planes of an RGB picture.
enum Picture {
    Gray(Buffer),
    Rgb([Buffer; 3]),
}

impl Picture {
    fn planes(self) -> Vec<Buffer> {
        match self {
            Picture::Gray(plane) => vec![plane],
            Picture::Rgb(planes) => planes.into(),
        }
    }
}

/// How one image presents its pictures to the calculator.
struct Operand {
    image: Image,
    rgb: bool,
}

impl Operand {
    fn new(image: &Image) -> Operand {
        Operand {
            image: image.clone(),
            rgb: image.meta().color == ColorModel::Rgb,
        }
    }

    fn meta(&self) -> &ImageMeta {
        self.image.meta()
    }

    /// Planes per picture.
    fn step(&self) -> usize {
        if self.rgb { 3 } else { 1 }
    }

    fn pictures(&self) -> usize {
        self.meta().plane_count() / self.step()
    }

    /// The picture containing plane number `plane`.
    fn picture_of(&self, plane: usize) -> Result<usize> {
        if !self.rgb {
            return Ok(plane);
        }
        // The channel axis may sit anywhere among the non-XY axes; number
        // pictures by the plane numbers of their red channel.
        let meta = self.meta();
        let mut position = meta.plane_position(plane)?;
        let axis = meta
            .axis_index(&AxisKind::Channel)
            .ok_or_else(|| Error::failed("an RGB image without a channel axis"))?;
        position[axis - 2] = 0;
        let red = meta.plane_index(&position)?;
        let reds = self.red_planes()?;
        reds.iter()
            .position(|&p| p == red)
            .ok_or_else(|| Error::failed("plane outside the image"))
    }

    fn red_planes(&self) -> Result<Vec<usize>> {
        let meta = self.meta();
        Ok((0..meta.plane_count())
            .filter(|&p| meta.plane_channel(p).is_ok_and(|c| c == 0))
            .collect())
    }

    /// The plane numbers of picture `index`.
    fn plane_numbers(&self, index: usize) -> Result<Vec<usize>> {
        if !self.rgb {
            return Ok(vec![index]);
        }
        let meta = self.meta();
        let axis = meta
            .axis_index(&AxisKind::Channel)
            .ok_or_else(|| Error::failed("an RGB image without a channel axis"))?;
        let red = self.red_planes()?[index];
        let mut position = meta.plane_position(red)?;
        (0..3)
            .map(|c| {
                position[axis - 2] = c;
                Ok(meta.plane_index(&position)?)
            })
            .collect()
    }

    fn picture(&self, index: usize) -> Result<Picture> {
        let numbers = self.plane_numbers(index)?;
        let mut planes = numbers
            .iter()
            .map(|&p| Ok(self.image.read_plane(p)?))
            .collect::<Result<Vec<_>>>()?;
        Ok(if self.rgb {
            let blue = planes.pop().expect("three planes");
            let green = planes.pop().expect("three planes");
            let red = planes.pop().expect("three planes");
            Picture::Rgb([red, green, blue])
        } else {
            Picture::Gray(planes.pop().expect("one plane"))
        })
    }
}

/// `TypeConverter.convertToByte(true)` for one plane of image 2: through
/// its display range.
fn to_byte_scaled(src: &Operand, index: usize, plane: &Buffer) -> Result<Buffer> {
    if plane.dtype() == DType::U8 {
        return Ok(plane.clone());
    }
    let number = src.plane_numbers(index)?[0];
    let range = calipix_render::effective_display_range(&src.image, number)?;
    let levels = LevelMap::new(plane.dtype(), false, range)?.levels(plane)?;
    Ok(Buffer::from_vec(levels))
}

/// Image 2's picture as a grey plane of pixel type `target`, converted the
/// way the `copyBits` method of the destination's processor converts it.
fn gray_source(src: &Operand, index: usize, target: DType) -> Result<Buffer> {
    let plane = match src.picture(index)? {
        Picture::Gray(plane) => plane,
        Picture::Rgb([r, g, b]) if target == DType::F32 || target == DType::F64 => {
            let grey = convert::rgb_to_gray_f32([&r, &g, &b], UNWEIGHTED)?;
            return Ok(convert::convert(&grey, target)?);
        }
        Picture::Rgb([r, g, b]) => convert::rgb_to_gray_u8([&r, &g, &b], UNWEIGHTED)?,
    };
    if target == DType::U8 {
        return to_byte_scaled(src, index, &plane);
    }
    Ok(convert::convert(&plane, target)?)
}

/// Image 2's picture as RGB (`TypeConverter.convertToRGB`): a grey plane is
/// shown through its display range and lookup table.
fn rgb_source(src: &Operand, index: usize) -> Result<[Buffer; 3]> {
    match src.picture(index)? {
        Picture::Rgb(planes) => Ok(planes),
        Picture::Gray(plane) => {
            let number = src.plane_numbers(index)?[0];
            let range = calipix_render::effective_display_range(&src.image, number)?;
            let levels = LevelMap::new(plane.dtype(), false, range)?.levels(&plane)?;
            let channel = src.meta().plane_channel(number)?;
            let lut = calipix_render::channel_lut(&src.image, channel, ChannelMode::Color);
            Ok([0, 1, 2].map(|c| {
                Buffer::from_vec(
                    levels
                        .iter()
                        .map(|&level| lut.table[level as usize][c])
                        .collect::<Vec<u8>>(),
                )
            }))
        }
    }
}

/// `dst` with its top-left `rect` replaced by `part`.
fn paste(dst: &Buffer, width: usize, part: &Buffer, rect: Rect) -> Result<Buffer> {
    fn go<T: Pixel>(dst: &[T], width: usize, part: &Buffer, r: Rect) -> Result<Buffer> {
        let part = part.try_slice::<T>()?;
        let mut out = dst.to_vec();
        for row in 0..r.h {
            out[(r.y + row) * width + r.x..(r.y + row) * width + r.x + r.w]
                .copy_from_slice(&part[row * r.w..(row + 1) * r.w]);
        }
        Ok(Buffer::from_vec(out))
    }
    dispatch!(dst, |d| go(d, width, part, rect))
}

/// Sizes of the two operands of one blit.
#[derive(Clone, Copy)]
struct Sizes {
    dst: (usize, usize),
    src: (usize, usize),
}

impl Sizes {
    /// Where the two images, aligned at their top-left corners, overlap.
    fn overlap(&self) -> Rect {
        Rect::new(0, 0, self.dst.0.min(self.src.0), self.dst.1.min(self.src.1))
    }
}

/// `dst.copyBits(src, 0, 0, mode)` for two planes of one pixel type.
fn blit(dst: &Buffer, src: &Buffer, sizes: Sizes, op: CalcOp, dbz: f64) -> Result<Buffer> {
    let overlap = sizes.overlap();
    if sizes.dst == sizes.src {
        return Ok(math::calculate(dst, src, op, dbz)?);
    }
    let d = crop_buffer(dst, sizes.dst.0, overlap)?;
    let s = crop_buffer(src, sizes.src.0, overlap)?;
    paste(
        dst,
        sizes.dst.0,
        &math::calculate(&d, &s, op, dbz)?,
        overlap,
    )
}

/// `ByteProcessor.setPixels(int, FloatProcessor)` and its 16-bit twin: back
/// from the temporary float image a multiplication or division by a float
/// image is done in.
fn from_temporary_float(float: &Buffer, target: DType) -> Result<Buffer> {
    Ok(match target {
        // 16-bit adds the half in single precision too.
        DType::U16 => Buffer::from_vec(
            float
                .try_slice::<f32>()?
                .iter()
                .map(|&v| (v + 0.5).clamp(0.0, 65535.0) as u16)
                .collect::<Vec<_>>(),
        ),
        _ => convert::convert(float, target)?,
    })
}

/// One picture of image 1 combined with one of image 2 (`copyBits` of the
/// destination's processor). `float` asks for the calculation in 32-bit
/// float, which turns a grey destination into float.
fn combine(
    dst: Picture,
    src: &Operand,
    index: usize,
    sizes: Sizes,
    op: CalcOp,
    float: bool,
    dbz: f64,
) -> Result<Picture> {
    let src_float = src.meta().dtype() == DType::F32 || src.meta().dtype() == DType::F64;
    match dst {
        Picture::Rgb(planes) => {
            let source = rgb_source(src, index)?;
            let overlap = sizes.overlap();
            let d: Vec<Buffer> = planes
                .iter()
                .map(|p| crop_buffer(p, sizes.dst.0, overlap))
                .collect::<calipix_core::Result<_>>()?;
            let s: Vec<Buffer> = source
                .iter()
                .map(|p| crop_buffer(p, sizes.src.0, overlap))
                .collect::<calipix_core::Result<_>>()?;
            let out = math::calculate_rgb([&d[0], &d[1], &d[2]], [&s[0], &s[1], &s[2]], op)?;
            let [r, g, b] = [0, 1, 2].map(|c| paste(&planes[c], sizes.dst.0, &out[c], overlap));
            let result = [r?, g?, b?];
            // ImageCalculator.doOperation: a float result of two RGB images
            // is the grey value of the RGB result.
            if float && src.rgb {
                let [r, g, b] = &result;
                return Ok(Picture::Gray(convert::rgb_to_gray_f32(
                    [r, g, b],
                    UNWEIGHTED,
                )?));
            }
            Ok(Picture::Rgb(result))
        }
        Picture::Gray(plane) => {
            let plane = if float {
                convert::convert(&plane, DType::F32)?
            } else {
                plane
            };
            let dtype = plane.dtype();
            // Multiplying or dividing an integer image by a float one is
            // done in float and rounded afterwards.
            let temporary = dtype.is_integer()
                && src_float
                && !src.rgb
                && matches!(op, CalcOp::Multiply | CalcOp::Divide);
            if temporary {
                let d = convert::convert(&plane, DType::F32)?;
                let s = gray_source(src, index, DType::F32)?;
                let out = blit(&d, &s, sizes, op, dbz)?;
                return Ok(Picture::Gray(from_temporary_float(&out, dtype)?));
            }
            let s = gray_source(src, index, dtype)?;
            Ok(Picture::Gray(blit(&plane, &s, sizes, op, dbz)?))
        }
    }
}

fn divide_by_zero_value(text: &str) -> Result<f64> {
    let lower = text.trim().to_ascii_lowercase();
    match lower.as_str() {
        "infinity" | "infinite" | "inf" => Ok(f64::INFINITY),
        "nan" => Ok(f64::NAN),
        "max" => Ok(f32::MAX as f64),
        other => other.parse::<f64>().map_err(|_| Error::InvalidArg {
            param: "divide_by_zero".to_string(),
            reason: format!("`{text}` is not `infinity`, `NaN`, `max` or a number"),
        }),
    }
}

/// The XY axes of `meta` for an image of another size.
fn xy_axes(meta: &ImageMeta, width: usize, height: usize) -> Vec<(Axis, usize)> {
    vec![
        (meta.axes()[0].clone(), width),
        (meta.axes()[1].clone(), height),
    ]
}

/// `ImageCalculator.calculate`.
fn calculator(ctx: &mut CommandCtx<'_>, p: CalculatorParams) -> Result<Outputs> {
    let op = CalcOp::from_name(&p.operation).ok_or_else(|| Error::InvalidArg {
        param: "operation".to_string(),
        reason: format!("`{}` is not an operation", p.operation),
    })?;
    let dbz = divide_by_zero_value(&p.divide_by_zero)?;
    let first = Operand::new(ctx.image(p.scope.input)?);
    let second = Operand::new(ctx.image(p.image2)?);
    let (meta1, meta2) = (first.meta().clone(), second.meta().clone());
    for meta in [&meta1, &meta2] {
        if meta.dtype().is_complex() {
            return Err(Error::failed(
                "the image calculator does not support complex images",
            ));
        }
    }
    let both_float = meta1.dtype() == DType::F32 && meta2.dtype() == DType::F32;
    let float = p.float_result;
    let create = p.create || (float && !both_float);
    let sizes = Sizes {
        dst: (meta1.width(), meta1.height()),
        src: (meta2.width(), meta2.height()),
    };
    let (count1, count2) = (first.pictures(), second.pictures());
    let whole_stack = p.scope.stack && (count1 > 1 || count2 > 1);

    // Which pictures of image 1 take part, and with which of image 2.
    let pairs: Vec<(usize, usize)> = if whole_stack {
        if count1 > 1 && count2 > 1 && count1 != count2 {
            return Err(Error::failed(format!(
                "image 1 has {count1} planes and image 2 has {count2}; to combine whole \
                 stacks they must have the same number, or image 2 just one"
            )));
        }
        (0..count1)
            .map(|i| (i, if count2 == count1 { i } else { 0 }))
            .collect()
    } else {
        let mut pictures = p
            .scope
            .planes(&meta1)?
            .into_iter()
            .map(|plane| first.picture_of(plane))
            .collect::<Result<Vec<_>>>()?;
        pictures.dedup();
        let same_layout = meta1.plane_shape() == meta2.plane_shape();
        pictures
            .into_iter()
            .map(|i| (i, if same_layout { i } else { 0 }))
            .collect()
    };

    let mut results = Vec::with_capacity(pairs.len());
    for (step, &(i, j)) in pairs.iter().enumerate() {
        ctx.check_cancelled()?;
        ctx.progress(step as f64 / pairs.len() as f64, "Calculating");
        let mut dst = first.picture(i)?;
        let mut blit_sizes = sizes;
        // A new image made from single pictures is only as large as the
        // overlap (ImageCalculator.createNewImage); a new stack keeps the
        // size of image 1 (duplicateStack).
        if create && !whole_stack {
            let overlap = sizes.overlap();
            blit_sizes.dst = (overlap.w, overlap.h);
            dst = match dst {
                Picture::Gray(plane) => Picture::Gray(crop_buffer(&plane, sizes.dst.0, overlap)?),
                Picture::Rgb(planes) => {
                    let [r, g, b] = planes.map(|p| crop_buffer(&p, sizes.dst.0, overlap));
                    Picture::Rgb([r?, g?, b?])
                }
            };
        }
        // duplicateStack turns an RGB stack into grey float before the
        // calculation; a single RGB picture stays RGB during it.
        if float
            && whole_stack
            && let Picture::Rgb([r, g, b]) = &dst
        {
            dst = Picture::Gray(convert::rgb_to_gray_f32([r, g, b], UNWEIGHTED)?);
        }
        results.push(combine(dst, &second, j, blit_sizes, op, float, dbz)?);
    }

    let result_rgb = matches!(results.first(), Some(Picture::Rgb(_)));
    let result_dtype = match results.first() {
        Some(Picture::Gray(plane)) => plane.dtype(),
        _ => DType::U8,
    };

    if !create {
        // Image 1 is changed where image 2 overlaps it.
        let mut image = first.image.clone();
        let mut channels = Vec::new();
        for (&(i, _), picture) in pairs.iter().zip(results) {
            for (number, plane) in first.plane_numbers(i)?.into_iter().zip(picture.planes()) {
                channels.push(meta1.plane_channel(number)?);
                image = image.with_plane(number, plane)?;
            }
        }
        ctx.workspace_mut().set_image(p.scope.input, image)?;
        if meta1.dtype() != DType::U8 {
            channels.sort_unstable();
            channels.dedup();
            reset_display_range(ctx, p.scope.input, &channels)?;
        }
        return Ok(Outputs::new());
    }

    // The new image: the calibration of image 1, and its lookup tables.
    let (width, height) = if whole_stack {
        sizes.dst
    } else {
        let overlap = sizes.overlap();
        (overlap.w, overlap.h)
    };
    let mut axes = xy_axes(&meta1, width, height);
    if result_rgb {
        axes.push((Axis::new(AxisKind::Channel), 3));
    }
    let mut channel_meta = Vec::new();
    if whole_stack {
        // An RGB stack keeps its channel axis in front or, turned into
        // float, loses it; any other stack keeps the axes of image 1.
        for (axis, &size) in meta1.axes().iter().zip(meta1.shape()).skip(2) {
            if !(axis.kind == AxisKind::Channel && first.rgb) {
                axes.push((axis.clone(), size));
            }
        }
        if !first.rgb {
            channel_meta = meta1.channels.clone();
        }
    } else if !first.rgb {
        // One plane, or the channels of one position of a composite.
        if pairs.len() > 1 {
            axes.push((Axis::new(AxisKind::Channel), pairs.len()));
        }
        if !meta1.channels.is_empty() {
            channel_meta = pairs
                .iter()
                .map(|&(i, _)| Ok(meta1.channels[meta1.plane_channel(i)?].clone()))
                .collect::<Result<Vec<_>>>()?;
        }
    }
    let mut meta = ImageMeta::new(if result_rgb { DType::U8 } else { result_dtype }, axes)?
        .with_name(format!("Result of {}", meta1.name));
    meta.color = if result_rgb {
        ColorModel::Rgb
    } else if whole_stack && !first.rgb {
        meta1.color
    } else if pairs.len() > 1 {
        ColorModel::Composite
    } else {
        ColorModel::Grayscale
    };
    if !result_rgb && !channel_meta.is_empty() {
        // Only 8-bit results keep a display range; others are shown over
        // their data range (resetMinAndMax).
        if result_dtype != DType::U8 {
            for entry in &mut channel_meta {
                entry.display_range = None;
            }
        }
        meta.channels = channel_meta;
    }
    if whole_stack {
        meta.properties = meta1.properties.clone();
    }

    // Pictures were calculated in the order of image 1's planes, which is
    // the order of the new image's axes.
    let planes: Vec<Buffer> = results.into_iter().flat_map(Picture::planes).collect();
    let image = Image::from_planes(meta, planes)?;
    let id = ctx.workspace_mut().insert(image, None);
    Ok(Outputs::new().with("image", id))
}
