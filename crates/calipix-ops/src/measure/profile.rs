//! Intensity profiles: the numbers behind Analyze > Plot Profile
//! (`ij.gui.ProfilePlot`).

use calipix_core::{Buffer, Error, RealPixel, Rect, Result, dispatch_real};
use calipix_roi::Roi;

use super::Samples;

/// The value of every sample of a plane as `f64`
/// (`ImageProcessor.getPixelValue`).
/// RGB pixels give ImageJ's unrounded grey value in single precision.
pub fn sample_values(samples: Samples<'_>) -> Result<Vec<f64>> {
    fn values<T: RealPixel>(s: &[T]) -> Vec<f64> {
        s.iter().map(|v| v.to_f64()).collect()
    }
    match samples {
        Samples::Gray(buffer) => dispatch_real!(buffer, |s| Ok(values(s)), complex => {
            Err(Error::Unsupported(
                "complex images have no intensity profile; take the magnitude or a component first"
                    .to_string(),
            ))
        }),
        Samples::Rgb { red, green, blue } => {
            let slice = |b: &Buffer| b.try_slice::<u8>().map(<[u8]>::to_vec);
            let (r, g, b) = (slice(red)?, slice(green)?, slice(blue)?);
            if r.len() != g.len() || r.len() != b.len() {
                return Err(Error::Shape("RGB channels differ in size".to_string()));
            }
            let w = 1.0 / 3.0;
            Ok((0..r.len())
                .map(|i| (r[i] as f64 * w + g[i] as f64 * w + b[i] as f64 * w) as f32 as f64)
                .collect())
        }
    }
}

fn check(values: &[f64], width: usize, height: usize) -> Result<()> {
    if width == 0 || height == 0 || values.len() != width * height {
        return Err(Error::Shape(format!(
            "plane of {} samples is not {width}x{height}",
            values.len()
        )));
    }
    Ok(())
}

/// The mean of each column of `rect`: Plot Profile on a rectangle
/// (`ProfilePlot.getColumnAverageProfile`). NaN samples are left out of
/// the mean.
pub fn column_average_profile(
    samples: Samples<'_>,
    width: usize,
    height: usize,
    rect: Rect,
) -> Result<Vec<f64>> {
    average_profile(samples, width, height, rect, false)
}

/// The mean of each row of `rect`: Plot Profile on a rectangle with the
/// "Vertical profile" option (`ProfilePlot.getRowAverageProfile`).
pub fn row_average_profile(
    samples: Samples<'_>,
    width: usize,
    height: usize,
    rect: Rect,
) -> Result<Vec<f64>> {
    average_profile(samples, width, height, rect, true)
}

fn average_profile(
    samples: Samples<'_>,
    width: usize,
    height: usize,
    rect: Rect,
    rows: bool,
) -> Result<Vec<f64>> {
    let values = sample_values(samples)?;
    check(&values, width, height)?;
    if !rect.fits(width, height) {
        return Err(Error::OutOfBounds(format!(
            "rectangle {rect:?} outside {width}x{height} plane"
        )));
    }
    let n = if rows { rect.h } else { rect.w };
    let mut sums = vec![0.0; n];
    let mut counts = vec![0u32; n];
    // ImageJ adds one line at a time across the averaging direction, so
    // the order of summation is fixed.
    let (outer, inner) = if rows {
        (rect.w, rect.h)
    } else {
        (rect.h, rect.w)
    };
    for o in 0..outer {
        for (i, (sum, count)) in sums.iter_mut().zip(&mut counts).enumerate().take(inner) {
            let (x, y) = if rows {
                (rect.x + o, rect.y + i)
            } else {
                (rect.x + i, rect.y + o)
            };
            let v = values[y * width + x];
            if !v.is_nan() {
                *sum += v;
                *count += 1;
            }
        }
    }
    Ok(sums
        .iter()
        .zip(&counts)
        .map(|(s, &c)| s / c as f64)
        .collect())
}

