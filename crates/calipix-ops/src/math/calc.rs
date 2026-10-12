use calipix_core::{Buffer, Error, Pixel, Result};

use super::java;

/// An operation of ImageJ's Process > Image Calculator, combining a
/// destination sample `d` (image 1) with a source sample `s` (image 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcOp {
    /// `d + s`
    Add,
    /// `d - s`
    Subtract,
    /// `d * s`
    Multiply,
    /// `d / s`
    Divide,
    And,
    Or,
    Xor,
    /// The smaller of `d` and `s`.
    Min,
    /// The larger of `d` and `s`.
    Max,
    /// `(d + s) / 2`
    Average,
    /// `|d - s|`
    Difference,
    /// `s`
    Copy,
    /// `s`, except where `s` is zero, which leaves `d`.
    TransparentZero,
}

impl CalcOp {
    pub const ALL: [CalcOp; 13] = [
        CalcOp::Add,
        CalcOp::Subtract,
        CalcOp::Multiply,
        CalcOp::Divide,
        CalcOp::And,
        CalcOp::Or,
        CalcOp::Xor,
        CalcOp::Min,
        CalcOp::Max,
        CalcOp::Average,
        CalcOp::Difference,
        CalcOp::Copy,
        CalcOp::TransparentZero,
    ];

    /// The operation's name in ImageJ's Image Calculator dialog.
    pub fn name(self) -> &'static str {
        match self {
            CalcOp::Add => "Add",
            CalcOp::Subtract => "Subtract",
            CalcOp::Multiply => "Multiply",
            CalcOp::Divide => "Divide",
            CalcOp::And => "AND",
            CalcOp::Or => "OR",
            CalcOp::Xor => "XOR",
            CalcOp::Min => "Min",
            CalcOp::Max => "Max",
            CalcOp::Average => "Average",
            CalcOp::Difference => "Difference",
            CalcOp::Copy => "Copy",
            CalcOp::TransparentZero => "Transparent-zero",
        }
    }

    pub fn from_name(name: &str) -> Option<CalcOp> {
        CalcOp::ALL.into_iter().find(|op| op.name() == name)
    }
}

/// Combines two planes of the same pixel type and size sample by sample, as
/// the `copyBits` method of ImageJ's blitters does for `ImageCalculator`.
///
/// * **Unsigned 8-bit** (`ByteBlitter`): Add and Multiply saturate at 255,
///   Subtract at 0. Divide is integer division, and 255 where the divisor
///   is zero. Average is `(d + s) / 2` rounded down.
/// * **Unsigned 16-bit** (`ShortBlitter`): the same with 65535, except
///   that a product of 2^31 or more becomes 0, because ImageJ multiplies in
///   32-bit integers and clamps the wrapped, negative result.
/// * **Other integers** (not in ImageJ): results saturate at the type's
///   range; dividing by zero gives the type's largest value.
/// * **Float** (`FloatBlitter`): plain arithmetic in the sample's
///   precision. `divide_by_zero` is ImageJ's "Divide by zero value"
///   preference: if it is finite, a quotient whose divisor is zero is
///   replaced by it; the default, positive infinity, leaves the IEEE
///   result (signed infinity, or NaN for 0 / 0). AND, OR and XOR act on
///   the samples truncated to 32-bit integers.
///
/// Min and Max keep `d` when either sample is NaN.
pub fn calculate(dst: &Buffer, src: &Buffer, op: CalcOp, divide_by_zero: f64) -> Result<Buffer> {
    if dst.dtype() != src.dtype() {
        return Err(Error::DType {
            expected: dst.dtype(),
            found: src.dtype(),
        });
    }
    if dst.len() != src.len() {
        return Err(Error::Shape(format!(
            "cannot combine planes of {} and {} samples",
            dst.len(),
            src.len()
        )));
    }
    fn zip<T: Pixel>(dst: &[T], src: &Buffer, f: impl Fn(T, T) -> T) -> Result<Buffer> {
        let src = src.try_slice::<T>()?;
        Ok(Buffer::from_vec(
            dst.iter().zip(src).map(|(&d, &s)| f(d, s)).collect::<Vec<_>>(),
        ))
    }
    match dst {
        Buffer::U8(d) => zip(d, src, |d, s| byte_op(d, s, op)),
        Buffer::U16(d) => zip(d, src, |d, s| short_op(d, s, op)),
        Buffer::I8(d) => zip(d, src, |d, s| int_op(d, s, op)),
        Buffer::I16(d) => zip(d, src, |d, s| int_op(d, s, op)),
        Buffer::U32(d) => zip(d, src, |d, s| int_op(d, s, op)),
        Buffer::I32(d) => zip(d, src, |d, s| int_op(d, s, op)),
        Buffer::U64(d) => zip(d, src, |d, s| int_op(d, s, op)),
        Buffer::I64(d) => zip(d, src, |d, s| int_op(d, s, op)),
        Buffer::F32(d) => {
            let dbz = divide_by_zero as f32;
            zip(d, src, |d, s| f32_op(d, s, op, dbz))
        }
        Buffer::F64(d) => zip(d, src, |d, s| f64_op(d, s, op, divide_by_zero)),
        Buffer::C32(_) | Buffer::C64(_) => Err(Error::Unsupported(
            "the image calculator does not support complex images".to_string(),
        )),
    }
}

