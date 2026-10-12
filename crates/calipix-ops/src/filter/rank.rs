//! Filters over a circular neighbourhood, after ImageJ's `RankFilters`
//! plugin: mean, minimum, maximum, variance, median, despeckle, remove
//! outliers, remove NaNs and top hat.
//!
//! The loops follow ImageJ's closely, including the running sums and the
//! running maximum it carries along each row from the left edge of the
//! rectangle, because floating-point results depend on them.

use calipix_core::{Buffer, Error, RealPixel, Rect, Result};

use super::tile::{Strip, default_strip_rows, filter_strips};
use super::{Frame, from_f32, max_value, real_plane, to_f32, wrap};

/// Largest radius accepted; ImageJ's dialogs allow up to 1000.
const MAX_RADIUS: f64 = 1000.0;

/// The statistic a [`rank_filter`] replaces each sample with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rank {
    Mean,
    Minimum,
    Maximum,
    Variance,
    Median,
}

/// Which outliers [`remove_outliers`] replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outliers {
    /// Samples brighter than the local median.
    Bright,
    /// Samples darker than the local median.
    Dark,
}

/// One pass of `RankFilters.doFiltering`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    Mean,
    Min,
    Max,
    Variance,
    Median,
    Outliers,
    RemoveNan,
}

/// The circular kernel of a given radius in the form of
/// `RankFilters.makeLineRadii`: for each kernel line from top to bottom,
/// the leftmost and rightmost offset it covers, followed by the number of
/// points in the kernel and its radius in whole pixels.
///
/// Radius 0.5 gives the four neighbours and the centre, radius 1 a 3 x 3
/// square. Radii from 1.5 up to 1.75 and from 2.5 up to 2.85 are raised to
/// 1.75 and 2.85, as in ImageJ.
pub fn kernel_line_radii(radius: f64) -> Vec<i32> {
    let radius = if (1.5..1.75).contains(&radius) {
        1.75
    } else if (2.5..2.85).contains(&radius) {
        2.85
    } else {
        radius
    };
    let r2 = (radius * radius) as i64 + 1;
    let k_radius = ((r2 as f64 + 1e-10).sqrt()) as i64;
    let k_height = 2 * k_radius + 1;
    let mut kernel = vec![0i32; (2 * k_height + 2) as usize];
    kernel[(2 * k_radius) as usize] = -k_radius as i32;
    kernel[(2 * k_radius + 1) as usize] = k_radius as i32;
    let mut n_points = 2 * k_radius + 1;
    for y in 1..=k_radius {
        let dx = (((r2 - y * y) as f64 + 1e-10).sqrt()) as i64;
        for line in [k_radius - y, k_radius + y] {
            kernel[(2 * line) as usize] = -dx as i32;
            kernel[(2 * line + 1) as usize] = dx as i32;
        }
        n_points += 4 * dx + 2;
    }
    let len = kernel.len();
    kernel[len - 2] = n_points as i32;
    kernel[len - 1] = k_radius as i32;
    kernel
}

/// A circular kernel, unpacked from [`kernel_line_radii`].
struct Kernel {
    /// Leftmost and rightmost offset of each line.
    lines: Vec<(isize, isize)>,
    /// Radius in X, in whole pixels.
    radius: usize,
    /// Radius in Y: half the kernel height, rounded down.
    half_height: usize,
    points: usize,
}

impl Kernel {
    fn new(radius: f64) -> Result<Kernel> {
        if !(0.0..=MAX_RADIUS).contains(&radius) {
            return Err(Error::Other(format!(
                "the radius must be between 0 and {MAX_RADIUS}, not {radius}"
            )));
        }
        let packed = kernel_line_radii(radius);
        let n = packed.len();
        let lines: Vec<_> = packed[..n - 2]
            .chunks(2)
            .map(|pair| (pair[0] as isize, pair[1] as isize))
            .collect();
        Ok(Kernel {
            half_height: lines.len() / 2,
            lines,
            radius: packed[n - 1] as usize,
            points: packed[n - 2] as usize,
        })
    }
}