/// The samples along the straight line from `(x1, y1)` to `(x2, y2)`
/// (`ImageProcessor.getLine`): one for each unit of length, rounded, plus
/// one. With `interpolate` (ImageJ's default for profiles) they are
/// bilinear interpolations; otherwise the nearest pixel's value, or NaN
/// outside the image.
pub fn line_profile(
    samples: Samples<'_>,
    width: usize,
    height: usize,
    (x1, y1): (f64, f64),
    (x2, y2): (f64, f64),
    interpolate: bool,
) -> Result<Vec<f64>> {
    let values = sample_values(samples)?;
    check(&values, width, height)?;
    let (dx, dy) = (x2 - x1, y2 - y1);
    let n = (dx * dx + dy * dy).sqrt().round() as usize;
    let (xinc, yinc) = if n > 0 {
        (dx / n as f64, dy / n as f64)
    } else {
        (0.0, 0.0)
    };
    let plane = Plane {
        values: &values,
        width,
        height,
    };
    let (mut rx, mut ry) = (x1, y1);
    let mut data = Vec::with_capacity(n + 1);
    for _ in 0..=n {
        data.push(if interpolate {
            plane.interpolated(rx, ry)
        } else {
            plane.value(java_round(rx), java_round(ry))
        });
        rx += xinc;
        ry += yinc;
    }
    Ok(data)
}

/// The profile along a line selection, as `ij.gui.ProfilePlot` takes it:
///
/// * a straight line one pixel wide: [`line_profile`];
/// * a segmented or freehand line one pixel wide: a sample for each unit
///   of length walked along it (`ProfilePlot.getIrregularProfile`);
/// * any line wider than one pixel: the line is straightened into an
///   image as high as the line is wide, whose columns are averaged
///   (`Straightener.straightenLine`, `ProfilePlot.getWideLineProfile`,
///   `Line.getPixels`). Wide lines are always interpolated;
/// * a rotated rectangle: the wide straight line along its axis.
///
/// The result is empty for a wide line too short to straighten.
pub fn selection_profile(
    samples: Samples<'_>,
    width: usize,
    height: usize,
    roi: &Roi,
    interpolate: bool,
) -> Result<Vec<f64>> {
    let (ends, stroke) = match roi {
        Roi::Line {
            x1,
            y1,
            x2,
            y2,
            width,
        }
        | Roi::RotatedRectangle {
            x1,
            y1,
            x2,
            y2,
            width,
        } => (Some(((*x1, *y1), (*x2, *y2))), *width),
        Roi::Polyline { width, .. } | Roi::Freeline { width, .. } => (None, *width),
        other => {
            return Err(Error::Unsupported(format!(
                "a {} selection has no profile along it",
                other.kind()
            )));
        }
    };
    if let (Some((start, end)), true) = (ends, stroke <= 1.0) {
        return line_profile(samples, width, height, start, end, interpolate);
    }
    let values = sample_values(samples)?;
    check(&values, width, height)?;
    let plane = Plane {
        values: &values,
        width,
        height,
    };
    Ok(match ends {
        // Line.getPixels hands the Straightener (int)getStrokeWidth().
        Some((start, end)) => {
            let line = Relative::float(&[start, end]);
            plane.straightened(&line, line.length(), stroke as usize)
        }
        None => {
            let line_width = java_round(stroke);
            if line_width <= 1 {
                plane.irregular(&roi.outline(), interpolate)
            } else {
                let vertices = roi.vertices().unwrap_or_default();
                let whole = |v: f64| v == v.trunc() && v.abs() < 1.0e9;
                let line = if vertices.iter().all(|&(x, y)| whole(x) && whole(y)) {
                    Relative::int(&vertices)
                } else {
                    Relative::float(&vertices)
                };
                plane.straightened(&line, roi.length(), line_width as usize)
            }
        }
    })
}

/// The vertices of a line as ImageJ's `PolygonRoi` holds them: in single
/// precision relative to a base point.
struct Relative {
    xs: Vec<f32>,
    ys: Vec<f32>,
    /// `(float)Roi.getXBase()` and `getYBase()`.
    base: (f32, f32),
}

impl Relative {
    /// `PolygonRoi(float[], float[], n, type)`: relative to the smallest
    /// coordinates.
    fn float(points: &[(f64, f64)]) -> Relative {
        let xs: Vec<f32> = points.iter().map(|p| p.0 as f32).collect();
        let ys: Vec<f32> = points.iter().map(|p| p.1 as f32).collect();
        let min_x = xs.iter().copied().fold(f32::MAX, f32::min);
        let min_y = ys.iter().copied().fold(f32::MAX, f32::min);
        Relative {
            xs: xs.iter().map(|v| v - min_x).collect(),
            ys: ys.iter().map(|v| v - min_y).collect(),
            base: (min_x, min_y),
        }
    }

