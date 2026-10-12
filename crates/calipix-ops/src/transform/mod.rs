//! Geometric transforms of one plane: flips, rotations, translation,
//! scaling and changing the canvas.
//!
//! Every function takes a row-major plane and its width and works on any
//! real pixel type. Functions that ImageJ confines to the selection take the
//! rectangle to change; samples outside it are returned unchanged.

mod resize;
mod rotate;
mod sample;
mod simple;

use calipix_core::{Error, Rect, Result};

pub use resize::{resize, resize_z, scale_in_place};
pub use rotate::{rotate, translate};
pub use sample::Interpolation;
pub use simple::{canvas, flip_horizontal, flip_vertical, rotate_left, rotate_right};

/// The height of a plane of `len` samples that is `width` wide, checking
/// that the two agree and that `rect` lies inside it.
fn plane_height(len: usize, width: usize, rect: Option<Rect>) -> Result<usize> {
    if width == 0 || len == 0 || !len.is_multiple_of(width) {
        return Err(Error::Shape(format!(
            "plane of {len} samples is not a whole number of rows of width {width}"
        )));
    }
    let height = len / width;
    if let Some(rect) = rect
        && !rect.fits(width, height)
    {
        return Err(Error::OutOfBounds(format!(
            "region {rect:?} outside {width}x{height} plane"
        )));
    }
    Ok(height)
}

fn complex_error() -> Error {
    Error::Unsupported("complex images cannot be transformed".to_string())
}

/// Runs a generic function of the typed samples on any real plane.
macro_rules! on_samples {
    ($buf:expr, |$s:ident| $body:expr) => {
        calipix_core::dispatch_real!($buf, |$s| Ok(Buffer::from_vec($body)), complex => Err(crate::transform::complex_error()))
    };
}
pub(crate) use on_samples;