/// [`calculate`] for the red, green and blue planes of two RGB images
/// (`ColorBlitter.copyBits`). Each channel follows the unsigned 8-bit rules
/// on its own, except that Transparent-zero keeps the destination only
/// where all three source channels are zero.
pub fn calculate_rgb(dst: [&Buffer; 3], src: [&Buffer; 3], op: CalcOp) -> Result<[Buffer; 3]> {
    if op != CalcOp::TransparentZero {
        return Ok([
            calculate(dst[0], src[0], op, f64::INFINITY)?,
            calculate(dst[1], src[1], op, f64::INFINITY)?,
            calculate(dst[2], src[2], op, f64::INFINITY)?,
        ]);
    }
    let s = [
        src[0].try_slice::<u8>()?,
        src[1].try_slice::<u8>()?,
        src[2].try_slice::<u8>()?,
    ];
    let len = s[0].len();
    if s[1].len() != len || s[2].len() != len {
        return Err(Error::Shape(
            "the channels of an RGB image differ in size".to_string(),
        ));
    }
    let channel = |c: usize| -> Result<Buffer> {
        let d = dst[c].try_slice::<u8>()?;
        if d.len() != len {
            return Err(Error::Shape(format!(
                "cannot combine planes of {} and {len} samples",
                d.len()
            )));
        }
        Ok(Buffer::from_vec(
            (0..len)
                .map(|i| {
                    if s[0][i] == 0 && s[1][i] == 0 && s[2][i] == 0 {
                        d[i]
                    } else {
                        s[c][i]
                    }
                })
                .collect::<Vec<_>>(),
        ))
    };
    Ok([channel(0)?, channel(1)?, channel(2)?])
}

/// `ByteBlitter.copyBits`.
fn byte_op(d: u8, s: u8, op: CalcOp) -> u8 {
    let (di, si) = (d as i32, s as i32);
    let v = match op {
        CalcOp::Copy => si,
        CalcOp::TransparentZero if s == 0 => di,
        CalcOp::TransparentZero => si,
        CalcOp::Add => (si + di).min(255),
        CalcOp::Average => (si + di) / 2,
        CalcOp::Subtract => (di - si).max(0),
        CalcOp::Difference => (di - si).abs(),
        CalcOp::Multiply => (si * di).min(255),
        CalcOp::Divide if s == 0 => 255,
        CalcOp::Divide => di / si,
        CalcOp::And => si & di,
        CalcOp::Or => si | di,
        CalcOp::Xor => si ^ di,
        CalcOp::Min => si.min(di),
        CalcOp::Max => si.max(di),
    };
    v as u8
}

/// `ShortBlitter.copyBits`.
fn short_op(d: u16, s: u16, op: CalcOp) -> u16 {
    let (di, si) = (d as i32, s as i32);
    let v = match op {
        CalcOp::Copy => si,
        CalcOp::TransparentZero if s == 0 => di,
        CalcOp::TransparentZero => si,
        CalcOp::Add => (si + di).min(65535),
        CalcOp::Average => (si + di) / 2,
        CalcOp::Subtract => (di - si).max(0),
        CalcOp::Difference => (di - si).abs(),
        // Java's int product wraps; a wrapped product is negative here.
        CalcOp::Multiply => si.wrapping_mul(di).clamp(0, 65535),
        CalcOp::Divide if s == 0 => 65535,
        CalcOp::Divide => di / si,
        CalcOp::And => si & di,
        CalcOp::Or => si | di,
        CalcOp::Xor => si ^ di,
        CalcOp::Min => si.min(di),
        CalcOp::Max => si.max(di),
    };
    v as u16
}

