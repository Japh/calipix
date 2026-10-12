//! Gaussian blur and unsharp mask, after ImageJ's `GaussianBlur` and
//! `UnsharpMask` plugins.

use calipix_core::{Buffer, DType, Error, RealPixel, Result};
use rayon::prelude::*;

use super::{Frame, from_f32, real_plane, to_f32, wrap};

/// Number of samples added on each side of a downscaled line for upscaling
/// (`UPSCALE_K_RADIUS` in ImageJ).
const UPSCALE_K_RADIUS: usize = 2;
/// Smallest standard deviation in a downscaled line
/// (`MIN_DOWNSCALED_SIGMA`).
const MIN_DOWNSCALED_SIGMA: f64 = 4.0;

/// The kernel accuracy ImageJ's Gaussian Blur command uses for a pixel
/// type: 0.002 for 8-bit (and so RGB), 0.0002 for everything else.
pub fn gaussian_accuracy(dtype: DType) -> f64 {
    if dtype == DType::U8 { 0.002 } else { 0.0002 }
}

/// Blurs `frame.rect` of a plane with a Gaussian of standard deviations
/// `sigma_x` and `sigma_y` pixels, as `GaussianBlur.blurGaussian(ip,
/// sigmaX, sigmaY, accuracy)` does. A sigma of zero leaves that direction
/// alone.
///
/// The plane is converted to float, blurred along rows and then along
/// columns with a kernel cut off where it falls below `accuracy` (see
/// [`gaussian_kernel`]), and converted back; see the module documentation
/// for rounding.
///
/// For a sigma above 8.5, ImageJ blurs a downscaled copy of each line and
/// scales it back up. The downscaling grid starts a kernel radius before
/// the rectangle, so there the result for a rectangle differs slightly
/// from the same samples of the blurred whole plane, here as in ImageJ.
/// For smaller sigmas the two agree exactly.
pub fn gaussian_blur(
    plane: &Buffer,
    frame: Frame,
    sigma_x: f64,
    sigma_y: f64,
    accuracy: f64,
) -> Result<Buffer> {
    check_sigma(sigma_x)?;
    check_sigma(sigma_y)?;
    if !(accuracy > 0.0 && accuracy < 1.0) {
        return Err(Error::Other(format!(
            "the kernel accuracy must be between 0 and 1, not {accuracy}"
        )));
    }
    real_plane!(plane, "blur", |s| via_float(s, frame, |pixels| {
        blur_float(pixels, frame, sigma_x, sigma_y, accuracy)
    })
    .map(wrap))
}

/// Sharpens `frame.rect` of a plane by subtracting a blurred copy, as
/// `UnsharpMask.sharpenFloat` does: with `b` the plane blurred by
/// `sigma` (kernel accuracy 0.01), each sample becomes
/// `(v - weight * b) / (1 - weight)`, in single precision.
///
/// `weight` must be at least 0 and below 1; ImageJ's dialog accepts up to
/// 0.99.
pub fn unsharp_mask(plane: &Buffer, frame: Frame, sigma: f64, weight: f64) -> Result<Buffer> {
    check_sigma(sigma)?;
    if !(0.0..1.0).contains(&weight) {
        return Err(Error::Other(format!(
            "the mask weight must be at least 0 and below 1, not {weight}"
        )));
    }
    let weight = weight as f32;
    fn sharpen<T: RealPixel>(s: &[T], frame: Frame, sigma: f64, weight: f32) -> Result<Vec<T>> {
        via_float(s, frame, |pixels| {
            blur_float(pixels, frame, sigma, sigma, 0.01);
            let r = frame.rect;
            for y in r.y..r.y + r.h {
                for p in y * frame.width + r.x..y * frame.width + r.x + r.w {
                    pixels[p] = (to_f32(s[p]) - weight * pixels[p]) / (1f32 - weight);
                }
            }
        })
    }
    real_plane!(plane, "sharpen", |s| sharpen(s, frame, sigma, weight)
        .map(wrap))
}

fn check_sigma(sigma: f64) -> Result<()> {
    if sigma.is_finite() && sigma >= 0.0 {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "sigma must be a number that is not negative, not {sigma}"
        )))
    }
}

