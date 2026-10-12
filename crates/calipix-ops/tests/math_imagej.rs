//! Process > Math against ImageJ 1.54p (`fixtures/imagej/gen/60_math.ijm`).

#[path = "math_support/mod.rs"]
mod support;

use std::collections::BTreeMap;

use calipix_core::{Buffer, Rect};
use calipix_ops::math::{MathOp, apply, rgb_set_components};
use support::{
    Difference, Raw, assert_matches, base, crop, csv, raw_planes, report, with_math_specials,
};

/// The rectangle every case is cropped to, and its size.
const CROP: Rect = Rect {
    x: 16,
    y: 12,
    w: 32,
    h: 24,
};

/// The input of every case for one base image: the cropped plane (three
/// for RGB) with the special values `60_math.ijm` writes into it, and the
/// display range the file was saved with.
fn input(name: &str) -> (Vec<Buffer>, Option<(f64, f64)>) {
    let image = base(name);
    let meta = image.meta();
    let planes = (0..meta.plane_count())
        .map(|p| crop(&image.read_plane(p).unwrap(), meta.width(), CROP))
        .map(|plane| with_math_specials(name, &plane))
        .collect();
    let range = meta.channels.first().and_then(|c| c.display_range);
    (planes, range)
}

fn op(command: &str, options: &str, range: &str) -> MathOp {
    let value = || -> f64 {
        options
            .strip_prefix("value=")
            .unwrap_or_else(|| panic!("{command} has no value in `{options}`"))
            .parse()
            .unwrap()
    };
    let bits =
        || i32::from_str_radix(options.strip_prefix("value=").unwrap(), 2).expect("binary number");
    match command {
        "Add..." => MathOp::Add(value()),
        "Subtract..." => MathOp::Subtract(value()),
        "Multiply..." => MathOp::Multiply(value()),
        "Divide..." => MathOp::Divide(value()),
        "AND..." => MathOp::And(bits()),
        "OR..." => MathOp::Or(bits()),
        "XOR..." => MathOp::Xor(bits()),
        "Min..." => MathOp::Min(value()),
        "Max..." => MathOp::Max(value()),
        "Gamma..." => MathOp::Gamma(value()),
        "Set..." => MathOp::Set(value()),
        "Log" => MathOp::Log,
        "Exp" => MathOp::Exp,
        "Square" => MathOp::Square,
        "Square Root" => MathOp::SquareRoot,
        "Abs" => MathOp::Abs,
        "Reciprocal" => MathOp::Reciprocal,
        "NaN Background" => {
            let limits: Vec<f64> = range
                .split(' ')
                .skip(1)
                .map(|v| v.parse().unwrap())
                .collect();
            MathOp::NanBackground {
                lower: limits[0],
                upper: limits[1],
            }
        }
        other => panic!("unknown command {other}"),
    }
}

#[test]
fn process_math_matches_imagej() {
    let mut report_by_op: BTreeMap<String, Difference> = BTreeMap::new();
    let mut cases = 0;
    for name in ["gray8", "gray16", "gray32f", "rgb"] {
        let (planes, file_range) = input(name);
        let kind = Raw::for_base(name);
        let expected = raw_planes(&format!("math_{name}.raw"), kind, CROP.w, CROP.h);
        let rows: Vec<_> = csv("math.csv")
            .into_iter()
            .filter(|row| row[0] == name)
            .collect();
        assert_eq!(rows.len(), expected.len(), "{name}: cases and planes");

        for row in rows {
            let (slice, command, options, range_before) =
                (row[1].parse::<usize>().unwrap(), &row[2], &row[3], &row[4]);
            let what = format!("{name} {command} {options} [{range_before}]");
            let range = match range_before.as_str() {
                "-" => file_range,
                r if r.starts_with("threshold") => file_range,
                r => {
                    let (lo, hi) = r.split_once(' ').unwrap();
                    Some((lo.parse().unwrap(), hi.parse().unwrap()))
                }
            };
            let op = op(command, options, range_before);
            // ImageJ leaves integer images alone where Calipix refuses.
            let refused = matches!(op, MathOp::Reciprocal) && name != "gray32f";
            for (channel, plane) in planes.iter().enumerate() {
                let ours = if refused {
                    assert!(apply(plane, op, range).is_err(), "{what}");
                    plane.clone()
                } else if let (Raw::Rgb, MathOp::Set(value)) = (kind, op) {
                    let component = rgb_set_components(value)[channel];
                    apply(plane, MathOp::Set(component as f64), None).unwrap()
                } else {
                    apply(plane, op, range).unwrap()
                };
                let difference = assert_matches(&ours, &expected[slice - 1][channel], &what);
                report_by_op
                    .entry(format!("{name} {command}"))
                    .or_default()
                    .merge(difference);
            }
            cases += 1;
        }
    }
    for (operation, difference) in &report_by_op {
        report(operation, *difference);
    }
    assert_eq!(cases, 118);
}
