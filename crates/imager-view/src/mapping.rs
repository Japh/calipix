//! How samples become colours. Everything the viewer knows about the
//! mapping from sample values to display levels and colours is in this
//! file, so that it can be checked against `imager-render` in one place.
//!
//! The definition is `imager_render`: a sample maps to a display level
//! 0..=255 with [`imager_render::to_levels`], and the level indexes the
//! channel's 256-entry lookup table. Several channels are added together
//! per colour component and clamped to 255.
//!
//! # Integer planes (u8, u16, i16) on the GPU
//!
//! ImageJ's rule for integers (`ShortProcessor.create8BitImage`) is
//!
//! ```text
//! level = min((int)((v - min) * (256.0 / (max - min + 1)) + 0.5), 255),  v - min clamped at 0
//! ```
//!
//! evaluated in double precision. The shader has neither doubles nor 64-bit
//! integers, and the rule cannot be reproduced with exact integer
//! arithmetic either: where `(v - min) * 256 / (max - min + 1) + 0.5` is a
//! whole number the double-precision product sometimes falls just short.
//! For example with the range 0..=25087 the sample 147 maps to level 1,
//! although `147 * 256 / 25088 + 0.5` is exactly 2.
//!
//! So the shader does no arithmetic on integer samples at all. For each
//! channel the viewer evaluates `to_levels` once for every value the pixel
//! type can hold (256 or 65536 values) and uploads the result as a table;
//! the shader looks the sample up in it:
//!
//! ```text
//! index = v                 (u8, u16)
//! index = v + 32768         (i16)
//! level = table[channel][index]
//! ```
//!
//! This is identical to the CPU renderer by construction, for any display
//! range, and stays identical if `to_levels` changes. The table is rebuilt
//! only when a display range changes.
//!
//! # Float planes (f32) on the GPU
//!
//! ImageJ's rule (`FloatProcessor.create8BitImage`) is evaluated directly,
//! in single precision:
//!
//! ```text
//! t = ((v - min_hi) - min_lo) * (255 / (max - min)) + 0.5
//! level = 0 if t < 1 or t is NaN, else min((int)t, 255)
//! ```
//!
//! where `min_hi` is `min` rounded to single precision and `min_lo` the
//! remainder, so that a minimum that is not a single-precision number (a
//! range typed by the user, say) is still subtracted accurately.
//! `to_levels` evaluates the same expression in double precision, so the
//! two can differ by one level for samples that land within rounding error
//! of a level boundary.

use imager_core::{
    AxisKind, Buffer, ChannelMeta, ColorModel, DType, Error, Image, ImageMeta, Lut, Result,
};
use imager_render::{Rgba8, render_plane, to_levels};

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
/// channels, each through its range and lookup table, added together.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayPlan {
    pub dtype: DType,
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

