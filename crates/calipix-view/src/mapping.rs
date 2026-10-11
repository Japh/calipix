//! How samples become colours. Everything the viewer knows about the
//! mapping from sample values to display levels and colours is in this
//! file, so that it can be checked against `calipix-render` in one place.
//!
//! The definition is `calipix-render`: a sample maps to a display level
//! 0..=255 by [`calipix_render::LevelMap`], the level indexes the channel's
//! 256-entry lookup table ([`calipix_render::channel_lut`]), and in
//! composite mode the colours of the visible channels are combined
//! ([`calipix_render::Blend`]). Which channels are drawn, with which range
//! and table, follows `calipix_render::render_plane_with`; see
//! [`channel_requests`].
//!
//! # Integer planes (u8, u16, i16) on the GPU
//!
//! The level rules for integers (`LevelRule::Byte`, `Integer`, `RgbByte`)
//! are evaluated in double precision and cannot be reproduced in a shader,
//! which has neither doubles nor 64-bit integers. Exact integer arithmetic
//! does not reproduce them either: for the 16-bit rule
//!
//! ```text
//! level = min((int)(max(v - min, 0) * (256.0 / (max - min + 1)) + 0.5), 255)
//! ```
//!
//! with the range 0..=25087 the sample 147 gets level 1, although
//! `147 * 256 / 25088 + 0.5` is exactly 2: the double-precision product
//! falls just short.
//!
//! So the shader does no arithmetic on integer samples. For each channel
//! the viewer asks `LevelMap::level` for the level of every value the pixel
//! type can hold (256 or 65536 values) and uploads the answers as a table,
//! which the shader indexes:
//!
//! ```text
//! index = v                 (u8, u16)
//! index = v + 32768         (i16)
//! level = table[channel][index]
//! ```
//!
//! This is identical to the CPU renderer by construction, whatever the
//! rule and the display range. The table is rebuilt only when a display
//! range changes.
//!
//! # Float planes (f32) on the GPU
//!
//! `LevelRule::Float` is evaluated directly, in single precision:
//!
//! ```text
//! t = (v - min) * scale + 0.5        scale = 255 / (max - min)
//! level = 0 if t < 1 or t is NaN, else min((int)t, 255)
//! ```
//!
//! `min` and `max` are the range rounded to single precision, exactly as
//! `LevelMap` rounds it for 32-bit samples (`LevelMap::range`), and `scale`
//! is computed in double precision and then rounded. `LevelMap` evaluates
//! the same expression in double precision, so the two can differ by one
//! level for samples within rounding error of a level boundary. Ranges that
//! are not finite with `max > min` are left to the CPU renderer.
//!
//! # Channels on the GPU
//!
//! Up to [`MAX_GPU_CHANNELS`] channels are added per colour component and
//! clamped to 255 (`Blend::Sum`). An RGB image is three channels through
//! red, green and blue ramps, which is the same thing as taking the three
//! levels as the colour. `Blend::Max` and `Blend::Min` with more than one
//! channel are left to the CPU renderer.

use calipix_core::{
    AxisKind, Buffer, ChannelMeta, ColorModel, DType, Error, Image, ImageMeta, Lut, Result,
};
use calipix_render::{
    Blend, ChannelMode, DisplayOptions, LevelMap, LevelRule, Rgba8, channel_lut, render_plane_with,
};

/// Most channels blended by the shader. More than this are drawn by the
/// CPU renderer. ImageJ's composite images have at most 7 channels.
pub const MAX_GPU_CHANNELS: usize = 7;

/// One channel as it is to be displayed.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelDisplay {
    /// The plane holding the channel's samples at the current position.
    pub plane: usize,
    pub channel: usize,
    /// Sample values mapped to the darkest and brightest levels.
    pub range: (f64, f64),
    pub lut: Lut,
}

