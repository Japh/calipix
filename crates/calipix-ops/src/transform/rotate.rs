//! Rotation by any angle, and translation.

use calipix_core::{Buffer, Rect, Result};

use super::sample::{Grid, Interpolation, Sample};
use super::{on_samples, plane_height};
use crate::math::java;

/// Rotates the part of a plane inside `rect` by `angle` degrees clockwise
/// about the centre of `rect`, as the `rotate` method of ImageJ's
/// processors does. The plane keeps its size: what turns out of `rect` is
/// lost, and samples of `rect` that nothing turns into are set to
/// `background`. Samples are read from the whole plane, so with a
/// selection the surroundings can turn into it.
///
/// With bicubic interpolation integer results are limited to the type's
/// range.
pub fn rotate(
    buf: &Buffer,
    width: usize,
    rect: Rect,
    angle: f64,
    interpolation: Interpolation,
    background: f64,
) -> Result<Buffer> {
    let height = plane_height(buf.len(), width, Some(rect))?;
    // ByteProcessor.rotate returns at once for a whole number of turns;
    // the other processors go through the motions, which is not quite the
    // identity at the right and bottom edges.
    if matches!(buf, Buffer::U8(_)) && angle % 360.0 == 0.0 {
        return Ok(buf.clone());
    }
    on_samples!(buf, |s| rotate_samples(
        s,
        width,
        height,
        rect,
        angle,
        interpolation,
        background
    ))
}

fn rotate_samples<T: Sample>(
    src: &[T],
    width: usize,
    height: usize,
    r: Rect,
    angle: f64,
    interpolation: Interpolation,
    background: f64,
) -> Vec<T> {
    let mut out = src.to_vec();
    let grid = Grid {
        px: src,
        w: width,
        h: height,
    };
    let center_x = r.x as f64 + (r.w as f64 - 1.0) / 2.0;
    let center_y = r.y as f64 + (r.h as f64 - 1.0) / 2.0;
    let angle_radians = -angle / (180.0 / std::f64::consts::PI);
    let ca = angle_radians.cos();
    let sa = angle_radians.sin();
    let tmp1 = center_y * sa - center_x * ca;
    let tmp2 = -center_x * sa - center_y * ca;
    let (dwidth, dheight) = (width as f64, height as f64);
    let (xlimit, xlimit2) = (dwidth - 1.0, dwidth - 1.001);
    let (ylimit, ylimit2) = (dheight - 1.0, dheight - 1.001);
    let fill = T::from_f64(background);

    for y in r.y..r.y + r.h {
        let tmp3 = tmp1 - y as f64 * sa + center_x;
        let tmp4 = tmp2 + y as f64 * ca + center_y;
        for x in r.x..r.x + r.w {
            let mut xs = x as f64 * ca + tmp3;
            let mut ys = x as f64 * sa + tmp4;
            let value = if interpolation == Interpolation::Bicubic {
                T::from_interpolated(grid.bicubic(xs, ys, background))
            } else if xs >= -0.01 && xs < dwidth && ys >= -0.01 && ys < dheight {
                if interpolation == Interpolation::Bilinear {
                    if xs < 0.0 {
                        xs = 0.0;
                    }
                    if xs >= xlimit {
                        xs = xlimit2;
                    }
                    if ys < 0.0 {
                        ys = 0.0;
                    }
                    if ys >= ylimit {
                        ys = ylimit2;
                    }
                    T::from_interpolated(grid.bilinear(xs, ys))
                } else {
                    let ixs = (java::int(xs + 0.5).max(0) as usize).min(width - 1);
                    let iys = (java::int(ys + 0.5).max(0) as usize).min(height - 1);
                    src[width * iys + ixs]
                }
            } else {
                fill
            };
            out[y * width + x] = value;
        }
    }
    out
}

/// Moves the contents of `rect` by `(x_offset, y_offset)` samples, positive
/// to the right and down (`ImageProcessor.translate`). Samples are read
/// from the whole plane; where the source lies outside the plane the result
/// is 0.
///
/// Whole offsets, or no interpolation, copy samples (the offsets are
/// truncated). Otherwise the plane is interpolated, and bilinear
/// interpolation gives 0 within one sample of the right and bottom edges
/// of the plane, as in ImageJ.
pub fn translate(
    buf: &Buffer,
    width: usize,
    rect: Rect,
    x_offset: f64,
    y_offset: f64,
    interpolation: Interpolation,
) -> Result<Buffer> {
    let height = plane_height(buf.len(), width, Some(rect))?;
    on_samples!(buf, |s| translate_samples(
        s,
        width,
        height,
        rect,
        x_offset,
        y_offset,
        interpolation
    ))
}

fn translate_samples<T: Sample>(
    src: &[T],
    width: usize,
    height: usize,
    r: Rect,
    x_offset: f64,
    y_offset: f64,
    interpolation: Interpolation,
) -> Vec<T> {
    let mut out = src.to_vec();
    let grid = Grid {
        px: src,
        w: width,
        h: height,
    };
    let zero = T::default();
    let (dx, dy) = (java::int(x_offset), java::int(y_offset));
    let whole_offsets = x_offset == dx as f64 && y_offset == dy as f64;
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let value = if whole_offsets || interpolation == Interpolation::None {
                let (sx, sy) = (x as i64 - dx as i64, y as i64 - dy as i64);
                if sx >= 0 && sx < width as i64 && sy >= 0 && sy < height as i64 {
                    src[sy as usize * width + sx as usize]
                } else {
                    zero
                }
            } else {
                let (xs, ys) = (x as f64 - x_offset, y as f64 - y_offset);
                if interpolation == Interpolation::Bilinear {
                    // getPixelInterpolated
                    if xs < 0.0 || ys < 0.0 || xs >= width as f64 - 1.0 || ys >= height as f64 - 1.0
                    {
                        zero
                    } else {
                        T::from_interpolated(grid.bilinear(xs, ys))
                    }
                } else {
                    T::from_interpolated(grid.bicubic(xs, ys, 0.0))
                }
            };
            out[y * width + x] = value;
        }
    }
    out
}
