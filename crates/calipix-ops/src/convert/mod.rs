//! Converting planes between pixel types.
//!
//! The conversions here need nothing but the samples. Converting 16-bit or
//! float data to 8-bit *through the display range* is the display mapping
//! itself and lives with it, in `calipix-render`'s `LevelMap`.

use calipix_core::{Buffer, DType, Error, RealPixel, Result, dispatch_real};

use crate::math::java;

/// The weights ImageJ gives red, green and blue when it turns colour into
/// grey with "Weighted RGB conversions" off (the default).
pub const UNWEIGHTED: [f64; 3] = [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0];

/// The weights with "Weighted RGB conversions" on.
pub const WEIGHTED: [f64; 3] = [0.299, 0.587, 0.114];

fn complex_error() -> Error {
    Error::Unsupported("complex images cannot be converted to another pixel type".to_string())
}

/// Converts a plane to another pixel type without rescaling its values, as
/// ImageJ's `TypeConverter` does with "Scale when converting" off.
///
/// The conversions ImageJ has are reproduced exactly:
///
/// * 16-bit to 8-bit (`convertShortToByte`): values above 255 become 255.
/// * 32-bit float to 8-bit (`ByteProcessor.setPixels(int, FloatProcessor)`):
///   `v + 0.5` in single precision, clamped to 0..=255, then truncated.
/// * 32-bit float to 16-bit (`convertFloatToShort`): clamped to 0..=65535,
///   then `v + 0.5` truncated.
/// * 8-bit to 16-bit, and 8 or 16-bit to float: the same values.
///
/// NaN becomes 0 in every conversion to an integer type. Calipix extends
/// these rules to the pixel types ImageJ lacks:
///
/// * integer to integer: values outside the target's range take its
///   nearest limit (so negative values become 0 in an unsigned type);
/// * float to a signed or wider integer: rounded to the nearest integer,
///   halves away from zero, then limited to the target's range; 64-bit
///   float to 8 and 16-bit follows the 32-bit rules in double precision;
/// * anything to float: the nearest representable value, so 64-bit
///   integers beyond 2^53 (2^24 for 32-bit float) lose their low bits.
///
/// Complex planes cannot be converted.
pub fn convert(buf: &Buffer, target: DType) -> Result<Buffer> {
    if buf.dtype() == target {
        return Ok(buf.clone());
    }
    if target.is_complex() {
        return Err(Error::Unsupported(
            "converting to a complex pixel type is not supported".to_string(),
        ));
    }
    fn each<S: RealPixel, T: RealPixel>(src: &[S], f: impl Fn(S) -> T) -> Buffer {
        Buffer::from_vec(src.iter().map(|&v| f(v)).collect::<Vec<_>>())
    }
    /// Integer source: limit to the target's range. Float source: round
    /// halves away from zero and limit. Both are what `from_f64` does,
    /// except that 64-bit integers must not pass through a double.
    fn general<S: RealPixel, T: RealPixel>(src: &[S]) -> Buffer {
        each(src, |v| T::from_f64(v.to_f64()))
    }
    fn to<T: RealPixel>(buf: &Buffer) -> Result<Buffer> {
        dispatch_real!(buf, |s| Ok(general::<_, T>(s)), complex => Err(complex_error()))
    }
    fn int_to<T: RealPixel + TryFrom<i128>>(buf: &Buffer, min: i128, max: i128) -> Option<Buffer> {
        fn go<S: RealPixel + Into<i128>, T: RealPixel + TryFrom<i128>>(
            src: &[S],
            min: i128,
            max: i128,
        ) -> Buffer {
            each(src, |v| {
                T::try_from(v.into().clamp(min, max)).unwrap_or_default()
            })
        }
        Some(match buf {
            Buffer::U8(s) => go::<_, T>(s, min, max),
            Buffer::I8(s) => go::<_, T>(s, min, max),
            Buffer::U16(s) => go::<_, T>(s, min, max),
            Buffer::I16(s) => go::<_, T>(s, min, max),
            Buffer::U32(s) => go::<_, T>(s, min, max),
            Buffer::I32(s) => go::<_, T>(s, min, max),
            Buffer::U64(s) => go::<_, T>(s, min, max),
            Buffer::I64(s) => go::<_, T>(s, min, max),
            _ => return None,
        })
    }
    macro_rules! int_target {
        ($ty:ty) => {
            match int_to::<$ty>(buf, <$ty>::MIN as i128, <$ty>::MAX as i128) {
                Some(out) => Ok(out),
                None => to::<$ty>(buf),
            }
        };
    }

    match (buf, target) {
        (Buffer::C32(_) | Buffer::C64(_), _) => Err(complex_error()),
        // The half is added in the sample's own precision. NaN stays NaN
        // through the limiting and becomes 0 in the cast, as (byte)NaN.
        (Buffer::F32(s), DType::U8) => Ok(each(s, |v| (v + 0.5f32).clamp(0.0, 255.0) as u8)),
        (Buffer::F64(s), DType::U8) => Ok(each(s, |v| (v + 0.5).clamp(0.0, 255.0) as u8)),
        (Buffer::F32(s), DType::U16) => Ok(each(s, |v| float_to_short(v as f64))),
        (Buffer::F64(s), DType::U16) => Ok(each(s, float_to_short)),
        (_, DType::U8) => int_target!(u8),
        (_, DType::I8) => int_target!(i8),
        (_, DType::U16) => int_target!(u16),
        (_, DType::I16) => int_target!(i16),
        (_, DType::U32) => int_target!(u32),
        (_, DType::I32) => int_target!(i32),
        (_, DType::U64) => int_target!(u64),
        (_, DType::I64) => int_target!(i64),
        (_, DType::F32) => to::<f32>(buf),
        (_, DType::F64) => to::<f64>(buf),
        (_, DType::C32 | DType::C64) => unreachable!("complex targets were refused above"),
    }
}