/// Replaces each sample of `frame.rect` with a statistic of the samples
/// within `radius` of it, as `RankFilters.rank(ip, radius, filterType)`
/// does for Process > Filters > Mean, Minimum, Maximum, Variance and
/// Median.
///
/// * Mean and variance are computed from double-precision sums. Variance
///   is clamped to the range of an integer type and never negative.
/// * In float planes, minimum and maximum ignore NaN. The median ignores
///   NaN neighbours but leaves a NaN sample as NaN; with an even number of
///   neighbours left it is the lower or upper middle value, whichever
///   ImageJ's search arrives at.
pub fn rank_filter(plane: &Buffer, frame: Frame, radius: f64, rank: Rank) -> Result<Buffer> {
    let kernel = Kernel::new(radius)?;
    let (pass, sign) = match rank {
        Rank::Mean => (Pass::Mean, 1.0),
        Rank::Minimum => (Pass::Min, -1.0),
        Rank::Maximum => (Pass::Max, 1.0),
        Rank::Variance => (Pass::Variance, 1.0),
        Rank::Median => (Pass::Median, 1.0),
    };
    real_plane!(plane, "filter", |s| single(
        s, frame, &kernel, pass, sign, 0.0
    )
    .map(wrap))
}

/// Process > Noise > Despeckle: a median filter of radius 1.
pub fn despeckle(plane: &Buffer, frame: Frame) -> Result<Buffer> {
    rank_filter(plane, frame, 1.0, Rank::Median)
}

/// Process > Noise > Remove Outliers (`RankFilters.OUTLIERS`): replaces a
/// sample by the median of its neighbourhood if it is brighter (or darker)
/// than that median by more than `threshold`.
///
/// `inverted_lut` says whether the image is displayed with an inverting
/// lookup table, in which case ImageJ takes bright and dark by appearance,
/// not by value.
pub fn remove_outliers(
    plane: &Buffer,
    frame: Frame,
    radius: f64,
    threshold: f64,
    which: Outliers,
    inverted_lut: bool,
) -> Result<Buffer> {
    let kernel = Kernel::new(radius)?;
    if threshold.is_nan() || threshold < 0.0 {
        return Err(Error::Other(format!(
            "the threshold must not be negative, not {threshold}"
        )));
    }
    // -1 compares each sample with the local minimum (bright outliers).
    let sign = if inverted_lut == (which == Outliers::Dark) {
        -1.0
    } else {
        1.0
    };
    real_plane!(plane, "filter", |s| single(
        s,
        frame,
        &kernel,
        Pass::Outliers,
        sign,
        threshold as f32
    )
    .map(wrap))
}

/// Process > Noise > Remove NaNs (`RankFilters.REMOVE_NAN`): replaces each
/// NaN in a float plane by the median of the samples within `radius` that
/// are not NaN. It stays NaN if there are none. Integer planes are
/// refused, as in ImageJ.
pub fn remove_nans(plane: &Buffer, frame: Frame, radius: f64) -> Result<Buffer> {
    let kernel = Kernel::new(radius)?;
    if !plane.dtype().is_float() {
        return Err(Error::Unsupported(format!(
            "Remove NaNs needs a float image, not {}",
            plane.dtype()
        )));
    }
    real_plane!(plane, "filter", |s| single(
        s,
        frame,
        &kernel,
        Pass::RemoveNan,
        1.0,
        0.0
    )
    .map(wrap))
}

