use calipix_core::{Buffer, Error, Pixel, Result};

use super::java;

/// One of the operations of ImageJ's Process > Math submenu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MathOp {
    Add(f64),
    Subtract(f64),
    Multiply(f64),
    /// Integer samples are left unchanged when the divisor is zero.
    Divide(f64),
    And(i32),
    Or(i32),
    Xor(i32),
    /// Samples below the value are raised to it.
    Min(f64),
    /// Samples above the value are lowered to it.
    Max(f64),
    Gamma(f64),
    Set(f64),
    Log,
    Exp,
    Square,
    SquareRoot,
    /// Float samples only: `1 / v`, and NaN where `v` is zero.
    Reciprocal,
    Abs,
    /// Float samples only: samples outside `lower..=upper` become NaN.
    NanBackground {
        lower: f64,
        upper: f64,
    },
}

/// Applies `op` to every sample of a plane, as `ImageMath.run` does through
/// the `process` method of the plane's `ImageProcessor`.
///
/// * **Unsigned 8-bit** (`ImageProcessor.process`, also used for each
///   channel of an RGB image): the result for each of the 256 possible
///   samples is computed once and clamped to 0..=255. Constants and
///   results are truncated to integers, except that Multiply rounds the
///   product. Gamma is `exp(ln(v / 255) * g) * 255`, Log is
///   `ln(v) * 255 / ln(255)` and Exp is the inverse of Log.
/// * **Other integers** (`ShortProcessor.process`, which ImageJ has for
///   unsigned 16-bit only): results are clamped to the type's range.
///   Gamma, Log and Exp depend on `range`, the image's display range, or
///   the plane's smallest and largest samples when `None`:
///   Gamma is `exp(g * ln((v - min) / (max - min))) * (max - min) + min`,
///   Log is `ln(v) * max / ln(max)` and Exp is `exp(v * ln(max) / max)`.
///   For 8 and 16-bit types the arithmetic is Java's 32-bit `int`
///   arithmetic, including its wrap-around; wider types do not wrap.
/// * **Float** (`FloatProcessor.process`): arithmetic in the sample's own
///   precision. Gamma and Square Root give 0 for samples that are not
///   positive; Log gives NaN for negative samples and negative infinity
///   for zero; AND, OR and XOR do nothing. Divide multiplies by the
///   reciprocal of the divisor, so dividing by zero gives infinities, and
///   NaN for samples that are zero.
///
/// The integer rules are those of ImageJ 1.54p, the version the reference
/// results come from. Later versions (1.54u at least) round the results of
/// Gamma, Log, Exp and Square Root instead of truncating them.
///
/// Abs does nothing to unsigned samples. Reciprocal and NaN Background
/// need float samples, and complex planes are not supported.
pub fn apply(buf: &Buffer, op: MathOp, range: Option<(f64, f64)>) -> Result<Buffer> {
    Ok(match buf {
        Buffer::U8(s) => {
            float_only(op, "8-bit")?;
            let table: [u8; 256] = std::array::from_fn(|i| byte_op(i as i32, op));
            Buffer::from_vec(s.iter().map(|&v| table[v as usize]).collect::<Vec<_>>())
        }
        Buffer::I8(s) => int_plane(s, op, range)?,
        Buffer::U16(s) => int_plane(s, op, range)?,
        Buffer::I16(s) => int_plane(s, op, range)?,
        Buffer::U32(s) => int_plane(s, op, range)?,
        Buffer::I32(s) => int_plane(s, op, range)?,
        Buffer::U64(s) => int_plane(s, op, range)?,
        Buffer::I64(s) => int_plane(s, op, range)?,
        Buffer::F32(s) => Buffer::from_vec(s.iter().map(|&v| f32_op(v, op)).collect::<Vec<_>>()),
        Buffer::F64(s) => Buffer::from_vec(s.iter().map(|&v| f64_op(v, op)).collect::<Vec<_>>()),
        Buffer::C32(_) | Buffer::C64(_) => {
            return Err(Error::Unsupported(
                "arithmetic on complex images is not supported".to_string(),
            ));
        }
    })
}

/// What Set with `value` stores in the red, green and blue channels of an
/// RGB image. ImageJ's `ColorProcessor.set` takes the truncated value as a
/// packed `0xRRGGBB` colour, so that 77 sets blue to 77 and red and green
/// to 0, and 0x123456 sets the three channels to 0x12, 0x34 and 0x56.
pub fn rgb_set_components(value: f64) -> [u8; 3] {
    let packed = java::int(value);
    [(packed >> 16) as u8, (packed >> 8) as u8, packed as u8]
}

