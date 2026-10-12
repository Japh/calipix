//! Algebraic laws of the sample arithmetic, and its rules for the pixel
//! types ImageJ lacks.

use calipix_core::{Buffer, Complex32, DType};
use calipix_ops::math::{CalcOp, MathOp, apply, calculate, rgb_set_components};
use proptest::prelude::*;

fn f32s(buf: &Buffer) -> &[f32] {
    buf.as_slice::<f32>().unwrap()
}

proptest! {
    #[test]
    fn adding_then_subtracting_restores_floats(
        // Whole numbers below 2^23 with a whole constant: every sum is exact.
        values in proptest::collection::vec(-4_000_000i32..4_000_000, 1..40),
        constant in -4_000_000i32..4_000_000,
    ) {
        let plane = Buffer::from_vec(values.iter().map(|&v| v as f32).collect::<Vec<_>>());
        let added = apply(&plane, MathOp::Add(constant as f64), None).unwrap();
        let back = apply(&added, MathOp::Subtract(constant as f64), None).unwrap();
        prop_assert_eq!(&back, &plane);
        // The same between two images.
        let other = Buffer::from_vec(vec![constant as f32; values.len()]);
        let sum = calculate(&plane, &other, CalcOp::Add, f64::INFINITY).unwrap();
        let difference = calculate(&sum, &other, CalcOp::Subtract, f64::INFINITY).unwrap();
        prop_assert_eq!(&difference, &plane);
    }

    #[test]
    fn integer_addition_saturates_instead_of_wrapping(
        values in proptest::collection::vec(any::<u16>(), 1..40),
        constant in -70_000i32..70_000,
    ) {
        let plane = Buffer::from_vec(values.clone());
        let out = apply(&plane, MathOp::Add(constant as f64), None).unwrap();
        let expected: Vec<u16> = values
            .iter()
            .map(|&v| (v as i32 + constant).clamp(0, 65535) as u16)
            .collect();
        prop_assert_eq!(out.as_slice::<u16>().unwrap(), &expected[..]);
        // Subtract is Add of the negative.
        let subtracted = apply(&plane, MathOp::Subtract(-constant as f64), None).unwrap();
        prop_assert_eq!(subtracted, out);
    }

    #[test]
    fn min_and_max_bound_every_sample(
        values in proptest::collection::vec(-1e6f32..1e6, 1..40),
        limit in -1e6f64..1e6,
    ) {
        let plane = Buffer::from_vec(values);
        let raised = apply(&plane, MathOp::Min(limit), None).unwrap();
        prop_assert!(f32s(&raised).iter().all(|&v| v >= limit as f32));
        let lowered = apply(&plane, MathOp::Max(limit), None).unwrap();
        prop_assert!(f32s(&lowered).iter().all(|&v| v <= limit as f32));
        // Both are idempotent.
        prop_assert_eq!(&apply(&raised, MathOp::Min(limit), None).unwrap(), &raised);
        prop_assert_eq!(&apply(&lowered, MathOp::Max(limit), None).unwrap(), &lowered);
    }

    #[test]
    fn calculator_identities(
        a in proptest::collection::vec(any::<u8>(), 1..40),
        seed in any::<u64>(),
    ) {
        let b: Vec<u8> = a
            .iter()
            .enumerate()
            .map(|(i, &v)| v.wrapping_mul(31).wrapping_add((seed >> (i % 56)) as u8))
            .collect();
        let (pa, pb) = (Buffer::from_vec(a.clone()), Buffer::from_vec(b.clone()));
        let run = |x: &Buffer, y: &Buffer, op| calculate(x, y, op, f64::INFINITY).unwrap();
        // Symmetric operations do not care about the order of the images.
        for op in [CalcOp::Add, CalcOp::Multiply, CalcOp::And, CalcOp::Or, CalcOp::Xor,
                   CalcOp::Min, CalcOp::Max, CalcOp::Average, CalcOp::Difference] {
            prop_assert_eq!(run(&pa, &pb, op), run(&pb, &pa, op), "{:?}", op);
        }
        prop_assert_eq!(&run(&pa, &pb, CalcOp::Copy), &pb);
        prop_assert_eq!(&run(&pa, &pa, CalcOp::Min), &pa);
        prop_assert_eq!(&run(&pa, &pa, CalcOp::Average), &pa);
        prop_assert!(run(&pa, &pa, CalcOp::Difference).as_slice::<u8>().unwrap().iter().all(|&v| v == 0));
        prop_assert_eq!(&run(&run(&pa, &pb, CalcOp::Xor), &pb, CalcOp::Xor), &pa);
        // Transparent-zero keeps image 1 exactly where image 2 is zero.
        let through = run(&pa, &pb, CalcOp::TransparentZero);
        for (i, &v) in through.as_slice::<u8>().unwrap().iter().enumerate() {
            prop_assert_eq!(v, if b[i] == 0 { a[i] } else { b[i] });
        }
    }
}