/// Process > Filters > Top Hat (`RankFilters.TOP_HAT`): subtracts from
/// the plane its grey-scale opening (a minimum filter followed by a
/// maximum filter), which removes everything larger than the kernel.
///
/// * With `light_background`, the closing (maximum, then minimum) is used
///   instead and, for integer types, the largest value of the type is
///   added, so that the background comes out at the top of the range.
/// * With `dont_subtract`, the result is the opening (or closing) itself.
/// * `inverted_lut` swaps the two cases, as ImageJ does for an image shown
///   with an inverting lookup table.
pub fn top_hat(
    plane: &Buffer,
    frame: Frame,
    radius: f64,
    light_background: bool,
    dont_subtract: bool,
    inverted_lut: bool,
) -> Result<Buffer> {
    let kernel = Kernel::new(radius)?;
    let sign = if inverted_lut != light_background {
        1.0
    } else {
        -1.0
    };
    fn run<T: RealPixel>(
        src: &[T],
        frame: Frame,
        kernel: &Kernel,
        sign: f32,
        subtract: bool,
    ) -> Result<Vec<T>> {
        // The second pass reads first-pass results up to half a kernel
        // away, which themselves read as far again.
        let halo = 2 * kernel.half_height;
        let rows = default_strip_rows(halo, frame.rect.h);
        let offset = if sign == 1.0 && !T::DTYPE.is_float() {
            max_value(T::DTYPE)
        } else {
            0.0
        };
        filter_strips(src, frame, halo, rows, |strip: Strip<'_, T>| {
            let mut opened = open_or_close(strip.rows, strip.frame, kernel, sign);
            if subtract {
                let r = strip.frame.rect;
                for (j, line) in opened.chunks_mut(r.w).enumerate() {
                    let start = (r.y + j) * strip.frame.width + r.x;
                    for (v, &original) in line.iter_mut().zip(&strip.rows[start..start + r.w]) {
                        *v = to_f32(original) - *v + offset;
                    }
                }
            }
            Ok(opened.into_iter().map(from_f32).collect())
        })
    }
    real_plane!(plane, "filter", |s| run(
        s,
        frame,
        &kernel,
        sign,
        !dont_subtract
    )
    .map(wrap))
}

/// A filter that is one pass over the kernel.
fn single<T: RealPixel>(
    src: &[T],
    frame: Frame,
    kernel: &Kernel,
    pass: Pass,
    sign: f32,
    threshold: f32,
) -> Result<Vec<T>> {
    let halo = kernel.half_height;
    let rows = default_strip_rows(halo, frame.rect.h);
    filter_strips(src, frame, halo, rows, |strip: Strip<'_, T>| {
        let filtered = filter_pass(strip.rows, strip.frame, kernel, pass, sign, threshold);
        Ok(filtered.into_iter().map(from_f32).collect())
    })
}

/// The two passes of ImageJ's grey-scale open (`sign` -1: minimum, then
/// maximum) or close (`sign` 1). The first pass covers the rectangle grown
/// by the kernel, so that the second finds filtered neighbours.
fn open_or_close<T: RealPixel>(rows: &[T], frame: Frame, kernel: &Kernel, sign: f32) -> Vec<f32> {
    let r = frame.rect;
    let x0 = r.x.saturating_sub(kernel.radius);
    let y0 = r.y.saturating_sub(kernel.half_height);
    let x1 = (r.x + r.w + kernel.radius).min(frame.width);
    let y1 = (r.y + r.h + kernel.half_height).min(frame.height);
    let grown = Frame::new(
        frame.width,
        frame.height,
        Rect::new(x0, y0, x1 - x0, y1 - y0),
    );
    let (first, second) = if sign == -1.0 {
        (Pass::Min, Pass::Max)
    } else {
        (Pass::Max, Pass::Min)
    };
    let filtered = filter_pass(rows, grown, kernel, first, sign, 0.0);
    let mut between: Vec<f32> = rows.iter().map(|&v| to_f32(v)).collect();
    for (j, line) in filtered.chunks(grown.rect.w).enumerate() {
        let start = (y0 + j) * frame.width + x0;
        between[start..start + line.len()].copy_from_slice(line);
    }
    filter_pass(&between, frame, kernel, second, -sign, 0.0)
}