/// Refuses the operations that only make sense for float samples.
fn float_only(op: MathOp, what: &str) -> Result<()> {
    let name = match op {
        MathOp::Reciprocal => "Reciprocal",
        MathOp::NanBackground { .. } => "NaN Background",
        _ => return Ok(()),
    };
    Err(Error::Unsupported(format!(
        "{name} needs a float image, and this one is {what}; convert it to 32-bit first"
    )))
}

/// `ImageProcessor.process` for one of the 256 possible samples.
fn byte_op(i: i32, op: MathOp) -> u8 {
    let scale = 255.0 / 255f64.ln();
    let v = match op {
        MathOp::Set(value) => java::int(value),
        MathOp::Add(value) => i.wrapping_add(java::int(value)),
        MathOp::Subtract(value) => i.wrapping_add(java::int(-value)),
        MathOp::Multiply(value) => java::round_int(i as f64 * value),
        // ImageMath.run: a zero divisor leaves integer images unchanged.
        MathOp::Divide(0.0) => i,
        MathOp::Divide(value) => java::round_int(i as f64 * (1.0 / value)),
        MathOp::And(value) => i & value,
        MathOp::Or(value) => i | value,
        MathOp::Xor(value) => i ^ value,
        MathOp::Gamma(value) => java::int(((i as f64 / 255.0).ln() * value).exp() * 255.0),
        MathOp::Log if i == 0 => 0,
        MathOp::Log => java::int((i as f64).ln() * scale),
        MathOp::Exp => java::int((i as f64 / scale).exp()),
        MathOp::Square => i * i,
        MathOp::SquareRoot => java::int((i as f64).sqrt()),
        MathOp::Min(value) if (i as f64) < value => java::int(value),
        MathOp::Max(value) if (i as f64) > value => java::int(value),
        MathOp::Min(_) | MathOp::Max(_) | MathOp::Abs => i,
        MathOp::Reciprocal | MathOp::NanBackground { .. } => i,
    };
    v.clamp(0, 255) as u8
}

/// An integer sample type other than unsigned 8-bit.
trait IntSample: Pixel + Ord {
    /// True if Java's 32-bit `int` holds every value of the type, so that
    /// ImageJ's arithmetic, wrap-around included, is reproduced.
    const JAVA_INT: bool;
    const NAME: &'static str;
    fn wide(self) -> i128;
    /// Clamps to the type's range.
    fn clamped(v: i128) -> Self;
}

macro_rules! int_sample {
    ($($ty:ty, $java:expr, $name:expr;)*) => {$(
        impl IntSample for $ty {
            const JAVA_INT: bool = $java;
            const NAME: &'static str = $name;
            fn wide(self) -> i128 {
                self as i128
            }
            fn clamped(v: i128) -> Self {
                v.clamp(<$ty>::MIN as i128, <$ty>::MAX as i128) as $ty
            }
        }
    )*};
}

int_sample! {
    i8, true, "signed 8-bit"; u16, true, "16-bit"; i16, true, "signed 16-bit";
    u32, false, "unsigned 32-bit"; i32, false, "signed 32-bit";
    u64, false, "unsigned 64-bit"; i64, false, "signed 64-bit";
}