#[test]
fn eight_bit_rules() {
    let all = Buffer::from_vec((0..=255u8).collect::<Vec<_>>());
    let out = |op| apply(&all, op, None).unwrap().as_slice::<u8>().unwrap().to_vec();
    // Constants are truncated, products rounded.
    assert_eq!(out(MathOp::Add(2.7))[..3], [2, 3, 4]);
    assert_eq!(out(MathOp::Add(-2.7))[..4], [0, 0, 0, 1]);
    assert_eq!(out(MathOp::Multiply(0.5))[..4], [0, 1, 1, 2]);
    assert_eq!(out(MathOp::Multiply(1.5))[253..], [255, 255, 255]);
    // A zero divisor leaves the image alone.
    assert_eq!(out(MathOp::Divide(0.0)), out(MathOp::Add(0.0)));
    assert_eq!(out(MathOp::Divide(2.0))[..4], [0, 1, 1, 2]);
    // Log and Exp map 0..=255 onto itself and truncate.
    assert_eq!(out(MathOp::Log)[..3], [0, 0, 31]);
    assert_eq!(out(MathOp::Log)[100], 211);
    assert_eq!((out(MathOp::Exp)[0], out(MathOp::Exp)[100], out(MathOp::Exp)[200]), (1, 8, 77));
    assert_eq!((out(MathOp::Square)[15], out(MathOp::Square)[16]), (225, 255));
    assert_eq!((out(MathOp::SquareRoot)[15], out(MathOp::SquareRoot)[16]), (3, 4));
    assert_eq!(out(MathOp::And(0b1111_0000))[0x3c], 0x30);
    assert_eq!(out(MathOp::Or(0b0101))[0x30], 0x35);
    assert_eq!(out(MathOp::Xor(0xff))[1], 254);
    assert_eq!(out(MathOp::Set(300.0))[7], 255);
    assert_eq!(out(MathOp::Abs), out(MathOp::Add(0.0)));
    assert_eq!(rgb_set_components(0x12_34_56 as f64), [0x12, 0x34, 0x56]);
    assert_eq!(rgb_set_components(77.9), [0, 0, 77]);
}