/// An integer sample type ImageJ lacks.
trait WideInt: Pixel {
    const LO: i128;
    const HI: i128;
    fn wide(self) -> i128;
    fn narrow(v: i128) -> Self;
}

macro_rules! wide_int {
    ($($ty:ty),*) => {$(
        impl WideInt for $ty {
            const LO: i128 = <$ty>::MIN as i128;
            const HI: i128 = <$ty>::MAX as i128;
            fn wide(self) -> i128 {
                self as i128
            }
            fn narrow(v: i128) -> Self {
                v.clamp(Self::LO, Self::HI) as $ty
            }
        }
    )*};
}

wide_int!(i8, i16, u32, i32, u64, i64);

/// The 16-bit rules for the integer types ImageJ lacks, saturating at the
/// type's range.
fn int_op<T: WideInt>(d: T, s: T, op: CalcOp) -> T {
    let (di, si) = (d.wide(), s.wide());
    T::narrow(match op {
        CalcOp::Copy => si,
        CalcOp::TransparentZero if si == 0 => di,
        CalcOp::TransparentZero => si,
        CalcOp::Add => si + di,
        CalcOp::Average => (si + di) / 2,
        CalcOp::Subtract => di - si,
        CalcOp::Difference => (di - si).abs(),
        // The product of two 64-bit values fits in 128 bits.
        CalcOp::Multiply => si * di,
        CalcOp::Divide if si == 0 => T::HI,
        CalcOp::Divide => di / si,
        CalcOp::And => si & di,
        CalcOp::Or => si | di,
        CalcOp::Xor => si ^ di,
        CalcOp::Min => si.min(di),
        CalcOp::Max => si.max(di),
    })
}

/// `FloatBlitter.copyBits`.
fn f32_op(d: f32, s: f32, op: CalcOp, divide_by_zero: f32) -> f32 {
    match op {
        CalcOp::Copy => s,
        CalcOp::TransparentZero if s == 0.0 => d,
        CalcOp::TransparentZero => s,
        CalcOp::Add => s + d,
        CalcOp::Average => (s + d) / 2.0,
        CalcOp::Difference => {
            let v = d - s;
            if v < 0.0 { -v } else { v }
        }
        CalcOp::Subtract => d - s,
        CalcOp::Multiply => s * d,
        CalcOp::Divide if !divide_by_zero.is_infinite() && s == 0.0 => divide_by_zero,
        CalcOp::Divide => d / s,
        CalcOp::And => (java::int(s as f64) & java::int(d as f64)) as f32,
        CalcOp::Or => (java::int(s as f64) | java::int(d as f64)) as f32,
        CalcOp::Xor => (java::int(s as f64) ^ java::int(d as f64)) as f32,
        CalcOp::Min if s < d => s,
        CalcOp::Max if s > d => s,
        CalcOp::Min | CalcOp::Max => d,
    }
}

/// The 32-bit float rules in double precision.
fn f64_op(d: f64, s: f64, op: CalcOp, divide_by_zero: f64) -> f64 {
    match op {
        CalcOp::Copy => s,
        CalcOp::TransparentZero if s == 0.0 => d,
        CalcOp::TransparentZero => s,
        CalcOp::Add => s + d,
        CalcOp::Average => (s + d) / 2.0,
        CalcOp::Difference => {
            let v = d - s;
            if v < 0.0 { -v } else { v }
        }
        CalcOp::Subtract => d - s,
        CalcOp::Multiply => s * d,
        CalcOp::Divide if !divide_by_zero.is_infinite() && s == 0.0 => divide_by_zero,
        CalcOp::Divide => d / s,
        CalcOp::And => (java::int(s) & java::int(d)) as f64,
        CalcOp::Or => (java::int(s) | java::int(d)) as f64,
        CalcOp::Xor => (java::int(s) ^ java::int(d)) as f64,
        CalcOp::Min if s < d => s,
        CalcOp::Max if s > d => s,
        CalcOp::Min | CalcOp::Max => d,
    }
}