/// Converts a plane to float, lets `filter` change it, and returns the
/// plane with the samples of `frame.rect` taken from the result.
fn via_float<T: RealPixel>(
    src: &[T],
    frame: Frame,
    filter: impl FnOnce(&mut [f32]),
) -> Result<Vec<T>> {
    frame.check(src.len())?;
    let mut pixels: Vec<f32> = src.iter().map(|&v| to_f32(v)).collect();
    filter(&mut pixels);
    let mut out = src.to_vec();
    let r = frame.rect;
    for y in r.y..r.y + r.h {
        let row = y * frame.width + r.x..y * frame.width + r.x + r.w;
        for (o, &v) in out[row.clone()].iter_mut().zip(&pixels[row]) {
            *o = from_f32(v);
        }
    }
    Ok(out)
}

/// `GaussianBlur.blurFloat`: blurs rows, then columns, in place. Only the
/// samples in `frame.rect` are meaningful afterwards; rows near it hold
/// row-blurred values.
fn blur_float(pixels: &mut [f32], frame: Frame, sigma_x: f64, sigma_y: f64, accuracy: f64) {
    if sigma_x > 0.0 {
        let extra = (5.0 * sigma_y).ceil() as usize;
        blur_rows(pixels, frame, sigma_x, accuracy, extra);
    }
    if sigma_y > 0.0 {
        blur_columns(pixels, frame, sigma_y, accuracy);
    }
}

/// How one direction is blurred: the values `GaussianBlur.blur1Direction`
/// derives before it loops over lines.
struct Plan {
    /// Samples per line.
    length: usize,
    write_from: usize,
    write_to: usize,
    read_from: usize,
    read_to: usize,
    /// Downscaling factor; 1 when lines are blurred at full size.
    reduce_by: usize,
    kernel: [Vec<f32>; 2],
    /// Length of a downscaled line.
    new_length: usize,
    /// Line position of downscaled sample 0; may be negative.
    unscaled0: isize,
    scaling: Option<(Vec<f32>, Vec<f32>)>,
}

impl Plan {
    fn new(length: usize, write_from: usize, write_to: usize, sigma: f64, accuracy: f64) -> Plan {
        let downscale = sigma > 2.0 * MIN_DOWNSCALED_SIGMA + 0.5;
        let reduce_by = if downscale {
            ((sigma / MIN_DOWNSCALED_SIGMA).floor() as usize).min(length)
        } else {
            1
        };
        // Down- and upscaling blur by 1/sqrt(3) and 1/2 downscaled pixels;
        // the Gaussian supplies the rest of the variance.
        let sigma_gauss = if downscale {
            (sigma * sigma / (reduce_by * reduce_by) as f64 - 1.0 / 3.0 - 1.0 / 4.0).sqrt()
        } else {
            sigma
        };
        let max_length = if downscale {
            length.div_ceil(reduce_by) + 2 * (UPSCALE_K_RADIUS + 1)
        } else {
            length
        };
        let kernel = gaussian_kernel(sigma_gauss, accuracy, max_length);
        let k_radius = kernel[0].len() * reduce_by;
        let read_from = write_from.saturating_sub(k_radius);
        let read_to = (write_to + k_radius).min(length);
        let new_length = if downscale {
            (read_to - read_from).div_ceil(reduce_by) + 2 * (UPSCALE_K_RADIUS + 1)
        } else {
            length
        };
        Plan {
            length,
            write_from,
            write_to,
            read_from,
            read_to,
            reduce_by,
            kernel,
            new_length,
            unscaled0: read_from as isize - ((UPSCALE_K_RADIUS + 1) * reduce_by) as isize,
            scaling: downscale.then(|| (downscale_kernel(reduce_by), upscale_kernel(reduce_by))),
        }
    }

    /// The part of a line that blurring reads.
    fn needed(&self) -> std::ops::Range<usize> {
        match self.scaling {
            None => self.read_from..self.read_to,
            Some(_) => {
                let r = self.reduce_by as isize;
                let start = self.unscaled0 - r * 3 / 2;
                let end = start + (self.new_length as isize + 2) * r;
                let clamp = |v: isize| v.clamp(0, self.length as isize) as usize;
                // Positions beyond the ends are read as the end samples.
                let (lo, hi) = (clamp(start), clamp(end));
                lo.min(self.length - 1)..hi.max(1)
            }
        }
    }

