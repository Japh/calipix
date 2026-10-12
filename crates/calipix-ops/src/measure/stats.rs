//! Statistics of the samples inside a region of one plane, computed the
//! way ImageJ's `ij.process.ImageStatistics` family computes them.

use calipix_core::{AxisKind, Buffer, Error, ImageMeta, RealPixel, Rect, Result, dispatch_real};
use calipix_roi::Mask;

use super::Measurements;
use super::ellipse::fit_ellipse;

/// The spatial calibration measurements are reported in, as ImageJ's
/// `ij.measure.Calibration` holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    /// Width and height of a pixel in `unit`.
    pub pixel_width: f64,
    pub pixel_height: f64,
    /// The pixel coordinates of the point that is reported as (0, 0).
    pub x_origin: f64,
    pub y_origin: f64,
    /// Report Y coordinates increasing upwards ("Invert Y coordinates").
    pub invert_y: bool,
    pub unit: Option<String>,
}

impl Default for Calibration {
    fn default() -> Calibration {
        Calibration {
            pixel_width: 1.0,
            pixel_height: 1.0,
            x_origin: 0.0,
            y_origin: 0.0,
            invert_y: false,
            unit: None,
        }
    }
}

impl Calibration {
    /// The calibration of an image's X and Y axes. An axis places sample
    /// `i` at `origin + i * scale`; ImageJ's origin is the sample index
    /// that lies at zero.
    pub fn from_meta(meta: &ImageMeta) -> Calibration {
        let axis = |kind: AxisKind| {
            let axis = &meta.axes()[meta.axis_index(&kind).unwrap_or(0)];
            let origin = if axis.origin == 0.0 {
                0.0
            } else {
                -axis.origin / axis.scale
            };
            (axis.scale, origin, axis.unit.clone())
        };
        let (pixel_width, x_origin, unit) = axis(AxisKind::X);
        let (pixel_height, y_origin, _) = axis(AxisKind::Y);
        Calibration {
            pixel_width,
            pixel_height,
            x_origin,
            y_origin,
            invert_y: false,
            unit,
        }
    }

    /// `Calibration.getX(double)`.
    pub fn x(&self, x: f64) -> f64 {
        (x - self.x_origin) * self.pixel_width
    }

    /// `Calibration.getY(double, int)`: with "Invert Y coordinates" the
    /// result counts up from the bottom row, or from the origin if one is
    /// set.
    pub fn y(&self, y: f64, image_height: usize) -> f64 {
        if self.invert_y {
            if self.y_origin != 0.0 {
                (self.y_origin - y) * self.pixel_height
            } else {
                (image_height as f64 - y - 1.0) * self.pixel_height
            }
        } else {
            (y - self.y_origin) * self.pixel_height
        }
    }
}

/// The samples of one plane as ImageJ would hold them in one
/// `ImageProcessor`.
#[derive(Clone, Copy, Debug)]
pub enum Samples<'a> {
    /// One channel.
    Gray(&'a Buffer),
    /// The three 8-bit channels of an RGB image, which ImageJ measures
    /// through its grey conversion.
    Rgb {
        red: &'a Buffer,
        green: &'a Buffer,
        blue: &'a Buffer,
    },
}

/// The part of a plane to take statistics of.
#[derive(Clone, Copy, Debug)]
pub struct Region<'a> {
    pub rect: Rect,
    /// One byte per pixel of `rect`, row-major, non-zero inside. `None`
    /// takes every pixel of `rect`.
    pub mask: Option<&'a [u8]>,
}

impl<'a> Region<'a> {
    /// Every pixel of a `width` x `height` plane.
    pub fn whole(width: usize, height: usize) -> Region<'static> {
        Region {
            rect: Rect::new(0, 0, width, height),
            mask: None,
        }
    }

    pub fn rect(rect: Rect) -> Region<'static> {
        Region { rect, mask: None }
    }

    /// The pixels a selection's mask covers.
    pub fn masked(mask: &'a Mask) -> Region<'a> {
        Region {
            rect: mask.rect,
            mask: Some(&mask.data),
        }
    }

    fn selected(&self, column: usize, row: usize) -> bool {
        self.mask.is_none_or(|m| m[row * self.rect.w + column] != 0)
    }

    /// Calls `f(x, y, index)` for each selected pixel, in row-major order.
    fn for_each(&self, width: usize, mut f: impl FnMut(usize, usize, usize)) {
        let r = self.rect;
        for row in 0..r.h {
            let y = r.y + row;
            for column in 0..r.w {
                if self.selected(column, row) {
                    f(r.x + column, y, y * width + r.x + column);
                }
            }
        }
    }
}