/// `RankFilters.doFiltering` for one thread: filters `frame.rect` of the
/// given rows and returns its samples as floats, row by row.
fn filter_pass<T: RealPixel>(
    rows: &[T],
    frame: Frame,
    kernel: &Kernel,
    pass: Pass,
    sign: f32,
    threshold: f32,
) -> Vec<f32> {
    let r = frame.rect;
    let (cache, cache_width) = padded(rows, frame, kernel);
    let is_float = T::DTYPE.is_float();
    let line = Line {
        points: kernel.points,
        pass,
        sign,
        threshold,
        is_float,
        max_value: max_value(T::DTYPE),
        small_kernel: kernel.radius < 2,
    };
    let mut state = State {
        sums: [0.0; 2],
        above: vec![0.0; kernel.points],
        below: vec![0.0; kernel.points],
    };
    let mut pointers = vec![0usize; 2 * kernel.lines.len()];
    let mut out = vec![0f32; r.w * r.h];
    for (j, values) in out.chunks_mut(r.w).enumerate() {
        // Where each kernel line starts and ends in the cache for the
        // first sample of this row.
        for (i, &(left, right)) in kernel.lines.iter().enumerate() {
            let centre = ((j + i) * cache_width + kernel.radius) as isize;
            pointers[2 * i] = (centre + left) as usize;
            pointers[2 * i + 1] = (centre + right) as usize;
        }
        let line_p = (j + kernel.half_height) * cache_width + kernel.radius;
        line.filter(values, &cache, &pointers, line_p, &mut state);
    }
    out
}

/// A float copy of the rectangle with a margin of the kernel's size on
/// every side; ImageJ's `cache`. Samples beyond the given rows are copies
/// of the nearest sample.
fn padded<T: RealPixel>(rows: &[T], frame: Frame, kernel: &Kernel) -> (Vec<f32>, usize) {
    let r = frame.rect;
    let cache_width = r.w + 2 * kernel.radius;
    let cache_height = r.h + 2 * kernel.half_height;
    let mut cache = Vec::with_capacity(cache_width * cache_height);
    let x_min = r.x as isize - kernel.radius as isize;
    let inside_from = x_min.max(0) as usize;
    let inside_to = (r.x + r.w + kernel.radius).min(frame.width);
    let pad_left = (-x_min).max(0) as usize;
    let pad_right = cache_width - pad_left - (inside_to - inside_from);
    for cy in 0..cache_height {
        let y = (r.y + cy)
            .saturating_sub(kernel.half_height)
            .min(frame.height - 1);
        let row = &rows[y * frame.width..(y + 1) * frame.width];
        let first = to_f32(row[inside_from]);
        let last = to_f32(row[inside_to - 1]);
        cache.extend(std::iter::repeat_n(first, pad_left));
        cache.extend(row[inside_from..inside_to].iter().map(|&v| to_f32(v)));
        cache.extend(std::iter::repeat_n(last, pad_right));
    }
    (cache, cache_width)
}

/// What `RankFilters.filterLine` needs to know besides the data.
struct Line {
    points: usize,
    pass: Pass,
    /// -1 to find minima as maxima of negated values.
    sign: f32,
    threshold: f32,
    is_float: bool,
    max_value: f32,
    /// Kernels of radius below 2 are always summed in full.
    small_kernel: bool,
}

/// Working storage reused from row to row.
struct State {
    sums: [f64; 2],
    above: Vec<f32>,
    below: Vec<f32>,
}

