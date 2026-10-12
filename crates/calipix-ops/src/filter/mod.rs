//! Filters: operations that compute each sample from its neighbourhood.
//!
//! Every function here filters one XY plane and reproduces a filter of
//! ImageJ 1.x, named in its documentation. They share these conventions:
//!
//! * A [`Frame`] gives the plane's size and the rectangle to filter. The
//!   result is a whole plane in which only the samples inside the rectangle
//!   have changed. Neighbours outside the rectangle are read from the plane,
//!   and neighbours outside the plane are replaced by the nearest edge
//!   sample, as in ImageJ, unless a filter says otherwise.
//! * ImageJ computes most filters in 32-bit float whatever the pixel type,
//!   and so do these. Integer results are rounded half up
//!   (`floor(v + 0.5)`, computed in single precision) and clamped to the
//!   type's range, which is what ImageJ does for 8-bit and 16-bit images.
//! * Pixel types that ImageJ lacks follow the nearest ImageJ rule: other
//!   integers are treated like 16-bit (with their own range), and 64-bit
//!   float like 32-bit float. Because the arithmetic is single precision,
//!   integers beyond 2^24 and 64-bit floats lose precision there. Complex
//!   planes are refused.
//! * RGB images are three 8-bit planes, each filtered like an 8-bit image.
//!   ImageJ does the same for every filter here except Sharpen and Shadows
//!   (see [`sharpen`]).
//!
//! Rows are filtered in parallel. This never changes results.

mod convolve;
mod gaussian;
mod rank;
mod three;
mod tile;

pub use convolve::{Kernel, convolve};
pub use gaussian::{gaussian_accuracy, gaussian_blur, gaussian_kernel, unsharp_mask};
pub use rank::{
    Outliers, Rank, despeckle, kernel_line_radii, rank_filter, remove_nans, remove_outliers, top_hat,
};
pub use three::{Direction, find_edges, shadows, sharpen, smooth};
pub use tile::{Strip, default_strip_rows, filter_strips};

use calipix_core::{Buffer, DType, Error, Rect, RealPixel, Result};

/// The size of a plane and the rectangle of it to filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// The samples to compute: a selection's bounding rectangle, or the
    /// whole plane.
    pub rect: Rect,
}

impl Frame {
    /// The whole of a `width` x `height` plane.
    pub fn whole(width: usize, height: usize) -> Frame {
        Frame {
            width,
            height,
            rect: Rect::new(0, 0, width, height),
        }
    }

    /// `rect` of a `width` x `height` plane.
    pub fn new(width: usize, height: usize, rect: Rect) -> Frame {
        Frame {
            width,
            height,
            rect,
        }
    }

    /// Checks that the rectangle lies in the plane and that a buffer of
    /// `len` samples is exactly one plane.
    pub fn check(&self, len: usize) -> Result<()> {
        if self.width.checked_mul(self.height) != Some(len) {
            return Err(Error::Shape(format!(
                "a plane of {} samples is not {} x {}",
                len, self.width, self.height
            )));
        }
        if !self.rect.fits(self.width, self.height) {
            return Err(Error::OutOfBounds(format!(
                "rectangle {:?} is empty or outside the {} x {} plane",
                self.rect, self.width, self.height
            )));
        }
        Ok(())
    }
}

/// A sample as ImageJ's `toFloat` gives it.
#[inline]
pub(crate) fn to_f32<T: RealPixel>(v: T) -> f32 {
    v.to_f64() as f32
}

/// A float result stored the way `ByteProcessor.setPixels(int,
/// FloatProcessor)` and `ShortProcessor.setPixels` store it: `v + 0.5`
/// in single precision, clamped to the type's range and truncated. For
/// types with negative values this rounds half up. Float types take the
/// value as it is.
#[inline]
pub(crate) fn from_f32<T: RealPixel>(v: f32) -> T {
    if T::DTYPE.is_float() {
        T::from_f64(v as f64)
    } else {
        // `from_f64` saturates and maps NaN to zero, as Java's casts do.
        T::from_f64((v + 0.5f32).floor() as f64)
    }
}

/// Largest value of an integer type as a float (ImageJ's
/// `ImageProcessor.maxValue`), NaN for float types.
pub(crate) fn max_value(dtype: DType) -> f32 {
    dtype.integer_range().map_or(f32::NAN, |(_, hi)| hi as f32)
}

pub(crate) fn complex_error(what: &str) -> Error {
    Error::Unsupported(format!("cannot {what} a complex image"))
}

/// Runs `$body` with `$s` bound to the typed samples of a real plane, and
/// fails for a complex one.
macro_rules! real_plane {
    ($buf:expr, $what:expr, |$s:ident| $body:expr) => {
        calipix_core::dispatch_real!($buf, |$s| $body, complex => Err($crate::filter::complex_error($what)))
    };
}
pub(crate) use real_plane;

/// Wraps typed samples as a buffer.
pub(crate) fn wrap<T: RealPixel>(samples: Vec<T>) -> Buffer {
    Buffer::from_vec(samples)
}
