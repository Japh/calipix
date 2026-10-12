//! Conversions that must not lose anything, and orderings they must keep.

use calipix_core::{Buffer, DType};
use calipix_ops::convert::{
    UNWEIGHTED, WEIGHTED, convert, float_to_u16_scaled, rgb_to_gray_f32, rgb_to_gray_u8,
};
use proptest::prelude::*;

/// `plane` converted along `path` and back to its own type.
fn round_trip(plane: &Buffer, path: &[DType]) -> Buffer {
    let mut current = plane.clone();
    for &dtype in path.iter().chain([&plane.dtype()]) {
        current = convert(&current, dtype).unwrap();
    }
    current
}

proptest! {
    #[test]
    fn widening_and_back_is_lossless(
        bytes in proptest::collection::vec(any::<u8>(), 1..50),
        shorts in proptest::collection::vec(any::<u16>(), 1..50),
        signed in proptest::collection::vec(any::<i16>(), 1..50),
        wide in proptest::collection::vec(any::<i32>(), 1..50),
        floats in proptest::collection::vec(
            any::<f32>().prop_filter("NaN payloads need not survive", |v| !v.is_nan()),
            1..50,
        ),
    ) {
        let bytes = Buffer::from_vec(bytes);
        for path in [
            &[DType::U16][..],
            &[DType::I16],
            &[DType::F32],
            &[DType::U32],
            &[DType::I64],
            &[DType::F64],
            &[DType::U16, DType::F32, DType::U64, DType::F64],
        ] {
            prop_assert_eq!(&round_trip(&bytes, path), &bytes, "{:?}", path);
        }
        let shorts = Buffer::from_vec(shorts);
        for path in [&[DType::F32][..], &[DType::I32], &[DType::U32], &[DType::F64]] {
            prop_assert_eq!(&round_trip(&shorts, path), &shorts, "{:?}", path);
        }
        let signed = Buffer::from_vec(signed);
        for path in [&[DType::F32][..], &[DType::I32], &[DType::I64], &[DType::F64]] {
            prop_assert_eq!(&round_trip(&signed, path), &signed, "{:?}", path);
        }
        // 32-bit integers fit a double but not a 32-bit float.
        let wide = Buffer::from_vec(wide);
        for path in [&[DType::I64][..], &[DType::F64]] {
            prop_assert_eq!(&round_trip(&wide, path), &wide, "{:?}", path);
        }
        // Single precision survives double precision bit for bit.
        let floats = Buffer::from_vec(floats);
        let back = round_trip(&floats, &[DType::F64]);
        let bits = |b: &Buffer| -> Vec<u32> {
            b.as_slice::<f32>().unwrap().iter().map(|v| v.to_bits()).collect()
        };
        prop_assert_eq!(bits(&back), bits(&floats));
    }

    #[test]
    fn narrowing_limits_to_the_range_and_keeps_order(
        values in proptest::collection::vec(-100_000i32..100_000, 2..50),
    ) {
        let plane = Buffer::from_vec(values.clone());
        for (dtype, lo, hi) in [
            (DType::U8, 0, 255),
            (DType::I8, -128, 127),
            (DType::U16, 0, 65535),
            (DType::I16, -32768, 32767),
        ] {
            let narrow = convert(&plane, dtype).unwrap();
            for (i, &v) in values.iter().enumerate() {
                prop_assert_eq!(narrow.get_f64(i), Some(v.clamp(lo, hi) as f64));
            }
            // The same from the float form of the values.
            let float = convert(&plane, DType::F64).unwrap();
            prop_assert_eq!(&convert(&float, dtype).unwrap(), &narrow);
        }
    }

    #[test]
    fn scaling_to_sixteen_bit_is_monotonic_and_spans_the_range(
        values in proptest::collection::vec(-1000.0f32..1000.0, 2..50),
        min in -500.0f64..0.0,
        span in 1.0f64..500.0,
    ) {
        let max = min + span;
        let plane = Buffer::from_vec(values.clone());
        let scaled = float_to_u16_scaled(&plane, (min, max)).unwrap();
        let out = scaled.as_slice::<u16>().unwrap();
        for (i, &a) in values.iter().enumerate() {
            if a as f64 <= min as f32 as f64 {
                prop_assert_eq!(out[i], 0);
            }
            if a as f64 >= max as f32 as f64 {
                prop_assert_eq!(out[i], 65535);
            }
            for (j, &b) in values.iter().enumerate() {
                if a <= b {
                    prop_assert!(out[i] <= out[j]);
                }
            }
        }
    }

    #[test]
    fn grey_stays_grey(values in proptest::collection::vec(any::<u8>(), 1..50)) {
        // A picture whose three channels agree is its own grey version.
        let plane = Buffer::from_vec(values.clone());
        for weights in [UNWEIGHTED, WEIGHTED] {
            let grey = rgb_to_gray_u8([&plane, &plane, &plane], weights).unwrap();
            prop_assert_eq!(&grey, &plane);
            let float = rgb_to_gray_f32([&plane, &plane, &plane], weights).unwrap();
            for (i, &v) in values.iter().enumerate() {
                let f = float.get_f64(i).unwrap();
                prop_assert!((f - v as f64).abs() < 1e-4, "{} became {}", v, f);
            }
        }
    }
}

#[test]
fn rgb_weights() {
    let r = Buffer::from_vec(vec![255u8, 0, 0, 10]);
    let g = Buffer::from_vec(vec![0u8, 255, 0, 20]);
    let b = Buffer::from_vec(vec![0u8, 0, 255, 33]);
    let grey = rgb_to_gray_u8([&r, &g, &b], UNWEIGHTED).unwrap();
    assert_eq!(grey.as_slice::<u8>().unwrap(), &[85, 85, 85, 21]);
    let grey = rgb_to_gray_u8([&r, &g, &b], WEIGHTED).unwrap();
    assert_eq!(grey.as_slice::<u8>().unwrap(), &[76, 150, 29, 18]);
    let float = rgb_to_gray_f32([&r, &g, &b], UNWEIGHTED).unwrap();
    assert_eq!(float.as_slice::<f32>().unwrap()[3], 21.0);
    // Channels must be 8-bit and of one size.
    let short = Buffer::from_vec(vec![1u8]);
    assert!(rgb_to_gray_u8([&r, &g, &short], UNWEIGHTED).is_err());
    let wide = Buffer::from_vec(vec![1u16, 2, 3, 4]);
    assert!(rgb_to_gray_f32([&r, &wide, &b], UNWEIGHTED).is_err());
}