/// How the planes at one position are turned into a picture: the listed
/// channels, each through its range and lookup table, combined by `blend`.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayPlan {
    pub dtype: DType,
    /// The image is RGB: `channels` are its three components and their
    /// lookup tables are red, green and blue ramps.
    pub rgb: bool,
    pub blend: Blend,
    pub channels: Vec<ChannelDisplay>,
}

/// The planes a position needs and how each is displayed, before display
/// ranges that depend on the samples are known.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelRequest {
    pub plane: usize,
    pub channel: usize,
    /// `None` if the range is the plane's own smallest and largest sample.
    pub range: Option<(f64, f64)>,
    pub lut: Lut,
}

fn primary(component: usize) -> Lut {
    let name = ["Red", "Green", "Blue"][component];
    Lut::from_fn(name, move |i| {
        let mut rgb = [0; 3];
        rgb[component] = i;
        rgb
    })
}

/// The display range of `channel` if it can be known without reading
/// samples; `None` means the smallest and largest sample of the plane.
///
/// This is `calipix_render::effective_display_range` without its read: the
/// channel's range if set (for anything but unsigned 8-bit, exactly
/// `(0, 0)` counts as not set); otherwise for RGB images the full range of
/// the type (0 to 1 for floats), and 0 to 255 for unsigned 8-bit.
pub fn known_range(meta: &ImageMeta, channel: usize) -> Result<Option<(f64, f64)>> {
    let rgb = meta.color == ColorModel::Rgb;
    let rule = LevelRule::for_dtype(meta.dtype(), rgb)?;
    let auto = matches!(rule, LevelRule::Integer | LevelRule::Float);
    Ok(
        match meta.channels.get(channel).and_then(|c| c.display_range) {
            Some(range) if !(auto && range == (0.0, 0.0)) => Some(range),
            _ if rgb => Some(meta.dtype().integer_range().unwrap_or((0.0, 1.0))),
            _ if !auto => Some((0.0, 255.0)),
            _ => None,
        },
    )
}

/// The channels to draw for `position`, as
/// `calipix_render::render_plane_with` chooses them:
///
/// - RGB images: the three channels, as red, green and blue.
/// - Composite mode: every visible channel.
/// - Otherwise the single plane at `position`.
///
/// Lookup tables come from `calipix_render::channel_lut`. Only
/// `options.mode` and `options.visible` are used here.
pub fn channel_requests(
    image: &Image,
    position: &[usize],
    options: &DisplayOptions,
) -> Result<Vec<ChannelRequest>> {
    let meta = image.meta();
    let index = meta.plane_index(position)?;
    let mode = options.resolved_mode(image);
    let plane_of = |c: usize| -> Result<usize> {
        match meta.axis_index(&AxisKind::Channel) {
            Some(axis) => {
                let mut p = position.to_vec();
                p[axis - 2] = c;
                meta.plane_index(&p)
            }
            None => Ok(index),
        }
    };
    let request = |c: usize, lut: Lut| -> Result<ChannelRequest> {
        Ok(ChannelRequest {
            plane: plane_of(c)?,
            channel: c,
            range: known_range(meta, c)?,
            lut,
        })
    };
    if meta.color == ColorModel::Rgb {
        (0..3).map(|c| request(c, primary(c))).collect()
    } else if mode == ChannelMode::Composite {
        (0..meta.size(&AxisKind::Channel))
            .filter(|&c| options.is_visible(c))
            .map(|c| request(c, channel_lut(image, c, mode)))
            .collect()
    } else {
        let c = meta.plane_channel(index)?;
        Ok(vec![request(c, channel_lut(image, c, mode))?])
    }
}

/// How the shader reads samples of a pixel type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SampleKind {
    /// `R8Uint` or `R16Uint`, looked up in the level table.
    Uint,
    /// `R16Sint`, looked up in the level table at `v + 32768`.
    Sint,
    /// `R32Float`, mapped arithmetically.
    Float,
}

