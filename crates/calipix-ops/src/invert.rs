use calipix_core::{Buffer, Error, Pixel, Result};

/// Inverts one plane, as ImageJ's Edit > Invert does with default settings.
///
/// * 8-bit: `255 - v`.
/// * Every other integer type: `max - (v - min)`, where `min` and `max` are
///   the smallest and largest samples in the plane (ImageJ's 16-bit rule
///   with "Full range 16-bit inversions" off).
/// * Float: `max - (v - min)`, where the range is `float_range` if given
///   (ImageJ uses the image's display range) and otherwise the plane's
///   smallest and largest non-NaN samples.
///
/// Complex planes are not supported.
pub fn invert_plane(buf: &Buffer, float_range: Option<(f64, f64)>) -> Result<Buffer> {
    Ok(match buf {
        Buffer::U8(s) => Buffer::from_vec(s.iter().map(|&v| 255 - v).collect::<Vec<_>>()),
        Buffer::I8(s) => invert_int(s),
        Buffer::U16(s) => invert_int(s),
        Buffer::I16(s) => invert_int(s),
        Buffer::U32(s) => invert_int(s),
        Buffer::I32(s) => invert_int(s),
        Buffer::U64(s) => invert_int(s),
        Buffer::I64(s) => invert_int(s),
        Buffer::F32(s) => {
            let (min, max) = float_range
                .map(|(lo, hi)| (lo as f32, hi as f32))
                .unwrap_or_else(|| finite_range(s.iter().map(|&v| v as f64), |v| v as f32));
            Buffer::from_vec(s.iter().map(|&v| max - (v - min)).collect::<Vec<_>>())
        }
        Buffer::F64(s) => {
            let (min, max) = float_range.unwrap_or_else(|| finite_range(s.iter().copied(), |v| v));
            Buffer::from_vec(s.iter().map(|&v| max - (v - min)).collect::<Vec<_>>())
        }
        Buffer::C32(_) | Buffer::C64(_) => {
            return Err(Error::Unsupported(
                "cannot invert a complex image".to_string(),
            ));
        }
    })
}

fn invert_int<T>(samples: &[T]) -> Buffer
where
    T: Pixel + Ord + Into<i128> + TryFrom<i128>,
{
    let (Some(&min), Some(&max)) = (samples.iter().min(), samples.iter().max()) else {
        return Buffer::from_vec(Vec::<T>::new());
    };
    let (min, max): (i128, i128) = (min.into(), max.into());
    let inverted = samples
        .iter()
        .map(|&v| {
            // Always within [min, max], so the conversion back cannot fail.
            T::try_from(max - (v.into() - min)).unwrap_or(v)
        })
        .collect::<Vec<_>>();
    Buffer::from_vec(inverted)
}

/// Smallest and largest non-NaN values, or NaN for both if there are none.
fn finite_range<T: Copy>(values: impl Iterator<Item = f64>, narrow: impl Fn(f64) -> T) -> (T, T) {
    let mut range: Option<(f64, f64)> = None;
    for v in values.filter(|v| !v.is_nan()) {
        range = Some(match range {
            None => (v, v),
            Some((lo, hi)) => (lo.min(v), hi.max(v)),
        });
    }
    let (lo, hi) = range.unwrap_or((f64::NAN, f64::NAN));
    (narrow(lo), narrow(hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use calipix_core::{Complex32, DType};

    #[test]
    fn eight_bit_uses_full_range() {
        let out = invert_plane(&Buffer::from_vec(vec![0u8, 1, 100, 255]), None).unwrap();
        assert_eq!(out.as_slice::<u8>().unwrap(), &[255, 254, 155, 0]);
    }

    #[test]
    fn sixteen_bit_uses_data_range() {
        let out = invert_plane(&Buffer::from_vec(vec![100u16, 150, 400]), None).unwrap();
        assert_eq!(out.as_slice::<u16>().unwrap(), &[400, 350, 100]);
    }

    #[test]
    fn signed_extremes_do_not_overflow() {
        let out = invert_plane(&Buffer::from_vec(vec![i8::MIN, 0, i8::MAX]), None).unwrap();
        assert_eq!(out.as_slice::<i8>().unwrap(), &[i8::MAX, -1, i8::MIN]);
        let out = invert_plane(&Buffer::from_vec(vec![i64::MIN, i64::MAX]), None).unwrap();
        assert_eq!(out.as_slice::<i64>().unwrap(), &[i64::MAX, i64::MIN]);
        let out = invert_plane(&Buffer::from_vec(vec![0u64, u64::MAX]), None).unwrap();
        assert_eq!(out.as_slice::<u64>().unwrap(), &[u64::MAX, 0]);
    }

    #[test]
    fn float_uses_data_range_and_ignores_nan() {
        let out = invert_plane(&Buffer::from_vec(vec![-1.0f32, f32::NAN, 0.5, 3.0]), None).unwrap();
        let s = out.as_slice::<f32>().unwrap();
        assert_eq!((s[0], s[2], s[3]), (3.0, 1.5, -1.0));
        assert!(s[1].is_nan());
    }

    #[test]
    fn float_uses_display_range_when_given() {
        let out = invert_plane(&Buffer::from_vec(vec![0.25f32, 0.5]), Some((0.0, 1.0))).unwrap();
        assert_eq!(out.as_slice::<f32>().unwrap(), &[0.75, 0.5]);
        let out = invert_plane(&Buffer::from_vec(vec![0.25f64]), Some((0.0, 1.0))).unwrap();
        assert_eq!(out.as_slice::<f64>().unwrap(), &[0.75]);
    }

    #[test]
    fn inverting_twice_restores_integers() {
        let original = Buffer::from_vec(vec![7u16, 9, 1000, 65535, 0]);
        let twice = invert_plane(&invert_plane(&original, None).unwrap(), None).unwrap();
        assert_eq!(twice, original);
    }

    #[test]
    fn empty_and_complex() {
        assert!(
            invert_plane(&Buffer::zeros(DType::U16, 0), None)
                .unwrap()
                .is_empty()
        );
        assert!(invert_plane(&Buffer::from_vec(vec![Complex32::new(1.0, 0.0)]), None).is_err());
    }
}
