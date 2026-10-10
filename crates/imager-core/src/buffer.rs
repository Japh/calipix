use std::sync::Arc;

use num_complex::{Complex32, Complex64};

use crate::{DType, Error, Pixel, Rect, Result};

/// One contiguous, typed block of samples: a whole XY plane or a region of
/// one, in row-major order (X varies fastest).
///
/// Cloning is cheap; the samples are shared.
#[derive(Clone, Debug, PartialEq)]
pub enum Buffer {
    U8(Arc<[u8]>),
    I8(Arc<[i8]>),
    U16(Arc<[u16]>),
    I16(Arc<[i16]>),
    U32(Arc<[u32]>),
    I32(Arc<[i32]>),
    U64(Arc<[u64]>),
    I64(Arc<[i64]>),
    F32(Arc<[f32]>),
    F64(Arc<[f64]>),
    C32(Arc<[Complex32]>),
    C64(Arc<[Complex64]>),
}

/// Runs `$body` with `$s` bound to the buffer's typed slice (`&[T]`),
/// whatever its pixel type. The body is instantiated once per pixel type.
#[macro_export]
macro_rules! dispatch {
    ($buf:expr, |$s:ident| $body:expr) => {
        match $buf {
            $crate::Buffer::U8(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::I8(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::U16(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::I16(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::U32(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::I32(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::U64(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::I64(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::F32(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::F64(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::C32(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::C64(__b) => {
                let $s = &__b[..];
                $body
            }
        }
    };
}

/// Like [`dispatch!`], but the body is only instantiated for real pixel
/// types ([`RealPixel`](crate::RealPixel)); complex buffers evaluate
/// `$complex` instead.
#[macro_export]
macro_rules! dispatch_real {
    ($buf:expr, |$s:ident| $body:expr, complex => $complex:expr) => {
        match $buf {
            $crate::Buffer::U8(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::I8(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::U16(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::I16(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::U32(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::I32(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::U64(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::I64(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::F32(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::F64(__b) => {
                let $s = &__b[..];
                $body
            }
            $crate::Buffer::C32(_) | $crate::Buffer::C64(_) => $complex,
        }
    };
}

impl Buffer {
    pub fn from_vec<T: Pixel>(data: Vec<T>) -> Buffer {
        T::wrap(data.into())
    }

    pub fn zeros(dtype: DType, len: usize) -> Buffer {
        fn z<T: Pixel>(len: usize) -> Buffer {
            Buffer::from_vec(vec![T::default(); len])
        }
        match dtype {
            DType::U8 => z::<u8>(len),
            DType::I8 => z::<i8>(len),
            DType::U16 => z::<u16>(len),
            DType::I16 => z::<i16>(len),
            DType::U32 => z::<u32>(len),
            DType::I32 => z::<i32>(len),
            DType::U64 => z::<u64>(len),
            DType::I64 => z::<i64>(len),
            DType::F32 => z::<f32>(len),
            DType::F64 => z::<f64>(len),
            DType::C32 => z::<Complex32>(len),
            DType::C64 => z::<Complex64>(len),
        }
    }

    pub fn dtype(&self) -> DType {
        fn d<T: Pixel>(_: &[T]) -> DType {
            T::DTYPE
        }
        dispatch!(self, |s| d(s))
    }

    /// Number of samples.
    pub fn len(&self) -> usize {
        dispatch!(self, |s| s.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Size of the sample data in bytes.
    pub fn byte_len(&self) -> usize {
        self.len() * self.dtype().size()
    }

    /// Typed view of the samples, or `None` if `T` is not this buffer's type.
    pub fn as_slice<T: Pixel>(&self) -> Option<&[T]> {
        T::slice(self)
    }

    /// Typed view of the samples, or a [`Error::DType`] error.
    pub fn try_slice<T: Pixel>(&self) -> Result<&[T]> {
        T::slice(self).ok_or(Error::DType {
            expected: T::DTYPE,
            found: self.dtype(),
        })
    }

    /// Sample `i` as `f64`, or `None` if out of range or complex.
    pub fn get_f64(&self, i: usize) -> Option<f64> {
        use crate::RealPixel;
        dispatch_real!(self, |s| s.get(i).map(|v| v.to_f64()), complex => None)
    }

    /// True if both buffers share the same allocation.
    pub fn shares_storage(&self, other: &Buffer) -> bool {
        fn addr<T>(s: &[T]) -> *const u8 {
            s.as_ptr().cast()
        }
        self.dtype() == other.dtype()
            && self.len() == other.len()
            && dispatch!(self, |s| addr(s)) == dispatch!(other, |s| addr(s))
    }
}

/// Copies `region` out of a row-major buffer that is `width` samples wide.
pub fn crop_buffer(buf: &Buffer, width: usize, region: Rect) -> Result<Buffer> {
    fn crop<T: Pixel>(src: &[T], width: usize, r: Rect) -> Buffer {
        let mut out = Vec::with_capacity(r.w * r.h);
        for y in r.y..r.y + r.h {
            let start = y * width + r.x;
            out.extend_from_slice(&src[start..start + r.w]);
        }
        Buffer::from_vec(out)
    }

    if width == 0 || !buf.len().is_multiple_of(width) {
        return Err(Error::Shape(format!(
            "buffer of {} samples is not a whole number of rows of width {width}",
            buf.len()
        )));
    }
    let height = buf.len() / width;
    if !region.fits(width, height) {
        return Err(Error::OutOfBounds(format!(
            "region {region:?} outside {width}x{height} plane"
        )));
    }
    if region.x == 0 && region.y == 0 && region.w == width && region.h == height {
        return Ok(buf.clone());
    }
    Ok(dispatch!(buf, |s| crop(s, width, region)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zeros_has_requested_type_and_len() {
        for d in DType::ALL {
            let b = Buffer::zeros(d, 7);
            assert_eq!(b.dtype(), d);
            assert_eq!(b.len(), 7);
            assert_eq!(b.byte_len(), 7 * d.size());
        }
    }

    #[test]
    fn typed_access() {
        let b = Buffer::from_vec(vec![1u16, 2, 3]);
        assert_eq!(b.as_slice::<u16>(), Some(&[1u16, 2, 3][..]));
        assert!(b.as_slice::<u8>().is_none());
        assert!(matches!(b.try_slice::<f32>(), Err(Error::DType { .. })));
        assert_eq!(b.get_f64(2), Some(3.0));
        assert_eq!(b.get_f64(3), None);
    }

    #[test]
    fn crop_copies_rows() {
        let b = Buffer::from_vec((0u8..12).collect());
        let c = crop_buffer(&b, 4, Rect::new(1, 1, 2, 2)).unwrap();
        assert_eq!(c.as_slice::<u8>().unwrap(), &[5, 6, 9, 10]);
        assert!(crop_buffer(&b, 4, Rect::new(3, 0, 2, 1)).is_err());
        assert!(crop_buffer(&b, 5, Rect::new(0, 0, 1, 1)).is_err());
    }

    #[test]
    fn full_crop_shares_storage() {
        let b = Buffer::from_vec(vec![0f32; 6]);
        let c = crop_buffer(&b, 3, Rect::new(0, 0, 3, 2)).unwrap();
        assert!(b.shares_storage(&c));
        assert!(!b.shares_storage(&Buffer::from_vec(vec![0f32; 6])));
    }
}