#[test]
fn integer_types_imagej_lacks_follow_the_sixteen_bit_rules() {
    // Signed: results stop at the type's limits, and Abs works.
    let signed = Buffer::from_vec(vec![i16::MIN, -100, 0, 100, i16::MAX]);
    let out = |op| apply(&signed, op, None).unwrap().as_slice::<i16>().unwrap().to_vec();
    assert_eq!(out(MathOp::Add(200.0)), [-32568, 100, 200, 300, i16::MAX]);
    assert_eq!(out(MathOp::Subtract(200.0)), [i16::MIN, -300, -200, -100, 32567]);
    assert_eq!(out(MathOp::Multiply(-2.0)), [i16::MAX, 200, 0, -200, i16::MIN]);
    assert_eq!(out(MathOp::Abs), [i16::MAX, 100, 0, 100, i16::MAX]);
    assert_eq!(out(MathOp::Min(-50.5)), [-50, -50, 0, 100, i16::MAX]);
    assert_eq!(out(MathOp::Max(50.5)), [i16::MIN, -100, 0, 50, 50]);
    assert_eq!(out(MathOp::Square), [i16::MAX, 10000, 0, 10000, i16::MAX]);
    // Log of a sample that is not positive is 0, as for 16-bit.
    assert_eq!(out(MathOp::Log)[..3], [0, 0, 0]);

    // 32 and 64-bit integers are not squeezed through Java's 32-bit int.
    let wide = Buffer::from_vec(vec![0u32, 3_000_000_000, u32::MAX]);
    let out = |op| apply(&wide, op, None).unwrap().as_slice::<u32>().unwrap().to_vec();
    assert_eq!(out(MathOp::Add(1e9)), [1_000_000_000, 4_000_000_000, u32::MAX]);
    assert_eq!(out(MathOp::Subtract(4e9)), [0, 0, 294_967_295]);
    assert_eq!(out(MathOp::Set(3.5e9)), [3_500_000_000; 3]);
    let huge = Buffer::from_vec(vec![i64::MIN, -1, i64::MAX]);
    let out = apply(&huge, MathOp::Add(1.0), None).unwrap();
    assert_eq!(out.as_slice::<i64>().unwrap(), &[i64::MIN + 1, 0, i64::MAX]);
    // The 16-bit quirk stays: a huge constant wraps in Java's int.
    let short = Buffer::from_vec(vec![1u16, 100]);
    let out = apply(&short, MathOp::Add(3e9), None).unwrap();
    assert_eq!(out.as_slice::<u16>().unwrap(), &[0, 0]);

    // Gamma, Log and Exp use the given range, or else the plane's own.
    let plane = Buffer::from_vec(vec![100u16, 200, 400]);
    let own = apply(&plane, MathOp::Gamma(0.5), None).unwrap();
    let given = apply(&plane, MathOp::Gamma(0.5), Some((100.0, 400.0))).unwrap();
    assert_eq!(own, given);
    assert_eq!(own.as_slice::<u16>().unwrap(), &[100, 273, 400]);
    let other = apply(&plane, MathOp::Gamma(0.5), Some((0.0, 1000.0))).unwrap();
    assert_eq!(other.as_slice::<u16>().unwrap(), &[316, 447, 632]);
}

