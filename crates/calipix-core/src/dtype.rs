use std::fmt;
use std::sync::Arc;

use num_complex::{Complex32, Complex64};

use crate::Buffer;

/// Pixel sample type of an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
    C32,
    C64,
}

impl DType {
    pub const ALL: [DType; 12] = [
        DType::U8,
        DType::I8,
        DType::U16,
        DType::I16,
        DType::U32,
        DType::I32,
        DType::U64,
        DType::I64,
        DType::F32,
        DType::F64,
        DType::C32,
        DType::C64,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DType::U8 => "u8",
            DType::I8 => "i8",
            DType::U16 => "u16",
            DType::I16 => "i16",
            DType::U32 => "u32",
            DType::I32 => "i32",
            DType::U64 => "u64",
            DType::I64 => "i64",
            DType::F32 => "f32",
            DType::F64 => "f64",
            DType::C32 => "c32",
            DType::C64 => "c64",
        }
    }

    pub fn from_name(name: &str) -> Option<DType> {
        DType::ALL.into_iter().find(|d| d.name() == name)
    }

    /// Size of one sample in bytes.
    pub fn size(self) -> usize {
        match self {
            DType::U8 | DType::I8 => 1,
            DType::U16 | DType::I16 => 2,
            DType::U32 | DType::I32 | DType::F32 => 4,
            DType::U64 | DType::I64 | DType::F64 | DType::C32 => 8,
            DType::C64 => 16,
        }
    }

    pub fn is_complex(self) -> bool {
        matches!(self, DType::C32 | DType::C64)
    }

    pub fn is_float(self) -> bool {
        matches!(self, DType::F32 | DType::F64)
    }

    pub fn is_integer(self) -> bool {
        !self.is_float() && !self.is_complex()
    }

    pub fn is_signed(self) -> bool {
        !matches!(self, DType::U8 | DType::U16 | DType::U32 | DType::U64)
    }

    /// Representable range for integer types; `None` for float and complex.
    pub fn integer_range(self) -> Option<(f64, f64)> {
        Some(match self {
            DType::U8 => (0.0, u8::MAX as f64),
            DType::I8 => (i8::MIN as f64, i8::MAX as f64),
            DType::U16 => (0.0, u16::MAX as f64),
            DType::I16 => (i16::MIN as f64, i16::MAX as f64),
            DType::U32 => (0.0, u32::MAX as f64),
            DType::I32 => (i32::MIN as f64, i32::MAX as f64),
            DType::U64 => (0.0, u64::MAX as f64),
            DType::I64 => (i64::MIN as f64, i64::MAX as f64),
            _ => return None,
        })
    }
}

impl fmt::Display for DType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A Rust type that can be stored as an image sample.
pub trait Pixel: Copy + Default + PartialEq + fmt::Debug + Send + Sync + 'static {
    const DTYPE: DType;

    /// Typed view of `buf`, or `None` if it holds a different pixel type.
    fn slice(buf: &Buffer) -> Option<&[Self]>;

    fn wrap(data: Arc<[Self]>) -> Buffer;
}

/// A non-complex [`Pixel`].
pub trait RealPixel: Pixel + PartialOrd {
    fn to_f64(self) -> f64;

    /// Converts from `f64`. Integer types round to nearest (ties away from
    /// zero) and saturate at the type's range; NaN becomes zero.
    fn from_f64(v: f64) -> Self;
}

macro_rules! impl_pixel {
    ($($variant:ident => $ty:ty),* $(,)?) => {$(
        impl Pixel for $ty {
            const DTYPE: DType = DType::$variant;

            fn slice(buf: &Buffer) -> Option<&[Self]> {
                match buf {
                    Buffer::$variant(data) => Some(data),
                    _ => None,
                }
            }

            fn wrap(data: Arc<[Self]>) -> Buffer {
                Buffer::$variant(data)
            }
        }
    )*};
}

impl_pixel! {
    U8 => u8, I8 => i8, U16 => u16, I16 => i16, U32 => u32, I32 => i32,
    U64 => u64, I64 => i64, F32 => f32, F64 => f64, C32 => Complex32, C64 => Complex64,
}

macro_rules! impl_real_int {
    ($($ty:ty),*) => {$(
        impl RealPixel for $ty {
            fn to_f64(self) -> f64 {
                self as f64
            }

            fn from_f64(v: f64) -> Self {
                // `as` saturates and maps NaN to zero.
                v.round() as $ty
            }
        }
    )*};
}

impl_real_int!(u8, i8, u16, i16, u32, i32, u64, i64);

impl RealPixel for f32 {
    fn to_f64(self) -> f64 {
        self as f64
    }

    fn from_f64(v: f64) -> Self {
        v as f32
    }
}

impl RealPixel for f64 {
    fn to_f64(self) -> f64 {
        self
    }

    fn from_f64(v: f64) -> Self {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for d in DType::ALL {
            assert_eq!(DType::from_name(d.name()), Some(d));
        }
        assert_eq!(DType::from_name("u7"), None);
    }

    #[test]
    fn from_f64_rounds_and_saturates() {
        assert_eq!(u8::from_f64(254.5), 255);
        assert_eq!(u8::from_f64(300.0), 255);
        assert_eq!(u8::from_f64(-3.0), 0);
        assert_eq!(u8::from_f64(f64::NAN), 0);
        assert_eq!(i16::from_f64(-2.5), -3);
        assert_eq!(i16::from_f64(-1e9), i16::MIN);
    }
}
