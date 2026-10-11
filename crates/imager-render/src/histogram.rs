//! Histograms and summary statistics with ImageJ's binning.

use imager_core::dispatch_real;
use imager_core::{AxisKind, Buffer, ColorModel, DType, Error, Image, RealPixel, Result};

use crate::mapping::complex_error;

/// What to count into. The default is ImageJ's: 256 bins over the range of
/// the data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HistogramOptions {
    /// Number of bins.
    pub bins: usize,
    /// The sample values the bins span (ImageJ's `histMin` and `histMax`),
    /// or `None` for the smallest and largest sample. As in ImageJ, `(0, 0)`
    /// also means "from the data". Samples outside the range are left out
    /// of the counts, the mean and the standard deviation.
    pub range: Option<(f64, f64)>,
}

impl Default for HistogramOptions {
    fn default() -> HistogramOptions {
        HistogramOptions {
            bins: 256,
            range: None,
        }
    }
}

impl HistogramOptions {
    fn is_default(&self) -> bool {
        self.bins == 256 && self.fixed_range().is_none()
    }

    fn fixed_range(&self) -> Option<(f64, f64)> {
        self.range.filter(|&range| range != (0.0, 0.0))
    }
}

/// Bin counts with the statistics ImageJ reports beside them
/// (`ij.process.ImageStatistics`).
///
/// ImageJ computes these differently for each pixel type, and differently
/// again for a whole stack; the differences are kept:
///
/// | Data | ImageJ class | Bins | Mode |
/// |---|---|---|---|
/// | 8-bit plane or stack, RGB | `ByteStatistics`, `ColorStatistics`, `StackStatistics.sum8BitHistograms` | 256, one per value | most frequent value |
/// | other integers, plane | `ShortStatistics` | `bins` over `min..=max` | most frequent value |
/// | other integers, stack | `StackStatistics.sum16BitHistograms` | 256 over `min..=max` | start of the fullest bin |
/// | float, plane | `FloatStatistics` | `bins` over `min..max` | centre of the fullest bin |
/// | float, stack; any stack with non-default options | `StackStatistics.doCalculations` | `bins` over `min..max` | start of the fullest bin |
///
/// The bin of a sample `v` is `(int)((v - (int)hist_min) * (1.0 / bin_width))`
/// in the `ShortStatistics` and `sum16BitHistograms` rows and
/// `(int)((v - hist_min) * (bins / (hist_max - hist_min)))` in the two rows
/// below them, in both cases limited to the last bin.
///
/// ImageJ has integer statistics for unsigned 16-bit only; ImageR2 uses the
/// same rules for signed and for 32 and 64-bit integers. 64-bit integers are
/// converted to double precision first.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    /// Samples per bin. Bin `i` starts at `hist_min + i * bin_width`; the
    /// last bin also takes samples equal to its upper edge.
    pub counts: Vec<u64>,
    /// The sample value at which the first bin starts (ImageJ's `histMin`).
    pub hist_min: f64,
    /// The largest sample value the bins take (ImageJ's `histMax`). For
    /// integer data the last bin ends at `hist_max + 1`.
    pub hist_max: f64,
    /// Width of a bin in sample values (ImageJ's `binSize`): 1 for 8-bit
    /// data, `(hist_max - hist_min + 1) / bins` for other integers and
    /// `(hist_max - hist_min) / bins` for floats.
    pub bin_width: f64,
    /// Number of samples counted (ImageJ's `pixelCount`). NaN and infinite
    /// floats are never counted.
    pub count: u64,
    /// Smallest and largest sample. NaN if nothing was counted.
    pub min: f64,
    pub max: f64,
    /// Mean of the counted samples. NaN if nothing was counted.
    pub mean: f64,
    /// Sample standard deviation (divisor `count - 1`); 0 for fewer than
    /// two samples.
    pub std_dev: f64,
    /// The mode as ImageJ reports it (`dmode`); see the table above.
    pub mode: f64,
    /// Index of the fullest bin (the first, if several are equally full).
    pub mode_bin: usize,
    /// Number of samples in the fullest bin (ImageJ's `maxCount`).
    pub mode_count: u64,
}

impl Histogram {
    /// The sample value at which bin `bin` starts.
    pub fn bin_start(&self, bin: usize) -> f64 {
        self.hist_min + bin as f64 * self.bin_width
    }

    /// The `counts.len() + 1` boundaries between bins.
    pub fn bin_edges(&self) -> Vec<f64> {
        (0..=self.counts.len()).map(|i| self.bin_start(i)).collect()
    }
}