/// `ShortProcessor.process`, generalised to every integer type but
/// unsigned 8-bit.
fn int_plane<T: IntSample>(samples: &[T], op: MathOp, range: Option<(f64, f64)>) -> Result<Buffer> {
    float_only(op, T::NAME)?;
    // (int)x, (int)Math.round(x) and int addition, without the 32-bit
    // limit for types that would not fit in it.
    let int = |x: f64| -> i128 {
        if T::JAVA_INT {
            java::int(x) as i128
        } else {
            x as i128
        }
    };
    let round = |x: f64| -> i128 {
        if T::JAVA_INT {
            java::round_int(x) as i128
        } else {
            (x + 0.5).floor() as i128
        }
    };
    let add = |a: i128, b: i128| -> i128 {
        if T::JAVA_INT {
            (a as i32).wrapping_add(b as i32) as i128
        } else {
            a + b
        }
    };

    let needs_range = matches!(op, MathOp::Gamma(_) | MathOp::Log | MathOp::Exp);
    let (min, max) = match range {
        Some(range) => range,
        None if needs_range => match (samples.iter().min(), samples.iter().max()) {
            (Some(lo), Some(hi)) => (lo.wide() as f64, hi.wide() as f64),
            _ => (0.0, 0.0),
        },
        None => (0.0, 0.0),
    };
    let span = max - min;
    let (min2, max2) = (int(min), int(max));
    let max2f = max2 as f64;

    let each = |v1: i128| -> i128 {
        let d1 = v1 as f64;
        match op {
            MathOp::Set(value) => int(value),
            MathOp::Add(value) => add(v1, int(value)),
            MathOp::Subtract(value) => add(v1, int(-value)),
            MathOp::Multiply(value) => round(d1 * value),
            MathOp::Divide(0.0) => v1,
            MathOp::Divide(value) => round(d1 * (1.0 / value)),
            MathOp::And(value) => v1 & value as i128,
            MathOp::Or(value) => v1 | value as i128,
            MathOp::Xor(value) => v1 ^ value as i128,
            MathOp::Gamma(_) if span <= 0.0 || v1 == min2 => v1,
            MathOp::Gamma(value) => {
                int((value * ((v1 - min2) as f64 / span).ln()).exp() * span + min2 as f64)
            }
            MathOp::Log if v1 <= 0 => 0,
            MathOp::Log => int(d1.ln() * (max2f / max2f.ln())),
            MathOp::Exp => int((d1 * (max2f.ln() / max2f)).exp()),
            MathOp::Square => int(d1 * d1),
            MathOp::SquareRoot => int(d1.sqrt()),
            MathOp::Abs => v1.abs(),
            MathOp::Min(value) if d1 < value => int(value),
            MathOp::Max(value) if d1 > value => int(value),
            MathOp::Min(_) | MathOp::Max(_) => v1,
            MathOp::Reciprocal | MathOp::NanBackground { .. } => v1,
        }
    };
    Ok(Buffer::from_vec(
        samples
            .iter()
            .map(|&v| T::clamped(each(v.wide())))
            .collect::<Vec<_>>(),
    ))
}

/// `FloatProcessor.process` and the float-only cases of `ImageMath.run`.
fn f32_op(v: f32, op: MathOp) -> f32 {
    match op {
        MathOp::Set(value) => value as f32,
        MathOp::Add(value) => v + value as f32,
        MathOp::Subtract(value) => v + (-value) as f32,
        MathOp::Multiply(value) => v * value as f32,
        MathOp::Divide(value) => v * (1.0 / value) as f32,
        MathOp::And(_) | MathOp::Or(_) | MathOp::Xor(_) => v,
        MathOp::Gamma(_) if v <= 0.0 => 0.0,
        MathOp::Gamma(value) => ((value as f32) as f64 * (v as f64).ln()).exp() as f32,
        MathOp::Log => (v as f64).ln() as f32,
        MathOp::Exp => (v as f64).exp() as f32,
        MathOp::Square => v * v,
        MathOp::SquareRoot if v <= 0.0 => 0.0,
        MathOp::SquareRoot => (v as f64).sqrt() as f32,
        MathOp::Abs => v.abs(),
        MathOp::Min(value) if (v as f64) < value => value as f32,
        MathOp::Max(value) if (v as f64) > value => value as f32,
        MathOp::Min(_) | MathOp::Max(_) => v,
        MathOp::Reciprocal if v == 0.0 => f32::NAN,
        MathOp::Reciprocal => 1.0 / v,
        MathOp::NanBackground { lower, upper } => {
            if (v as f64) < lower || (v as f64) > upper {
                f32::NAN
            } else {
                v
            }
        }
    }
}

/// The 32-bit float rules in double precision, for the 64-bit float type
/// ImageJ lacks.
fn f64_op(v: f64, op: MathOp) -> f64 {
    match op {
        MathOp::Set(value) => value,
        MathOp::Add(value) => v + value,
        MathOp::Subtract(value) => v - value,
        MathOp::Multiply(value) => v * value,
        MathOp::Divide(value) => v * (1.0 / value),
        MathOp::And(_) | MathOp::Or(_) | MathOp::Xor(_) => v,
        MathOp::Gamma(_) if v <= 0.0 => 0.0,
        MathOp::Gamma(value) => (value * v.ln()).exp(),
        MathOp::Log => v.ln(),
        MathOp::Exp => v.exp(),
        MathOp::Square => v * v,
        MathOp::SquareRoot if v <= 0.0 => 0.0,
        MathOp::SquareRoot => v.sqrt(),
        MathOp::Abs => v.abs(),
        MathOp::Min(value) if v < value => value,
        MathOp::Max(value) if v > value => value,
        MathOp::Min(_) | MathOp::Max(_) => v,
        MathOp::Reciprocal if v == 0.0 => f64::NAN,
        MathOp::Reciprocal => 1.0 / v,
        MathOp::NanBackground { lower, upper } => {
            if v < lower || v > upper {
                f64::NAN
            } else {
                v
            }
        }
    }
}