impl Line {
    /// `RankFilters.filterLine`: filters one row of the rectangle.
    /// `kernel` holds the cache positions of the ends of each kernel line
    /// for the first sample, and `line_p` the position of that sample.
    fn filter(
        &self,
        values: &mut [f32],
        cache: &[f32],
        kernel: &[usize],
        line_p: usize,
        state: &mut State,
    ) {
        let min_or_max = matches!(self.pass, Pass::Min | Pass::Max);
        let min_max_outliers = min_or_max || self.pass == Pass::Outliers;
        let sum_filter = matches!(self.pass, Pass::Mean | Pass::Variance);
        let sign = self.sign;
        let n = self.points as f64;
        let mut max = 0f32;
        // A first guess for the median.
        let mut median = if cache[line_p].is_nan() {
            0.0
        } else {
            cache[line_p]
        };
        let mut full = true;
        for x in 0..values.len() {
            if full {
                full = self.small_kernel;
                if min_max_outliers {
                    max = area_max(cache, x, kernel, 0, -f32::MAX, sign);
                }
                if min_or_max {
                    values[x] = max * sign;
                    continue;
                } else if sum_filter {
                    state.sums = area_sums(cache, x, kernel);
                }
            } else if min_max_outliers {
                let new_max = side_max(cache, x, kernel, true, sign);
                if new_max >= max {
                    max = new_max;
                } else {
                    let removed_max = side_max(cache, x, kernel, false, sign);
                    if removed_max >= max {
                        max = area_max(cache, x, kernel, 1, new_max, sign);
                    }
                }
                if min_or_max {
                    values[x] = max * sign;
                    continue;
                }
            } else if sum_filter {
                add_side_sums(cache, x, kernel, &mut state.sums);
                // Do not carry a NaN along the rest of the row.
                if state.sums[0].is_nan() {
                    full = true;
                }
            }
            let original = cache[line_p + x];
            let (above, below) = (&mut state.above, &mut state.below);
            values[x] = match self.pass {
                Pass::Mean => (state.sums[0] / n) as f32,
                Pass::Variance => {
                    let [sum, sum2] = state.sums;
                    let mut value = ((sum2 - sum * sum / n) / n) as f32;
                    if value > self.max_value {
                        value = self.max_value;
                    }
                    // Numeric noise can give values below zero.
                    if value < 0.0 {
                        value = 0.0;
                    }
                    value
                }
                Pass::Median => {
                    median = if self.is_float {
                        nan_aware_median(cache, x, kernel, above, below, self.points, original)
                    } else {
                        plain_median(cache, x, kernel, above, below, self.points, median)
                    };
                    median
                }
                Pass::Outliers => {
                    let mut v = original;
                    // With sign +1 (dark outliers) the median cannot be
                    // above the maximum, so most samples need no median.
                    if v * sign + self.threshold < max {
                        median = plain_median(cache, x, kernel, above, below, self.points, median);
                        if v * sign + self.threshold < median * sign {
                            v = median;
                        }
                    }
                    v
                }
                Pass::RemoveNan => {
                    if original.is_nan() {
                        nan_aware_median(cache, x, kernel, above, below, self.points, median)
                    } else {
                        // The guess for the next NaN.
                        median = original;
                        original
                    }
                }
                Pass::Min | Pass::Max => unreachable!("written above"),
            };
        }
    }
}

/// `RankFilters.getAreaMax`: the largest `value * sign` in the kernel at
/// `x`, starting from `max`; `ignore_right` leaves out the last column.
fn area_max(
    cache: &[f32],
    x: usize,
    kernel: &[usize],
    ignore_right: usize,
    mut max: f32,
    sign: f32,
) -> f32 {
    for ends in kernel.chunks(2) {
        // The last column of a one-sample line may be the only one.
        let end = (ends[1] + x + 1).saturating_sub(ignore_right);
        for &v in cache.get(ends[0] + x..end).unwrap_or(&[]) {
            let v = v * sign;
            if max < v {
                max = v;
            }
        }
    }
    max
}

/// `RankFilters.getSideMax`: the largest `value * sign` in the column
/// that entered the kernel at `x` (`is_right`) or the one that left it.
fn side_max(cache: &[f32], x: usize, kernel: &[usize], is_right: bool, sign: f32) -> f32 {
    let mut max = -f32::MAX;
    for ends in kernel.chunks(2) {
        let p = if is_right {
            ends[1] + x
        } else {
            ends[0] + x - 1
        };
        let v = cache[p] * sign;
        if max < v {
            max = v;
        }
    }
    max
}

