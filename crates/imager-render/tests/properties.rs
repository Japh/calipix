//! Laws the renderer must obey whatever the image.

use imager_core::{Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Rect};
use imager_render::{
    Blend, ChannelMode, DisplayOptions, HistogramOptions, LevelMap, Reduction, Rgba8,
    buffer_histogram, builtin_luts, effective_display_range, plane_histogram, render_plane_with,
    render_region, stack_histogram, to_levels,
};
use proptest::prelude::*;

/// A plane of `len` samples of the pixel type numbered `kind`, derived from
/// `seeds` (cycled). Floats include negative values but no NaN.
fn plane(kind: u8, seeds: &[u16], offset: usize, len: usize) -> Buffer {
    let seed = |i: usize| seeds[(offset * 31 + i) % seeds.len()];
    match kind % 5 {
        0 => Buffer::from_vec((0..len).map(|i| seed(i) as u8).collect::<Vec<_>>()),
        1 => Buffer::from_vec((0..len).map(seed).collect::<Vec<_>>()),
        2 => Buffer::from_vec((0..len).map(|i| seed(i) as i16).collect::<Vec<_>>()),
        3 => Buffer::from_vec(
            (0..len)
                .map(|i| seed(i) as i32 * 30_001 - 1_000_000)
                .collect::<Vec<_>>(),
        ),
        _ => Buffer::from_vec(
            (0..len)
                .map(|i| seed(i) as f32 / 37.0 - 500.0)
                .collect::<Vec<_>>(),
        ),
    }
}

#[derive(Clone, Debug)]
struct Case {
    image: Image,
    position: Vec<usize>,
    options: DisplayOptions,
}

/// Images of every colour model with 1 to 3 channels and 1 or 2 slices,
/// some channels with lookup tables and display ranges, plus display
/// options.
fn case() -> impl Strategy<Value = Case> {
    (
        (1usize..12, 1usize..12, 1usize..4, 1usize..3),
        (0u8..5, 0u8..3),
        prop::collection::vec(any::<u16>(), 1..40),
        prop::collection::vec(
            (
                any::<bool>(),
                0usize..12,
                any::<bool>(),
                0u16..300,
                0u16..600,
            ),
            3,
        ),
        (
            0u8..4,
            0u8..3,
            any::<bool>(),
            prop::collection::vec(any::<bool>(), 0..4),
        ),
        (0usize..3, 0usize..2),
    )
        .prop_map(
            |((w, h, channels, slices), (kind, color), seeds, looks, display, at)| {
                let rgb = color == 2 && channels == 3;
                let mut meta =
                    ImageMeta::xyczt(plane(kind, &seeds, 0, 1).dtype(), w, h, channels, slices, 1)
                        .unwrap();
                meta.color = match color {
                    _ if rgb => ColorModel::Rgb,
                    1 => ColorModel::Composite,
                    _ => ColorModel::Grayscale,
                };
                meta.channels = looks[..channels]
                    .iter()
                    .map(|&(has_lut, lut, has_range, low, width)| ChannelMeta {
                        name: None,
                        lut: has_lut.then(|| builtin_luts()[lut].lut()),
                        display_range: has_range
                            .then(|| (low as f64 - 20.0, low as f64 - 20.0 + width as f64 / 2.0)),
                    })
                    .collect();
                let planes = (0..channels * slices)
                    .map(|p| plane(kind, &seeds, p, w * h))
                    .collect();
                let image = Image::from_planes(meta, planes).unwrap();

                let (mode, blend, nearest, visible) = display;
                let options = DisplayOptions {
                    mode: [
                        ChannelMode::Auto,
                        ChannelMode::Composite,
                        ChannelMode::Color,
                        ChannelMode::Grayscale,
                    ][mode as usize],
                    visible,
                    blend: [Blend::Sum, Blend::Max, Blend::Min][blend as usize],
                    reduction: if nearest {
                        Reduction::Nearest
                    } else {
                        Reduction::BoxAverage
                    },
                };
                let position = [(channels, at.0), (slices, at.1)]
                    .into_iter()
                    .filter(|&(size, _)| size > 1)
                    .map(|(size, index)| index % size)
                    .collect();
                Case {
                    image,
                    position,
                    options,
                }
            },
        )
}

/// The rectangle `region` cut out of `picture`.
fn crop(picture: &Rgba8, region: Rect) -> Vec<u8> {
    (region.y..region.y + region.h)
        .flat_map(|y| {
            let start = (y * picture.width + region.x) * 4;
            picture.data[start..start + region.w * 4].iter().copied()
        })
        .collect()
}