/// Which histogram to build alongside the statistics. The default is
/// ImageJ's: 256 bins over the range of the data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HistogramSpec {
    pub bins: usize,
    /// The values the bins span (`histMin`, `histMax`); `None` or `(0, 0)`
    /// for the smallest and largest sample.
    pub range: Option<(f64, f64)>,
}

impl Default for HistogramSpec {
    fn default() -> HistogramSpec {
        HistogramSpec {
            bins: 256,
            range: None,
        }
    }
}

impl HistogramSpec {
    fn fixed_range(&self) -> Option<(f64, f64)> {
        self.range.filter(|&range| range != (0.0, 0.0))
    }

    fn is_default(&self) -> bool {
        self.bins == 256 && self.fixed_range().is_none()
    }
}

/// Everything `ImageStatistics` reports. Lengths and positions are in
/// calibrated units; sample values are as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Statistics {
    /// Number of samples counted (`pixelCount`).
    pub pixel_count: u64,
    pub area: f64,
    pub mean: f64,
    /// Mean of the stored values (`umean`); differs from `mean` only under
    /// a value calibration, which Calipix does not have yet.
    pub raw_mean: f64,
    pub std_dev: f64,
    /// The mode as ImageJ reports it (`dmode`).
    pub mode: f64,
    pub min: f64,
    pub max: f64,
    pub median: f64,
    pub skewness: f64,
    pub kurtosis: f64,
    /// Percentage of non-zero samples (`areaFraction` without a threshold).
    pub area_fraction: f64,
    pub x_centroid: f64,
    pub y_centroid: f64,
    pub x_center_of_mass: f64,
    pub y_center_of_mass: f64,
    /// The bounding rectangle (`roiX`, `roiY`, `roiWidth`, `roiHeight`).
    pub roi_x: f64,
    pub roi_y: f64,
    pub roi_width: f64,
    pub roi_height: f64,
    /// Axes and angle (degrees) of the best-fitting ellipse.
    pub major: f64,
    pub minor: f64,
    pub angle: f64,
    /// Samples per bin; bin `i` starts at `hist_min + i * bin_size`.
    pub histogram: Vec<u64>,
    pub hist_min: f64,
    pub hist_max: f64,
    pub bin_size: f64,
    /// Index and count of the fullest bin (`mode`, `maxCount`).
    pub mode_bin: usize,
    pub max_count: u64,
}

impl Statistics {
    /// The values ImageJ's `new ImageStatistics()` starts from.
    pub(super) fn zeroed() -> Statistics {
        Statistics {
            pixel_count: 0,
            area: 0.0,
            mean: 0.0,
            raw_mean: 0.0,
            std_dev: 0.0,
            mode: 0.0,
            min: 0.0,
            max: 0.0,
            median: 0.0,
            skewness: 0.0,
            kurtosis: 0.0,
            area_fraction: 0.0,
            x_centroid: 0.0,
            y_centroid: 0.0,
            x_center_of_mass: 0.0,
            y_center_of_mass: 0.0,
            roi_x: 0.0,
            roi_y: 0.0,
            roi_width: 0.0,
            roi_height: 0.0,
            major: 0.0,
            minor: 0.0,
            angle: 0.0,
            histogram: vec![0; 256],
            hist_min: 0.0,
            hist_max: 0.0,
            bin_size: 1.0,
            mode_bin: 0,
            max_count: 0,
        }
    }
}

/// Java's `Double.MIN_VALUE`, which ImageJ adds to 8-bit and float samples
/// when summing moments so that an all-zero region still has a centre of
/// mass.
const TINY: f64 = f64::from_bits(1);