/// `RankFilters.getAreaSums`: the sum and the sum of squares over the
/// kernel at `x`.
fn area_sums(cache: &[f32], x: usize, kernel: &[usize]) -> [f64; 2] {
    let (mut sum, mut sum2) = (0f64, 0f64);
    for ends in kernel.chunks(2) {
        for &v in &cache[ends[0] + x..=ends[1] + x] {
            let v = v as f64;
            sum += v;
            sum2 += v * v;
        }
    }
    [sum, sum2]
}

/// `RankFilters.addSideSums`: moves the sums one sample to the right.
fn add_side_sums(cache: &[f32], x: usize, kernel: &[usize], sums: &mut [f64; 2]) {
    let (mut sum, mut sum2) = (0f64, 0f64);
    for ends in kernel.chunks(2) {
        // This value has left the kernel.
        let v = cache[ends[0] + x - 1] as f64;
        sum -= v;
        sum2 -= v * v;
        // This one has entered it.
        let v = cache[ends[1] + x] as f64;
        sum += v;
        sum2 += v * v;
    }
    sums[0] += sum;
    sums[1] += sum2;
}

/// `RankFilters.getMedian`: the median of the kernel at `x`, found by
/// sorting only the side of `guess` that holds it. Values that compare
/// neither above nor below the guess (NaN among them) count as equal to
/// it.
fn plain_median(
    cache: &[f32],
    x: usize,
    kernel: &[usize],
    above: &mut [f32],
    below: &mut [f32],
    points: usize,
    guess: f32,
) -> f32 {
    let (mut n_above, mut n_below) = (0, 0);
    for ends in kernel.chunks(2) {
        for &v in &cache[ends[0] + x..=ends[1] + x] {
            if v > guess {
                above[n_above] = v;
                n_above += 1;
            } else if v < guess {
                below[n_below] = v;
                n_below += 1;
            }
        }
    }
    pick_median(above, n_above, below, n_below, points / 2, guess)
}

/// `RankFilters.getNaNAwareMedian`: as [`plain_median`], but NaN values
/// are left out; NaN if nothing else is there.
fn nan_aware_median(
    cache: &[f32],
    x: usize,
    kernel: &[usize],
    above: &mut [f32],
    below: &mut [f32],
    mut points: usize,
    guess: f32,
) -> f32 {
    let (mut n_above, mut n_below) = (0, 0);
    for ends in kernel.chunks(2) {
        for &v in &cache[ends[0] + x..=ends[1] + x] {
            if v.is_nan() {
                points -= 1;
            } else if v > guess {
                above[n_above] = v;
                n_above += 1;
            } else if v < guess {
                below[n_below] = v;
                n_below += 1;
            }
        }
    }
    if points == 0 {
        return f32::NAN;
    }
    pick_median(above, n_above, below, n_below, points / 2, guess)
}

fn pick_median(
    above: &mut [f32],
    n_above: usize,
    below: &mut [f32],
    n_below: usize,
    half: usize,
    guess: f32,
) -> f32 {
    if n_above > half {
        nth_lowest(&mut above[..n_above], n_above - half - 1)
    } else if n_below > half {
        nth_lowest(&mut below[..n_below], half)
    } else {
        guess
    }
}