/// The tail of `TypeConverter.convertFloatToShort`: clamp, add a half and
/// truncate. NaN, which fails both comparisons, becomes 0.
fn float_to_short(value: f64) -> u16 {
    java::int(value.clamp(0.0, 65535.0) + 0.5) as u16
}

/// Converts a float plane to 16-bit, stretching the display range
/// `(min, max)` over 0..=65535 (`TypeConverter.convertFloatToShort` with
/// "Scale when converting" on):
///
/// ```text
/// v2 = clamp((v - min) * 65535 / (max - min), 0, 65535) + 0.5, truncated
/// ```
///
/// When `max == min` the scale factor is 1. For 32-bit samples the range
/// is first rounded to single precision, which is how ImageJ holds it.
pub fn float_to_u16_scaled(buf: &Buffer, (min, max): (f64, f64)) -> Result<Buffer> {
    let scaled = |v: f64, min: f64, max: f64| {
        let scale = if max - min == 0.0 {
            1.0
        } else {
            65535.0 / (max - min)
        };
        float_to_short((v - min) * scale)
    };
    match buf {
        Buffer::F32(s) => {
            let (min, max) = (min as f32 as f64, max as f32 as f64);
            Ok(Buffer::from_vec(
                s.iter()
                    .map(|&v| scaled(v as f64, min, max))
                    .collect::<Vec<_>>(),
            ))
        }
        Buffer::F64(s) => Ok(Buffer::from_vec(
            s.iter().map(|&v| scaled(v, min, max)).collect::<Vec<_>>(),
        )),
        other => Err(Error::Unsupported(format!(
            "only float planes are scaled when converting to 16-bit, not {}",
            other.dtype()
        ))),
    }
}

fn rgb_slices(rgb: [&Buffer; 3]) -> Result<[&[u8]; 3]> {
    let [r, g, b] = [
        rgb[0].try_slice::<u8>()?,
        rgb[1].try_slice::<u8>()?,
        rgb[2].try_slice::<u8>()?,
    ];
    if r.len() != g.len() || r.len() != b.len() {
        return Err(Error::Shape(
            "the channels of an RGB image differ in size".to_string(),
        ));
    }
    Ok([r, g, b])
}

/// Turns the red, green and blue planes of an RGB image into one 8-bit
/// grey plane: `r * wr + g * wg + b * wb + 0.5`, truncated
/// (`TypeConverter.convertRGBToByte`). `weights` is [`UNWEIGHTED`] or
/// [`WEIGHTED`], or any other triple.
pub fn rgb_to_gray_u8(rgb: [&Buffer; 3], weights: [f64; 3]) -> Result<Buffer> {
    let [r, g, b] = rgb_slices(rgb)?;
    let [rw, gw, bw] = weights;
    Ok(Buffer::from_vec(
        (0..r.len())
            .map(|i| java::int(r[i] as f64 * rw + g[i] as f64 * gw + b[i] as f64 * bw + 0.5) as u8)
            .collect::<Vec<_>>(),
    ))
}

/// Turns the red, green and blue planes of an RGB image into one 32-bit
/// float grey plane: `r * wr + g * wg + b * wb`, not rounded
/// (`TypeConverter.convertRGBToFloat`).
pub fn rgb_to_gray_f32(rgb: [&Buffer; 3], weights: [f64; 3]) -> Result<Buffer> {
    let [r, g, b] = rgb_slices(rgb)?;
    let [rw, gw, bw] = weights;
    Ok(Buffer::from_vec(
        (0..r.len())
            .map(|i| (r[i] as f64 * rw + g[i] as f64 * gw + b[i] as f64 * bw) as f32)
            .collect::<Vec<_>>(),
    ))
}