    /// Blurs `line` and writes samples `write_from..write_to` to `out`.
    /// `line` needs to be valid only over [`needed`](Self::needed).
    fn blur_line(&self, line: &[f32], out: &mut [f32], scratch: &mut Scratch) {
        match &self.scaling {
            None => convolve_line(line, &self.kernel, self.write_from, self.write_to, out),
            Some((down, up)) => {
                scratch.small.resize(self.new_length, 0.0);
                scratch.blurred.resize(self.new_length, 0.0);
                downscale_line(
                    line,
                    &mut scratch.small,
                    down,
                    self.reduce_by,
                    self.unscaled0,
                );
                let n = self.new_length;
                convolve_line(
                    &scratch.small,
                    &self.kernel,
                    1,
                    n - 1,
                    &mut scratch.blurred[1..n - 1],
                );
                upscale_line(
                    &scratch.blurred,
                    out,
                    up,
                    self.reduce_by,
                    self.unscaled0,
                    self.write_from,
                    self.write_to,
                );
            }
        }
    }
}

/// Per-thread working lines.
#[derive(Default)]
struct Scratch {
    line: Vec<f32>,
    small: Vec<f32>,
    blurred: Vec<f32>,
}

/// `blur1Direction` along X: rows from `extra` above the rectangle to
/// `extra` below it are blurred between the rectangle's left and right
/// edges.
fn blur_rows(pixels: &mut [f32], frame: Frame, sigma: f64, accuracy: f64, extra: usize) {
    let (width, r) = (frame.width, frame.rect);
    let line_from = r.y.saturating_sub(extra);
    let line_to = (r.y + r.h).saturating_add(extra).min(frame.height);
    let plan = Plan::new(width, r.x, r.x + r.w, sigma, accuracy);
    let needed = plan.needed();
    pixels[line_from * width..line_to * width]
        .par_chunks_mut(width)
        .for_each_init(Scratch::default, |scratch, row| {
            let mut line = std::mem::take(&mut scratch.line);
            line.resize(width, 0.0);
            line[needed.clone()].copy_from_slice(&row[needed.clone()]);
            plan.blur_line(&line, &mut row[plan.write_from..plan.write_to], scratch);
            scratch.line = line;
        });
}

/// `blur1Direction` along Y: the rectangle's columns are blurred between
/// its top and bottom edges.
fn blur_columns(pixels: &mut [f32], frame: Frame, sigma: f64, accuracy: f64) {
    let (width, r) = (frame.width, frame.rect);
    let plan = Plan::new(frame.height, r.y, r.y + r.h, sigma, accuracy);
    let needed = plan.needed();
    // Blurred columns, one after another, so that threads write disjoint
    // slices.
    let mut columns = vec![0f32; r.w * r.h];
    let source = &*pixels;
    columns
        .par_chunks_mut(r.h)
        .enumerate()
        .for_each_init(Scratch::default, |scratch, (i, out)| {
            let mut line = std::mem::take(&mut scratch.line);
            line.resize(frame.height, 0.0);
            for y in needed.clone() {
                line[y] = source[y * width + r.x + i];
            }
            plan.blur_line(&line, out, scratch);
            scratch.line = line;
        });
    for (i, column) in columns.chunks(r.h).enumerate() {
        for (j, &v) in column.iter().enumerate() {
            pixels[(r.y + j) * width + r.x + i] = v;
        }
    }
}

/// `GaussianBlur.convolveLine`: convolves `input` with a symmetric kernel
/// given as one side plus running sums, writing samples
/// `write_from..write_to` to `out`. Samples beyond the ends of the line
/// count as the end samples. The three loops differ in how they treat the
/// ends, down to the last bit, so they are kept as ImageJ has them.
fn convolve_line(
    input: &[f32],
    kernel: &[Vec<f32>; 2],
    write_from: usize,
    write_to: usize,
    out: &mut [f32],
) {
    let length = input.len();
    let first = input[0];
    let last = input[length - 1];
    let kern = &kernel[0];
    let kern_sum = &kernel[1];
    let kern0 = kern[0];
    let k_radius = kern.len();
    let first_part = k_radius.min(length);
    let mut i = write_from;
    // While the sum would include samples before the line. (ImageJ does
    // not stop this loop at `write_to`, and so overwrites up to a kernel
    // radius of samples beside a narrow rectangle near the start of a
    // line. Those lie outside the rectangle and are not produced here.)
    while i < first_part && i < write_to {
        let mut result = input[i] * kern0;
        result += kern_sum[i] * first;
        if i + k_radius > length {
            result += kern_sum[length - i - 1] * last;
        }
        for k in 1..k_radius {
            let mut v = 0f32;
            if i >= k {
                v += input[i - k];
            }
            if i + k < length {
                v += input[i + k];
            }
            result += kern[k] * v;
        }
        out[i - write_from] = result;
        i += 1;
    }
    // While only samples within the line are addressed.
    let end_inside = length.saturating_sub(k_radius).min(write_to);
    while i < end_inside {
        let mut result = input[i] * kern0;
        for k in 1..k_radius {
            result += kern[k] * (input[i - k] + input[i + k]);
        }
        out[i - write_from] = result;
        i += 1;
    }
    // While the sum would include samples after the line.
    while i < write_to {
        let mut result = input[i] * kern0;
        if i < k_radius {
            result += kern_sum[i] * first;
        }
        if i + k_radius >= length {
            result += kern_sum[length - i - 1] * last;
        }
        for k in 1..k_radius {
            let mut v = 0f32;
            if i >= k {
                v += input[i - k];
            }
            if i + k < length {
                v += input[i + k];
            }
            result += kern[k] * v;
        }
        out[i - write_from] = result;
        i += 1;
    }
}