/// `ImageStatistics.calculateStdDev`.
fn std_dev(n: f64, sum: f64, sum2: f64) -> f64 {
    if n > 0.0 {
        let variance = (n * sum2 - sum * sum) / n;
        if variance > 0.0 {
            (variance / (n - 1.0)).sqrt()
        } else {
            0.0
        }
    } else {
        0.0
    }
}

/// The first bin with the largest count, and that count.
fn fullest(counts: &[u64]) -> (usize, u64) {
    let mut best = (0, 0);
    for (i, &count) in counts.iter().enumerate() {
        if count > best.1 {
            best = (i, count);
        }
    }
    best
}

/// A bin index from Java's `(int)(scale * (v - histMin))`, kept in range.
fn bin_index(scaled: f64, bins: usize) -> usize {
    (scaled as i32).clamp(0, bins as i32 - 1) as usize
}

fn check_bins(options: &HistogramOptions) -> Result<()> {
    if options.bins == 0 || options.bins > i32::MAX as usize {
        return Err(Error::Shape(format!(
            "a histogram cannot have {} bins",
            options.bins
        )));
    }
    Ok(())
}

/// How often each distinct sample value occurs in an integer plane, in
/// ascending order of value. This is the information in ImageJ's 65536-bin
/// 16-bit histogram (`ShortProcessor.getHistogram`), without the empty bins.
pub fn value_counts(plane: &Buffer) -> Result<Vec<(f64, u64)>> {
    let mut counts = ValueCounts::new(plane.dtype())?;
    counts.add(plane)?;
    Ok(counts.finish())
}

/// Accumulates exact value counts over one or more integer planes.
enum ValueCounts {
    /// Types with at most 65536 values: one counter per value from `offset`.
    Dense { offset: f64, counts: Vec<u64> },
    /// Wider types: every sample, sorted at the end.
    Sparse(Vec<f64>),
}

impl ValueCounts {
    fn new(dtype: DType) -> Result<ValueCounts> {
        if !dtype.is_integer() {
            return Err(Error::Unsupported(format!(
                "exact value counts need integer samples, not {dtype}"
            )));
        }
        Ok(match dtype.integer_range() {
            Some((lo, hi)) if hi - lo < 65536.0 => ValueCounts::Dense {
                offset: lo,
                counts: vec![0; (hi - lo) as usize + 1],
            },
            _ => ValueCounts::Sparse(Vec::new()),
        })
    }

    fn add(&mut self, plane: &Buffer) -> Result<()> {
        fn dense<T: RealPixel>(samples: &[T], offset: f64, counts: &mut [u64]) {
            for v in samples {
                counts[(v.to_f64() - offset) as usize] += 1;
            }
        }
        fn sparse<T: RealPixel>(samples: &[T], values: &mut Vec<f64>) {
            values.extend(samples.iter().map(|v| v.to_f64()));
        }
        match self {
            ValueCounts::Dense { offset, counts } => {
                dispatch_real!(plane, |s| dense(s, *offset, counts), complex => return Err(complex_error()))
            }
            ValueCounts::Sparse(values) => {
                dispatch_real!(plane, |s| sparse(s, values), complex => return Err(complex_error()))
            }
        }
        Ok(())
    }

    fn finish(self) -> Vec<(f64, u64)> {
        match self {
            ValueCounts::Dense { offset, counts } => counts
                .into_iter()
                .enumerate()
                .filter(|&(_, count)| count > 0)
                .map(|(i, count)| (offset + i as f64, count))
                .collect(),
            ValueCounts::Sparse(mut values) => {
                values.sort_unstable_by(f64::total_cmp);
                let mut out: Vec<(f64, u64)> = Vec::new();
                for v in values {
                    match out.last_mut() {
                        Some((last, count)) if *last == v => *count += 1,
                        _ => out.push((v, 1)),
                    }
                }
                out
            }
        }
    }
}

/// 8-bit data: `ImageStatistics.getRawStatistics(0, 255)` and
/// `getRawMinAndMax(0, 255)` on a 256-bin histogram.
fn byte_histogram(values: &[(f64, u64)]) -> Histogram {
    let mut counts = vec![0u64; 256];
    for &(v, count) in values {
        counts[v as usize] = count;
    }
    let (mut n, mut sum, mut sum2) = (0u64, 0.0, 0.0);
    for (i, &count) in counts.iter().enumerate() {
        let value = i as f64;
        n += count;
        sum += value * count as f64;
        sum2 += (value * value) * count as f64;
    }
    let (mode_bin, mode_count) = fullest(&counts);
    let (min, max) = match (values.first(), values.last()) {
        (Some(first), Some(last)) => (first.0, last.0),
        _ => (f64::NAN, f64::NAN),
    };
    Histogram {
        counts,
        hist_min: 0.0,
        hist_max: 255.0,
        bin_width: 1.0,
        count: n,
        min,
        max,
        mean: sum / n as f64,
        std_dev: std_dev(n as f64, sum, sum2),
        mode: mode_bin as f64,
        mode_bin,
        mode_count,
    }
}