/// The grey value ImageJ takes statistics of for an RGB pixel with its
/// default, unweighted conversion (`ColorProcessor.getHistogram`). The same
/// formula is in `calipix_render::rgb_to_gray`; the two could share one
/// home in `calipix-core`.
fn gray_level(r: u8, g: u8, b: u8) -> u8 {
    let w = 1.0 / 3.0;
    (r as f64 * w + g as f64 * w + b as f64 * w + 0.5) as u8
}

/// `ColorProcessor.getPixelValue`: the unrounded grey value, in single
/// precision.
fn gray_value(r: u8, g: u8, b: u8) -> f64 {
    let w = 1.0 / 3.0;
    (r as f64 * w + g as f64 * w + b as f64 * w) as f32 as f64
}

fn rgb_slices<'a>(
    red: &'a Buffer,
    green: &'a Buffer,
    blue: &'a Buffer,
) -> Result<(&'a [u8], &'a [u8], &'a [u8])> {
    let unsupported = |b: &Buffer| {
        Error::Unsupported(format!(
            "RGB images are measured through 8-bit channels, not {}",
            b.dtype()
        ))
    };
    let slice = |b: &'a Buffer| b.as_slice::<u8>().ok_or_else(|| unsupported(b));
    let (r, g, b) = (slice(red)?, slice(green)?, slice(blue)?);
    if r.len() != g.len() || r.len() != b.len() {
        return Err(Error::Shape("RGB channels differ in size".to_string()));
    }
    Ok((r, g, b))
}

/// `ImageStatistics.calculateStdDev`.
fn std_dev(n: f64, sum: f64, sum2: f64) -> f64 {
    if n > 0.0 {
        let v = (n * sum2 - sum * sum) / n;
        if v > 0.0 { (v / (n - 1.0)).sqrt() } else { 0.0 }
    } else {
        0.0
    }
}

/// Statistics of the samples of `region` in a `width` x `height` plane.
///
/// Follows `ImageStatistics.getStatistics(ip, mOptions, cal)` as called by
/// `ImagePlus.getStatistics`:
///
/// | Samples | ImageJ class |
/// |---|---|
/// | 8-bit with the default histogram | `ByteStatistics` |
/// | other integers; 8-bit with other bins or range | `ShortStatistics` |
/// | float | `FloatStatistics` |
/// | RGB | `ColorStatistics` on the grey conversion |
///
/// `what` matters in one place only, as in ImageJ: with "Fit ellipse" or
/// "Shape descriptors" the centroid is the centre of the fitted ellipse,
/// otherwise the mean position of the pixels. Everything else is always
/// computed. Integer types ImageJ lacks follow the 16-bit rules and 64-bit
/// floats the 32-bit rules (with the median averaged in double precision).
/// NaN and infinite floats are not counted.
pub fn statistics(
    samples: Samples<'_>,
    width: usize,
    height: usize,
    region: Region<'_>,
    cal: &Calibration,
    what: Measurements,
    histogram: HistogramSpec,
) -> Result<Statistics> {
    let rect = region.rect;
    if !rect.fits(width, height) {
        return Err(Error::OutOfBounds(format!(
            "region {rect:?} outside {width}x{height} plane"
        )));
    }
    if region.mask.is_some_and(|m| m.len() != rect.w * rect.h) {
        return Err(Error::Shape(format!(
            "mask does not have one entry per pixel of {rect:?}"
        )));
    }
    if histogram.bins == 0 {
        return Err(Error::Other(
            "a histogram needs at least one bin".to_string(),
        ));
    }
    let len = match samples {
        Samples::Gray(b) => b.len(),
        Samples::Rgb { red, .. } => red.len(),
    };
    if len != width * height {
        return Err(Error::Shape(format!(
            "plane of {len} samples is not {width}x{height}"
        )));
    }

    let mut stats = Statistics::zeroed();
    // ImageStatistics.setup
    stats.roi_x = cal.x(rect.x as f64);
    stats.roi_y = cal.y(rect.y as f64, height);
    stats.roi_width = rect.w as f64 * cal.pixel_width;
    stats.roi_height = rect.h as f64 * cal.pixel_height;

    let job = Job {
        width,
        height,
        region,
        cal,
        what,
        histogram,
    };
    match samples {
        Samples::Rgb { red, green, blue } => {
            let (r, g, b) = rgb_slices(red, green, blue)?;
            if histogram.is_default() {
                job.color(r, g, b, &mut stats);
            } else {
                // ImagePlus.getStatistics converts to 16-bit first.
                let gray: Vec<u8> = (0..r.len()).map(|i| gray_level(r[i], g[i], b[i])).collect();
                job.integer(&gray, &mut stats);
            }
        }
        Samples::Gray(buffer) => match buffer {
            Buffer::U8(s) if histogram.is_default() => job.byte(s, &mut stats),
            Buffer::F32(s) => job.float(s, f32::MAX as f64, true, &mut stats),
            Buffer::F64(s) => job.float(s, f64::MAX, false, &mut stats),
            other => dispatch_real!(other, |s| job.integer(s, &mut stats), complex => {
                return Err(Error::Unsupported(
                    "complex images cannot be measured; take the magnitude or a component first"
                        .to_string(),
                ));
            }),
        },
    }
    Ok(stats)
}