/// `GaussianBlur.downscaleLine`: reduces a line by `reduce_by`, with
/// line position `unscaled0` landing on `cache[0]`. Positions outside the
/// line read the end samples.
fn downscale_line(
    line: &[f32],
    cache: &mut [f32],
    kernel: &[f32],
    reduce_by: usize,
    unscaled0: isize,
) {
    let new_length = cache.len() as isize;
    let last = line.len() as isize - 1;
    let mut p = unscaled0 - (reduce_by * 3 / 2) as isize;
    for xout in -1..=new_length {
        let (mut sum0, mut sum1, mut sum2) = (0f32, 0f32, 0f32);
        for x in 0..reduce_by {
            let v = line[p.clamp(0, last) as usize];
            sum0 += v * kernel[x + 2 * reduce_by];
            sum1 += v * kernel[x + reduce_by];
            sum2 += v * kernel[x];
            p += 1;
        }
        if xout > 0 {
            cache[(xout - 1) as usize] += sum0;
        }
        if xout >= 0 && xout < new_length {
            cache[xout as usize] += sum1;
        }
        if xout + 1 < new_length {
            cache[(xout + 1) as usize] = sum2;
        }
    }
}

/// `GaussianBlur.makeDownscaleKernel`: runs from -1.5 to +1.5 downscaled
/// samples and keeps the sum and the position of what it reduces.
fn downscale_kernel(unit: usize) -> Vec<f32> {
    let mid = unit * 3 / 2;
    let mut kernel = vec![0f32; 3 * unit];
    for i in 0..=unit / 2 {
        let x = i as f64 / unit as f64;
        let v = ((0.75 - x * x) / unit as f64) as f32;
        kernel[mid - i] = v;
        kernel[mid + i] = v;
    }
    for i in unit / 2 + 1..(unit * 3).div_ceil(2) {
        let x = i as f64 / unit as f64;
        let v = ((0.125 + 0.5 * (x - 1.0) * (x - 2.0)) / unit as f64) as f32;
        kernel[mid - i] = v;
        kernel[mid + i] = v;
    }
    kernel
}

/// `GaussianBlur.upscaleLine`: writes samples `write_from..write_to` of
/// the full-size line from the downscaled `cache`.
fn upscale_line(
    cache: &[f32],
    out: &mut [f32],
    kernel: &[f32],
    reduce_by: usize,
    unscaled0: isize,
    write_from: usize,
    write_to: usize,
) {
    for xout in write_from..write_to {
        let offset = (xout as isize - unscaled0) as usize + reduce_by - 1;
        // The cache sample at this position, or the one above it.
        let xin = offset / reduce_by;
        let x = reduce_by - 1 - offset % reduce_by;
        out[xout - write_from] = cache[xin - 2] * kernel[x]
            + cache[xin - 1] * kernel[x + reduce_by]
            + cache[xin] * kernel[x + 2 * reduce_by]
            + cache[xin + 1] * kernel[x + 3 * reduce_by];
    }
}

/// `GaussianBlur.makeUpscaleKernel`: four unit boxes convolved together,
/// running from -2 to +2 downscaled samples.
fn upscale_kernel(unit: usize) -> Vec<f32> {
    let mut kernel = vec![0f32; 4 * unit];
    let mid = 2 * unit;
    for i in 0..unit {
        let x = i as f64 / unit as f64;
        let v = (2.0 / 3.0 - x * x * (1.0 - 0.5 * x)) as f32;
        kernel[mid + i] = v;
        kernel[mid - i] = v;
    }
    for i in unit..2 * unit {
        let x = i as f64 / unit as f64;
        let v = ((2.0 - x) * (2.0 - x) * (2.0 - x) / 6.0) as f32;
        kernel[mid + i] = v;
        kernel[mid - i] = v;
    }
    kernel
}

