//! Convolution with an arbitrary kernel, after ImageJ's `Convolver`
//! plugin.

use calipix_core::{Buffer, Error, RealPixel, Result};

use super::tile::{Strip, default_strip_rows, filter_strips};
use super::{Frame, from_f32, real_plane, to_f32, wrap};

/// A convolution kernel with an odd number of columns and of rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Kernel {
    width: usize,
    height: usize,
    /// Coefficients row by row, single precision as in ImageJ.
    values: Vec<f32>,
}

impl Kernel {
    /// A kernel of `width` x `height` coefficients given row by row.
    pub fn new(width: usize, height: usize, values: Vec<f32>) -> Result<Kernel> {
        if width % 2 != 1 || height % 2 != 1 {
            return Err(Error::Other(format!(
                "a kernel must have an odd width and height; this one is {width} x {height}"
            )));
        }
        if values.len() != width * height {
            return Err(Error::Other(format!(
                "a {width} x {height} kernel needs {} values, not {}",
                width * height,
                values.len()
            )));
        }
        Ok(Kernel {
            width,
            height,
            values,
        })
    }

    /// Reads a kernel from text.
    ///
    /// Values are separated by spaces, tabs or commas, and rows by line
    /// breaks or semicolons: `"1 2 1; 2 4 2; 1 2 1"`. Every row must have
    /// the same number of values.
    ///
    /// Text without any row separator is read as ImageJ reads a kernel in
    /// a macro: 9, 25, 49... values make a square kernel, and any other
    /// odd count a single row. End a single row of 9 (25, ...) values with
    /// a semicolon to keep it a row.
    pub fn parse(text: &str) -> Result<Kernel> {
        let is_row_end = |c: char| c == '\n' || c == ';';
        let rows: Vec<Vec<f32>> = text
            .split(is_row_end)
            .map(|row| {
                row.split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|v| !v.is_empty())
                    .map(|v| {
                        v.parse::<f64>().map(|v| v as f32).map_err(|_| {
                            Error::Other(format!("`{v}` in the kernel is not a number"))
                        })
                    })
                    .collect::<Result<Vec<f32>>>()
            })
            .filter(|row| !matches!(row, Ok(values) if values.is_empty()))
            .collect::<Result<_>>()?;
        let Some(first) = rows.first() else {
            return Err(Error::Other("the kernel has no values".to_string()));
        };
        let width = first.len();
        if let Some(bad) = rows.iter().position(|row| row.len() != width) {
            return Err(Error::Other(format!(
                "row {} of the kernel has {} values but the first row has {width}",
                bad + 1,
                rows[bad].len()
            )));
        }
        let height = rows.len();
        let values = rows.concat();
        if height == 1 && !text.contains(is_row_end) {
            let side = (width as f64).sqrt().round() as usize;
            if side >= 3 && side * side == width && side % 2 == 1 {
                return Kernel::new(side, side, values);
            }
        }
        Kernel::new(width, height, values)
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// `Convolver.getScale`: one over the sum of the coefficients, or 1 if
    /// they sum to zero.
    pub fn scale(&self) -> f64 {
        let sum: f64 = self.values.iter().map(|&v| v as f64).sum();
        if sum != 0.0 { 1.0 / sum } else { 1.0 }
    }
}

/// Convolves `frame.rect` of a plane with `kernel`, as Process > Filters
/// > Convolve does (`Convolver.convolveFloat`; strictly a correlation, as
/// the kernel is not flipped).
///
/// Products are formed in single precision and summed in double. With
/// `normalize`, the sum is multiplied by [`Kernel::scale`]. See the module
/// documentation for pixel types and rounding.
pub fn convolve(plane: &Buffer, frame: Frame, kernel: &Kernel, normalize: bool) -> Result<Buffer> {
    let scale = if normalize { kernel.scale() } else { 1.0 };
    fn run<T: RealPixel>(src: &[T], frame: Frame, kernel: &Kernel, scale: f64) -> Result<Vec<T>> {
        let halo = kernel.height / 2;
        let rows = default_strip_rows(halo, frame.rect.h);
        filter_strips(src, frame, halo, rows, |strip: Strip<'_, T>| {
            Ok(convolve_strip(strip, kernel, scale))
        })
    }
    real_plane!(plane, "convolve", |s| run(s, frame, kernel, scale).map(wrap))
}