struct Job<'a> {
    width: usize,
    height: usize,
    region: Region<'a>,
    cal: &'a Calibration,
    what: Measurements,
    histogram: HistogramSpec,
}

/// The power sums `calculateMoments` accumulates.
#[derive(Default)]
struct Moments {
    sum1: f64,
    sum2: f64,
    sum3: f64,
    sum4: f64,
    xsum: f64,
    ysum: f64,
}

impl Moments {
    fn add(&mut self, x: usize, y: usize, v: f64) {
        let v2 = v * v;
        self.sum1 += v;
        self.sum2 += v2;
        self.sum3 += v * v2;
        self.sum4 += v2 * v2;
        self.xsum += x as f64 * v;
        self.ysum += y as f64 * v;
    }
}

impl Job<'_> {
    fn pixel_area(&self) -> f64 {
        self.cal.pixel_width * self.cal.pixel_height
    }

    /// The centroid: `fitEllipse` when the ellipse is wanted, else
    /// `getCentroid` over the pixels `counted` accepts.
    fn shape(&self, stats: &mut Statistics, counted: impl Fn(usize) -> bool) {
        let (x, y);
        if self.what.fit_ellipse || self.what.shape_descriptors {
            let e = fit_ellipse(self.region.rect, self.region.mask);
            // ImageStatistics.fitEllipse: no axis lengths unless pixels are
            // square to within 1%.
            let (pw, ph) = (self.cal.pixel_width, self.cal.pixel_height);
            let size = if ((pw - ph).abs() / pw) < 0.01 {
                pw
            } else {
                0.0
            };
            stats.major = e.major * size;
            stats.minor = e.minor * size;
            stats.angle = e.angle;
            (x, y) = (e.x_center, e.y_center);
        } else {
            let (mut count, mut xsum, mut ysum) = (0.0, 0.0, 0.0);
            self.region.for_each(self.width, |px, py, i| {
                if counted(i) {
                    count += 1.0;
                    xsum += px as f64;
                    ysum += py as f64;
                }
            });
            (x, y) = (xsum / count + 0.5, ysum / count + 0.5);
        }
        stats.x_centroid = self.cal.x(x);
        stats.y_centroid = self.cal.y(y, self.height);
    }

    /// The tail of every `calculateMoments`.
    fn finish_moments(&self, m: &Moments, stats: &mut Statistics) {
        let n = stats.pixel_count as f64;
        let mean = stats.mean;
        let mean2 = mean * mean;
        let variance = m.sum2 / n - mean2;
        let deviation = variance.sqrt();
        stats.skewness =
            ((m.sum3 - 3.0 * mean * m.sum2) / n + 2.0 * mean * mean2) / (variance * deviation);
        stats.kurtosis = ((m.sum4 - 4.0 * mean * m.sum3 + 6.0 * mean2 * m.sum2) / n
            - 3.0 * mean2 * mean2)
            / (variance * variance)
            - 3.0;
        stats.x_center_of_mass = self.cal.x(m.xsum / m.sum1 + 0.5);
        stats.y_center_of_mass = self.cal.y(m.ysum / m.sum1 + 0.5, self.height);
    }

    /// `getRawStatistics(0, 255)`, `getRawMinAndMax(0, 255)` and
    /// `calculateMedian` over a 256-level histogram.
    fn levels(&self, hist: &[u64; 256], stats: &mut Statistics) {
        let (mut n, mut sum, mut sum2) = (0u64, 0.0, 0.0);
        for (i, &count) in hist.iter().enumerate() {
            n += count;
            let value = i as f64;
            sum += value * count as f64;
            sum2 += value * value * count as f64;
            if count > stats.max_count {
                stats.max_count = count;
                stats.mode_bin = i;
            }
        }
        stats.pixel_count = n;
        stats.area = n as f64 * self.pixel_area();
        stats.mean = sum / n as f64;
        stats.raw_mean = stats.mean;
        stats.mode = stats.mode_bin as f64;
        stats.std_dev = std_dev(n as f64, sum, sum2);
        stats.histogram = hist.to_vec();
        stats.hist_min = 0.0;
        stats.hist_max = 255.0;
        stats.bin_size = 1.0;
        if n == 0 {
            stats.min = f64::NAN;
            stats.max = f64::NAN;
            stats.median = f64::NAN;
            return;
        }
        stats.min = hist.iter().position(|&c| c != 0).unwrap_or(255) as f64;
        stats.max = hist.iter().rposition(|&c| c != 0).unwrap_or(0) as f64;
        // calculateMedian
        let half = n as f64 / 2.0;
        let mut running = 0.0;
        let mut i = 0;
        loop {
            running += hist[i] as f64;
            if running > half || i == 255 {
                break;
            }
            i += 1;
        }
        stats.median = i as f64;
    }

    /// `ByteStatistics`.
    fn byte(&self, s: &[u8], stats: &mut Statistics) {
        let mut hist = [0u64; 256];
        self.region
            .for_each(self.width, |_, _, i| hist[s[i] as usize] += 1);
        self.levels(&hist, stats);
        self.shape(stats, |_| true);
        let mut m = Moments::default();
        self.region
            .for_each(self.width, |x, y, i| m.add(x, y, s[i] as f64 + TINY));
        self.finish_moments(&m, stats);
        // calculateAreaFraction without a threshold: the non-zero share.
        let n = stats.pixel_count;
        stats.area_fraction = (n - hist[0]) as f64 * 100.0 / n as f64;
    }

    /// `ColorStatistics`.
    fn color(&self, r: &[u8], g: &[u8], b: &[u8], stats: &mut Statistics) {
        let mut hist = [0u64; 256];
        self.region.for_each(self.width, |_, _, i| {
            hist[gray_level(r[i], g[i], b[i]) as usize] += 1
        });
        self.levels(&hist, stats);
        self.shape(stats, |_| true);
        let mut m = Moments::default();
        self.region.for_each(self.width, |x, y, i| {
            m.add(x, y, gray_value(r[i], g[i], b[i]))
        });
        self.finish_moments(&m, stats);
        // ColorStatistics never computes the area fraction; it stays 0.
    }

    /// `ShortStatistics`, generalised to any integer type by working on
    /// the distinct values in increasing order.
    fn integer<T: RealPixel>(&self, s: &[T], stats: &mut Statistics) {
        // How often each value occurs, in increasing order of value.
        let mut counts: Vec<(f64, u64)> = Vec::new();
        if T::DTYPE.size() <= 2 {
            // Few enough possible values to count into a table.
            const OFFSET: i64 = 1 << 15;
            let mut table = vec![0u64; 3 << 15];
            self.region.for_each(self.width, |_, _, i| {
                table[(s[i].to_f64() as i64 + OFFSET) as usize] += 1
            });
            counts.extend(
                table
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| **c != 0)
                    .map(|(i, &c)| ((i as i64 - OFFSET) as f64, c)),
            );
        } else {
            let mut values: Vec<f64> = Vec::new();
            self.region
                .for_each(self.width, |_, _, i| values.push(s[i].to_f64()));
            values.sort_unstable_by(f64::total_cmp);
            for v in values {
                match counts.last_mut() {
                    Some((last, count)) if *last == v => *count += 1,
                    _ => counts.push((v, 1)),
                }
            }
        }
        let bins = self.histogram.bins;
        stats.histogram = vec![0; bins];
        let (Some(&(data_min, _)), Some(&(data_max, _))) = (counts.first(), counts.last()) else {
            stats.min = f64::NAN;
            stats.max = f64::NAN;
            stats.mean = f64::NAN;
            stats.raw_mean = f64::NAN;
            stats.median = f64::NAN;
            return;
        };
        stats.min = data_min;
        stats.max = data_max;

        // getStatistics(ip, hist, min, max, cTable)
        let (mut low, mut high) = (data_min, data_max);
        let (hist_min, hist_max) = match self.histogram.fixed_range() {
            None => (data_min, data_max),
            Some((a, b)) => {
                if low < a {
                    low = a.trunc();
                }
                if high > b {
                    high = b.trunc();
                }
                (a, b)
            }
        };
        stats.hist_min = hist_min;
        stats.hist_max = hist_max;
        stats.bin_size = (hist_max - hist_min + 1.0) / bins as f64;
        let scale = 1.0 / stats.bin_size;
        let first = hist_min.trunc();
        let (mut n, mut sum, mut sum2, mut best) = (0u64, 0.0, 0.0, 0u64);
        for &(v, count) in counts.iter().filter(|(v, _)| *v >= low && *v <= high) {
            if count > best {
                best = count;
                stats.mode = v;
            }
            n += count;
            sum += v * count as f64;
            sum2 += v * v * count as f64;
            let index = ((scale * (v - first)) as i64).clamp(0, bins as i64 - 1) as usize;
            stats.histogram[index] += count;
        }
        stats.pixel_count = n;
        stats.area = n as f64 * self.pixel_area();
        stats.mean = sum / n as f64;
        stats.raw_mean = stats.mean;
        stats.std_dev = std_dev(n as f64, sum, sum2);
        // getMode
        for (i, &count) in stats.histogram.iter().enumerate() {
            if count > stats.max_count {
                stats.max_count = count;
                stats.mode_bin = i;
            }
        }

        self.shape(stats, |_| true);
        let mut m = Moments::default();
        self.region
            .for_each(self.width, |x, y, i| m.add(x, y, s[i].to_f64()));
        self.finish_moments(&m, stats);

        // calculateMedian walks every value, in the histogram range or not.
        if n == 0 {
            stats.median = f64::NAN;
        } else {
            let half = n as f64 / 2.0;
            let mut running = 0.0;
            stats.median = data_max;
            for &(v, count) in &counts {
                running += count as f64;
                if running > half {
                    stats.median = v;
                    break;
                }
            }
        }
        // calculateAreaFraction without a threshold: the non-zero share.
        let total: u64 = counts.iter().map(|&(_, c)| c).sum();
        let zeros: u64 = counts
            .iter()
            .filter(|&&(v, _)| v == 0.0)
            .map(|&(_, c)| c)
            .sum();
        stats.area_fraction = (total - zeros) as f64 * 100.0 / total as f64;
    }

    /// `FloatStatistics`. `limit` is the largest finite value of the pixel
    /// type; `single` averages the two middle values of an even-sized
    /// median in single precision, as ImageJ does.
    fn float<T: RealPixel>(&self, s: &[T], limit: f64, single: bool, stats: &mut Statistics) {
        let finite = |v: f64| v >= -limit && v <= limit;
        let bins = self.histogram.bins;
        let (mut min, mut max) = (f64::MAX, -f64::MAX);
        self.region.for_each(self.width, |_, _, i| {
            let v = s[i].to_f64();
            if finite(v) {
                if v < min {
                    min = v;
                }
                if v > max {
                    max = v;
                }
            }
        });
        let (hist_min, hist_max) = match self.histogram.fixed_range() {
            None => (min, max),
            Some((a, b)) => {
                min = min.max(a);
                max = max.min(b);
                (a, b)
            }
        };
        stats.min = min;
        stats.max = max;
        stats.hist_min = hist_min;
        stats.hist_max = hist_max;
        stats.bin_size = (hist_max - hist_min) / bins as f64;
        stats.histogram = vec![0; bins];
        let scale = bins as f64 / (hist_max - hist_min);
        let counted = |v: f64| finite(v) && v >= hist_min && v <= hist_max;
        let (mut n, mut sum, mut sum2) = (0u64, 0.0, 0.0);
        let mut included: Vec<f64> = Vec::new();
        self.region.for_each(self.width, |_, _, i| {
            let v = s[i].to_f64();
            if finite(v) {
                included.push(v);
            }
            if counted(v) {
                n += 1;
                sum += v;
                sum2 += v * v;
                // A NaN product (zero range) lands in the first bin, as
                // Java's cast does.
                let index = ((scale * (v - hist_min)) as i64).clamp(0, bins as i64 - 1) as usize;
                stats.histogram[index] += 1;
            }
        });
        stats.pixel_count = n;
        stats.area = n as f64 * self.pixel_area();
        stats.mean = sum / n as f64;
        stats.raw_mean = stats.mean;
        stats.std_dev = std_dev(n as f64, sum, sum2);
        // getMode
        for (i, &count) in stats.histogram.iter().enumerate() {
            if count > stats.max_count {
                stats.max_count = count;
                stats.mode_bin = i;
            }
        }
        stats.mode = hist_min + stats.mode_bin as f64 * stats.bin_size;
        if stats.bin_size != 1.0 {
            stats.mode += stats.bin_size / 2.0;
        }

        self.shape(stats, |i| finite(s[i].to_f64()));
        let mut m = Moments::default();
        self.region.for_each(self.width, |x, y, i| {
            let v = s[i].to_f64() + TINY;
            if finite(v) {
                m.add(x, y, v);
            }
        });
        self.finish_moments(&m, stats);

        // getMedian: of every finite sample; NaN if the histogram range
        // left some of them out.
        stats.median = if n == 0 || included.len() != n as usize {
            f64::NAN
        } else {
            included.sort_unstable_by(f64::total_cmp);
            let middle = included.len() / 2;
            if included.len() % 2 == 1 {
                included[middle]
            } else if single {
                ((included[middle - 1] as f32 + included[middle] as f32) / 2.0) as f64
            } else {
                (included[middle - 1] + included[middle]) / 2.0
            }
        };

        // calculateAreaFraction without a threshold: the share that is not
        // zero, NaN included.
        let (mut total, mut nonzero) = (0u64, 0u64);
        self.region.for_each(self.width, |_, _, i| {
            total += 1;
            if s[i].to_f64() != 0.0 {
                nonzero += 1;
            }
        });
        stats.area_fraction = nonzero as f64 * 100.0 / total as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calipix_core::Complex32;

    fn of(plane: &Buffer, width: usize, height: usize) -> Statistics {
        statistics(
            Samples::Gray(plane),
            width,
            height,
            Region::whole(width, height),
            &Calibration::default(),
            Measurements::all(),
            HistogramSpec::default(),
        )
        .unwrap()
    }

    /// Integer types ImageJ lacks are measured by its 16-bit rules.
    #[test]
    fn other_integers_follow_the_sixteen_bit_rules() {
        let values: Vec<u16> = (0..35u32).map(|i| (i * i * 37 % 5000) as u16).collect();
        let reference = of(&Buffer::from_vec(values.clone()), 7, 5);
        let as_u32 = Buffer::from_vec(values.iter().map(|&v| v as u32).collect::<Vec<_>>());
        let as_i64 = Buffer::from_vec(values.iter().map(|&v| v as i64).collect::<Vec<_>>());
        assert_eq!(of(&as_u32, 7, 5), reference);
        assert_eq!(of(&as_i64, 7, 5), reference);

        // Signed samples: the same values moved down by 3000.
        let signed = of(
            &Buffer::from_vec(
                values
                    .iter()
                    .map(|&v| v as i16 - 3000)
                    .collect::<Vec<i16>>(),
            ),
            7,
            5,
        );
        let wide = of(
            &Buffer::from_vec(
                values
                    .iter()
                    .map(|&v| v as i32 - 3000)
                    .collect::<Vec<i32>>(),
            ),
            7,
            5,
        );
        assert_eq!(signed, wide);
        assert_eq!(signed.pixel_count, 35);
        assert_eq!(signed.min, reference.min - 3000.0);
        assert_eq!(signed.max, reference.max - 3000.0);
        assert_eq!(signed.median, reference.median - 3000.0);
        assert_eq!(signed.mode, reference.mode - 3000.0);
        assert_eq!(signed.histogram, reference.histogram);
        assert!((signed.mean - (reference.mean - 3000.0)).abs() < 1e-9);
        assert!((signed.std_dev - reference.std_dev).abs() < 1e-9);
    }

    /// 64-bit floats are measured by the 32-bit rules.
    #[test]
    fn doubles_follow_the_float_rules() {
        let values: Vec<f32> = (0..35).map(|i| (i * i) as f32 / 8.0 - 20.0).collect();
        let single = of(&Buffer::from_vec(values.clone()), 5, 7);
        let double = of(
            &Buffer::from_vec(values.iter().map(|&v| v as f64).collect::<Vec<f64>>()),
            5,
            7,
        );
        assert_eq!(double, single);

        let mut odd = vec![1.0f64, f64::NAN, f64::INFINITY, 3.0, 1e300, -2.0];
        odd.extend([0.0; 3]);
        let s = of(&Buffer::from_vec(odd), 3, 3);
        assert_eq!(s.pixel_count, 7, "NaN and infinity are not counted");
        assert_eq!((s.min, s.max), (-2.0, 1e300));
        assert_eq!(s.median, 0.0);
    }

    #[test]
    fn what_cannot_be_measured_says_why() {
        let complex = Buffer::from_vec(vec![Complex32::new(1.0, 2.0); 4]);
        let run = |samples: Samples<'_>, region: Region<'_>, bins: usize| {
            statistics(
                samples,
                2,
                2,
                region,
                &Calibration::default(),
                Measurements::default(),
                HistogramSpec { bins, range: None },
            )
            .unwrap_err()
            .to_string()
        };
        let whole = Region::whole(2, 2);
        assert!(run(Samples::Gray(&complex), whole, 256).contains("complex"));
        let bytes = Buffer::from_vec(vec![1u8; 4]);
        let outside = Region::rect(Rect::new(1, 1, 2, 2));
        assert!(run(Samples::Gray(&bytes), outside, 256).contains("outside"));
        let short_mask = Region {
            rect: Rect::new(0, 0, 2, 2),
            mask: Some(&[255]),
        };
        assert!(run(Samples::Gray(&bytes), short_mask, 256).contains("mask"));
        assert!(run(Samples::Gray(&bytes), whole, 0).contains("bin"));
        let words = Buffer::from_vec(vec![1u16; 4]);
        let rgb = Samples::Rgb {
            red: &words,
            green: &words,
            blue: &words,
        };
        assert!(run(rgb, whole, 256).contains("8-bit"));
    }

    /// A region with nothing selected has no statistics, and says so with
    /// NaN rather than by failing.
    #[test]
    fn empty_mask() {
        let bytes = Buffer::from_vec(vec![5u8; 6]);
        let region = Region {
            rect: Rect::new(0, 0, 3, 2),
            mask: Some(&[0; 6]),
        };
        let s = statistics(
            Samples::Gray(&bytes),
            3,
            2,
            region,
            &Calibration::default(),
            Measurements::all(),
            HistogramSpec::default(),
        )
        .unwrap();
        assert_eq!(s.pixel_count, 0);
        assert_eq!(s.area, 0.0);
        assert!(s.mean.is_nan() && s.min.is_nan() && s.median.is_nan());
    }

    #[test]
    fn calibration_from_axes() {
        use calipix_core::{Axis, DType};
        let meta = ImageMeta::new(
            DType::U8,
            vec![
                (
                    Axis {
                        kind: AxisKind::X,
                        scale: 0.5,
                        origin: -1.5,
                        unit: Some("mm".into()),
                    },
                    4,
                ),
                (Axis::calibrated(AxisKind::Y, 0.25, "mm"), 4),
            ],
        )
        .unwrap();
        let cal = Calibration::from_meta(&meta);
        assert_eq!((cal.pixel_width, cal.pixel_height), (0.5, 0.25));
        assert_eq!((cal.x_origin, cal.y_origin), (3.0, 0.0));
        assert_eq!(cal.unit.as_deref(), Some("mm"));
        assert_eq!(cal.x(8.0), 2.5);
        assert_eq!(cal.y(6.0, 48), 1.5);
        let up = Calibration {
            invert_y: true,
            ..cal.clone()
        };
        assert_eq!(up.y(6.0, 48), 41.0 * 0.25);
        let from_origin = Calibration {
            y_origin: 4.0,
            ..up
        };
        assert_eq!(from_origin.y(6.0, 48), -0.5);
    }
}