/// Integer data from exact value counts: `ShortStatistics.getStatistics`
/// for a plane, `StackStatistics.get16BitStatistics` for a stack. `stack`
/// only changes what the mode is.
fn integer_histogram(values: &[(f64, u64)], options: &HistogramOptions, stack: bool) -> Histogram {
    let bins = options.bins;
    let (data_min, data_max) = match (values.first(), values.last()) {
        (Some(first), Some(last)) => (first.0, last.0),
        _ => (0.0, 0.0),
    };
    // The values that are counted: all of them, or those inside a set range.
    let (mut lo, mut hi) = (data_min, data_max);
    let (hist_min, hist_max) = match options.fixed_range() {
        Some((hist_min, hist_max)) => {
            if lo < hist_min {
                lo = hist_min.trunc();
            }
            if hi > hist_max {
                hi = hist_max.trunc();
            }
            (hist_min, hist_max)
        }
        None => (data_min, data_max),
    };
    let bin_width = (hist_max - hist_min + 1.0) / bins as f64;
    let scale = 1.0 / bin_width;
    let first_bin = hist_min.trunc();
    let mut counts = vec![0u64; bins];
    let (mut n, mut sum, mut sum2) = (0u64, 0.0, 0.0);
    let (mut most_frequent, mut most) = (0.0, 0u64);
    for &(v, count) in values.iter().filter(|&&(v, _)| v >= lo && v <= hi) {
        if count > most {
            most = count;
            most_frequent = v;
        }
        n += count;
        sum += v * count as f64;
        sum2 += (v * v) * count as f64;
        counts[bin_index(scale * (v - first_bin), bins)] += count;
    }
    let (mode_bin, mode_count) = fullest(&counts);
    Histogram {
        counts,
        hist_min,
        hist_max,
        bin_width,
        count: n,
        min: if values.is_empty() {
            f64::NAN
        } else {
            data_min
        },
        max: if values.is_empty() {
            f64::NAN
        } else {
            data_max
        },
        mean: sum / n as f64,
        std_dev: std_dev(n as f64, sum, sum2),
        mode: if stack {
            hist_min + mode_bin as f64 * bin_width
        } else {
            most_frequent
        },
        mode_bin,
        mode_count,
    }
}

/// Calls its argument once for each plane that takes part in a histogram.
type EachPlane<'a> = dyn Fn(&mut dyn FnMut(&Buffer) -> Result<()>) -> Result<()> + 'a;

/// Which of ImageJ's two sample-by-sample algorithms to follow.
#[derive(Clone, Copy, PartialEq)]
enum Scan {
    /// `FloatStatistics`: one float plane.
    FloatPlane,
    /// `StackStatistics.doCalculations`, for data of the given kind.
    Stack { float: bool, byte: bool },
}

