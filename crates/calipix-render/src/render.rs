//! Planes, regions and thumbnails as RGBA pictures.

use calipix_core::{AxisKind, ColorModel, Error, Image, Lut, Rect, Result};

use crate::luts::default_channel_lut;
use crate::mapping::{LevelMap, LevelRule, default_display_range};

/// An 8-bit RGBA picture, row-major, not premultiplied. Rendering always
/// produces opaque pixels (alpha 255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba8 {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// How the channels of a multi-channel image are shown. These are the
/// display modes of ImageJ's `CompositeImage`. Images whose colour model is
/// [`ColorModel::Rgb`] ignore the mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChannelMode {
    /// [`Composite`](Self::Composite) if the image's colour model is
    /// [`ColorModel::Composite`], otherwise [`Color`](Self::Color).
    #[default]
    Auto,
    /// All visible channels at once, each through its own lookup table and
    /// display range, combined per colour component.
    Composite,
    /// Only the channel named by the position, through its lookup table.
    Color,
    /// Only the channel named by the position, in shades of grey.
    Grayscale,
}

/// How composite mode combines the colours of the visible channels
/// (`ImageProcessor.updateComposite`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    /// Each colour component is the sum over the channels, clamped to 255.
    /// This is ImageJ's default.
    #[default]
    Sum,
    /// The largest component over the channels.
    Max,
    /// The smallest component over the channels (white if none is visible).
    Min,
}

/// How a picture is made smaller than the region it shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reduction {
    /// Each output pixel is the mean of the displayed colours of the source
    /// pixels it covers.
    #[default]
    BoxAverage,
    /// Each output pixel shows the source pixel under its centre.
    Nearest,
}

/// Display state that is not part of the image's metadata. Lookup tables and
/// display ranges live in [`calipix_core::ChannelMeta`]; none of this, and
/// nothing else in this crate, changes sample values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayOptions {
    pub mode: ChannelMode,
    /// Whether each channel is shown in composite mode. Channels beyond the
    /// end of the list are visible, so an empty list shows everything. The
    /// single-channel modes show the selected channel regardless.
    pub visible: Vec<bool>,
    pub blend: Blend,
    pub reduction: Reduction,
}

impl DisplayOptions {
    pub fn is_visible(&self, channel: usize) -> bool {
        self.visible.get(channel).copied().unwrap_or(true)
    }

    /// The mode that applies to `image` once [`ChannelMode::Auto`] is
    /// resolved.
    pub fn resolved_mode(&self, image: &Image) -> ChannelMode {
        match self.mode {
            ChannelMode::Auto if image.meta().color == ColorModel::Composite => {
                ChannelMode::Composite
            }
            ChannelMode::Auto => ChannelMode::Color,
            mode => mode,
        }
    }
}

/// The lookup table `channel` is shown through: the channel's own if it has
/// one; otherwise ImageJ's default channel colour
/// ([`default_channel_lut`]) when the image has several channels and is a
/// composite or is being shown as one, and grey in every other case.
/// [`ChannelMode::Grayscale`] always gives grey.
pub fn channel_lut(image: &Image, channel: usize, mode: ChannelMode) -> Lut {
    let meta = image.meta();
    if mode == ChannelMode::Grayscale {
        return Lut::grays();
    }
    if let Some(lut) = meta.channels.get(channel).and_then(|c| c.lut.clone()) {
        return lut;
    }
    let composite = meta.color == ColorModel::Composite || mode == ChannelMode::Composite;
    if composite && meta.size(&AxisKind::Channel) > 1 {
        default_channel_lut(channel)
    } else {
        Lut::grays()
    }
}