    /// `PolygonRoi(int[], int[], n, type)`, converted to floats when a
    /// spline is fitted.
    fn int(points: &[(f64, f64)]) -> Relative {
        let xs: Vec<i64> = points.iter().map(|p| p.0 as i64).collect();
        let ys: Vec<i64> = points.iter().map(|p| p.1 as i64).collect();
        let min_x = xs.iter().copied().min().unwrap_or(0);
        let min_y = ys.iter().copied().min().unwrap_or(0);
        Relative {
            xs: xs.iter().map(|v| (v - min_x) as f32).collect(),
            ys: ys.iter().map(|v| (v - min_y) as f32).collect(),
            base: (min_x as f32, min_y as f32),
        }
    }

    /// `PolygonRoi.getLength(xpf, ypf, n, false, null)`.
    fn length(&self) -> f64 {
        polyline_length(&self.xs, &self.ys)
    }

    /// `PolygonRoi.fitSplineForStraightening` followed by
    /// `getFloatPolygon`: points about one pixel apart along a spline
    /// through the vertices, in image coordinates. `length` is the line's
    /// `getUncalibratedLength()`.
    fn spline_for_straightening(&self, length: f64) -> (Vec<f32>, Vec<f32>) {
        // A preliminary spline in half-pixel steps...
        let evaluation_points = (length * 2.0) as usize + 1;
        let (sx, sy) = self.spline(evaluation_points);
        // ...read off at steps as close to one pixel as divide its length.
        let (ex, ey) = equidistant_points(&sx, &sy, 1.0);
        (
            ex.iter().map(|v| v + self.base.0).collect(),
            ey.iter().map(|v| v + self.base.1).collect(),
        )
    }

    /// `PolygonRoi.getSpline` for an open line.
    fn spline(&self, evaluation_points: usize) -> (Vec<f32>, Vec<f32>) {
        let n = self.xs.len();
        let mut nodes = vec![0f32; n];
        let mut last = 0f32;
        for i in 1..n {
            let dx = self.xs[i] - self.xs[i - 1];
            let dy = self.ys[i] - self.ys[i - 1];
            // The independent coordinate grows by the square root of the
            // distance between vertices.
            let mut step = f64::from(dx * dx + dy * dy).sqrt().sqrt() as f32;
            if step < 0.001 {
                step = 0.001;
            }
            last += step;
            nodes[i] = last;
        }
        let fit_x = SplineFitter::new(&nodes, &self.xs);
        let fit_y = SplineFitter::new(&nodes, &self.ys);
        let scale = f64::from(last) / (evaluation_points as f64 - 1.0);
        (0..evaluation_points)
            .map(|i| {
                let at = i as f64 * scale;
                (fit_x.eval(at) as f32, fit_y.eval(at) as f32)
            })
            .unzip()
    }
}

/// `PolygonRoi.getLength(float[], float[], n, false, null)`.
fn polyline_length(xs: &[f32], ys: &[f32]) -> f64 {
    let mut length = 0.0;
    for i in 1..xs.len().min(ys.len()) {
        let dx = f64::from(xs[i] - xs[i - 1]);
        let dy = f64::from(ys[i] - ys[i - 1]);
        length += (dx * dx + dy * dy).sqrt();
    }
    length
}

/// `PolygonRoi.getEquidistantPoints` without calibration: points along the
/// open polygon `xs`, `ys` at steps of about `segment_length`.
fn equidistant_points(xs: &[f32], ys: &[f32], segment_length: f64) -> (Vec<f32>, Vec<f32>) {
    let npoints = xs.len().min(ys.len());
    if npoints == 0 {
        return (Vec::new(), Vec::new());
    }
    let length = polyline_length(xs, ys);
    let n_out = java_round(length / segment_length).max(0) as usize + 1;
    let step = length / (n_out as f64 - 1.0);
    let mut out_x = vec![0f32; n_out];
    let mut out_y = vec![0f32; n_out];
    out_x[0] = xs[0];
    out_y[0] = ys[0];
    let mut length_read = 0.0;
    let mut written = 1;
    let (mut x2, mut y2) = (f64::from(xs[0]), f64::from(ys[0]));
    for i in 1..npoints {
        let (x1, y1) = (x2, y2);
        x2 = f64::from(xs[i]);
        y2 = f64::from(ys[i]);
        let (dx, dy) = (x2 - x1, y2 - y1);
        let distance = (dx * dx + dy * dy).sqrt();
        length_read += distance;
        let mut over = length_read - written as f64 * step;
        while (over >= 0.0 || i == npoints - 1) && written < n_out {
            let fraction = if distance == 0.0 {
                0.0
            } else {
                over / distance
            };
            out_x[written] = (x2 - fraction * dx) as f32;
            out_y[written] = (y2 - fraction * dy) as f32;
            over -= step;
            written += 1;
        }
    }
    (out_x, out_y)
}

