//! Selections in the other places ImageJ keeps them: ROI Manager sets and
//! TIFF files. And the way back: files for ImageJ to open.

use std::fmt::Write as _;
use std::path::PathBuf;

use calipix_roi::{Color, Roi, RoiEntry, decode, decode_hex, encode, read_zip, write_zip};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn parse(text: &str) -> Roi {
    text.parse().unwrap()
}

/// `roiset.zip` is what ImageJ's ROI Manager saved for five selections
/// (`fixtures/imagej/gen/07_rois.ijm`).
#[test]
fn a_roi_manager_set_written_by_imagej_is_read() {
    let set = read_zip(&std::fs::read(fixture("roiset.zip")).unwrap()).unwrap();
    assert!(set.skipped.is_empty(), "{:?}", set.skipped);
    let found: Vec<(String, String)> = set
        .entries
        .iter()
        .map(|e| (e.props.name.clone().unwrap(), e.roi.to_string()))
        .collect();
    let expected = [
        ("0009-0009", "rectangle 4 4 10 10"),
        ("0020-0027", "oval 20 10 15 20"),
        ("0022-0050", "polygon 40,5 60,10 50,40"),
        ("0042-0031", "line 2 40 60 44"),
        ("0017-0033", "points 33,17"),
    ];
    assert_eq!(found.len(), expected.len());
    for ((name, roi), (expected_name, expected_roi)) in found.iter().zip(expected) {
        assert_eq!((name.as_str(), roi.as_str()), (expected_name, expected_roi));
    }

    // Written again, the entries hold the bytes ImageJ stored, under the
    // names ImageJ gave them.
    let ours = read_zip(&write_zip(&set.entries).unwrap()).unwrap();
    assert_eq!(ours, set);
    let single = std::fs::read(fixture("roi_oval.roi")).unwrap();
    let oval = decode(&single).unwrap();
    assert_eq!(oval.roi, parse("oval 20 10 15 20"));
    assert_eq!(encode(&oval).unwrap(), single);
    // A selection without a name is filed under the centre of its bounds,
    // as the ROI Manager files it.
    let unnamed: Vec<RoiEntry> = set
        .entries
        .iter()
        .map(|e| RoiEntry::new(e.roi.clone()))
        .collect();
    let names: Vec<String> = read_zip(&write_zip(&unnamed).unwrap())
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.props.name.unwrap())
        .collect();
    assert_eq!(names, expected.map(|(name, _)| name));
}

/// `gray8_roi_overlay.tif` was saved by ImageJ with an oval selected and
/// an overlay of a red rectangle and a green line two pixels wide
/// (`fixtures/imagej/gen/04_luts_calibration.ijm`). The TIFF reader keeps
/// the stored selections as hexadecimal text in the image properties.
#[test]
fn the_selection_and_overlay_of_an_imagej_tiff_are_read() {
    let image = calipix_io::open(fixture("gray8_roi_overlay.tif")).unwrap();
    let properties = &image.meta().properties;
    let selection = decode_hex(&properties["imagej.roi"]).unwrap();
    assert_eq!(selection.roi, parse("oval 20 10 15 20"));

    let mut overlay = Vec::new();
    while let Some(hex) = properties.get(&format!("imagej.overlay.{}", overlay.len())) {
        overlay.push(decode_hex(hex).unwrap());
    }
    assert_eq!(overlay.len(), 2);
    assert_eq!(overlay[0].roi, parse("rectangle 4 4 10 10"));
    assert_eq!(overlay[0].props.stroke_color, Some(Color::rgb(255, 0, 0)));
    assert_eq!(overlay[1].roi, parse("line 2 40 60 44 width 2"));
    assert_eq!(overlay[1].props.stroke_color, Some(Color::rgb(0, 255, 0)));

    // What would be written back is what ImageJ wrote.
    let hex = |bytes: Vec<u8>| -> String {
        bytes.iter().fold(String::new(), |mut s, b| {
            write!(s, "{b:02x}").unwrap();
            s
        })
    };
    assert_eq!(hex(encode(&selection).unwrap()), properties["imagej.roi"]);
    for (i, entry) in overlay.iter().enumerate() {
        assert_eq!(
            hex(encode(entry).unwrap()),
            properties[&format!("imagej.overlay.{i}")],
            "overlay {i}"
        );
    }
}