/// Histogram by two passes over the samples, the first finding their range.
fn scan_histogram(each: &EachPlane, options: &HistogramOptions, scan: Scan) -> Result<Histogram> {
    fn fold<T: RealPixel>(samples: &[T], limit: f64, f: &mut dyn FnMut(f64)) {
        // ImageJ's implicit thresholds of -Float.MAX_VALUE and
        // Float.MAX_VALUE drop NaN and the infinities; 64-bit samples use
        // the limits of their own type.
        for v in samples.iter().map(|v| v.to_f64()) {
            if v >= -limit && v <= limit {
                f(v);
            }
        }
    }
    let visit = |f: &mut dyn FnMut(f64)| -> Result<()> {
        each(&mut |plane: &Buffer| {
            let limit = if plane.dtype() == DType::F64 {
                f64::MAX
            } else {
                f32::MAX as f64
            };
            dispatch_real!(plane, |s| fold(s, limit, &mut *f), complex => return Err(complex_error()));
            Ok(())
        })
    };

    let bins = options.bins;
    let (mut min, mut max) = (f64::MAX, -f64::MAX);
    let mut any = false;
    visit(&mut |v| {
        any = true;
        min = min.min(v);
        max = max.max(v);
    })?;
    if !any {
        return Ok(Histogram {
            counts: vec![0; bins],
            hist_min: 0.0,
            hist_max: 0.0,
            bin_width: 0.0,
            count: 0,
            min: f64::NAN,
            max: f64::NAN,
            mean: f64::NAN,
            std_dev: 0.0,
            mode: f64::NAN,
            mode_bin: 0,
            mode_count: 0,
        });
    }
    let (hist_min, mut hist_max) = match options.fixed_range() {
        Some((hist_min, hist_max)) => {
            min = min.max(hist_min);
            max = max.min(hist_max);
            (hist_min, hist_max)
        }
        None => (min, max),
    };

    let scale = bins as f64 / (hist_max - hist_min);
    let mut counts = vec![0u64; bins];
    let (mut n, mut sum, mut sum2) = (0u64, 0.0, 0.0);
    visit(&mut |v| {
        if v >= hist_min && v <= hist_max {
            n += 1;
            sum += v;
            sum2 += v * v;
            counts[bin_index(scale * (v - hist_min), bins)] += 1;
        }
    })?;
    let (mode_bin, mode_count) = fullest(&counts);
    let float = matches!(scan, Scan::FloatPlane | Scan::Stack { float: true, .. });
    let bin_width = if float {
        (hist_max - hist_min) / bins as f64
    } else {
        (hist_max - hist_min + 1.0) / bins as f64
    };
    let mut mode = hist_min + mode_bin as f64 * bin_width;
    match scan {
        Scan::FloatPlane if bin_width != 1.0 => mode += bin_width / 2.0,
        // An 8-bit stack is binned over 0..256 but reported as 0..255.
        Scan::Stack { byte: true, .. } if hist_min == 0.0 && hist_max == 256.0 => hist_max = 255.0,
        _ => {}
    }
    Ok(Histogram {
        counts,
        hist_min,
        hist_max,
        bin_width,
        count: n,
        min,
        max,
        mean: sum / n as f64,
        std_dev: std_dev(n as f64, sum, sum2),
        mode,
        mode_bin,
        mode_count,
    })
}

/// The histogram of some planes of one pixel type, as one plane
/// (`stack == false`, exactly one plane) or as a stack.
fn histogram_of(
    dtype: DType,
    each: &EachPlane,
    options: &HistogramOptions,
    stack: bool,
) -> Result<Histogram> {
    check_bins(options)?;
    if dtype.is_complex() {
        return Err(Error::Unsupported(
            "complex images have no histogram".to_string(),
        ));
    }
    let exact = || -> Result<Vec<(f64, u64)>> {
        let mut counts = ValueCounts::new(dtype)?;
        each(&mut |plane: &Buffer| counts.add(plane))?;
        Ok(counts.finish())
    };
    let byte = dtype == DType::U8;
    if dtype.is_float() {
        let scan = if stack {
            Scan::Stack {
                float: true,
                byte: false,
            }
        } else {
            Scan::FloatPlane
        };
        scan_histogram(each, options, scan)
    } else if byte && options.is_default() {
        Ok(byte_histogram(&exact()?))
    } else if stack && (byte || !options.is_default()) {
        // ImageJ bins 8-bit data with other settings over 0 to 256.
        let options = HistogramOptions {
            range: options.fixed_range().or(byte.then_some((0.0, 256.0))),
            ..*options
        };
        scan_histogram(each, &options, Scan::Stack { float: false, byte })
    } else if byte {
        // As ImageJ does, by converting to 16-bit and binning 0 to 256.
        let options = HistogramOptions {
            range: options.fixed_range().or(Some((0.0, 256.0))),
            ..*options
        };
        Ok(integer_histogram(&exact()?, &options, false))
    } else {
        Ok(integer_histogram(&exact()?, options, stack))
    }
}

/// The histogram and statistics of one plane of samples.
///
/// Unsigned 8-bit planes follow `ByteStatistics` (with other than 256 bins
/// or with a range they are binned as 16-bit data over 0 to 256, as the
/// macro function `getHistogram` does), other integers `ShortStatistics`
/// and floats `FloatStatistics`. See [`Histogram`] for how
/// the bins and the mode differ between them.
pub fn buffer_histogram(plane: &Buffer, options: &HistogramOptions) -> Result<Histogram> {
    histogram_of(plane.dtype(), &|f| f(plane), options, false)
}

/// The values ImageJ takes statistics of for an 8-bit RGB pixel
/// (`ColorProcessor.getHistogram` with unweighted conversion, ImageJ's
/// default): `(int)(r/3 + g/3 + b/3 + 0.5)`, computed in double precision
/// as `r*(1/3) + g*(1/3) + b*(1/3) + 0.5`.
pub fn rgb_to_gray(red: &Buffer, green: &Buffer, blue: &Buffer) -> Result<Buffer> {
    let (r, g, b) = (
        red.try_slice::<u8>()?,
        green.try_slice::<u8>()?,
        blue.try_slice::<u8>()?,
    );
    if r.len() != g.len() || r.len() != b.len() {
        return Err(Error::Shape("RGB channels differ in size".to_string()));
    }
    let weight = 1.0 / 3.0;
    let gray = |((&r, &g), &b): ((&u8, &u8), &u8)| {
        (r as f64 * weight + g as f64 * weight + b as f64 * weight + 0.5) as u8
    };
    Ok(Buffer::from_vec(
        r.iter().zip(g).zip(b).map(gray).collect::<Vec<u8>>(),
    ))
}