/// The channels to draw for `position`, following
/// `imager_render::render_plane`:
///
/// - RGB: the three channels through red, green and blue ramps, each with
///   its display range or else the full range of the pixel type (0..1 for
///   floats).
/// - Composite: every channel marked in `visible`, through its lookup
///   table (grays if it has none).
/// - Grayscale: the single plane at `position`.
///
/// A channel without a display range uses the full range for 8-bit data
/// and otherwise the smallest and largest sample of its plane, reported
/// here as `None`.
pub fn channel_requests(
    meta: &ImageMeta,
    position: &[usize],
    visible: &[bool],
) -> Result<Vec<ChannelRequest>> {
    let channel_axis = meta.axis_index(&AxisKind::Channel);
    let plane_of = |c: usize| -> Result<usize> {
        let mut p = position.to_vec();
        if let Some(axis) = channel_axis {
            *p.get_mut(axis - 2)
                .ok_or_else(|| Error::Shape("position is too short".to_string()))? = c;
        }
        meta.plane_index(&p)
    };
    let stored = |c: usize| meta.channels.get(c).cloned().unwrap_or_default();
    let auto = |range: Option<(f64, f64)>| match (range, meta.dtype()) {
        (None, DType::U8) => Some((0.0, 255.0)),
        (range, _) => range,
    };
    match meta.color {
        ColorModel::Rgb => {
            let full = meta.dtype().integer_range().unwrap_or((0.0, 1.0));
            (0..3)
                .map(|c| {
                    Ok(ChannelRequest {
                        plane: plane_of(c)?,
                        channel: c,
                        range: Some(stored(c).display_range.unwrap_or(full)),
                        lut: primary(c),
                    })
                })
                .collect()
        }
        ColorModel::Composite => (0..meta.size(&AxisKind::Channel))
            .filter(|&c| visible.get(c).copied().unwrap_or(true))
            .map(|c| {
                let ChannelMeta {
                    lut, display_range, ..
                } = stored(c);
                Ok(ChannelRequest {
                    plane: plane_of(c)?,
                    channel: c,
                    range: auto(display_range),
                    lut: lut.unwrap_or_else(Lut::grays),
                })
            })
            .collect(),
        // `ColorModel` is non-exhaustive; anything unknown is shown one
        // channel at a time.
        _ => {
            let plane = meta.plane_index(position)?;
            let channel = meta.plane_channel(plane)?;
            let ChannelMeta {
                lut, display_range, ..
            } = stored(channel);
            Ok(vec![ChannelRequest {
                plane,
                channel,
                range: auto(display_range),
                lut: lut.unwrap_or_else(Lut::grays),
            }])
        }
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
/// computed by `imager_render::to_levels` itself.
pub fn level_table(dtype: DType, range: (f64, f64)) -> Result<Vec<u8>> {
    let all = match dtype {
        DType::U8 => Buffer::from_vec((0..=u8::MAX).collect()),
        DType::U16 => Buffer::from_vec((0..=u16::MAX).collect()),
        DType::I16 => Buffer::from_vec((i16::MIN..=i16::MAX).collect()),
        other => {
            return Err(Error::Unsupported(format!(
                "no level table for {other} samples"
            )));
        }
    };
    to_levels(&all, range)
}

/// Constants of the shader's float rule for one channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatParams {
    /// The range minimum rounded to single precision.
    pub min_hi: f32,
    /// What rounding left over: `min - min_hi`.
    pub min_lo: f32,
    /// `255 / (max - min)`.
    pub scale: f32,
}

/// The constants for a display range, or `None` if the range cannot be
/// drawn on the GPU (not finite, or not `max > min`; the CPU renderer
/// defines what those show).
pub fn float_params(range: (f64, f64)) -> Option<FloatParams> {
    let (min, max) = range;
    let scale = (255.0 / (max - min)) as f32;
    let min_hi = min as f32;
    (min_hi.is_finite() && max > min && scale.is_finite() && scale > 0.0).then(|| FloatParams {
        min_hi,
        min_lo: (min - min_hi as f64) as f32,
        scale,
    })
}

/// The shader's float rule on the CPU, for tests and documentation.
pub fn float_level(v: f32, p: FloatParams) -> u8 {
    let t = ((v - p.min_hi) - p.min_lo) * p.scale + 0.5;
    if t >= 1.0 { t.min(255.0) as u8 } else { 0 }
}

/// True if the shader can draw `plan` exactly as the CPU renderer would
/// (within one level for floats).
pub fn gpu_can_draw(plan: &DisplayPlan) -> bool {
    match SampleKind::of(plan.dtype) {
        None => false,
        Some(_) if plan.channels.len() > MAX_GPU_CHANNELS => false,
        Some(SampleKind::Float) => plan
            .channels
            .iter()
            .all(|c| float_params(c.range).is_some()),
        Some(_) => true,
    }
}

/// WGSL for `fn level(c: u32, p: vec2<i32>, ...)`-style sample mapping:
/// the body that turns the raw texel `raw` of channel `c` into a level.
/// `levels` is the table texture, `params.table_rows` the rows per channel
/// and `params.chan[c].xyz` the float `(min_hi, min_lo, scale)`.
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
    let t = ((raw - params.chan[c].x) - params.chan[c].y) * params.chan[c].z + 0.5;
    // NaN fails the comparison and shows level 0, as on the CPU.
    if (t >= 1.0) {
        return u32(min(t, 255.0));
    }
    return 0u;
}"
        }
    }
}