impl SampleKind {
    /// The kind for a pixel type, or `None` if the type has no GPU path.
    pub fn of(dtype: DType) -> Option<SampleKind> {
        match dtype {
            DType::U8 | DType::U16 => Some(SampleKind::Uint),
            DType::I16 => Some(SampleKind::Sint),
            DType::F32 => Some(SampleKind::Float),
            _ => None,
        }
    }
}

/// Rows of 256 entries in one channel's level table: 1 for 8-bit samples,
/// 256 for 16-bit, 0 where no table is used.
pub fn table_rows(dtype: DType) -> u32 {
    match dtype {
        DType::U8 => 1,
        DType::U16 | DType::I16 => 256,
        _ => 0,
    }
}

/// The display level of every value an integer pixel type can hold, in
/// the order the shader indexes them (value minus the type's minimum),
/// each computed by `calipix_render::LevelMap::level`. `rgb` selects the
/// rule for components of an RGB image.
pub fn level_table(dtype: DType, rgb: bool, range: (f64, f64)) -> Result<Vec<u8>> {
    let map = LevelMap::new(dtype, rgb, range)?;
    let values = match dtype {
        DType::U8 => 0..=u8::MAX as i32,
        DType::U16 => 0..=u16::MAX as i32,
        DType::I16 => i16::MIN as i32..=i16::MAX as i32,
        other => {
            return Err(Error::Unsupported(format!(
                "no level table for {other} samples"
            )));
        }
    };
    Ok(values.map(|v| map.level(v as f64)).collect())
}

/// Constants of the shader's float rule for one channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatParams {
    /// The range minimum, rounded to single precision.
    pub min: f32,
    /// `255 / (max - min)` with both rounded to single precision first.
    pub scale: f32,
}

/// The constants for a display range of 32-bit float samples, or `None` if
/// the range is left to the CPU renderer (not finite, or not `max > min`).
pub fn float_params(range: (f64, f64)) -> Option<FloatParams> {
    // The range as LevelMap uses it for f32 data.
    let (min, max) = LevelMap::new(DType::F32, false, range).ok()?.range();
    let scale = (255.0 / (max - min)) as f32;
    let params = FloatParams {
        min: min as f32,
        scale,
    };
    (min.is_finite() && max > min && scale.is_finite() && scale > 0.0).then_some(params)
}

/// The shader's float rule on the CPU, for tests and documentation.
pub fn float_level(v: f32, p: FloatParams) -> u8 {
    let t = (v - p.min) * p.scale + 0.5;
    if t >= 1.0 { t.min(255.0) as u8 } else { 0 }
}

/// True if the shader can draw `plan` exactly as the CPU renderer would
/// (within one level for floats).
pub fn gpu_can_draw(plan: &DisplayPlan) -> bool {
    let Some(kind) = SampleKind::of(plan.dtype) else {
        return false;
    };
    let blend_ok = plan.rgb || plan.blend == Blend::Sum || plan.channels.len() == 1;
    let ranges_ok = kind != SampleKind::Float
        || plan
            .channels
            .iter()
            .all(|c| float_params(c.range).is_some());
    plan.channels.len() <= MAX_GPU_CHANNELS && blend_ok && ranges_ok
}