/// One side of a normalised Gaussian kernel and its running sums, as
/// `GaussianBlur.makeGaussianKernel` builds them.
///
/// The first array is the kernel from its centre outwards. Its radius is
/// where the Gaussian falls to `accuracy`, plus one, but at most
/// `max_radius` (itself at least 50). Unless the radius was limited, the
/// outer values are replaced by a parabola that reaches zero just past the
/// end, so that the cut-off leaves no step. The second array holds, for
/// each position, the sum of all kernel values further out; it weights the
/// edge sample for the part of the kernel that hangs over the end of a
/// line.
pub fn gaussian_kernel(sigma: f64, accuracy: f64, max_radius: usize) -> [Vec<f32>; 2] {
    let max_radius = max_radius.max(50);
    let wanted = ((sigma * (-2.0 * accuracy.ln()).sqrt()).ceil() as usize).saturating_add(1);
    let k_radius = wanted.min(max_radius);
    let mut kernel: Vec<f32> = (0..k_radius)
        .map(|i| (-0.5 * i as f64 * i as f64 / sigma / sigma).exp() as f32)
        .collect();
    if k_radius < max_radius && k_radius > 3 {
        let mut sqrt_slope = f64::MAX;
        let mut r = k_radius;
        while r > k_radius / 2 {
            r -= 1;
            let a = (kernel[r] as f64).sqrt() / (k_radius - r) as f64;
            if a < sqrt_slope {
                sqrt_slope = a;
            } else {
                break;
            }
        }
        for (r1, k) in kernel.iter_mut().enumerate().skip(r + 2) {
            let d = k_radius - r1;
            *k = ((d * d) as f64 * sqrt_slope * sqrt_slope) as f32;
        }
    }
    let sum = if k_radius < max_radius {
        let mut sum = kernel[0] as f64;
        for &k in &kernel[1..] {
            sum += (2f32 * k) as f64;
        }
        sum
    } else {
        sigma * (2.0 * std::f64::consts::PI).sqrt()
    };
    let mut rsum = 0.5 + 0.5 * kernel[0] as f64 / sum;
    let mut running = Vec::with_capacity(k_radius);
    for k in &mut kernel {
        let v = *k as f64 / sum;
        *k = v as f32;
        rsum -= v;
        running.push(rsum as f32);
    }
    [kernel, running]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_sums_to_one_and_running_sums_reach_zero() {
        for (sigma, accuracy) in [(0.5, 0.002), (2.0, 0.002), (2.0, 0.0002), (7.9, 0.01)] {
            let [kernel, running] = gaussian_kernel(sigma, accuracy, 1000);
            let sum: f64 =
                kernel[0] as f64 + 2.0 * kernel[1..].iter().map(|&k| k as f64).sum::<f64>();
            assert!((sum - 1.0).abs() < 1e-6, "sigma {sigma}: {sum}");
            assert!(running.last().unwrap().abs() < 1e-6);
            assert!(kernel.windows(2).all(|w| w[0] >= w[1]));
        }
        // ceil(2 * sqrt(-2 ln 0.002)) + 1
        assert_eq!(gaussian_kernel(2.0, 0.002, 1000)[0].len(), 9);
    }

    #[test]
    fn scaling_kernels_are_normalised() {
        for unit in 1..9 {
            let down: f64 = downscale_kernel(unit).iter().map(|&k| k as f64).sum();
            assert!((down - 1.0).abs() < 1e-6, "{unit}: {down}");
            let up: f64 = upscale_kernel(unit).iter().map(|&k| k as f64).sum();
            assert!((up - unit as f64).abs() < 1e-5, "{unit}: {up}");
        }
    }

    #[test]
    fn rejects_bad_parameters() {
        let plane = Buffer::from_vec(vec![0u8; 4]);
        let frame = Frame::whole(2, 2);
        assert!(gaussian_blur(&plane, frame, -1.0, 1.0, 0.002).is_err());
        assert!(gaussian_blur(&plane, frame, f64::NAN, 1.0, 0.002).is_err());
        assert!(gaussian_blur(&plane, frame, 1.0, 1.0, 0.0).is_err());
        assert!(gaussian_blur(&plane, Frame::whole(3, 2), 1.0, 1.0, 0.002).is_err());
        assert!(unsharp_mask(&plane, frame, 1.0, 1.0).is_err());
    }
}
