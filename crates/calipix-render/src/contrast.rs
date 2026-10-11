//! Choosing a display range automatically: the B&C window's "Auto" and
//! "Reset" buttons and Process > Enhance Contrast.
//!
//! These only compute a range. The caller stores it in the channel's
//! [`display_range`](calipix_core::ChannelMeta::display_range); sample values
//! are never changed (ImageJ rewrites the pixels of RGB images at this
//! point; Calipix does not).

use calipix_core::{ColorModel, DType, Image, Result};

use crate::histogram::{
    Histogram, HistogramOptions, plane_histogram, stack_histogram, value_counts,
};
use crate::mapping::default_display_range;

/// The saturation percentage Enhance Contrast offers by default.
pub const DEFAULT_SATURATED: f64 = 0.35;

/// `ContrastAdjuster.AUTO_THRESHOLD`.
const AUTO_THRESHOLD: u64 = 5000;

/// The divisor the B&C "Auto" button uses on its `click`-th consecutive
/// press (from 1) on the same image: 5000, then half the previous value
/// (rounded down) each time until that would be below 10, when it starts
/// again from 5000. The sequence is 5000, 2500, 1250, 625, 312, 156, 78,
/// 39, 19, 9 and then repeats. "Reset" starts the count again, and so does
/// a press that finds no range (see [`AutoContrast::reset`]).
pub fn auto_threshold(click: u32) -> u64 {
    let mut threshold = 0;
    for _ in 0..click.max(1) {
        threshold = if threshold < 10 {
            AUTO_THRESHOLD
        } else {
            threshold / 2
        };
    }
    threshold
}

/// The display range the B&C "Auto" button chooses from a plane's 256-bin
/// histogram (`ContrastAdjuster.autoAdjust`), on its `click`-th consecutive
/// press. `None` means ImageJ falls back to "Reset".
///
/// ```text
/// limit     = count / 10                       (integer division)
/// threshold = count / auto_threshold(click)
/// hmin = first bin from the bottom whose count is > threshold, bins holding
///        more than `limit` samples counting as empty
/// hmax = the same from the top
/// if hmax < hmin: reset
/// min = hist_min + hmin * bin_width,  max = hist_min + hmax * bin_width
/// if min == max: use the smallest and largest sample instead
/// ```
///
/// Each further press halves the threshold's divisor, so the range narrows
/// until the sequence wraps round (see [`auto_threshold`]).
pub fn auto_range(histogram: &Histogram, click: u32) -> Option<(f64, f64)> {
    let counts = &histogram.counts;
    let last = counts.len().checked_sub(1)?;
    let limit = histogram.count / 10;
    let threshold = histogram.count / auto_threshold(click);
    let found = |bin: usize| {
        let count = if counts[bin] > limit { 0 } else { counts[bin] };
        count > threshold
    };
    // Like ImageJ's loops, a search that finds nothing stops at the far end.
    let hmin = (0..=last).find(|&bin| found(bin)).unwrap_or(last);
    let hmax = (0..=last).rev().find(|&bin| found(bin)).unwrap_or(0);
    if hmax < hmin {
        return None;
    }
    let min = histogram.hist_min + hmin as f64 * histogram.bin_width;
    let max = histogram.hist_min + hmax as f64 * histogram.bin_width;
    Some(if min == max {
        (histogram.min, histogram.max)
    } else {
        (min, max)
    })
}

/// The display range "Reset" gives the plane at `position`
/// (`ContrastAdjuster.reset`): 0 to 255 for 8-bit and RGB images, and the
/// smallest and largest sample of the plane otherwise.
pub fn reset_range(image: &Image, position: &[usize]) -> Result<(f64, f64)> {
    let meta = image.meta();
    let plane = meta.plane_index(position)?;
    if meta.dtype() == DType::U8 {
        return Ok((0.0, 255.0));
    }
    if meta.color == ColorModel::Rgb {
        return Ok(meta.dtype().integer_range().unwrap_or((0.0, 1.0)));
    }
    default_display_range(&image.read_plane(plane)?)
}

/// What one press of the B&C "Auto" button does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoContrast {
    /// The display range to show the plane with.
    pub range: (f64, f64),
    /// True if no range was found and the default range was restored
    /// instead. ImageJ then starts counting presses again, so the next
    /// press is `click` 1.
    pub reset: bool,
}

