//! The image calculator's sample arithmetic against ImageJ 1.54p
//! (`fixtures/imagej/gen/61_calculator.ijm`), for two images of one type.
//! Mixed types, sizes and stacks are rules of the command and are checked
//! in `calipix-commands`.

#[path = "math_support/mod.rs"]
mod support;

use std::collections::BTreeMap;

use calipix_core::{Buffer, Rect};
use calipix_ops::math::{CalcOp, calculate, calculate_rgb};
use support::{
    Difference, Raw, assert_matches, base, crop, csv, generated, raw_planes, report,
    with_math_specials,
};

const W: usize = 32;
const H: usize = 24;
const CROP: Rect = Rect {
    x: 16,
    y: 12,
    w: W,
    h: H,
};

/// Image 1: a base plane, cropped and given its special values.
fn first(name: &str) -> Vec<Buffer> {
    let image = base(name);
    (0..image.meta().plane_count())
        .map(|p| crop(&image.read_plane(p).unwrap(), image.meta().width(), CROP))
        .map(|plane| with_math_specials(name, &plane))
        .collect()
}

#[test]
fn same_type_arithmetic_matches_imagej() {
    let mut by_op: BTreeMap<String, Difference> = BTreeMap::new();
    let mut cases = 0;
    for row in csv("calc.csv") {
        let (group, slice, name1, name2, options) = (
            &row[0],
            row[1].parse::<usize>().unwrap(),
            &row[3],
            &row[4],
            &row[5],
        );
        if !["gray8", "gray16", "gray32f", "rgb"].contains(&group.as_str()) {
            continue;
        }
        let op = CalcOp::from_name(options.split(' ').next().unwrap()).expect("operation");
        let divide_by_zero = options
            .split(' ')
            .find_map(|o| o.strip_prefix("divide="))
            .map_or(f64::INFINITY, |v| v.parse().unwrap());
        let (a, b) = (first(name1), generated(name2));
        let expected = &raw_planes(&format!("calc_{group}.raw"), Raw::for_base(group), W, H)
            [slice - 1];
        let ours: Vec<Buffer> = if a.len() == 3 {
            calculate_rgb([&a[0], &a[1], &a[2]], [&b[0], &b[1], &b[2]], op)
                .unwrap()
                .into()
        } else {
            vec![calculate(&a[0], &b[0], op, divide_by_zero).unwrap()]
        };
        for (ours, theirs) in ours.iter().zip(expected) {
            let what = format!("{name1} {options} {name2}");
            let difference = assert_matches(ours, theirs, &what);
            by_op
                .entry(format!("{group} {}", op.name()))
                .or_default()
                .merge(difference);
        }
        cases += 1;
    }
    for (operation, difference) in &by_op {
        report(operation, *difference);
    }
    assert_eq!(cases, 4 * 13 + 4);
}

#[test]
fn operations_have_imagej_names() {
    for op in CalcOp::ALL {
        assert_eq!(CalcOp::from_name(op.name()), Some(op));
    }
    assert_eq!(CalcOp::from_name("Blend"), None);
}

#[test]
fn planes_must_agree_in_type_and_size() {
    let a = Buffer::from_vec(vec![1u8, 2]);
    assert!(calculate(&a, &Buffer::from_vec(vec![1u16, 2]), CalcOp::Add, 0.0).is_err());
    assert!(calculate(&a, &Buffer::from_vec(vec![1u8]), CalcOp::Add, 0.0).is_err());
}

#[test]
fn wider_integers_saturate() {
    let add = |a: Buffer, b: Buffer, op| calculate(&a, &b, op, f64::INFINITY).unwrap();
    let out = add(
        Buffer::from_vec(vec![i16::MAX, -5, 7]),
        Buffer::from_vec(vec![1i16, 10, 0]),
        CalcOp::Add,
    );
    assert_eq!(out.as_slice::<i16>().unwrap(), &[i16::MAX, 5, 7]);
    let out = add(
        Buffer::from_vec(vec![i16::MIN, -5, 7]),
        Buffer::from_vec(vec![1i16, 10, 0]),
        CalcOp::Subtract,
    );
    assert_eq!(out.as_slice::<i16>().unwrap(), &[i16::MIN, -15, 7]);
    let out = add(
        Buffer::from_vec(vec![u64::MAX, 3, 9]),
        Buffer::from_vec(vec![2u64, 0, 2]),
        CalcOp::Multiply,
    );
    assert_eq!(out.as_slice::<u64>().unwrap(), &[u64::MAX, 0, 18]);
    let out = add(
        Buffer::from_vec(vec![5u32, 6]),
        Buffer::from_vec(vec![0u32, 4]),
        CalcOp::Divide,
    );
    assert_eq!(out.as_slice::<u32>().unwrap(), &[u32::MAX, 1]);
    // The 16-bit quirk: a product that overflows 32 bits becomes 0.
    let out = add(
        Buffer::from_vec(vec![65535u16, 300, 46341]),
        Buffer::from_vec(vec![65535u16, 300, 46341]),
        CalcOp::Multiply,
    );
    assert_eq!(out.as_slice::<u16>().unwrap(), &[0, 65535, 0]);
}