/// The display range plane number `plane` is shown with: its channel's
/// [`display_range`](calipix_core::ChannelMeta::display_range) if set, and
/// otherwise the default.
///
/// The default is 0 to 255 for unsigned 8-bit samples, and for other types
/// the smallest and largest sample of the whole plane
/// ([`default_display_range`]), which means reading the plane. As in ImageJ
/// (`ShortProcessor.setMinAndMax`, `FloatProcessor.setMinAndMax`), a range of
/// exactly `(0, 0)` on anything but unsigned 8-bit also means the default.
/// Channels of an RGB image default to 0 to 255 if 8-bit, to the type's full
/// range for other integers and to 0 to 1 for floats.
pub fn effective_display_range(image: &Image, plane: usize) -> Result<(f64, f64)> {
    let meta = image.meta();
    let channel = meta.plane_channel(plane)?;
    let rgb = meta.color == ColorModel::Rgb;
    let rule = LevelRule::for_dtype(meta.dtype(), rgb)?;
    let auto = matches!(rule, LevelRule::Integer | LevelRule::Float);
    match meta.channels.get(channel).and_then(|c| c.display_range) {
        Some(range) if !(auto && range == (0.0, 0.0)) => Ok(range),
        _ if rgb => Ok(meta.dtype().integer_range().unwrap_or((0.0, 1.0))),
        _ if !auto => Ok((0.0, 255.0)),
        _ => default_display_range(&image.read_plane(plane)?),
    }
}

/// One channel's contribution to a picture.
struct Layer {
    plane: usize,
    map: LevelMap,
    /// `None` for a component of an RGB image, whose level is the colour.
    lut: Option<Lut>,
}

/// Everything needed to colour any region of one displayed plane.
struct Plan {
    layers: Vec<Layer>,
    rgb: bool,
    blend: Blend,
}

impl Plan {
    fn new(image: &Image, position: &[usize], options: &DisplayOptions) -> Result<Plan> {
        let meta = image.meta();
        let index = meta.plane_index(position)?;
        let rgb = meta.color == ColorModel::Rgb;
        let mode = options.resolved_mode(image);
        let n_channels = meta.size(&AxisKind::Channel);
        // The plane showing channel `c` at the same place along other axes.
        let plane_of = |c: usize| -> Result<usize> {
            match meta.axis_index(&AxisKind::Channel) {
                Some(axis) => {
                    let mut position = position.to_vec();
                    position[axis - 2] = c;
                    meta.plane_index(&position)
                }
                None => Ok(index),
            }
        };
        let layer = |c: usize, lut: Option<Lut>| -> Result<Layer> {
            let plane = plane_of(c)?;
            let range = effective_display_range(image, plane)?;
            Ok(Layer {
                plane,
                map: LevelMap::new(meta.dtype(), rgb, range)?,
                lut,
            })
        };
        let layers = if rgb {
            (0..3).map(|c| layer(c, None)).collect::<Result<_>>()?
        } else if mode == ChannelMode::Composite {
            (0..n_channels)
                .filter(|&c| options.is_visible(c))
                .map(|c| layer(c, Some(channel_lut(image, c, mode))))
                .collect::<Result<_>>()?
        } else {
            let c = meta.plane_channel(index)?;
            vec![layer(c, Some(channel_lut(image, c, mode)))?]
        };
        Ok(Plan {
            layers,
            rgb,
            blend: options.blend,
        })
    }