/// The display range the B&C "Auto" button gives the plane at `position` on
/// its `click`-th consecutive press: [`auto_range`] of the plane's default
/// histogram, or [`reset_range`] where that gives none. For an RGB image
/// the histogram is of its grey values and the range is meant for all three
/// channels.
pub fn auto_contrast(image: &Image, position: &[usize], click: u32) -> Result<AutoContrast> {
    let histogram = plane_histogram(image, position, &HistogramOptions::default())?;
    Ok(match auto_range(&histogram, click) {
        Some(range) => AutoContrast {
            range,
            reset: false,
        },
        None => AutoContrast {
            range: reset_range(image, position)?,
            reset: true,
        },
    })
}

/// `ContrastEnhancer.getMinAndMax`: the positions in a list of counts where
/// the running total from the bottom, and from the top, first exceeds
/// `(int)(count * saturated / 200)`, so that `saturated / 2` percent of the
/// samples lie beyond each. `counts` yields `(position, count)` in ascending
/// order. `None` if the totals never exceed the threshold.
fn stretch_bounds<T: Copy>(
    counts: impl DoubleEndedIterator<Item = (T, u64)> + Clone,
    count: u64,
    saturated: f64,
) -> Option<(T, T)> {
    let threshold = if saturated > 0.0 {
        (count as f64 * saturated / 200.0) as i32 as i64
    } else {
        0
    };
    fn first_over<T>(mut counts: impl Iterator<Item = (T, u64)>, threshold: i64) -> Option<T> {
        let mut total = 0i64;
        counts.find_map(|(position, count)| {
            total += count as i64;
            (total > threshold).then_some(position)
        })
    }
    let low = first_over(counts.clone(), threshold)?;
    let high = first_over(counts.rev(), threshold)?;
    Some((low, high))
}

/// The display range Process > Enhance Contrast would set for a saturation
/// of `saturated` percent (`ContrastEnhancer.stretchHistogram`, without its
/// "normalize" and "equalize" options), or `None` if ImageJ would leave the
/// range as it is.
///
/// The range comes from the plane at `position`, or with
/// `use_stack_histogram` from all planes together (the "Use stack
/// histogram" option):
///
/// - Integer types other than unsigned 8-bit use the exact value counts
///   (ImageJ's 65536-bin histogram): `min` is the smallest value `v` for
///   which more than `threshold = (int)(count * saturated / 200)` samples
///   are `<= v`, `max` the largest for which more than `threshold` are
///   `>= v`.
/// - Everything else uses the default 256-bin histogram
///   ([`plane_histogram`] or [`stack_histogram`]): the same search gives
///   bins `hmin` and `hmax`, and the range is
///   `hist_min + hmin * bin_width` to `hist_min + hmax * bin_width`.
///
/// In both cases the result is `None` unless `max > min`. For an RGB image
/// the histogram is of its grey values and the range is meant for all three
/// channels.
pub fn enhance_contrast(
    image: &Image,
    position: &[usize],
    saturated: f64,
    use_stack_histogram: bool,
) -> Result<Option<(f64, f64)>> {
    let meta = image.meta();
    let plane = meta.plane_index(position)?;
    let exact =
        meta.dtype().is_integer() && meta.dtype() != DType::U8 && meta.color != ColorModel::Rgb;
    let range = if exact {
        let counts = if use_stack_histogram {
            stack_value_counts(image)?
        } else {
            value_counts(&image.read_plane(plane)?)?
        };
        let count = counts.iter().map(|&(_, count)| count).sum();
        stretch_bounds(counts.iter().copied(), count, saturated)
    } else {
        let options = HistogramOptions::default();
        let histogram = if use_stack_histogram {
            stack_histogram(image, &options)?
        } else {
            plane_histogram(image, position, &options)?
        };
        let bins = histogram.counts.iter().copied().enumerate();
        stretch_bounds(bins, histogram.count, saturated).map(|(hmin, hmax)| {
            let value = |bin: usize| histogram.hist_min + bin as f64 * histogram.bin_width;
            (value(hmin), value(hmax))
        })
    };
    Ok(range.filter(|&(min, max)| max > min))
}