/// `ij.measure.SplineFitter` for single-precision knots: a natural cubic
/// spline through `(x[i], y[i])`, with `x` increasing.
struct SplineFitter<'a> {
    x: &'a [f32],
    y: &'a [f32],
    /// Second derivative at each knot.
    y2: Vec<f64>,
}

impl<'a> SplineFitter<'a> {
    fn new(x: &'a [f32], y: &'a [f32]) -> SplineFitter<'a> {
        let n = x.len();
        let mut y2 = vec![0.0f64; n];
        let mut c_p = vec![0.0f64; n];
        if n >= 2 {
            let mut new_x = f64::from(x[1]);
            let mut new_y = f64::from(y[1]);
            let mut cj = f64::from(x[1] - x[0]);
            let mut new_dj = f64::from(y[1] - y[0]) / cj;
            let mut j = 1;
            // Forward substitution.
            while j < n - 1 {
                let (old_x, old_y) = (new_x, new_y);
                let aj = cj;
                let old_dj = new_dj;
                new_x = f64::from(x[j + 1]);
                new_y = f64::from(y[j + 1]);
                cj = new_x - old_x;
                new_dj = (new_y - old_y) / cj;
                let bj = 2.0 * (cj + aj);
                let inv_denom = 1.0 / (bj - aj * c_p[j - 1]);
                let dj = 6.0 * (new_dj - old_dj);
                y2[j] = (dj - aj * y2[j - 1]) * inv_denom;
                c_p[j] = cj * inv_denom;
                j += 1;
            }
            // Backward substitution.
            while j > 0 {
                j -= 1;
                y2[j] -= c_p[j] * y2[j + 1];
            }
        }
        SplineFitter { x, y, y2 }
    }

    /// `SplineFitter.evalSpline`.
    fn eval(&self, xp: f64) -> f64 {
        let (mut ls, mut rs) = (0usize, self.x.len() - 1);
        while rs > 1 + ls {
            let m = (ls + rs) / 2;
            if f64::from(self.x[m]) < xp {
                ls = m;
            } else {
                rs = m;
            }
        }
        let ba = f64::from(self.x[rs] - self.x[ls]);
        let xa = xp - f64::from(self.x[ls]);
        let bx = f64::from(self.x[rs]) - xp;
        let ba2 = ba * ba;
        let lower = xa * f64::from(self.y[rs]) + bx * f64::from(self.y[ls]);
        let c = (xa * xa - ba2) * xa * self.y2[rs];
        let d = (bx * bx - ba2) * bx * self.y2[ls];
        (lower + (c + d) / 6.0) / ba
    }
}

/// Java's `Math.round`: half rounds up.
fn java_round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

struct Plane<'a> {
    values: &'a [f64],
    width: usize,
    height: usize,
}