    /// The displayed colours of `rect`, three bytes per pixel.
    fn shade(&self, image: &Image, rect: Rect) -> Result<Vec<u8>> {
        let len = rect.w * rect.h;
        let start = if !self.rgb && self.blend == Blend::Min {
            255
        } else {
            0
        };
        let mut out = vec![start; len * 3];
        for (i, layer) in self.layers.iter().enumerate() {
            let levels = layer.map.levels(&image.read_region(layer.plane, rect)?)?;
            let pixels = out.as_chunks_mut::<3>().0.iter_mut().zip(levels);
            match &layer.lut {
                None => pixels.for_each(|(pixel, level)| pixel[i] = level),
                Some(lut) => {
                    let combine: fn(u8, u8) -> u8 = match self.blend {
                        Blend::Sum => u8::saturating_add,
                        Blend::Max => u8::max,
                        Blend::Min => u8::min,
                    };
                    for (pixel, level) in pixels {
                        let color = lut.table[level as usize];
                        for (component, value) in pixel.iter_mut().zip(color) {
                            *component = combine(*component, value);
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

/// The source index ranges that each of `dst` output pixels shows along one
/// axis of `src` source pixels.
///
/// Magnifying (and [`Reduction::Nearest`]) shows the single source pixel
/// under the centre of the output pixel: `floor((o + 0.5) * src / dst)`.
/// Reducing with [`Reduction::BoxAverage`] shares the source pixels out
/// without overlap: output pixel `o` covers
/// `floor(o * src / dst) .. floor((o + 1) * src / dst)`.
fn spans(src: usize, dst: usize, reduction: Reduction) -> Vec<(usize, usize)> {
    let at = |numerator: usize, denominator: usize| {
        (numerator as u128 * src as u128 / denominator as u128) as usize
    };
    (0..dst)
        .map(|o| {
            if dst >= src || reduction == Reduction::Nearest {
                let s = at(2 * o + 1, 2 * dst);
                (s, s + 1)
            } else {
                (at(o, dst), at(o + 1, dst))
            }
        })
        .collect()
}

/// Source samples read at a time while rendering a region, so that reducing
/// a very large region does not hold all of it in memory.
const STRIP_SAMPLES: usize = 1 << 22;

/// Renders the XY plane at `position` (one index per non-XY axis) at full
/// size with default [`DisplayOptions`].
///
/// RGB images combine their three channels directly and composite images
/// blend all their channels; in both cases the channel index in `position`
/// is ignored. Other images show the single plane at `position` through its
/// channel's lookup table and display range.
pub fn render_plane(image: &Image, position: &[usize]) -> Result<Rgba8> {
    render_plane_with(image, position, &DisplayOptions::default())
}

/// Renders the XY plane at `position` at full size.
pub fn render_plane_with(
    image: &Image,
    position: &[usize],
    options: &DisplayOptions,
) -> Result<Rgba8> {
    let (width, height) = (image.meta().width(), image.meta().height());
    let region = Rect::new(0, 0, width, height);
    render_region(image, position, region, width, height, options)
}

/// Renders `region` of the XY plane at `position` as a picture of
/// `out_width` by `out_height` pixels.
///
/// Each axis is scaled on its own. Where the output is at least as large as
/// the region, every output pixel shows the source pixel under its centre
/// (nearest neighbour), so whole-number magnifications draw each source
/// pixel as a block. Where it is smaller, [`DisplayOptions::reduction`]
/// decides: by default each output pixel is the mean, rounded to nearest,
/// of the *displayed colours* of the source pixels it covers, with output
/// pixel `o` of `n` covering source pixels `floor(o * size / n)` up to but
/// not including `floor((o + 1) * size / n)`. (ImageJ leaves reduction to
/// Java2D's quality rendering, whose result depends on the platform, so
/// there is nothing exact to match.)
///
/// Only `region` is read from the image, in strips, except that a channel
/// without a display range needs its whole plane once to find the default
/// range (see [`effective_display_range`]). A region rendered at its own
/// size equals the same rectangle cut from the full rendering.
pub fn render_region(
    image: &Image,
    position: &[usize],
    region: Rect,
    out_width: usize,
    out_height: usize,
    options: &DisplayOptions,
) -> Result<Rgba8> {
    let meta = image.meta();
    if !region.fits(meta.width(), meta.height()) {
        return Err(Error::OutOfBounds(format!(
            "region {region:?} outside {}x{} plane",
            meta.width(),
            meta.height()
        )));
    }
    if out_width == 0 || out_height == 0 {
        return Err(Error::Shape(format!(
            "cannot render a {out_width}x{out_height} picture"
        )));
    }
    let plan = Plan::new(image, position, options)?;
    let columns = spans(region.w, out_width, options.reduction);
    let rows = spans(region.h, out_height, options.reduction);
    let mut data = Vec::with_capacity(out_width * out_height * 4);

    let mut first = 0;
    while first < rows.len() {
        // Take output rows while their source rows fit in one strip.
        let top = rows[first].0;
        let max_rows = (STRIP_SAMPLES / region.w).max(rows[first].1 - top);
        let mut last = first + 1;
        while last < rows.len() && rows[last].1 - top <= max_rows {
            last += 1;
        }
        let strip = Rect::new(region.x, region.y + top, region.w, rows[last - 1].1 - top);
        let colors = plan.shade(image, strip)?;
        let colors = colors.as_chunks::<3>().0;
        for &(y0, y1) in &rows[first..last] {
            for &(x0, x1) in &columns {
                if y1 - y0 == 1 && x1 - x0 == 1 {
                    data.extend_from_slice(&colors[(y0 - top) * region.w + x0]);
                } else {
                    let mut sum = [0u64; 3];
                    for y in y0..y1 {
                        let row = &colors[(y - top) * region.w..][x0..x1];
                        for pixel in row {
                            for (total, &component) in sum.iter_mut().zip(pixel) {
                                *total += component as u64;
                            }
                        }
                    }
                    let n = ((y1 - y0) * (x1 - x0)) as u64;
                    data.extend(sum.map(|total| ((total + n / 2) / n) as u8));
                }
                data.push(255);
            }
        }
        first = last;
    }
    Ok(Rgba8 {
        width: out_width,
        height: out_height,
        data,
    })
}

/// A picture of the first plane of `image` (position 0 along every non-XY
/// axis) that fits in a square of `max_side` pixels, with default
/// [`DisplayOptions`]. See [`thumbnail_at`].
pub fn thumbnail(image: &Image, max_side: usize) -> Result<Rgba8> {
    let position = vec![0; image.meta().plane_shape().len()];
    thumbnail_at(image, &position, max_side, &DisplayOptions::default())
}

/// A picture of the whole XY plane at `position` that fits in a square of
/// `max_side` pixels. The aspect ratio is kept (each side is rounded to the
/// nearest pixel and is at least 1), and an image that already fits is
/// rendered at its own size, never enlarged.
pub fn thumbnail_at(
    image: &Image,
    position: &[usize],
    max_side: usize,
    options: &DisplayOptions,
) -> Result<Rgba8> {
    if max_side == 0 {
        return Err(Error::Shape(
            "a thumbnail needs a size of at least 1 pixel".to_string(),
        ));
    }
    let (width, height) = (image.meta().width(), image.meta().height());
    let longest = width.max(height);
    let scaled = |side: usize| {
        if longest <= max_side {
            side
        } else {
            let exact = (side as u128 * max_side as u128 + longest as u128 / 2) / longest as u128;
            (exact as usize).max(1)
        }
    };
    let region = Rect::new(0, 0, width, height);
    render_region(
        image,
        position,
        region,
        scaled(width),
        scaled(height),
        options,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use calipix_core::{Buffer, ChannelMeta, DType, ImageMeta};

    fn channel(lut: Option<Lut>, display_range: Option<(f64, f64)>) -> ChannelMeta {
        ChannelMeta {
            name: None,
            lut,
            display_range,
        }
    }

    /// A 1x1 image with one 8-bit plane per value, flagged as `color`.
    fn channels(values: &[u8], color: ColorModel) -> Image {
        let mut meta = ImageMeta::xyczt(DType::U8, 1, 1, values.len(), 1, 1).unwrap();
        meta.color = color;
        let planes = values.iter().map(|&v| Buffer::from_vec(vec![v])).collect();
        Image::from_planes(meta, planes).unwrap()
    }

    fn rgb(picture: &Rgba8) -> Vec<[u8; 3]> {
        let pixels = picture.data.as_chunks::<4>().0;
        assert!(pixels.iter().all(|p| p[3] == 255));
        pixels.iter().map(|p| [p[0], p[1], p[2]]).collect()
    }

    #[test]
    fn grayscale_uses_lut_and_display_range() {
        let image = Image::from_vec(2, 1, vec![0u16, 1000]).unwrap();
        let out = render_plane(&image, &[]).unwrap();
        assert_eq!(out.data, [0, 0, 0, 255, 255, 255, 255, 255]);

        let mut meta = image.meta().clone();
        meta.channels = vec![channel(
            Some(Lut::from_fn("Red", |i| [i, 0, 0])),
            Some((0.0, 2000.0)),
        )];
        let image = image.with_meta(meta).unwrap();
        let out = render_plane(&image, &[]).unwrap();
        assert_eq!(out.data, [0, 0, 0, 255, 128, 0, 0, 255]);
        // Grayscale mode ignores the lookup table but keeps the range.
        let options = DisplayOptions {
            mode: ChannelMode::Grayscale,
            ..Default::default()
        };
        let out = render_plane_with(&image, &[], &options).unwrap();
        assert_eq!(rgb(&out), [[0, 0, 0], [128, 128, 128]]);
    }

    #[test]
    fn zero_range_means_default_except_for_eight_bit() {
        let with_range = |image: Image, range| {
            let mut meta = image.meta().clone();
            meta.channels = vec![channel(None, Some(range))];
            image.with_meta(meta).unwrap()
        };
        let image = with_range(Image::from_vec(2, 1, vec![10u16, 20]).unwrap(), (0.0, 0.0));
        assert_eq!(effective_display_range(&image, 0).unwrap(), (10.0, 20.0));
        let image = with_range(
            Image::from_vec(2, 1, vec![-1.5f32, 4.0]).unwrap(),
            (0.0, 0.0),
        );
        assert_eq!(effective_display_range(&image, 0).unwrap(), (-1.5, 4.0));
        let image = with_range(Image::from_vec(2, 1, vec![10u8, 20]).unwrap(), (0.0, 0.0));
        assert_eq!(effective_display_range(&image, 0).unwrap(), (0.0, 0.0));
        let image = Image::from_vec(2, 1, vec![10u8, 20]).unwrap();
        assert_eq!(effective_display_range(&image, 0).unwrap(), (0.0, 255.0));
    }

    #[test]
    fn rgb_combines_channels() {
        let image = channels(&[10, 20, 30], ColorModel::Rgb);
        assert_eq!(render_plane(&image, &[0]).unwrap().data, [10, 20, 30, 255]);
        // The channel coordinate does not matter for RGB.
        assert_eq!(render_plane(&image, &[2]).unwrap().data, [10, 20, 30, 255]);
        assert!(render_plane(&image, &[]).is_err());
        // Neither do the channel mode or visibility.
        let options = DisplayOptions {
            mode: ChannelMode::Grayscale,
            visible: vec![false; 3],
            blend: Blend::Min,
            ..Default::default()
        };
        let out = render_plane_with(&image, &[1], &options).unwrap();
        assert_eq!(out.data, [10, 20, 30, 255]);
    }

    #[test]
    fn rgb_display_range_uses_the_color_processor_rule() {
        let image = channels(&[49, 125, 200], ColorModel::Rgb);
        let mut meta = image.meta().clone();
        meta.channels = vec![channel(None, Some((50.0, 200.0))); 3];
        // (int)(256 * (v - 50) / 150): 49 -> 0, 125 -> 128, 200 -> 255.
        let ranged = image.with_meta(meta.clone()).unwrap();
        assert_eq!(
            render_plane(&ranged, &[0]).unwrap().data,
            [0, 128, 255, 255]
        );
        // A range on one channel leaves the others alone.
        meta.channels[1].display_range = None;
        let ranged = image.with_meta(meta).unwrap();
        assert_eq!(
            render_plane(&ranged, &[0]).unwrap().data,
            [0, 125, 255, 255]
        );
    }

    #[test]
    fn rgb_of_other_types_defaults_to_the_full_range() {
        let mut meta = ImageMeta::xyczt(DType::U16, 1, 1, 3, 1, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let planes = [0u16, 32768, 65535]
            .map(|v| Buffer::from_vec(vec![v]))
            .to_vec();
        let image = Image::from_planes(meta, planes).unwrap();
        assert_eq!(render_plane(&image, &[0]).unwrap().data, [0, 128, 255, 255]);
    }

    #[test]
    fn picks_the_requested_plane() {
        let meta = ImageMeta::xyczt(DType::U8, 1, 1, 1, 2, 1).unwrap();
        let planes = vec![Buffer::from_vec(vec![5u8]), Buffer::from_vec(vec![200u8])];
        let image = Image::from_planes(meta, planes).unwrap();
        assert_eq!(
            render_plane(&image, &[1]).unwrap().data,
            [200, 200, 200, 255]
        );
        assert!(render_plane(&image, &[2]).is_err());
    }

    #[test]
    fn composite_sums_channels_and_clamps() {
        // Default colours: red, green, blue, then white.
        let image = channels(&[10, 20, 30, 40], ColorModel::Composite);
        let out = render_plane(&image, &[0]).unwrap();
        assert_eq!(rgb(&out), [[50, 60, 70]]);
        // The channel coordinate is irrelevant in composite mode.
        assert_eq!(render_plane(&image, &[3]).unwrap(), out);

        let image = channels(&[200, 100, 30, 90], ColorModel::Composite);
        assert_eq!(rgb(&render_plane(&image, &[0]).unwrap()), [[255, 190, 120]]);
        let with = |blend, visible: &[bool]| {
            let options = DisplayOptions {
                blend,
                visible: visible.to_vec(),
                ..Default::default()
            };
            rgb(&render_plane_with(&image, &[0], &options).unwrap())[0]
        };
        assert_eq!(with(Blend::Max, &[]), [200, 100, 90]);
        assert_eq!(with(Blend::Min, &[]), [0, 0, 0]);
        // Red and white only.
        assert_eq!(with(Blend::Sum, &[true, false, false]), [255, 90, 90]);
        assert_eq!(with(Blend::Min, &[false, false, false, true]), [90, 90, 90]);
        assert_eq!(with(Blend::Sum, &[false; 4]), [0, 0, 0]);
        assert_eq!(with(Blend::Max, &[false; 4]), [0, 0, 0]);
        assert_eq!(with(Blend::Min, &[false; 4]), [255, 255, 255]);
    }

    #[test]
    fn composite_uses_each_channels_lut_and_range() {
        let image = channels(&[100, 100], ColorModel::Composite);
        let mut meta = image.meta().clone();
        meta.channels = vec![
            channel(
                Some(crate::builtin_lut("cyan").unwrap()),
                Some((50.0, 150.0)),
            ),
            channel(None, Some((0.0, 200.0))),
        ];
        let image = image.with_meta(meta).unwrap();
        // Channel 0: (int)(256 * 50 / 100) = 128 as cyan; channel 1 keeps
        // the default green at (int)(256 * 100 / 200) = 128.
        assert_eq!(rgb(&render_plane(&image, &[0]).unwrap()), [[0, 255, 128]]);
    }

    #[test]
    fn modes_select_what_is_shown() {
        let composite = channels(&[10, 20, 30], ColorModel::Composite);
        let plain = channels(&[10, 20, 30], ColorModel::Grayscale);
        let show = |image: &Image, mode, c: usize| {
            let options = DisplayOptions {
                mode,
                // Visibility only matters in composite mode.
                visible: vec![false, false, true],
                ..Default::default()
            };
            rgb(&render_plane_with(image, &[c], &options).unwrap())[0]
        };
        assert_eq!(show(&composite, ChannelMode::Auto, 0), [0, 0, 30]);
        assert_eq!(show(&composite, ChannelMode::Color, 1), [0, 20, 0]);
        assert_eq!(show(&composite, ChannelMode::Grayscale, 1), [20, 20, 20]);
        assert_eq!(show(&plain, ChannelMode::Auto, 1), [20, 20, 20]);
        assert_eq!(show(&plain, ChannelMode::Color, 1), [20, 20, 20]);
        assert_eq!(show(&plain, ChannelMode::Composite, 1), [0, 0, 30]);
        // A single channel has no default colour, even as a composite.
        let single = Image::from_vec(1, 1, vec![77u8]).unwrap();
        let mut options = DisplayOptions {
            mode: ChannelMode::Composite,
            ..Default::default()
        };
        let out = render_plane_with(&single, &[], &options).unwrap();
        assert_eq!(rgb(&out), [[77, 77, 77]]);
        options.visible = vec![false];
        let out = render_plane_with(&single, &[], &options).unwrap();
        assert_eq!(rgb(&out), [[0, 0, 0]]);
    }

    #[test]
    fn span_formulas() {
        // Magnifying 3 -> 6 doubles every pixel; 2 -> 5 samples at centres
        // 0.2, 0.6, 1.0, 1.4, 1.8.
        let single = |spans: Vec<(usize, usize)>| -> Vec<usize> {
            assert!(spans.iter().all(|&(a, b)| b == a + 1));
            spans.into_iter().map(|s| s.0).collect()
        };
        assert_eq!(
            single(spans(3, 6, Reduction::BoxAverage)),
            [0, 0, 1, 1, 2, 2]
        );
        assert_eq!(single(spans(2, 5, Reduction::BoxAverage)), [0, 0, 1, 1, 1]);
        // Reducing 10 -> 4 partitions the source: boundaries 0, 2, 5, 7, 10.
        assert_eq!(
            spans(10, 4, Reduction::BoxAverage),
            [(0, 2), (2, 5), (5, 7), (7, 10)]
        );
        // Nearest takes the pixel under each centre: 1.25, 3.75, 6.25, 8.75.
        assert_eq!(single(spans(10, 4, Reduction::Nearest)), [1, 3, 6, 8]);
    }

    #[test]
    fn region_and_scaling() {
        let image = Image::from_vec(4, 2, vec![0u8, 10, 20, 30, 40, 50, 60, 70]).unwrap();
        let options = DisplayOptions::default();
        let gray = |picture: &Rgba8| -> Vec<u8> { rgb(picture).iter().map(|p| p[0]).collect() };

        let out = render_region(&image, &[], Rect::new(1, 0, 2, 2), 2, 2, &options).unwrap();
        assert_eq!(gray(&out), [10, 20, 50, 60]);
        // Twice the size: each source pixel becomes a 2x2 block.
        let out = render_region(&image, &[], Rect::new(1, 0, 2, 1), 4, 2, &options).unwrap();
        assert_eq!((out.width, out.height), (4, 2));
        assert_eq!(gray(&out), [10, 10, 20, 20, 10, 10, 20, 20]);
        // Half the size: means of 2x2 blocks, (0+10+40+50)/4 and (20+30+60+70)/4.
        let out = render_region(&image, &[], Rect::new(0, 0, 4, 2), 2, 1, &options).unwrap();
        assert_eq!(gray(&out), [25, 45]);
        // Three pixels into one: (0 + 10 + 20) / 3.
        let out = render_region(&image, &[], Rect::new(0, 0, 3, 1), 1, 1, &options).unwrap();
        assert_eq!(gray(&out), [10]);
        let nearest = DisplayOptions {
            reduction: Reduction::Nearest,
            ..Default::default()
        };
        let out = render_region(&image, &[], Rect::new(0, 0, 4, 2), 2, 1, &nearest).unwrap();
        assert_eq!(gray(&out), [50, 70]);
        // One axis can grow while the other shrinks. Means round to nearest:
        // (0 + 10) / 2 = 5.
        let out = render_region(&image, &[], Rect::new(0, 0, 4, 1), 2, 2, &options).unwrap();
        assert_eq!(gray(&out), [5, 25, 5, 25]);

        assert!(render_region(&image, &[], Rect::new(3, 0, 2, 1), 2, 1, &options).is_err());
        assert!(render_region(&image, &[], Rect::new(0, 0, 0, 1), 2, 1, &options).is_err());
        assert!(render_region(&image, &[], Rect::new(0, 0, 1, 1), 0, 1, &options).is_err());
    }

    #[test]
    fn reduction_averages_colours_not_samples() {
        // Two samples that display as black and white average to mid grey,
        // although the mean sample would display as white.
        let image = Image::from_vec(2, 1, vec![0u16, 60000]).unwrap();
        let mut meta = image.meta().clone();
        meta.channels = vec![channel(None, Some((0.0, 100.0)))];
        let image = image.with_meta(meta).unwrap();
        let out = thumbnail(&image, 1).unwrap();
        assert_eq!(out.data, [128, 128, 128, 255]);
    }

    #[test]
    fn rendering_in_strips_matches_one_pass() {
        // Taller than one strip at this width, so several strips are read.
        let (width, height) = (STRIP_SAMPLES / 4, 9);
        let samples: Vec<u8> = (0..width * height)
            .map(|i| (i * 7 + i / width) as u8)
            .collect();
        let image = Image::from_vec(width, height, samples.clone()).unwrap();
        let options = DisplayOptions::default();
        let region = Rect::new(0, 0, width, height);
        let out = render_region(&image, &[], region, 2, 3, &options).unwrap();
        let mean = |x0: usize, y0: usize| {
            let (w, h) = (width / 2, 3);
            let sum: u64 = (y0..y0 + h)
                .flat_map(|y| (x0..x0 + w).map(move |x| y * width + x))
                .map(|i| samples[i] as u64)
                .sum();
            ((sum + (w * h) as u64 / 2) / (w * h) as u64) as u8
        };
        let expected: Vec<u8> = (0..3)
            .flat_map(|oy| (0..2).map(move |ox| (ox, oy)))
            .map(|(ox, oy)| mean(ox * width / 2, oy * 3))
            .collect();
        let got: Vec<u8> = rgb(&out).iter().map(|p| p[0]).collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn thumbnail_sizes() {
        let image = Image::from_vec(40, 10, vec![9u8; 400]).unwrap();
        let size = |max_side| {
            let out = thumbnail(&image, max_side).unwrap();
            assert!(rgb(&out).iter().all(|&p| p == [9, 9, 9]));
            (out.width, out.height)
        };
        assert_eq!(size(20), (20, 5));
        // 10 * 3 / 40 = 0.75 rounds to 1; 10 * 1 / 40 would be 0 and is kept at 1.
        assert_eq!(size(3), (3, 1));
        assert_eq!(size(1), (1, 1));
        // Never enlarged.
        assert_eq!(size(40), (40, 10));
        assert_eq!(size(500), (40, 10));
        assert!(thumbnail(&image, 0).is_err());

        // The first plane of a stack; `thumbnail_at` reaches the others.
        let meta = ImageMeta::xyczt(DType::U8, 1, 1, 1, 2, 1).unwrap();
        let planes = vec![Buffer::from_vec(vec![5u8]), Buffer::from_vec(vec![200u8])];
        let stack = Image::from_planes(meta, planes).unwrap();
        assert_eq!(thumbnail(&stack, 8).unwrap().data, [5, 5, 5, 255]);
        let out = thumbnail_at(&stack, &[1], 8, &DisplayOptions::default()).unwrap();
        assert_eq!(out.data, [200, 200, 200, 255]);
    }
}