/// Exact value counts over every plane of an integer image.
fn stack_value_counts(image: &Image) -> Result<Vec<(f64, u64)>> {
    let mut merged = std::collections::BTreeMap::new();
    for plane in 0..image.meta().plane_count() {
        for (value, count) in value_counts(&image.read_plane(plane)?)? {
            *merged.entry(OrderedValue(value)).or_insert(0u64) += count;
        }
    }
    Ok(merged.into_iter().map(|(v, count)| (v.0, count)).collect())
}

/// A sample value as a map key, in ascending order. Integer samples are
/// never NaN, so `total_cmp` orders them by value.
#[derive(Clone, Copy, PartialEq)]
struct OrderedValue(f64);

impl Eq for OrderedValue {}

impl PartialOrd for OrderedValue {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedValue {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calipix_core::{Buffer, ImageMeta};

    #[test]
    fn auto_threshold_sequence() {
        let sequence: Vec<u64> = (1..=12).map(auto_threshold).collect();
        assert_eq!(
            sequence,
            [5000, 2500, 1250, 625, 312, 156, 78, 39, 19, 9, 5000, 2500]
        );
        assert_eq!(auto_threshold(0), 5000);
    }

    fn auto(image: &Image, position: &[usize], click: u32) -> AutoContrast {
        auto_contrast(image, position, click).unwrap()
    }

    /// An 8-bit image whose value `v` occurs `counts[v]` times.
    fn with_counts(counts: &[(u8, usize)]) -> Image {
        let samples: Vec<u8> = counts
            .iter()
            .flat_map(|&(v, n)| std::iter::repeat_n(v, n))
            .collect();
        Image::from_vec(samples.len(), 1, samples).unwrap()
    }

    #[test]
    fn auto_ignores_sparse_tails_and_dominant_bins() {
        // 10000 samples: limit = 1000, first threshold = 10000 / 5000 = 2.
        // Value 0 holds 6000 (> limit, so it is treated as empty), 10 and
        // 240 hold 2 each (not > 2), 20..=219 hold 19 or 20 each.
        let mut counts = vec![(0u8, 6000), (10, 2), (240, 2)];
        counts.extend((20..=219u8).map(|v| (v, if v < 216 { 20 } else { 19 })));
        let image = with_counts(&counts);
        assert_eq!(image.meta().width(), 10_000);
        assert_eq!(auto(&image, &[], 1).range, (20.0, 219.0));
        // Sixth press: threshold = 10000 / 156 = 64; nothing qualifies, so
        // hmin = 255 > hmax = 0 and the range is reset.
        assert_eq!(auto(&image, &[], 6).range, (0.0, 255.0));
        assert!(auto(&image, &[], 6).reset && !auto(&image, &[], 1).reset);
        // Fifth press: 10000 / 312 = 32, also above every count.
        let histogram = plane_histogram(&image, &[], &Default::default()).unwrap();
        assert_eq!(auto_range(&histogram, 5), None);
        // Fourth press: 10000 / 625 = 16, below the counts of 19 and 20.
        assert_eq!(auto_range(&histogram, 4), Some((20.0, 219.0)));
        // Eleventh press is the first again.
        assert_eq!(auto_range(&histogram, 11), Some((20.0, 219.0)));
    }

    #[test]
    fn auto_narrows_with_repeated_presses() {
        // 20000 samples, limit 2000. Thresholds: 4, 8, 16, 32.
        let counts: Vec<(u8, usize)> = vec![
            (10, 5),
            (20, 9),
            (30, 17),
            (40, 33),
            (100, 1900),
            (101, 1900),
            (150, 12186),
            (200, 1900),
            (201, 1900),
            (210, 100),
            (220, 30),
            (230, 12),
            (240, 6),
            (250, 2),
        ];
        let image = with_counts(&counts);
        assert_eq!(image.meta().width(), 20_000);
        let range = |click| auto(&image, &[], click).range;
        assert_eq!(range(1), (10.0, 240.0));
        assert_eq!(range(2), (20.0, 230.0));
        assert_eq!(range(3), (30.0, 220.0));
        assert_eq!(range(4), (40.0, 210.0));
    }