/// What one displayed plane contributes to a histogram: the plane itself,
/// or for an RGB image the grey value of its three channels.
fn samples_at(image: &Image, plane: usize) -> Result<Buffer> {
    let meta = image.meta();
    if meta.color != ColorModel::Rgb {
        return image.read_plane(plane);
    }
    if meta.dtype() != DType::U8 {
        return Err(Error::Unsupported(format!(
            "histograms of RGB images need 8-bit channels, not {}",
            meta.dtype()
        )));
    }
    let axis = meta
        .axis_index(&AxisKind::Channel)
        .ok_or_else(|| Error::Shape("RGB image without a channel axis".to_string()))?;
    let mut position = meta.plane_position(plane)?;
    let mut channel = |c: usize| {
        position[axis - 2] = c;
        image.read_plane(meta.plane_index(&position)?)
    };
    rgb_to_gray(&channel(0)?, &channel(1)?, &channel(2)?)
}

/// The histogram and statistics of the XY plane at `position`, as Analyze >
/// Histogram and the macro functions `getHistogram` and `getStatistics`
/// give for a single image. For an RGB image they are those of the grey
/// values from [`rgb_to_gray`], and the channel index in `position` is
/// ignored.
pub fn plane_histogram(
    image: &Image,
    position: &[usize],
    options: &HistogramOptions,
) -> Result<Histogram> {
    let samples = samples_at(image, image.meta().plane_index(position)?)?;
    buffer_histogram(&samples, options)
}