/// The selections written for ImageJ to open, by name.
const FOR_IMAGEJ: &[(&str, &str)] = &[
    ("rectangle", "rectangle 10 8 20 12"),
    ("rectangle_sub", "rectangle 10.5 8.25 20.25 12.5"),
    ("rounded", "rounded-rectangle 10 8 30 20 10"),
    ("oval", "oval 20 10 15 20"),
    ("oval_sub", "oval 10.5 8.25 20.25 12.5"),
    ("polygon", "polygon 5,5 30,8 18,20 32,40 4,30 12,18"),
    ("polygon_sub", "polygon 10.5,5.25 40.75,8.5 22.25,30.75"),
    // Crossing the top and left edges only: ImageJ opens a selection that
    // reaches past the right or bottom edge in a new, larger image.
    ("polygon_cross", "polygon -10,5 30,-8 60,30 20,45"),
    (
        "freehand",
        "freehand 10,12 14,6 22,4 30,8 36,16 34,28 26,36 15,34 8,22",
    ),
    ("traced", "traced 5,5 9,5 9,7 12,7 12,12 8,12 8,10 5,10"),
    ("line", "line 2 40 60 44"),
    ("line_sub_wide", "line 2.5 3.25 40.75 30.5 width 5"),
    ("polyline", "polyline 5,5 20,30 40,10 60,40"),
    (
        "polyline_sub_wide",
        "polyline 5.5,5.5 20.25,30.25 40.75,10.5 width 4",
    ),
    (
        "freeline",
        "freeline 5,40 9,30 15,24 22,22 30,26 36,34 44,38",
    ),
    ("angle", "angle 10,40 30,10 50,35"),
    ("point", "points 33,17"),
    ("points_sub", "points 5.5,5.5 20.25,30.25 40.75,12.75"),
    ("ellipse", "ellipse 10 10 50 35 0.5"),
    ("rotated", "rotated-rectangle 10 10 50 35 12"),
    (
        "composite_hole",
        "composite M 5,5 L 45,5 L 45,35 L 5,35 Z M 15,12 L 30,12 L 30,25 L 15,25 Z",
    ),
    (
        "composite_curves",
        "composite M 10,24 C 10,12 20,6 32,6 C 44,6 54,12 54,24 Q 32,50 10,24 Z M 60,40 L 63,40 L 63,46 Z",
    ),
];

/// One line of the report `tests/support/imagej_open_check.ijm` prints for
/// a selection opened in ImageJ on a 64x48 image.
fn report_line(name: &str, roi: &Roi) -> String {
    // ImageJ reads a rotated rectangle back as the polygon of its four
    // corners, whose bounds it works out slightly differently.
    let as_read = match roi {
        Roi::RotatedRectangle { .. } => Roi::polygon(roi.outline()),
        _ => roi.clone(),
    };
    let roi = &as_read;
    let (x, y, w, h) = roi.int_bounds();
    let area = if roi.is_area() {
        roi.mask(64, 48).map_or(0, |m| m.count()).to_string()
    } else {
        "-".to_string()
    };
    let length = if roi.is_points() {
        "-".to_string()
    } else {
        format!("{:.4}", roi.length())
    };
    format!(
        "{name} type={} bounds={x},{y},{w},{h} area={area} length={length}",
        roi.ij_type()
    )
}

/// Writes selection files, a set of them, and the report ImageJ should
/// print when it opens them, into `$CALIPIX_ROI_CHECK_DIR`. Not part of
/// `cargo test`; see `tests/support/imagej_open_check.ijm`.
#[test]
#[ignore = "writes files for a manual check with ImageJ"]
fn write_files_for_imagej_check() {
    let dir = PathBuf::from(
        std::env::var("CALIPIX_ROI_CHECK_DIR").expect("set CALIPIX_ROI_CHECK_DIR to a directory"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let mut expected = String::new();
    let mut entries = Vec::new();
    for (name, text) in FOR_IMAGEJ {
        let roi = parse(text);
        std::fs::write(
            dir.join(format!("{name}.roi")),
            encode(&RoiEntry::new(roi.clone())).unwrap(),
        )
        .unwrap();
        writeln!(expected, "{}", report_line(name, &roi)).unwrap();
        entries.push(RoiEntry::named(roi, format!("set-{name}")));
    }
    for entry in &entries {
        let name = entry.props.name.as_deref().unwrap();
        writeln!(expected, "{}", report_line(name, &entry.roi)).unwrap();
    }
    std::fs::write(dir.join("set.zip"), write_zip(&entries).unwrap()).unwrap();
    expected.push_str("CHECK_DONE\n");
    std::fs::write(dir.join("expected.txt"), expected).unwrap();
}

#[test]
fn the_files_written_for_imagej_read_back() {
    let entries: Vec<RoiEntry> = FOR_IMAGEJ
        .iter()
        .map(|(name, text)| RoiEntry::named(parse(text), *name))
        .collect();
    let set = read_zip(&write_zip(&entries).unwrap()).unwrap();
    assert!(set.skipped.is_empty());
    for (back, entry) in set.entries.iter().zip(&entries) {
        // Ellipses and rotated rectangles come back to within rounding.
        assert_eq!(back.roi.kind(), entry.roi.kind());
        assert_eq!(back.roi.mask(64, 48), entry.roi.mask(64, 48));
        assert_eq!(back.props.name, entry.props.name);
    }
}