    #[test]
    fn auto_on_flat_and_tiny_images() {
        // One value: its bin holds more than a tenth of the samples, so
        // nothing is found and the range is reset.
        let image = Image::from_vec(4, 1, vec![7u8; 4]).unwrap();
        assert_eq!(auto(&image, &[], 1).range, (0.0, 255.0));
        let image = Image::from_vec(4, 1, vec![700u16; 4]).unwrap();
        assert_eq!(auto(&image, &[], 1).range, (700.0, 700.0));
        // Fewer than 10 samples: limit 0, so every occupied bin is ignored.
        let image = Image::from_vec(3, 1, vec![1.0f32, 2.0, 3.0]).unwrap();
        assert_eq!(auto(&image, &[], 1).range, (1.0, 3.0));
    }

    #[test]
    fn auto_uses_bin_positions_for_sixteen_bit_and_float() {
        // 100 distinct values 1000, 1010, ..., 1990 twenty times each, plus
        // one outlier at 0 and one at 5119: 2002 samples, limit 200,
        // threshold 2002 / 5000 = 0. binSize = 5120 / 256 = 20, so the
        // outliers' bins (count 1 > 0) are found: hmin = 0, hmax = 255.
        let mut samples: Vec<u16> = (0..100).flat_map(|i| [1000 + i * 10; 20]).collect();
        samples.extend([0, 5119]);
        let image = Image::from_vec(samples.len(), 1, samples.clone()).unwrap();
        assert_eq!(auto(&image, &[], 1).range, (0.0, 5100.0));
        // Third press: threshold 2002 / 1250 = 1; the outliers no longer
        // count. Bins are 20 wide and hold 40 samples each (two values):
        // hmin = 50 -> 1000, hmax = (1990 / 20) = 99 -> 1980.
        assert_eq!(auto(&image, &[], 3).range, (1000.0, 1980.0));

        // The same values as floats: binSize = 5119 / 256.
        let floats: Vec<f32> = samples.iter().map(|&v| v as f32).collect();
        let image = Image::from_vec(floats.len(), 1, floats).unwrap();
        let (min, max) = auto(&image, &[], 3).range;
        let bin = 5119.0 / 256.0;
        // (int)(1000 * 256 / 5119) = 50 and (int)(1990 * 256 / 5119) = 99.
        assert_eq!((min, max), (50.0 * bin, 99.0 * bin));
    }

    #[test]
    fn reset_ranges() {
        let image = Image::from_vec(2, 1, vec![3u8, 9]).unwrap();
        assert_eq!(reset_range(&image, &[]).unwrap(), (0.0, 255.0));
        let image = Image::from_vec(2, 1, vec![3u16, 9]).unwrap();
        assert_eq!(reset_range(&image, &[]).unwrap(), (3.0, 9.0));
        let image = Image::from_vec(3, 1, vec![f32::NAN, -1.0, 9.5]).unwrap();
        assert_eq!(reset_range(&image, &[]).unwrap(), (-1.0, 9.5));
        assert!(reset_range(&image, &[0]).is_err());
    }

    #[test]
    fn stretch_finds_the_saturation_points() {
        // 1000 samples of 0..=99, ten each. saturated = 10%:
        // threshold = (int)(1000 * 10 / 200) = 50. The running total first
        // exceeds 50 at value 5 (60 samples) and from the top at 94.
        let samples: Vec<u8> = (0..1000).map(|i| (i % 100) as u8).collect();
        let image = Image::from_vec(1000, 1, samples.clone()).unwrap();
        let stretch = |image: &Image, saturated| enhance_contrast(image, &[], saturated, false);
        assert_eq!(stretch(&image, 10.0).unwrap(), Some((5.0, 94.0)));
        // 0.35%: threshold = (int)1.75 = 1; the first bin already holds 10.
        assert_eq!(stretch(&image, 0.35).unwrap(), Some((0.0, 99.0)));
        assert_eq!(stretch(&image, 0.0).unwrap(), Some((0.0, 99.0)));
        // Half from each end leaves nothing: 100% -> threshold 500, low = 50
        // (510 samples), high = 49; not max > min, so unchanged.
        assert_eq!(stretch(&image, 100.0).unwrap(), None);
        assert_eq!(stretch(&image, 250.0).unwrap(), None);

        // 16-bit uses exact values, not bin positions.
        let wide: Vec<u16> = samples.iter().map(|&v| 1000 + 7 * v as u16).collect();
        let image = Image::from_vec(1000, 1, wide).unwrap();
        assert_eq!(stretch(&image, 10.0).unwrap(), Some((1035.0, 1658.0)));
        assert_eq!(stretch(&image, 0.0).unwrap(), Some((1000.0, 1693.0)));
        assert_eq!(stretch(&image, 200.0).unwrap(), None);

        // Float uses the 256-bin histogram over 0..99: binSize = 99 / 256.
        // Value v is in bin (int)(v * 256 / 99); 5 -> 12 and 94 -> 243.
        let floats: Vec<f32> = samples.iter().map(|&v| v as f32).collect();
        let image = Image::from_vec(1000, 1, floats).unwrap();
        let bin = 99.0 / 256.0;
        assert_eq!(
            stretch(&image, 10.0).unwrap(),
            Some((12.0 * bin, 243.0 * bin))
        );
    }