proptest! {
    // Failures are not written to a regressions file: these are integration
    // tests, for which proptest cannot find a place to keep one.
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn region_rendering_equals_cropped_full_rendering(
        case in case(),
        corner in (0usize..12, 0usize..12, 1usize..12, 1usize..12),
    ) {
        let Case { image, position, options } = case;
        let (width, height) = (image.meta().width(), image.meta().height());
        let (x, y) = (corner.0 % width, corner.1 % height);
        let region = Rect::new(x, y, corner.2.min(width - x), corner.3.min(height - y));

        let full = render_plane_with(&image, &position, &options).unwrap();
        prop_assert_eq!((full.width, full.height), (width, height));
        prop_assert_eq!(full.data.len(), width * height * 4);
        let part = render_region(&image, &position, region, region.w, region.h, &options).unwrap();
        prop_assert_eq!(&part.data, &crop(&full, region));

        // Whole-number magnification repeats each pixel of the region.
        let (mx, my) = (1 + corner.2 % 3, 1 + corner.3 % 3);
        let big = render_region(
            &image, &position, region, region.w * mx, region.h * my, &options,
        ).unwrap();
        for oy in 0..region.h * my {
            for ox in 0..region.w * mx {
                let from = ((oy / my) * region.w + ox / mx) * 4;
                let to = (oy * region.w * mx + ox) * 4;
                prop_assert_eq!(&big.data[to..to + 4], &part.data[from..from + 4]);
            }
        }
    }

    #[test]
    fn composite_of_one_channel_equals_that_channel_alone(case in case(), channel in 0usize..3) {
        let Case { image, mut position, .. } = case;
        let meta = image.meta();
        prop_assume!(meta.color != ColorModel::Rgb);
        let channels = meta.size(&imager_core::AxisKind::Channel);
        let channel = channel % channels;
        if channels > 1 {
            position[0] = channel;
        }
        for blend in [Blend::Sum, Blend::Max, Blend::Min] {
            let only = DisplayOptions {
                mode: ChannelMode::Composite,
                visible: (0..channels).map(|c| c == channel).collect(),
                blend,
                ..Default::default()
            };
            let composite = render_plane_with(&image, &position, &only).unwrap();
            // The single-channel modes of ImageJ: the same lookup table...
            let alone = DisplayOptions {
                mode: ChannelMode::Color,
                ..Default::default()
            };
            // ...which for an image that is not a composite is only the same
            // table if the channel names one.
            let same_lut = meta.color == ColorModel::Composite
                || channels == 1
                || meta.channels[channel].lut.is_some();
            if same_lut {
                let color = render_plane_with(&image, &position, &alone).unwrap();
                prop_assert_eq!(&composite, &color);
            }
        }
    }

    #[test]
    fn rendering_is_lut_of_level(case in case()) {
        // Colour mode is exactly: effective range -> LevelMap -> lookup table.
        let Case { image, position, .. } = case;
        let meta = image.meta();
        prop_assume!(meta.color != ColorModel::Rgb);
        let options = DisplayOptions { mode: ChannelMode::Color, ..Default::default() };
        let picture = render_plane_with(&image, &position, &options).unwrap();
        let index = meta.plane_index(&position).unwrap();
        let channel = meta.plane_channel(index).unwrap();
        let range = effective_display_range(&image, index).unwrap();
        let map = LevelMap::new(meta.dtype(), false, range).unwrap();
        let lut = imager_render::channel_lut(&image, channel, ChannelMode::Color);
        let samples = image.read_plane(index).unwrap();
        let levels = to_levels(&samples, range).unwrap();
        for (i, pixel) in picture.data.as_chunks::<4>().0.iter().enumerate() {
            let level = map.level(samples.get_f64(i).unwrap());
            prop_assert_eq!(level, levels[i]);
            prop_assert_eq!(&pixel[..3], &lut.table[level as usize][..]);
            prop_assert_eq!(pixel[3], 255);
        }
    }

    #[test]
    fn histogram_counts_sum_to_the_sample_count(
        case in case(),
        bins in prop_oneof![Just(256usize), 1usize..400],
    ) {
        let Case { image, position, .. } = case;
        let meta = image.meta();
        let options = HistogramOptions { bins, range: None };
        let rgb = meta.color == ColorModel::Rgb;
        prop_assume!(!rgb || meta.dtype() == DType::U8);
        let per_plane = meta.plane_len() as u64;
        let samples = if rgb {
            per_plane * meta.plane_count() as u64 / 3
        } else {
            per_plane * meta.plane_count() as u64
        };

        let plane = plane_histogram(&image, &position, &options).unwrap();
        prop_assert_eq!(plane.counts.len(), bins);
        prop_assert_eq!(plane.counts.iter().sum::<u64>(), per_plane);
        prop_assert_eq!(plane.count, per_plane);
        let slack = 1e-9 * plane.min.abs().max(plane.max.abs()).max(1.0);
        prop_assert!(plane.min - slack <= plane.mean && plane.mean <= plane.max + slack);
        prop_assert_eq!(plane.mode_count, *plane.counts.iter().max().unwrap());
        prop_assert_eq!(plane.counts[plane.mode_bin], plane.mode_count);

        let stack = stack_histogram(&image, &options).unwrap();
        prop_assert_eq!(stack.counts.len(), bins);
        prop_assert_eq!(stack.counts.iter().sum::<u64>(), samples);
        prop_assert_eq!(stack.count, samples);

        if !rgb {
            let index = meta.plane_index(&position).unwrap();
            let direct = buffer_histogram(&image.read_plane(index).unwrap(), &options).unwrap();
            prop_assert_eq!(direct, plane);
        }
    }
}
