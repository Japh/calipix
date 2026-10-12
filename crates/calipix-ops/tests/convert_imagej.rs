//! Pixel type conversions that need only the samples against ImageJ 1.54p
//! (`fixtures/imagej/gen/62_convert.ijm`). Conversions through the display
//! range or a lookup table are checked in `calipix-commands`.

#[path = "math_support/mod.rs"]
mod support;

use std::collections::BTreeMap;

use calipix_core::{Buffer, DType, Rect};
use calipix_ops::convert::{
    UNWEIGHTED, WEIGHTED, convert, float_to_u16_scaled, rgb_to_gray_f32, rgb_to_gray_u8,
};
use support::{
    Difference, Raw, assert_matches, base, crop, csv, raw_planes, report, with_convert_specials,
};

const W: usize = 32;
const H: usize = 24;

/// A source of the macro: its channel planes and saved display range.
fn source(name: &str) -> (Vec<Buffer>, Option<(f64, f64)>) {
    let image = base(name);
    let meta = image.meta();
    let rect = Rect::new(16, 12, W, H);
    let planes = (0..meta.plane_count())
        .map(|p| crop(&image.read_plane(p).unwrap(), meta.width(), rect))
        .map(|plane| with_convert_specials(name, &plane))
        .collect();
    (planes, meta.channels.first().and_then(|c| c.display_range))
}

#[test]
fn sample_conversions_match_imagej() {
    let mut by_kind: BTreeMap<String, Difference> = BTreeMap::new();
    let mut cases = 0;
    for row in csv("convert.csv") {
        let (group, slice, name, setup, command) = (
            &row[0],
            row[1].parse::<usize>().unwrap(),
            row[3].as_str(),
            row[4].as_str(),
            row[5].as_str(),
        );
        if !["gray8", "gray16", "gray32f", "rgb"].contains(&name) {
            continue;
        }
        let (planes, file_range) = source(name);
        let words: Vec<&str> = setup.split(' ').collect();
        let noscale = words.contains(&"noscale");
        let range = words
            .iter()
            .find_map(|w| w.strip_prefix("range="))
            .map(|r| {
                let (lo, hi) = r.split_once(':').unwrap();
                (lo.parse().unwrap(), hi.parse().unwrap())
            })
            .or(file_range);
        let weights = if words.contains(&"weighted") {
            WEIGHTED
        } else {
            UNWEIGHTED
        };
        let rgb = || [&planes[0], &planes[1], &planes[2]];
        let ours = match (name, command) {
            ("gray8", "16-bit") => convert(&planes[0], DType::U16),
            ("gray8" | "gray16", "32-bit") => convert(&planes[0], DType::F32),
            ("gray16" | "gray32f", "8-bit") if noscale => convert(&planes[0], DType::U8),
            ("gray32f", "16-bit") if noscale => convert(&planes[0], DType::U16),
            ("gray32f", "16-bit") => float_to_u16_scaled(&planes[0], range.unwrap()),
            ("rgb", "8-bit") => rgb_to_gray_u8(rgb(), weights),
            ("rgb", "16-bit") => {
                rgb_to_gray_u8(rgb(), weights).and_then(|grey| convert(&grey, DType::U16))
            }
            ("rgb", "32-bit") => rgb_to_gray_f32(rgb(), weights),
            // Through the display range or a lookup table.
            _ => continue,
        }
        .unwrap();
        let expected = &raw_planes(
            &format!("conv_{group}.raw"),
            Raw::from_bit_depth(&row[6]),
            W,
            H,
        )[slice - 1];
        let what = format!("{name} [{setup}] {command}");
        let difference = assert_matches(&ours, &expected[0], &what);
        by_kind
            .entry(format!("{name} to {command}"))
            .or_default()
            .merge(difference);
        cases += 1;
    }
    for (kind, difference) in &by_kind {
        report(kind, *difference);
    }
    assert_eq!(cases, 19);
}

fn values<T: calipix_core::Pixel>(buf: &Buffer) -> Vec<T> {
    buf.as_slice::<T>().unwrap().to_vec()
}

#[test]
fn types_imagej_lacks_follow_the_documented_rules() {
    // Integer to integer: limited to the target's range.
    let wide = Buffer::from_vec(vec![-70000i32, -1, 0, 255, 256, 70000]);
    assert_eq!(
        values::<u8>(&convert(&wide, DType::U8).unwrap()),
        [0, 0, 0, 255, 255, 255]
    );
    assert_eq!(
        values::<i8>(&convert(&wide, DType::I8).unwrap()),
        [-128, -1, 0, 127, 127, 127]
    );
    assert_eq!(
        values::<u16>(&convert(&wide, DType::U16).unwrap()),
        [0, 0, 0, 255, 256, 65535]
    );
    assert_eq!(
        values::<i16>(&convert(&wide, DType::I16).unwrap()),
        [-32768, -1, 0, 255, 256, 32767]
    );
    // 64-bit integers do not pass through a double on the way.
    let huge = Buffer::from_vec(vec![u64::MAX, (1 << 53) + 1, 7]);
    assert_eq!(
        values::<i64>(&convert(&huge, DType::I64).unwrap()),
        [i64::MAX, (1 << 53) + 1, 7]
    );
    assert_eq!(
        values::<u32>(&convert(&huge, DType::U32).unwrap()),
        [u32::MAX, u32::MAX, 7]
    );

    // Float to a signed or wider integer: nearest, halves away from zero.
    let float = Buffer::from_vec(vec![-2.5f32, -0.4, 0.5, 1.5, 3e9, -3e9, f32::NAN]);
    assert_eq!(
        values::<i16>(&convert(&float, DType::I16).unwrap()),
        [-3, 0, 1, 2, i16::MAX, i16::MIN, 0]
    );
    assert_eq!(
        values::<u32>(&convert(&float, DType::U32).unwrap()),
        [0, 0, 1, 2, 3_000_000_000, 0, 0]
    );
    // Float to 8 and 16-bit: ImageJ's rules, also from 64-bit floats.
    let double = Buffer::from_vec(vec![-2.5f64, 0.49, 0.5, 254.5, 300.0, f64::NAN]);
    assert_eq!(
        values::<u8>(&convert(&double, DType::U8).unwrap()),
        [0, 0, 1, 255, 255, 0]
    );
    assert_eq!(
        values::<u16>(&convert(&double, DType::U16).unwrap()),
        [0, 0, 1, 255, 300, 0]
    );

    // Anything to float: the nearest representable value.
    assert_eq!(
        values::<f64>(&convert(&wide, DType::F64).unwrap()),
        [-70000.0, -1.0, 0.0, 255.0, 256.0, 70000.0]
    );
    assert_eq!(
        values::<f32>(&convert(&double, DType::F32).unwrap())[..5],
        [-2.5, 0.49, 0.5, 254.5, 300.0]
    );
}

#[test]
fn same_type_shares_the_samples_and_complex_is_refused() {
    let plane = Buffer::from_vec(vec![1u16, 2, 3]);
    assert!(convert(&plane, DType::U16).unwrap().shares_storage(&plane));
    assert!(convert(&plane, DType::C32).is_err());
    let complex = Buffer::zeros(DType::C64, 2);
    assert!(convert(&complex, DType::F32).is_err());
    assert!(float_to_u16_scaled(&plane, (0.0, 1.0)).is_err());
}