/// Renders a tile on the CPU with `imager_render::render_plane`: each
/// channel's samples (`width` x `height`, in plan order) through its range
/// and lookup table, added and clamped. This is what the viewer shows when
/// the GPU path cannot be used.
pub fn render_tile_cpu(
    plan: &DisplayPlan,
    tiles: &[Buffer],
    width: usize,
    height: usize,
) -> Result<Rgba8> {
    let mut out = Rgba8 {
        width,
        height,
        data: [0, 0, 0, 255].repeat(width * height),
    };
    for (channel, samples) in plan.channels.iter().zip(tiles) {
        let mut meta = ImageMeta::plane(plan.dtype, width, height)?;
        meta.channels = vec![ChannelMeta {
            name: None,
            lut: Some(channel.lut.clone()),
            display_range: Some(channel.range),
        }];
        let image = Image::from_planes(meta, vec![samples.clone()])?;
        let layer = render_plane(&image, &[])?;
        for (sum, add) in out.data.chunks_exact_mut(4).zip(layer.data.chunks_exact(4)) {
            for c in 0..3 {
                sum[c] = sum[c].saturating_add(add[c]);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn table_is_to_levels_for_every_value() {
        let t = level_table(DType::U8, (0.0, 255.0)).unwrap();
        assert!(t.iter().enumerate().all(|(i, &l)| l as usize == i));
        let t = level_table(DType::U16, (100.0, 355.0)).unwrap();
        assert_eq!(t.len(), 65536);
        assert_eq!(
            [t[99], t[100], t[101], t[227], t[228], t[355], t[400]],
            [0, 0, 1, 127, 128, 255, 255]
        );
        // Signed samples are indexed from the type's minimum.
        let t = level_table(DType::I16, (-10.0, 245.0)).unwrap();
        assert_eq!(t[(-11i32 + 32768) as usize], 0);
        assert_eq!(t[(-9i32 + 32768) as usize], 1);
        assert_eq!(t[(245 + 32768) as usize], 255);
        assert!(level_table(DType::F32, (0.0, 1.0)).is_err());
    }

    /// The reason the integer path is a table rather than arithmetic.
    #[test]
    fn exact_integer_arithmetic_would_not_match_imagej() {
        let level = to_levels(&Buffer::from_vec(vec![147u16]), (0.0, 25087.0)).unwrap()[0];
        let exact = (147u32 * 512 + 25088) / (2 * 25088);
        assert_eq!((level, exact), (1, 2));
        assert_eq!(level_table(DType::U16, (0.0, 25087.0)).unwrap()[147], 1);
    }

    #[test]
    fn float_rule_edge_cases() {
        let p = float_params((0.0, 1.0)).unwrap();
        let levels: Vec<u8> = [
            -1.0f32,
            0.0,
            0.5,
            1.0,
            2.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ]
        .iter()
        .map(|&v| float_level(v, p))
        .collect();
        assert_eq!(levels, [0, 0, 128, 255, 255, 0, 255, 0]);
        // Ranges the CPU renderer must decide.
        assert!(float_params((3.0, 3.0)).is_none());
        assert!(float_params((2.0, 1.0)).is_none());
        assert!(float_params((f64::NAN, 1.0)).is_none());
        assert!(float_params((0.0, f64::INFINITY)).is_none());
    }

    #[test]
    fn requests_follow_the_colour_model() {
        let mut meta = ImageMeta::xyczt(DType::U16, 4, 4, 3, 2, 1).unwrap();
        meta.channels = (0..3)
            .map(|c| ChannelMeta {
                name: None,
                lut: Some(primary(c)),
                display_range: (c == 1).then_some((5.0, 50.0)),
            })
            .collect();
        // Grayscale: only the plane at the position.
        let r = channel_requests(&meta, &[2, 1], &[]).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].plane, r[0].channel, r[0].range), (5, 2, None));
        assert_eq!(r[0].lut.name, "Blue");
        // Composite: visible channels at the position's Z.
        meta.color = ColorModel::Composite;
        let r = channel_requests(&meta, &[2, 1], &[true, true, false]).unwrap();
        assert_eq!(
            r.iter()
                .map(|c| (c.plane, c.channel, c.range))
                .collect::<Vec<_>>(),
            [(3, 0, None), (4, 1, Some((5.0, 50.0)))]
        );
        assert!(channel_requests(&meta, &[0], &[]).is_err());

        // RGB: three channels, full range by default, whatever the position.
        let mut rgb = ImageMeta::xyczt(DType::U8, 4, 4, 3, 1, 1).unwrap();
        rgb.color = ColorModel::Rgb;
        let r = channel_requests(&rgb, &[1], &[false; 3]).unwrap();
        assert_eq!(r.len(), 3);
        assert!(r.iter().all(|c| c.range == Some((0.0, 255.0))));
        assert_eq!(r[1].lut.table[200], [0, 200, 0]);

        // 8-bit data defaults to the full range without a scan.
        let gray = ImageMeta::plane(DType::U8, 4, 4).unwrap();
        let r = channel_requests(&gray, &[], &[]).unwrap();
        assert_eq!(r[0].range, Some((0.0, 255.0)));
    }

    #[test]
    fn gpu_eligibility() {
        let channel = |range| ChannelDisplay {
            plane: 0,
            channel: 0,
            range,
            lut: Lut::grays(),
        };
        let plan = |dtype, n: usize, range| DisplayPlan {
            dtype,
            channels: vec![channel(range); n],
        };
        assert!(gpu_can_draw(&plan(DType::U16, 7, (0.0, 1.0))));
        assert!(!gpu_can_draw(&plan(DType::U16, 8, (0.0, 1.0))));
        assert!(!gpu_can_draw(&plan(DType::U32, 1, (0.0, 1.0))));
        assert!(!gpu_can_draw(&plan(DType::F64, 1, (0.0, 1.0))));
        assert!(gpu_can_draw(&plan(DType::F32, 1, (0.0, 1.0))));
        assert!(!gpu_can_draw(&plan(DType::F32, 1, (1.0, 1.0))));
        // Integer tables reproduce even degenerate ranges.
        assert!(gpu_can_draw(&plan(DType::I16, 1, (5.0, -5.0))));
    }

    #[test]
    fn cpu_tile_is_render_plane_summed() {
        let plan = DisplayPlan {
            dtype: DType::U8,
            channels: vec![
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
            ],
        };
        let tiles = [
            Buffer::from_vec(vec![10u8, 200]),
            Buffer::from_vec(vec![5u8, 100]),
        ];
        let out = render_tile_cpu(&plan, &tiles, 2, 1).unwrap();
        assert_eq!(out.data, [15, 5, 5, 255, 255, 100, 100, 255]);
        // No channels: black, opaque.
        let none = DisplayPlan {
            dtype: DType::U8,
            channels: vec![],
        };
        assert_eq!(
            render_tile_cpu(&none, &[], 1, 1).unwrap().data,
            [0, 0, 0, 255]
        );
    }

    proptest! {
        /// The shader's float rule stays within one level of `to_levels`.
        #[test]
        fn float_rule_is_within_one_level(
            min in -1e4f64..1e4, width in 1e-3f64..1e5,
            values in prop::collection::vec(-2e5f32..2e5, 1..200),
        ) {
            let range = (min, min + width);
            let p = float_params(range).unwrap();
            let reference = to_levels(&Buffer::from_vec(values.clone()), range).unwrap();
            for (v, want) in values.iter().zip(reference) {
                let got = float_level(*v, p);
                prop_assert!(got.abs_diff(want) <= 1, "{v} in {range:?}: {got} vs {want}");
            }
        }
    }
}
