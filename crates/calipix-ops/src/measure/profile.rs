//! Intensity profiles: the numbers behind Analyze > Plot Profile
//! (`ij.gui.ProfilePlot`).

use calipix_core::{Buffer, Error, RealPixel, Rect, Result, dispatch_real};

use super::Samples;

/// `ImageProcessor.getPixelValue` for every sample of a plane, as `f64`.
/// RGB pixels give ImageJ's unrounded grey value in single precision.
pub(super) fn pixel_values(samples: Samples<'_>) -> Result<Vec<f64>> {
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
    let values = pixel_values(samples)?;
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
    let values = pixel_values(samples)?;
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
