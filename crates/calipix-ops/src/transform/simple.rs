//! Transforms that only move samples: flips, quarter turns and the canvas.

use calipix_core::{Buffer, Pixel, RealPixel, Rect, Result, dispatch};

use super::{on_samples, plane_height};

/// Mirrors `rect` left to right (`ImageProcessor.flipHorizontal`). Samples
/// outside `rect` are unchanged.
pub fn flip_horizontal(buf: &Buffer, width: usize, rect: Rect) -> Result<Buffer> {
    fn flip<T: Pixel>(src: &[T], width: usize, r: Rect) -> Buffer {
        let mut out = src.to_vec();
        for y in r.y..r.y + r.h {
            out[y * width + r.x..y * width + r.x + r.w].reverse();
        }
        Buffer::from_vec(out)
    }
    plane_height(buf.len(), width, Some(rect))?;
    Ok(dispatch!(buf, |s| flip(s, width, rect)))
}

/// Mirrors `rect` top to bottom (`flipVertical`). Samples outside `rect`
/// are unchanged.
pub fn flip_vertical(buf: &Buffer, width: usize, rect: Rect) -> Result<Buffer> {
    fn flip<T: Pixel>(src: &[T], width: usize, r: Rect) -> Buffer {
        let mut out = src.to_vec();
        for row in 0..r.h {
            let from = (r.y + r.h - 1 - row) * width + r.x;
            let to = (r.y + row) * width + r.x;
            out[to..to + r.w].copy_from_slice(&src[from..from + r.w]);
        }
        Buffer::from_vec(out)
    }
    plane_height(buf.len(), width, Some(rect))?;
    Ok(dispatch!(buf, |s| flip(s, width, rect)))
}

/// Turns the plane a quarter turn clockwise (`ImageProcessor.rotateRight`).
/// The result is as wide as the plane was high.
pub fn rotate_right(buf: &Buffer, width: usize) -> Result<Buffer> {
    fn turn<T: Pixel>(src: &[T], width: usize, height: usize) -> Buffer {
        let mut out = Vec::with_capacity(src.len());
        for y in 0..width {
            for x in 0..height {
                out.push(src[(height - 1 - x) * width + y]);
            }
        }
        Buffer::from_vec(out)
    }
    let height = plane_height(buf.len(), width, None)?;
    Ok(dispatch!(buf, |s| turn(s, width, height)))
}

/// Turns the plane a quarter turn anticlockwise (`rotateLeft`).
pub fn rotate_left(buf: &Buffer, width: usize) -> Result<Buffer> {
    fn turn<T: Pixel>(src: &[T], width: usize, height: usize) -> Buffer {
        let mut out = Vec::with_capacity(src.len());
        for y in 0..width {
            for x in 0..height {
                out.push(src[x * width + (width - 1 - y)]);
            }
        }
        Buffer::from_vec(out)
    }
    let height = plane_height(buf.len(), width, None)?;
    Ok(dispatch!(buf, |s| turn(s, width, height)))
}

/// Places the plane on a new canvas of `new_width` x `new_height` samples
/// with its top-left corner at `(x_offset, y_offset)`, which may be
/// negative; whatever falls outside the canvas is cut off, and canvas the
/// plane does not cover is set to `fill`
/// (`CanvasResizer.expandImage`). `fill` is converted to the pixel type by
/// rounding and limiting to its range.
pub fn canvas(
    buf: &Buffer,
    width: usize,
    new_width: usize,
    new_height: usize,
    x_offset: i64,
    y_offset: i64,
    fill: f64,
) -> Result<Buffer> {
    #[allow(clippy::too_many_arguments)]
    fn place<T: RealPixel>(
        src: &[T],
        width: usize,
        height: usize,
        new_width: usize,
        new_height: usize,
        x_offset: i64,
        y_offset: i64,
        fill: f64,
    ) -> Vec<T> {
        let mut out = vec![T::from_f64(fill); new_width * new_height];
        // The part of the canvas the plane covers.
        let x0 = x_offset.max(0);
        let x1 = (x_offset + width as i64).min(new_width as i64);
        let y0 = y_offset.max(0);
        let y1 = (y_offset + height as i64).min(new_height as i64);
        if x1 > x0 {
            let len = (x1 - x0) as usize;
            for y in y0..y1 {
                let from = (y - y_offset) as usize * width + (x0 - x_offset) as usize;
                let to = y as usize * new_width + x0 as usize;
                out[to..to + len].copy_from_slice(&src[from..from + len]);
            }
        }
        out
    }
    let height = plane_height(buf.len(), width, None)?;
    if new_width == 0 || new_height == 0 {
        return Err(calipix_core::Error::Shape(
            "the canvas must be at least one sample wide and high".to_string(),
        ));
    }
    on_samples!(buf, |s| place(
        s, width, height, new_width, new_height, x_offset, y_offset, fill
    ))
}