/// `RankFilters.findNthLowestNumber`: Hoare's selection; `n` 0 gives the
/// lowest value. Reorders `buf`.
fn nth_lowest(buf: &mut [f32], n: usize) -> f32 {
    let n = n as isize;
    let (mut l, mut m) = (0isize, buf.len() as isize - 1);
    let mut med = buf[n as usize];
    while l < m {
        let (mut i, mut j) = (l, m);
        loop {
            while buf[i as usize] < med {
                i += 1;
            }
            while med < buf[j as usize] {
                j -= 1;
            }
            buf.swap(i as usize, j as usize);
            i += 1;
            j -= 1;
            if !(j >= n && i <= n) {
                break;
            }
        }
        if j < n {
            l = i;
        }
        if n < i {
            m = j;
        }
        med = buf[n as usize];
    }
    med
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kernel sizes listed by ImageJ's Process > Filters > Show Circular
    /// Masks, and the special cases in `makeLineRadii`.
    #[test]
    fn kernels_have_imagej_shapes() {
        let points = |radius: f64| {
            let k = kernel_line_radii(radius);
            (k[k.len() - 2], k[k.len() - 1])
        };
        assert_eq!(points(0.0), (5, 1));
        assert_eq!(points(0.5), (5, 1));
        assert_eq!(points(1.0), (9, 1));
        assert_eq!(points(1.5), (13, 2));
        assert_eq!(points(1.6), points(1.75));
        assert_eq!(points(2.0), (21, 2));
        assert_eq!(points(2.5), (29, 3));
        assert_eq!(points(2.6), points(2.85));
        assert_eq!(points(3.0), (37, 3));
        assert_eq!(&kernel_line_radii(0.5)[..6], &[0, 0, -1, 1, 0, 0]);
        assert_eq!(
            &kernel_line_radii(2.0)[..10],
            &[-1, 1, -2, 2, -2, 2, -2, 2, -1, 1]
        );
    }

    #[test]
    fn nth_lowest_selects() {
        let values = [5.0f32, 1.0, 4.0, 4.0, -2.0, 9.0, 0.5];
        let mut sorted = values;
        sorted.sort_by(f32::total_cmp);
        for (n, &expected) in sorted.iter().enumerate() {
            assert_eq!(nth_lowest(&mut values.clone(), n), expected);
        }
    }

    #[test]
    fn small_cases() {
        let plane = Buffer::from_vec(vec![0u8, 0, 0, 0, 90, 0, 0, 0, 0]);
        let frame = Frame::whole(3, 3);
        let mean = rank_filter(&plane, frame, 1.0, Rank::Mean).unwrap();
        assert_eq!(mean.as_slice::<u8>().unwrap(), &[10; 9]);
        let max = rank_filter(&plane, frame, 0.5, Rank::Maximum).unwrap();
        assert_eq!(
            max.as_slice::<u8>().unwrap(),
            &[0, 90, 0, 90, 90, 90, 0, 90, 0]
        );
        let median = despeckle(&plane, frame).unwrap();
        assert_eq!(median.as_slice::<u8>().unwrap(), &[0; 9]);
        let outliers = remove_outliers(&plane, frame, 1.0, 50.0, Outliers::Bright, false).unwrap();
        assert_eq!(outliers.as_slice::<u8>().unwrap(), &[0; 9]);
        let kept = remove_outliers(&plane, frame, 1.0, 50.0, Outliers::Dark, false).unwrap();
        assert_eq!(kept, plane);
        let hat = top_hat(&plane, frame, 1.0, false, false, false).unwrap();
        assert_eq!(hat, plane);
        assert!(remove_nans(&plane, frame, 1.0).is_err());
        assert!(rank_filter(&plane, frame, -1.0, Rank::Mean).is_err());
    }

    #[test]
    fn nans_are_replaced_by_the_median_of_the_rest() {
        let nan = f32::NAN;
        let plane = Buffer::from_vec(vec![1.0f32, 2.0, 3.0, 4.0, nan, 6.0, 7.0, 8.0, 9.0]);
        let out = remove_nans(&plane, Frame::whole(3, 3), 0.5).unwrap();
        let s = out.as_slice::<f32>().unwrap();
        // Neighbours 2, 4, 6, 8: an even count. ImageJ starts from the last
        // sample before the NaN, 4, and keeps it because neither side of it
        // holds more than half of the values.
        assert_eq!(s[4], 4.0);
        assert_eq!(&s[..4], &[1.0, 2.0, 3.0, 4.0]);
    }
}