/// WGSL for `fn level(c, raw)`: turns the raw texel `raw` of channel `c`
/// into a level. `levels` is the table texture, `params.table_rows` the
/// rows per channel and `params.chan[c].xy` the float `(min, scale)`.
pub(crate) fn level_wgsl(kind: SampleKind) -> &'static str {
    match kind {
        SampleKind::Uint => {
            "fn level(c: u32, raw: u32) -> u32 {
    let at = vec2<i32>(i32(raw & 255u), i32(c * params.table_rows + (raw >> 8u)));
    return textureLoad(levels, at, 0).r;
}"
        }
        SampleKind::Sint => {
            "fn level(c: u32, raw: i32) -> u32 {
    let index = u32(raw + 32768);
    let at = vec2<i32>(i32(index & 255u), i32(c * params.table_rows + (index >> 8u)));
    return textureLoad(levels, at, 0).r;
}"
        }
        SampleKind::Float => {
            "fn level(c: u32, raw: f32) -> u32 {
    // NaN shows level 0, as on the CPU. It is recognised by its bits (all
    // exponent bits set, fraction not zero) because not every backend
    // compares NaN as IEEE 754 requires.
    let bits = bitcast<u32>(raw);
    if ((bits & 0x7f800000u) == 0x7f800000u && (bits & 0x007fffffu) != 0u) {
        return 0u;
    }
    let t = (raw - params.chan[c].x) * params.chan[c].y + 0.5;
    if (t >= 1.0) {
        return u32(min(t, 255.0));
    }
    return 0u;
}"
        }
    }
}