/// The histogram and statistics of every plane of `image` together
/// (`ij.process.StackStatistics`): all channels, slices and frames. RGB
/// images contribute one grey value per pixel ([`rgb_to_gray`]).
///
/// With default options, 8-bit data gives the same as a single plane would,
/// and other integers are binned into 256 bins over the range of the whole
/// stack. Floats, and any data with non-default options, follow
/// `StackStatistics.doCalculations`: `bins` equal bins over the range, with
/// `bin_width` nevertheless reported as `(hist_max - hist_min + 1) / bins`
/// for integer data. Planes are read one at a time, twice for floats.
pub fn stack_histogram(image: &Image, options: &HistogramOptions) -> Result<Histogram> {
    let meta = image.meta();
    let rgb = meta.color == ColorModel::Rgb;
    let each = |f: &mut dyn FnMut(&Buffer) -> Result<()>| -> Result<()> {
        for plane in 0..meta.plane_count() {
            // The three channels of an RGB image are one set of samples.
            if !rgb || meta.plane_channel(plane)? == 0 {
                f(&samples_at(image, plane)?)?;
            }
        }
        Ok(())
    };
    histogram_of(meta.dtype(), &each, options, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use imager_core::ImageMeta;

    fn hist<T: imager_core::Pixel>(samples: Vec<T>) -> Histogram {
        buffer_histogram(&Buffer::from_vec(samples), &HistogramOptions::default()).unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * b.abs().max(1.0)
    }

    #[test]
    fn eight_bit_plane() {
        let h = hist(vec![3u8, 3, 7, 7, 200, 0]);
        assert_eq!(h.counts.len(), 256);
        assert_eq!(
            (h.counts[0], h.counts[3], h.counts[7], h.counts[200]),
            (1, 2, 2, 1)
        );
        assert_eq!(h.counts.iter().sum::<u64>(), 6);
        assert_eq!((h.hist_min, h.hist_max, h.bin_width), (0.0, 255.0, 1.0));
        assert_eq!((h.count, h.min, h.max), (6, 0.0, 200.0));
        // sum = 220, sum of squares = 40116.
        assert!(close(h.mean, 220.0 / 6.0));
        assert!(close(
            h.std_dev,
            ((6.0 * 40116.0 - 220.0 * 220.0) / 6.0 / 5.0f64).sqrt()
        ));
        // The first of the equally frequent values.
        assert_eq!((h.mode, h.mode_bin, h.mode_count), (3.0, 3, 2));
        assert_eq!(h.bin_edges().len(), 257);
        assert_eq!(h.bin_start(200), 200.0);
    }

    #[test]
    fn flat_images() {
        let h = hist(vec![9u8; 4]);
        assert_eq!(
            (h.min, h.max, h.mean, h.std_dev, h.mode),
            (9.0, 9.0, 9.0, 0.0, 9.0)
        );
        // 16-bit: one value, binSize = 1/256, everything in bin 0.
        let h = hist(vec![500u16; 4]);
        assert_eq!(
            (h.hist_min, h.hist_max, h.bin_width),
            (500.0, 500.0, 1.0 / 256.0)
        );
        assert_eq!(
            (h.counts[0], h.count, h.mode, h.std_dev),
            (4, 4, 500.0, 0.0)
        );
        // Float: zero-width bins; (int)(inf * 0) = 0 puts everything in bin 0,
        // and the mode is histMin + 0 + 0/2.
        let h = hist(vec![2.5f32; 4]);
        assert_eq!((h.hist_min, h.hist_max, h.bin_width), (2.5, 2.5, 0.0));
        assert_eq!((h.counts[0], h.count, h.mode, h.mean), (4, 4, 2.5, 2.5));
        // A single sample has no deviation.
        assert_eq!(hist(vec![7.0f64]).std_dev, 0.0);
    }

    #[test]
    fn sixteen_bit_plane_bins_over_the_data_range() {
        // min 100, max 611: binSize = 512 / 256 = 2, index = (int)((v - 100) / 2).
        let h = hist(vec![100u16, 101, 102, 611, 611, 611, 300]);
        assert_eq!((h.hist_min, h.hist_max, h.bin_width), (100.0, 611.0, 2.0));
        assert_eq!(
            (h.counts[0], h.counts[1], h.counts[100], h.counts[255]),
            (2, 1, 1, 3)
        );
        assert_eq!(h.counts.iter().sum::<u64>(), 7);
        assert_eq!((h.count, h.min, h.max), (7, 100.0, 611.0));
        assert!(close(h.mean, 2436.0 / 7.0));
        // The mode is the most frequent value, not a bin position.
        assert_eq!((h.mode, h.mode_bin, h.mode_count), (611.0, 255, 3));
        // The top edge: a range of 1000 values in 256 bins, binSize = 3.90625;
        // (int)(999 / 3.90625) = 255.
        let h = hist(vec![0u16, 999]);
        assert_eq!((h.counts[0], h.counts[255]), (1, 1));
        assert_eq!(h.bin_edges()[256], 1000.0);
    }

    #[test]
    fn sixteen_bit_plane_with_bins_and_range() {
        let options = HistogramOptions {
            bins: 4,
            range: Some((10.0, 49.0)),
        };
        // binSize = 40 / 4 = 10. 5 and 50 are outside and not counted, but
        // min and max still report them (ShortStatistics keeps the raw ones).
        let plane = Buffer::from_vec(vec![5u16, 10, 19, 20, 49, 50, 49]);
        let h = buffer_histogram(&plane, &options).unwrap();
        assert_eq!(h.counts, [2, 1, 0, 2]);
        assert_eq!((h.count, h.min, h.max, h.bin_width), (5, 5.0, 50.0, 10.0));
        assert!(close(h.mean, 147.0 / 5.0));
        assert_eq!((h.mode, h.mode_bin), (49.0, 0));
        // (0, 0) means "from the data".
        let auto = HistogramOptions {
            bins: 4,
            range: Some((0.0, 0.0)),
        };
        let h = buffer_histogram(&plane, &auto).unwrap();
        assert_eq!((h.hist_min, h.hist_max, h.count), (5.0, 50.0, 7));
    }

    #[test]
    fn eight_bit_plane_with_other_bin_counts() {
        // As 16-bit data over 0..256: binSize = 257 / 64.
        let options = HistogramOptions {
            bins: 64,
            range: None,
        };
        let h = buffer_histogram(&Buffer::from_vec(vec![0u8, 4, 5, 255]), &options).unwrap();
        assert_eq!(
            (h.hist_min, h.hist_max, h.bin_width),
            (0.0, 256.0, 257.0 / 64.0)
        );
        // 4 / 4.0156 = 0.996 -> bin 0; 5 -> 1; 255 -> 63.
        assert_eq!((h.counts[0], h.counts[1], h.counts[63]), (2, 1, 1));
        assert_eq!((h.min, h.max, h.mode), (0.0, 255.0, 0.0));
    }

    #[test]
    fn signed_and_wide_integers_use_the_sixteen_bit_rule() {
        let h = hist(vec![-3i16, -3, 0, 252]);
        assert_eq!((h.hist_min, h.hist_max, h.bin_width), (-3.0, 252.0, 1.0));
        assert_eq!((h.counts[0], h.counts[3], h.counts[255]), (2, 1, 1));
        assert_eq!((h.mode, h.min, h.max), (-3.0, -3.0, 252.0));
        let h = hist(vec![1_000_000i32, 1_000_000, 3_000_000_i32, -5]);
        assert_eq!((h.min, h.max, h.mode, h.count), (-5.0, 3e6, 1e6, 4));
        assert_eq!((h.counts[0], h.counts[255]), (1, 1));
        assert_eq!(h.counts.iter().sum::<u64>(), 4);
        let counts = value_counts(&Buffer::from_vec(vec![7u64, 1 << 40, 7])).unwrap();
        assert_eq!(counts, [(7.0, 2), ((1u64 << 40) as f64, 1)]);
        let counts = value_counts(&Buffer::from_vec(vec![-1i8, 5, -1, -128])).unwrap();
        assert_eq!(counts, [(-128.0, 1), (-1.0, 2), (5.0, 1)]);
        assert!(value_counts(&Buffer::from_vec(vec![1.0f32])).is_err());
    }

    #[test]
    fn float_plane() {
        // min 0, max 8: scale = 256 / 8 = 32, binSize = 1/32.
        let h = hist(vec![
            0.0f32,
            0.03,
            0.04,
            4.0,
            4.0,
            4.01,
            8.0,
            f32::NAN,
            f32::INFINITY,
        ]);
        assert_eq!((h.hist_min, h.hist_max, h.bin_width), (0.0, 8.0, 0.03125));
        // 0.03 * 32 = 0.96 -> 0; 0.04 * 32 = 1.28 -> 1; 4 -> 128; 8 -> 256 -> 255.
        assert_eq!(
            (h.counts[0], h.counts[1], h.counts[128], h.counts[255]),
            (2, 1, 3, 1)
        );
        // NaN and infinity are not counted at all.
        assert_eq!((h.count, h.min, h.max), (7, 0.0, 8.0));
        assert_eq!(h.counts.iter().sum::<u64>(), 7);
        let values = [0.0f32, 0.03, 0.04, 4.0, 4.0, 4.01, 8.0].map(|v| v as f64);
        assert!(close(h.mean, values.iter().sum::<f64>() / 7.0));
        // The centre of the fullest bin: 128/32 + 1/64.
        assert_eq!((h.mode, h.mode_bin, h.mode_count), (4.015625, 128, 3));
    }

    #[test]
    fn float_plane_with_bins_and_range() {
        let options = HistogramOptions {
            bins: 4,
            range: Some((0.0, 2.0)),
        };
        let plane = Buffer::from_vec(vec![-1.0f32, 0.0, 0.4, 0.5, 1.9, 2.0, 3.0]);
        let h = buffer_histogram(&plane, &options).unwrap();
        assert_eq!(h.counts, [2, 1, 0, 2]);
        // FloatStatistics clips the reported min and max to the range.
        assert_eq!((h.count, h.min, h.max, h.bin_width), (5, 0.0, 2.0, 0.5));
        // binSize 0.5: histMin + 0 * 0.5 + 0.25.
        assert_eq!(h.mode, 0.25);
        // With a bin width of exactly 1 ImageJ reports the start of the bin.
        let unit = HistogramOptions {
            bins: 2,
            range: Some((0.0, 2.0)),
        };
        assert_eq!(buffer_histogram(&plane, &unit).unwrap().mode, 0.0);
    }

    #[test]
    fn nothing_to_count() {
        let h = hist(vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
        assert_eq!((h.count, h.counts.iter().sum::<u64>()), (0, 0));
        assert!(h.min.is_nan() && h.max.is_nan() && h.mean.is_nan() && h.mode.is_nan());
        assert_eq!(h.std_dev, 0.0);
        // f64 keeps values that would be infinite as f32.
        let h = hist(vec![1e300f64, -1e300, f64::INFINITY]);
        assert_eq!((h.count, h.min, h.max), (2, -1e300, 1e300));
        let zero = HistogramOptions {
            bins: 0,
            range: None,
        };
        assert!(buffer_histogram(&Buffer::from_vec(vec![1u8]), &zero).is_err());
        assert!(buffer_histogram(&Buffer::zeros(DType::C64, 1), &Default::default()).is_err());
    }

    #[test]
    fn rgb_gray_values() {
        let plane = |v: Vec<u8>| Buffer::from_vec(v);
        // (int)((r + g + b) / 3 + 0.5): 1,1,0 -> 1.17 -> 1; 255,255,254 -> 255.17.
        let gray = rgb_to_gray(
            &plane(vec![0, 1, 255, 10, 255]),
            &plane(vec![0, 1, 255, 20, 255]),
            &plane(vec![0, 0, 254, 31, 255]),
        )
        .unwrap();
        assert_eq!(gray.as_slice::<u8>().unwrap(), [0, 1, 255, 20, 255]);
        assert!(rgb_to_gray(&plane(vec![0]), &plane(vec![0, 1]), &plane(vec![0])).is_err());

        let mut meta = ImageMeta::xyczt(DType::U8, 2, 1, 3, 2, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let planes = [[10u8, 0], [20, 0], [31, 3], [90, 90], [90, 90], [90, 90]]
            .map(|p| Buffer::from_vec(p.to_vec()))
            .to_vec();
        let image = Image::from_planes(meta, planes).unwrap();
        let h = plane_histogram(&image, &[0, 0], &Default::default()).unwrap();
        assert_eq!((h.count, h.counts[20], h.counts[1]), (2, 1, 1));
        // The channel coordinate is ignored.
        assert_eq!(
            plane_histogram(&image, &[2, 0], &Default::default()).unwrap(),
            h
        );
        let h = stack_histogram(&image, &Default::default()).unwrap();
        assert_eq!((h.count, h.counts[90], h.mode), (4, 2, 90.0));
    }

    fn stack<T: imager_core::Pixel>(planes: Vec<Vec<T>>) -> Image {
        let meta = ImageMeta::xyczt(T::DTYPE, planes[0].len(), 1, 1, planes.len(), 1).unwrap();
        Image::from_planes(meta, planes.into_iter().map(Buffer::from_vec).collect()).unwrap()
    }

    #[test]
    fn stacks_of_integers() {
        let image = stack(vec![vec![1u8, 2], vec![2, 9]]);
        let h = stack_histogram(&image, &Default::default()).unwrap();
        assert_eq!(
            (h.count, h.counts[2], h.mode, h.min, h.max),
            (4, 2, 2.0, 1.0, 9.0)
        );
        let plane = plane_histogram(&image, &[1], &Default::default()).unwrap();
        assert_eq!((plane.count, plane.min, plane.max), (2, 2.0, 9.0));

        // 16-bit: range 100..=611 over both planes, binSize 2. The stack's
        // mode is the start of the fullest bin (100 + 255 * 2), where a
        // plane's would be the most frequent value (611).
        let image = stack(vec![vec![100u16, 101, 102], vec![611, 611, 300]]);
        let h = stack_histogram(&image, &Default::default()).unwrap();
        assert_eq!((h.hist_min, h.hist_max, h.bin_width), (100.0, 611.0, 2.0));
        assert_eq!(
            (h.counts[0], h.counts[1], h.counts[100], h.counts[255]),
            (2, 1, 1, 2)
        );
        assert_eq!((h.count, h.mode, h.mode_bin), (6, 100.0, 0));
        let image = stack(vec![vec![100u16, 101, 102], vec![611, 611, 611]]);
        let h = stack_histogram(&image, &Default::default()).unwrap();
        assert_eq!((h.mode, h.mode_bin, h.mode_count), (610.0, 255, 3));
    }

    #[test]
    fn stacks_of_floats_and_custom_options() {
        // Range 0..8 over both planes; the mode is the start of the fullest
        // bin (no half-bin offset, unlike a single float plane).
        let image = stack(vec![vec![0.0f32, 4.0, f32::NAN], vec![4.0, 4.01, 8.0]]);
        let h = stack_histogram(&image, &Default::default()).unwrap();
        assert_eq!((h.hist_min, h.hist_max, h.bin_width), (0.0, 8.0, 0.03125));
        assert_eq!(
            (h.count, h.counts[128], h.counts[255], h.mode),
            (5, 3, 1, 4.0)
        );

        // Integer stacks with custom options are binned like floats, with
        // scale = bins / (max - min), but report binSize with the + 1.
        let image = stack(vec![vec![0u16, 10, 20], vec![30, 39, 40]]);
        let options = HistogramOptions {
            bins: 4,
            range: None,
        };
        let h = stack_histogram(&image, &options).unwrap();
        // scale = 4 / 40: 10 -> 1, 20 -> 2, 30 -> 3, 39 -> 3, 40 -> 4 -> 3.
        assert_eq!(h.counts, [1, 1, 1, 3]);
        assert_eq!((h.bin_width, h.mode), (41.0 / 4.0, 3.0 * 41.0 / 4.0));

        // An 8-bit stack with other than 256 bins spans 0..256 and reports 0..255.
        let image = stack(vec![vec![0u8, 63], vec![64, 255]]);
        let h = stack_histogram(&image, &options).unwrap();
        assert_eq!(h.counts, [2, 1, 0, 1]);
        assert_eq!(
            (h.hist_min, h.hist_max, h.bin_width),
            (0.0, 255.0, 257.0 / 4.0)
        );
    }
}