    #[test]
    fn stretch_leaves_flat_images_alone() {
        for image in [
            Image::from_vec(3, 1, vec![5u8; 3]).unwrap(),
            Image::from_vec(3, 1, vec![5u16; 3]).unwrap(),
            Image::from_vec(3, 1, vec![5.0f32; 3]).unwrap(),
            Image::from_vec(3, 1, vec![f32::NAN; 3]).unwrap(),
        ] {
            assert_eq!(enhance_contrast(&image, &[], 0.35, false).unwrap(), None);
        }
    }

    #[test]
    fn stretch_with_the_stack_histogram() {
        let stack = |planes: Vec<Buffer>| {
            let meta = ImageMeta::xyczt(planes[0].dtype(), 4, 1, 1, planes.len(), 1).unwrap();
            Image::from_planes(meta, planes).unwrap()
        };
        let image = stack(vec![
            Buffer::from_vec(vec![10u16, 20, 30, 40]),
            Buffer::from_vec(vec![5u16, 20, 300, 40]),
        ]);
        let range = |use_stack| enhance_contrast(&image, &[1], 0.0, use_stack).unwrap();
        assert_eq!(range(false), Some((5.0, 300.0)));
        assert_eq!(range(true), Some((5.0, 300.0)));
        let first = enhance_contrast(&image, &[0], 0.0, false).unwrap();
        assert_eq!(first, Some((10.0, 40.0)));
        // 8 samples at 50%: threshold 2; totals pass it at the third value
        // from each end of 5 10 20 20 30 40 40 300.
        let half = enhance_contrast(&image, &[0], 50.0, true).unwrap();
        assert_eq!(half, Some((20.0, 40.0)));

        let image = stack(vec![
            Buffer::from_vec(vec![0.0f32, 1.0, 2.0, 3.0]),
            Buffer::from_vec(vec![4.0f32, 5.0, 6.0, 8.0]),
        ]);
        let all = enhance_contrast(&image, &[0], 0.0, true).unwrap();
        // Bins over 0..8: the last sample is in bin 255 of width 1/32.
        assert_eq!(all, Some((0.0, 255.0 / 32.0)));
    }

    #[test]
    fn rgb_ranges_come_from_grey_values() {
        let mut meta = ImageMeta::xyczt(DType::U8, 4, 1, 3, 1, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let planes = vec![
            Buffer::from_vec(vec![30u8, 60, 90, 120]),
            Buffer::from_vec(vec![30u8, 60, 90, 120]),
            Buffer::from_vec(vec![30u8, 60, 90, 126]),
        ];
        let image = Image::from_planes(meta, planes).unwrap();
        // Grey values 30, 60, 90, 122.
        let range = enhance_contrast(&image, &[0], 0.0, false).unwrap();
        assert_eq!(range, Some((30.0, 122.0)));
        assert_eq!(reset_range(&image, &[1]).unwrap(), (0.0, 255.0));
        // Too few samples for Auto to accept any bin: min == max is not
        // reached either, so it resets.
        assert_eq!(auto(&image, &[0], 1).range, (0.0, 255.0));
    }
}