#[test]
fn float_rules_hold_in_both_precisions() {
    let single = Buffer::from_vec(vec![-4.0f32, 0.0, 0.25, 9.0, f32::NAN]);
    let double = Buffer::from_vec(vec![-4.0f64, 0.0, 0.25, 9.0, f64::NAN]);
    for op in [
        MathOp::Add(1.5),
        MathOp::Subtract(1.5),
        MathOp::Multiply(-2.0),
        MathOp::Divide(4.0),
        MathOp::Divide(0.0),
        MathOp::Min(0.5),
        MathOp::Max(0.5),
        MathOp::Gamma(2.0),
        MathOp::Set(3.0),
        MathOp::Square,
        MathOp::SquareRoot,
        MathOp::Abs,
        MathOp::Reciprocal,
        MathOp::And(0xf0),
        MathOp::NanBackground { lower: 0.0, upper: 1.0 },
    ] {
        let a = apply(&single, op, None).unwrap();
        let b = apply(&double, op, None).unwrap();
        let (a, b) = (a.as_slice::<f32>().unwrap(), b.as_slice::<f64>().unwrap());
        for (x, y) in a.iter().zip(b) {
            assert!(
                (x.is_nan() && y.is_nan()) || *x == *y as f32,
                "{op:?}: {x} in single precision, {y} in double"
            );
        }
    }
    let out = |op| apply(&single, op, None).unwrap().as_slice::<f32>().unwrap().to_vec();
    // Dividing by zero: infinities, and NaN for zero.
    let divided = out(MathOp::Divide(0.0));
    assert_eq!((divided[0], divided[3]), (f32::NEG_INFINITY, f32::INFINITY));
    assert!(divided[1].is_nan());
    // Reciprocal of zero is NaN, not infinity.
    assert!(out(MathOp::Reciprocal)[1].is_nan());
    assert_eq!(out(MathOp::Reciprocal)[2], 4.0);
    // Square root and gamma of what is not positive are 0; log is NaN or
    // negative infinity.
    assert_eq!(out(MathOp::SquareRoot)[..4], [0.0, 0.0, 0.5, 3.0]);
    assert_eq!(out(MathOp::Gamma(0.5))[..2], [0.0, 0.0]);
    assert!(out(MathOp::Log)[0].is_nan());
    assert_eq!(out(MathOp::Log)[1], f32::NEG_INFINITY);
    // NaN is not background: comparisons with it are false.
    let kept = out(MathOp::NanBackground { lower: 0.0, upper: 1.0 });
    assert!(kept[0].is_nan() && kept[3].is_nan() && kept[4].is_nan());
    assert_eq!(kept[1..3], [0.0, 0.25]);
    // Min and Max leave NaN alone.
    assert!(out(MathOp::Min(0.0))[4].is_nan());
}

#[test]
fn what_cannot_be_done_is_refused() {
    let complex = Buffer::from_vec(vec![Complex32::new(1.0, 2.0)]);
    assert!(apply(&complex, MathOp::Add(1.0), None).is_err());
    assert!(calculate(&complex, &complex, CalcOp::Add, 0.0).is_err());
    for dtype in [DType::U8, DType::I8, DType::U16, DType::I32, DType::U64] {
        let plane = Buffer::zeros(dtype, 3);
        for op in [MathOp::Reciprocal, MathOp::NanBackground { lower: 0.0, upper: 1.0 }] {
            let error = apply(&plane, op, None).unwrap_err().to_string();
            assert!(error.contains("needs a float image"), "{dtype}: {error}");
        }
    }
    assert!(apply(&Buffer::zeros(DType::F64, 0), MathOp::Log, None).unwrap().is_empty());
    assert!(apply(&Buffer::zeros(DType::U16, 0), MathOp::Gamma(0.5), None).unwrap().is_empty());
}

#[test]
fn float_calculator_rules() {
    let d = Buffer::from_vec(vec![6.0f64, -6.0, 0.0, f64::NAN, 2.5]);
    let s = Buffer::from_vec(vec![3.0f64, 0.0, 0.0, 1.0, f64::NAN]);
    let run = |op, dbz| calculate(&d, &s, op, dbz).unwrap().as_slice::<f64>().unwrap().to_vec();
    let ieee = run(CalcOp::Divide, f64::INFINITY);
    assert_eq!((ieee[0], ieee[1]), (2.0, f64::NEG_INFINITY));
    assert!(ieee[2].is_nan());
    // A finite or NaN "divide by zero value" replaces every such quotient.
    assert_eq!(run(CalcOp::Divide, -7.5)[..3], [2.0, -7.5, -7.5]);
    assert!(run(CalcOp::Divide, f64::NAN)[1].is_nan());
    // Min and Max keep image 1 when either sample is NaN.
    let min = run(CalcOp::Min, 0.0);
    assert!(min[3].is_nan());
    assert_eq!(min[4], 2.5);
    assert_eq!(run(CalcOp::And, 0.0)[0], 2.0);
    assert_eq!(run(CalcOp::Difference, 0.0)[..2], [3.0, 6.0]);
    assert_eq!(run(CalcOp::TransparentZero, 0.0)[..3], [3.0, -6.0, 0.0]);
}