impl Plane<'_> {
    /// `ProfilePlot.getIrregularProfile`: walks the polyline `points` in
    /// steps of a hundredth of a pixel and takes a sample each time it has
    /// come a pixel further (within half a step). As in ImageJ, the
    /// distance is first counted from the image's origin, so a line that
    /// starts within a pixel of it starts its profile a little later.
    fn irregular(&self, points: &[(f64, f64)], interpolate: bool) -> Vec<f64> {
        const INC: f64 = 0.01;
        let n = points.len();
        let mut values = Vec::new();
        let Some(&(mut x2, mut y2)) = points.first() else {
            return values;
        };
        let (mut last_x, mut last_y) = (0.0f64, 0.0f64);
        for &(next_x, next_y) in &points[1..] {
            let (x1, y1) = (x2, y2);
            let (mut x, mut y) = (x1, y1);
            x2 = next_x;
            y2 = next_y;
            let (dx, dy) = (x2 - x1, y2 - y1);
            let distance = (dx * dx + dy * dy).sqrt();
            let xinc = dx * INC / distance;
            let yinc = dy * INC / distance;
            let mut n2 = (distance / INC) as i64;
            if n == 2 {
                n2 += 1;
            }
            loop {
                let (dx, dy) = (x - last_x, y - last_y);
                if (dx * dx + dy * dy).sqrt() >= 1.0 - INC / 2.0 {
                    values.push(if interpolate {
                        self.interpolated(x, y)
                    } else {
                        self.value(java_round(x), java_round(y))
                    });
                    last_x = x;
                    last_y = y;
                }
                x += xinc;
                y += yinc;
                n2 -= 1;
                if n2 <= 0 {
                    break;
                }
            }
        }
        values
    }

    /// `Straightener.straightenLine` and the column averages
    /// `ProfilePlot` takes of its result: at each point of the spline
    /// fitted to `line`, `line_width` samples across the line, averaged.
    fn straightened(&self, line: &Relative, length: f64, line_width: usize) -> Vec<f64> {
        if line.xs.len() < 2 || line_width == 0 || !length.is_finite() {
            return Vec::new();
        }
        let (px, py) = line.spline_for_straightening(length);
        let n = px.len();
        if n < 2 {
            return Vec::new();
        }
        // The point before the first is extrapolated back one step.
        let mut x2 = f64::from(px[0] - (px[1] - px[0]));
        let mut y2 = f64::from(py[0] - (py[1] - py[0]));
        let mut profile = Vec::with_capacity(n);
        for i in 0..n {
            let (x1, y1) = (x2, y2);
            x2 = f64::from(px[i]);
            y2 = f64::from(py[i]);
            if line_width == 1 {
                profile.push(f64::from(self.interpolated(x2, y2) as f32));
                continue;
            }
            let mut dx = x2 - x1;
            let mut dy = y1 - y2;
            let length = f64::from((dx * dx + dy * dy).sqrt() as f32);
            dx /= length;
            dy /= length;
            let mut x = x2 - dy * (line_width as f64 - 1.0) / 2.0;
            let mut y = y2 - dx * (line_width as f64 - 1.0) / 2.0;
            // The straightened image holds single-precision samples;
            // those that are NaN are left out of the average.
            let (mut sum, mut count) = (0.0f64, 0u32);
            for _ in 0..line_width {
                let v = self.interpolated(x, y) as f32;
                if !v.is_nan() {
                    sum += f64::from(v);
                    count += 1;
                }
                x += dy;
                y += dx;
            }
            profile.push(sum / f64::from(count));
        }
        profile
    }

    /// `getPixelValue`: NaN outside the image.
    fn value(&self, x: i64, y: i64) -> f64 {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            self.values[y as usize * self.width + x as usize]
        } else {
            f64::NAN
        }
    }

    /// `getEdgeValue`: coordinates are clamped to the image, and the value
    /// passes through single precision.
    fn edge(&self, x: i64, y: i64) -> f64 {
        let x = x.clamp(0, self.width as i64 - 1);
        let y = y.clamp(0, self.height as i64 - 1);
        self.value(x, y) as f32 as f64
    }

    /// `ImageProcessor.getInterpolatedValue` with bilinear interpolation.
    fn interpolated(&self, x: f64, y: f64) -> f64 {
        let (w, h) = (self.width as f64, self.height as f64);
        let edge = x < 0.0 || x >= w - 1.0 || y < 0.0 || y >= h - 1.0;
        if edge && (x < -1.0 || x >= w || y < -1.0 || y >= h) {
            return 0.0;
        }
        let (xbase, ybase) = (x as i64, y as i64);
        let x_fraction = (x - xbase as f64).max(0.0);
        let y_fraction = (y - ybase as f64).max(0.0);
        let at = |x: i64, y: i64| {
            if edge {
                self.edge(x, y)
            } else {
                self.value(x, y)
            }
        };
        let lower_left = at(xbase, ybase);
        let lower_right = at(xbase + 1, ybase);
        let upper_right = at(xbase + 1, ybase + 1);
        let upper_left = at(xbase, ybase + 1);
        let upper = upper_left + x_fraction * (upper_right - upper_left);
        let lower = lower_left + x_fraction * (lower_right - lower_left);
        lower + y_fraction * (upper - lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_columns_and_rows() {
        let plane = Buffer::from_vec(vec![1u8, 2, 3, 4, 5, 6]);
        let s = Samples::Gray(&plane);
        let rect = Rect::new(1, 0, 2, 2);
        assert_eq!(column_average_profile(s, 3, 2, rect).unwrap(), [3.5, 4.5]);
        assert_eq!(row_average_profile(s, 3, 2, rect).unwrap(), [2.5, 5.5]);
        assert!(column_average_profile(s, 3, 2, Rect::new(2, 0, 2, 2)).is_err());
    }

    #[test]
    fn line_samples() {
        let plane = Buffer::from_vec(vec![0.0f32, 10.0, 20.0, 30.0]);
        let s = Samples::Gray(&plane);
        let nearest = line_profile(s, 4, 1, (0.0, 0.0), (3.0, 0.0), false).unwrap();
        assert_eq!(nearest, [0.0, 10.0, 20.0, 30.0]);
        let point = line_profile(s, 4, 1, (2.0, 0.0), (2.0, 0.0), true).unwrap();
        assert_eq!(point, [20.0]);
    }
}