fn convolve_strip<T: RealPixel>(strip: Strip<'_, T>, kernel: &Kernel, scale: f64) -> Vec<T> {
    let Frame {
        width,
        height,
        rect: r,
    } = strip.frame;
    let pixels: Vec<f32> = strip.rows.iter().map(|&v| to_f32(v)).collect();
    let (uc, vc) = (kernel.width / 2, kernel.height / 2);
    let mut out = Vec::with_capacity(r.w * r.h);
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let inside = y >= vc && y + vc < height && x >= uc && x + uc < width;
            let mut sum = 0f64;
            if inside {
                let mut k = kernel.values.iter();
                for v in y - vc..=y + vc {
                    let start = v * width + x - uc;
                    for (&p, &c) in pixels[start..start + kernel.width].iter().zip(&mut k) {
                        sum += (p * c) as f64;
                    }
                }
            } else {
                // Near an edge, positions outside the plane read the
                // nearest sample.
                let mut k = kernel.values.iter();
                for v in 0..kernel.height {
                    let yy = (y + v).saturating_sub(vc).min(height - 1);
                    for u in 0..kernel.width {
                        let xx = (x + u).saturating_sub(uc).min(width - 1);
                        sum += (pixels[yy * width + xx] * k.next().copied().unwrap_or(0.0)) as f64;
                    }
                }
            }
            out.push(from_f32((sum * scale) as f32));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows_squares_and_single_rows() {
        let k = Kernel::parse("1 2 1\n2 4 2\n1 2 1\n").unwrap();
        assert_eq!((k.width(), k.height()), (3, 3));
        assert_eq!(k.scale(), 1.0 / 16.0);
        assert_eq!(Kernel::parse("1 2 1; 2 4 2; 1 2 1").unwrap(), k);
        assert_eq!(Kernel::parse("1,2,1,2,4,2,1,2,1").unwrap(), k);
        let row = Kernel::parse("1 1 1 1 1").unwrap();
        assert_eq!((row.width(), row.height()), (5, 1));
        let nine = Kernel::parse("1 1 1 1 1 1 1 1 1;").unwrap();
        assert_eq!((nine.width(), nine.height()), (9, 1));
        let column = Kernel::parse("1;2;1").unwrap();
        assert_eq!((column.width(), column.height()), (1, 3));
        assert_eq!(Kernel::parse("-1 0 1").unwrap().scale(), 1.0);
    }

    #[test]
    fn bad_kernels_say_what_is_wrong() {
        let message = |text: &str| Kernel::parse(text).unwrap_err().to_string();
        assert!(message("1 2\n3 4").contains("odd width and height"));
        assert!(message("1 2 3\n4 5").contains("row 2"));
        assert!(message("1 x 3").contains("`x`"));
        assert!(message("  ").contains("no values"));
    }

    #[test]
    fn identity_and_shift() {
        let plane = Buffer::from_vec(vec![1u8, 2, 3, 4, 5, 6]);
        let frame = Frame::whole(3, 2);
        let same = convolve(&plane, frame, &Kernel::parse("0 0 0 0 1 0 0 0 0").unwrap(), true);
        assert_eq!(same.unwrap(), plane);
        // The coefficient left of centre takes each sample's left
        // neighbour; the first column repeats itself.
        let shifted = convolve(&plane, frame, &Kernel::parse("1 0 0").unwrap(), true).unwrap();
        assert_eq!(shifted.as_slice::<u8>().unwrap(), &[1, 1, 2, 4, 4, 5]);
    }
}