/// Renders a tile on the CPU with `calipix_render::render_plane_with`: the
/// channels' samples (`width` x `height`, in plan order) are presented to
/// it as a small image carrying the plan's ranges and lookup tables. This
/// is what the viewer shows when the shader cannot be used.
pub fn render_tile_cpu(
    plan: &DisplayPlan,
    tiles: &[Buffer],
    width: usize,
    height: usize,
) -> Result<Rgba8> {
    let n = plan.channels.len();
    if n == 0 || tiles.len() != n {
        // Nothing visible: what an empty blend leaves.
        let fill = if !plan.rgb && plan.blend == Blend::Min {
            255
        } else {
            0
        };
        return Ok(Rgba8 {
            width,
            height,
            data: [fill, fill, fill, 255].repeat(width * height),
        });
    }
    let mut meta = ImageMeta::xyczt(plan.dtype, width, height, n, 1, 1)?;
    meta.color = if plan.rgb {
        ColorModel::Rgb
    } else {
        ColorModel::Composite
    };
    meta.channels = plan
        .channels
        .iter()
        .map(|channel| ChannelMeta {
            name: None,
            lut: (!plan.rgb).then(|| channel.lut.clone()),
            display_range: Some(channel.range),
        })
        .collect();
    let image = Image::from_planes(meta, tiles.to_vec())?;
    let options = DisplayOptions {
        mode: ChannelMode::Composite,
        blend: plan.blend,
        ..DisplayOptions::default()
    };
    let position = vec![0; image.meta().plane_shape().len()];
    render_plane_with(&image, &position, &options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use calipix_render::{effective_display_range, to_levels};
    use proptest::prelude::*;

    #[test]
    fn table_is_the_level_map_for_every_value() {
        let t = level_table(DType::U8, false, (0.0, 255.0)).unwrap();
        assert!(t.iter().enumerate().all(|(i, &l)| l as usize == i));
        // The 8-bit rule, not the 16-bit one: (int)(256 * (v - 50) / 150).
        let t = level_table(DType::U8, false, (50.0, 200.0)).unwrap();
        assert_eq!(
            [t[49], t[51], t[125], t[199], t[200]],
            [0, 1, 128, 254, 255]
        );
        // RGB components truncate the minimum.
        let t = level_table(DType::U8, true, (40.7, 180.2)).unwrap();
        assert_eq!([t[40], t[41]], [0, 1]);
        let t = level_table(DType::U16, false, (100.0, 355.0)).unwrap();
        assert_eq!(t.len(), 65536);
        assert_eq!(
            [t[99], t[100], t[101], t[227], t[228], t[355], t[400]],
            [0, 0, 1, 127, 128, 255, 255]
        );
        // Signed samples are indexed from the type's minimum.
        let t = level_table(DType::I16, false, (-10.0, 245.0)).unwrap();
        assert_eq!(t[(-11i32 + 32768) as usize], 0);
        assert_eq!(t[(-9i32 + 32768) as usize], 1);
        assert_eq!(t[(245 + 32768) as usize], 255);
        assert!(level_table(DType::F32, false, (0.0, 1.0)).is_err());
        // Every entry agrees with to_levels on a plane of all values.
        for range in [(0.0, 25087.0), (99.5, 354.5), (600.0, 600.0), (5.0, 3.0)] {
            let all = Buffer::from_vec((0..=u16::MAX).collect::<Vec<u16>>());
            assert_eq!(
                level_table(DType::U16, false, range).unwrap(),
                to_levels(&all, range).unwrap()
            );
        }
    }

    /// The reason the integer path is a table rather than arithmetic.
    #[test]
    fn exact_integer_arithmetic_would_not_match_imagej() {
        let level = to_levels(&Buffer::from_vec(vec![147u16]), (0.0, 25087.0)).unwrap()[0];
        let exact = (147u32 * 512 + 25088) / (2 * 25088);
        assert_eq!((level, exact), (1, 2));
        let table = level_table(DType::U16, false, (0.0, 25087.0)).unwrap();
        assert_eq!(table[147], 1);
    }

    #[test]
    fn float_rule_edge_cases() {
        let p = float_params((0.0, 1.0)).unwrap();
        let samples = [
            -1.0f32,
            0.0,
            0.5,
            1.0,
            2.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];
        let levels: Vec<u8> = samples.iter().map(|&v| float_level(v, p)).collect();
        assert_eq!(levels, [0, 0, 128, 255, 255, 0, 255, 0]);
        assert_eq!(
            levels,
            to_levels(&Buffer::from_vec(samples.to_vec()), (0.0, 1.0)).unwrap()
        );
        // The range is rounded to single precision, as LevelMap does.
        assert_eq!(float_params((0.1, 0.7)).unwrap().min, 0.1f32);
        // Ranges the CPU renderer must decide.
        assert!(float_params((3.0, 3.0)).is_none());
        assert!(float_params((2.0, 1.0)).is_none());
        assert!(float_params((f64::NAN, 1.0)).is_none());
        assert!(float_params((0.0, f64::INFINITY)).is_none());
    }

    fn stack(dtype: DType, color: ColorModel) -> Image {
        let mut meta = ImageMeta::xyczt(dtype, 2, 2, 3, 2, 1).unwrap();
        meta.color = color;
        meta.channels = (0..3)
            .map(|c| ChannelMeta {
                name: None,
                lut: (c == 0).then(|| primary(2)),
                display_range: match c {
                    1 => Some((5.0, 50.0)),
                    2 => Some((0.0, 0.0)),
                    _ => None,
                },
            })
            .collect();
        let planes = (0..6).map(|_| Buffer::zeros(dtype, 4)).collect();
        Image::from_planes(meta, planes).unwrap()
    }

    #[test]
    fn requests_follow_the_renderer() {
        let options = DisplayOptions::default();
        // Grayscale model, automatic mode: only the plane at the position,
        // with its own table or grey.
        let image = stack(DType::U16, ColorModel::Grayscale);
        let r = channel_requests(&image, &[1, 1], &options).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(
            (r[0].plane, r[0].channel, r[0].range),
            (4, 1, Some((5.0, 50.0)))
        );
        assert_eq!(r[0].lut, Lut::grays());
        let r = channel_requests(&image, &[0, 0], &options).unwrap();
        assert_eq!((r[0].range, r[0].lut.name.as_str()), (None, "Blue"));
        // Shown as a composite on request: default channel colours.
        let composite = DisplayOptions {
            mode: ChannelMode::Composite,
            visible: vec![true, false],
            ..DisplayOptions::default()
        };
        let r = channel_requests(&image, &[1, 1], &composite).unwrap();
        assert_eq!(
            r.iter().map(|c| (c.plane, c.channel)).collect::<Vec<_>>(),
            [(3, 0), (5, 2)]
        );
        // (0, 0) on 16-bit data means the plane's own range.
        assert_eq!(r[1].range, None);
        assert_eq!(r[1].lut, channel_lut(&image, 2, ChannelMode::Composite));
        // Grayscale mode ignores tables.
        let gray = DisplayOptions {
            mode: ChannelMode::Grayscale,
            ..DisplayOptions::default()
        };
        let r = channel_requests(&image, &[0, 0], &gray).unwrap();
        assert_eq!(r[0].lut, Lut::grays());
        assert!(channel_requests(&image, &[0], &options).is_err());

        // A composite image shows all channels by default.
        let image = stack(DType::U16, ColorModel::Composite);
        let r = channel_requests(&image, &[2, 0], &options).unwrap();
        assert_eq!(r.len(), 3);

        // RGB: three channels whatever the mode; (0, 0) is a real range for
        // 8-bit data.
        let mut meta = ImageMeta::xyczt(DType::U8, 2, 2, 3, 1, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let planes = (0..3).map(|_| Buffer::zeros(DType::U8, 4)).collect();
        let rgb = Image::from_planes(meta, planes).unwrap();
        let r = channel_requests(&rgb, &[1], &gray).unwrap();
        assert_eq!(r.len(), 3);
        assert!(r.iter().all(|c| c.range == Some((0.0, 255.0))));
        assert_eq!(r[1].lut.table[200], [0, 200, 0]);
    }

    #[test]
    fn known_ranges_agree_with_the_renderer() {
        for dtype in [DType::U8, DType::U16, DType::I16, DType::F32, DType::F64] {
            for color in [ColorModel::Grayscale, ColorModel::Composite] {
                let image = stack(dtype, color);
                for plane in 0..6 {
                    let channel = image.meta().plane_channel(plane).unwrap();
                    let reference = effective_display_range(&image, plane).unwrap();
                    match known_range(image.meta(), channel).unwrap() {
                        Some(range) => assert_eq!(range, reference, "{dtype} {plane}"),
                        // All samples are zero, so the plane's range is (0, 0).
                        None => assert_eq!(reference, (0.0, 0.0), "{dtype} {plane}"),
                    }
                }
            }
        }
        let mut meta = ImageMeta::xyczt(DType::U16, 2, 2, 3, 1, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let planes = (0..3).map(|_| Buffer::zeros(DType::U16, 4)).collect();
        let rgb = Image::from_planes(meta, planes).unwrap();
        assert_eq!(known_range(rgb.meta(), 0).unwrap(), Some((0.0, 65535.0)));
        assert_eq!(effective_display_range(&rgb, 0).unwrap(), (0.0, 65535.0));
        let complex = ImageMeta::plane(DType::C32, 2, 2).unwrap();
        assert!(known_range(&complex, 0).is_err());
    }

    #[test]
    fn gpu_eligibility() {
        let channel = |range| ChannelDisplay {
            plane: 0,
            channel: 0,
            range,
            lut: Lut::grays(),
        };
        let plan = |dtype, n: usize, range, blend| DisplayPlan {
            dtype,
            rgb: false,
            blend,
            channels: vec![channel(range); n],
        };
        let sum = Blend::Sum;
        assert!(gpu_can_draw(&plan(DType::U16, 7, (0.0, 1.0), sum)));
        assert!(!gpu_can_draw(&plan(DType::U16, 8, (0.0, 1.0), sum)));
        assert!(!gpu_can_draw(&plan(DType::U32, 1, (0.0, 1.0), sum)));
        assert!(!gpu_can_draw(&plan(DType::F64, 1, (0.0, 1.0), sum)));
        assert!(gpu_can_draw(&plan(DType::F32, 1, (0.0, 1.0), sum)));
        assert!(!gpu_can_draw(&plan(DType::F32, 1, (1.0, 1.0), sum)));
        // Integer tables reproduce even degenerate ranges.
        assert!(gpu_can_draw(&plan(DType::I16, 1, (5.0, -5.0), sum)));
        // Only sums are blended in the shader.
        assert!(!gpu_can_draw(&plan(DType::U16, 2, (0.0, 1.0), Blend::Max)));
        assert!(!gpu_can_draw(&plan(DType::U16, 0, (0.0, 1.0), Blend::Min)));
        assert!(gpu_can_draw(&plan(DType::U16, 1, (0.0, 1.0), Blend::Min)));
        assert!(gpu_can_draw(&plan(DType::U16, 0, (0.0, 1.0), sum)));
    }

    #[test]
    fn cpu_tile_is_the_renderer_on_the_tile() {
        let channels = vec![
            ChannelDisplay {
                plane: 0,
                channel: 0,
                range: (0.0, 255.0),
                lut: primary(0),
            },
            ChannelDisplay {
                plane: 1,
                channel: 1,
                range: (0.0, 255.0),
                lut: Lut::grays(),
            },
        ];
        let plan = |blend| DisplayPlan {
            dtype: DType::U8,
            rgb: false,
            blend,
            channels: channels.clone(),
        };
        let tiles = [
            Buffer::from_vec(vec![10u8, 200]),
            Buffer::from_vec(vec![5u8, 100]),
        ];
        let out = render_tile_cpu(&plan(Blend::Sum), &tiles, 2, 1).unwrap();
        assert_eq!(out.data, [15, 5, 5, 255, 255, 100, 100, 255]);
        let out = render_tile_cpu(&plan(Blend::Max), &tiles, 2, 1).unwrap();
        assert_eq!(out.data, [10, 5, 5, 255, 200, 100, 100, 255]);
        let out = render_tile_cpu(&plan(Blend::Min), &tiles, 2, 1).unwrap();
        assert_eq!(out.data, [5, 0, 0, 255, 100, 0, 0, 255]);
        // One channel, no channel axis.
        let single = DisplayPlan {
            channels: channels[..1].to_vec(),
            ..plan(Blend::Sum)
        };
        let out = render_tile_cpu(&single, &tiles[..1], 2, 1).unwrap();
        assert_eq!(out.data, [10, 0, 0, 255, 200, 0, 0, 255]);
        // No channels: black, or white for a minimum.
        let none = |blend| DisplayPlan {
            channels: vec![],
            ..plan(blend)
        };
        let out = render_tile_cpu(&none(Blend::Sum), &[], 1, 1).unwrap();
        assert_eq!(out.data, [0, 0, 0, 255]);
        let out = render_tile_cpu(&none(Blend::Min), &[], 1, 1).unwrap();
        assert_eq!(out.data, [255, 255, 255, 255]);
        // RGB: the levels are the colour, by the RGB rule.
        let rgb = DisplayPlan {
            dtype: DType::U8,
            rgb: true,
            blend: Blend::Sum,
            channels: (0..3)
                .map(|c| ChannelDisplay {
                    plane: c,
                    channel: c,
                    range: (40.7, 180.2),
                    lut: primary(c),
                })
                .collect(),
        };
        let tiles = [40u8, 41, 200].map(|v| Buffer::from_vec(vec![v]));
        let out = render_tile_cpu(&rgb, &tiles, 1, 1).unwrap();
        assert_eq!(out.data, [0, 1, 255, 255]);
    }

    proptest! {
        /// The shader's float rule stays within one level of the renderer.
        #[test]
        fn float_rule_is_within_one_level(
            min in -1e4f64..1e4, width in 1e-2f64..1e5,
            values in prop::collection::vec(-2e5f32..2e5, 1..200),
        ) {
            let range = (min, min + width);
            if let Some(p) = float_params(range) {
                let reference = to_levels(&Buffer::from_vec(values.clone()), range).unwrap();
                for (v, want) in values.iter().zip(reference) {
                    let got = float_level(*v, p);
                    prop_assert!(got.abs_diff(want) <= 1, "{v} in {range:?}: {got} vs {want}");
                }
            }
        }
    }
}
