//! Selections against ImageJ 1.54p.
//!
//! `fixtures/imagej/gen/40_roi_shapes.ijm` makes selections of every kind
//! on a 64x48 image and records, for each, the `.roi` file ImageJ saves,
//! the pixels it fills, its bounds, its vertices and its measurements.
//! Each test reads the `.roi` file and requires Calipix to agree with
//! what ImageJ recorded.

use std::collections::BTreeMap;
use std::path::PathBuf;

use calipix_roi::{Color, Kind, Roi, RoiEntry, decode, draw_line_pixels, encode};

const WIDTH: usize = 64;
const HEIGHT: usize = 48;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// One case of the fixture: what ImageJ reported for a selection.
struct Case {
    name: String,
    values: BTreeMap<String, f64>,
    coords: Vec<(f64, f64)>,
    /// The pixels ImageJ filled, row-major.
    mask: Vec<bool>,
    bytes: Vec<u8>,
    entry: RoiEntry,
}

impl Case {
    fn get(&self, key: &str) -> f64 {
        self.values[key]
    }

    fn int(&self, key: &str) -> i32 {
        let v = self.values[key];
        assert_eq!(v, v.round(), "{}: {key}", self.name);
        v as i32
    }

    fn roi(&self) -> &Roi {
        &self.entry.roi
    }
}

fn cases() -> Vec<Case> {
    let info = read("roi_info.csv");
    let mut lines = info.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    let mut coords: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    for line in read("roi_coords.csv").lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        coords
            .entry(f[0].to_string())
            .or_default()
            .push((f[2].parse().unwrap(), f[3].parse().unwrap()));
    }
    let mut masks: BTreeMap<String, Vec<bool>> = BTreeMap::new();
    for line in read("roi_masks.csv").lines() {
        let f: Vec<&str> = line.split(',').collect();
        assert_eq!(f.len(), 1 + HEIGHT);
        let mut mask = Vec::with_capacity(WIDTH * HEIGHT);
        for row in &f[1..] {
            for digit in row.chars() {
                let v = digit.to_digit(16).unwrap();
                mask.extend((0..4).rev().map(|bit| v >> bit & 1 == 1));
            }
        }
        assert_eq!(mask.len(), WIDTH * HEIGHT);
        masks.insert(f[0].to_string(), mask);
    }
    lines
        .map(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            let name = fields[0].to_string();
            let values = header[1..]
                .iter()
                .zip(&fields[1..])
                .map(|(k, v)| (k.to_string(), v.parse::<f64>().unwrap()))
                .collect();
            let bytes = std::fs::read(fixture(&format!("roi_{name}.roi"))).unwrap();
            let entry = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            Case {
                coords: coords.remove(&name).unwrap_or_default(),
                mask: masks.remove(&name).unwrap(),
                name,
                values,
                bytes,
                entry,
            }
        })
        .collect()
}

fn case(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no case {name}"))
}

/// The pixels of a 64x48 image a selection's mask covers.
fn mask_bits(roi: &Roi) -> Vec<bool> {
    let mut bits = vec![false; WIDTH * HEIGHT];
    if let Some(mask) = roi.mask(WIDTH, HEIGHT) {
        for y in 0..mask.rect.h {
            for x in 0..mask.rect.w {
                if mask.data[y * mask.rect.w + x] != 0 {
                    bits[(y + mask.rect.y) * WIDTH + x + mask.rect.x] = true;
                }
            }
        }
    }
    bits
}

fn pixel_bits(pixels: &[(i32, i32)]) -> Vec<bool> {
    let mut bits = vec![false; WIDTH * HEIGHT];
    for &(x, y) in pixels {
        if x >= 0 && y >= 0 && (x as usize) < WIDTH && (y as usize) < HEIGHT {
            bits[y as usize * WIDTH + x as usize] = true;
        }
    }
    bits
}

fn picture(bits: &[bool], other: &[bool]) -> String {
    let mut s = String::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            s.push(match (bits[y * WIDTH + x], other[y * WIDTH + x]) {
                (true, true) => '#',
                (false, false) => '.',
                (true, false) => 'C', // Calipix only
                (false, true) => 'J', // ImageJ only
            });
        }
        s.push('\n');
    }
    s
}

fn assert_mask(name: &str, ours: &[bool], theirs: &[bool]) {
    assert!(
        ours == theirs,
        "{name}: mask differs from ImageJ's (C: Calipix only, J: ImageJ only)\n{}",
        picture(ours, theirs)
    );
}

fn close(a: f64, b: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return a.is_nan() && b.is_nan();
    }
    // The fixture holds nine decimals.
    (a - b).abs() <= 1e-5 * a.abs().max(b.abs()) + 2e-9
}

/// What filling a selection sets in ImageJ: the mask of an area, of
/// points or of a wide straight line; the drawn strokes of a thin line.
fn filled(roi: &Roi) -> Option<Vec<bool>> {
    if !roi.is_line() || matches!(roi, Roi::Line { width, .. } if *width > 1.0) {
        return Some(mask_bits(roi));
    }
    if roi.line_width() > 1.0 {
        return None; // drawn with a wide pen, which Calipix does not have
    }
    Some(pixel_bits(&roi.drawn_pixels()?))
}

/// Fails with every collected disagreement, so that one run shows them all.
fn report(problems: Vec<String>) {
    assert!(problems.is_empty(), "{} problems:
{}", problems.len(), problems.join("
"));
}

#[test]
fn every_case_decodes_to_the_kind_and_bounds_imagej_reports() {
    let cases = cases();
    assert_eq!(cases.len(), 140);
    let mut problems = Vec::new();
    for c in &cases {
        let roi = c.roi();
        if roi.ij_type() != c.int("type") {
            problems.push(format!("{}: type {} vs {}", c.name, roi.ij_type(), c.int("type")));
        }
        let theirs = (c.int("bx"), c.int("by"), c.int("bw"), c.int("bh"));
        if roi.int_bounds() != theirs {
            problems.push(format!("{}: bounds {:?} vs {theirs:?}", c.name, roi.int_bounds()));
        }
        let (x, y, w, h) = roi.bounds();
        for (ours, key) in [(x, "fx"), (y, "fy"), (w, "fw"), (h, "fh")] {
            if (ours - c.get(key)).abs() >= 1e-6 {
                problems.push(format!("{}: {key} {ours} vs {}", c.name, c.get(key)));
            }
        }
    }
    report(problems);
}

#[test]
fn masks_equal_imagej_pixel_for_pixel() {
    let mut checked = 0;
    let mut problems = Vec::new();
    for c in cases() {
        let Some(ours) = filled(c.roi()) else {
            continue;
        };
        checked += 1;
        if ours != c.mask {
            problems.push(format!(
                "{}: mask differs (C: Calipix only, J: ImageJ only)
{}",
                c.name,
                picture(&ours, &c.mask)
            ));
        } else if c.roi().is_area() {
            let count = ours.iter().filter(|&&b| b).count();
            if count as f64 != c.get("Area") {
                problems.push(format!("{}: {count} pixels vs {}", c.name, c.get("Area")));
            }
        }
    }
    report(problems);
    assert_eq!(checked, 139);
}

#[test]
fn wide_lines_cover_the_area_imagej_measures() {
    // ImageJ measures a wide line over the area Line to Area gives it.
    for (line, area) in [
        ("line_wide", "area_line_wide"),
        ("polyline_wide", "area_polyline_wide"),
    ] {
        let (line, area) = (case(line), case(area));
        assert_mask(&line.name, &mask_bits(line.roi()), &area.mask);
        let count = mask_bits(line.roi()).iter().filter(|&&b| b).count();
        assert_eq!(count as f64, line.get("Area"), "{}", line.name);
    }
    assert_eq!(case("line_int").roi().mask(WIDTH, HEIGHT), None);
}

#[test]
fn outlines_equal_get_selection_coordinates() {
    for c in cases() {
        let ours = c.roi().outline();
        assert_eq!(ours.len(), c.coords.len(), "{}: vertex count", c.name);
        for (i, (a, b)) in ours.iter().zip(&c.coords).enumerate() {
            assert!(
                (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4,
                "{}: vertex {i} is {a:?}, ImageJ has {b:?}",
                c.name
            );
        }
        // Roi.size(): for an oval, the corners of its mask's outline; for
        // a wide straight line, the four corners of its outline.
        let size = match c.roi() {
            Roi::Oval { .. } | Roi::Line { .. } => c.roi().int_outline().len(),
            _ => ours.len(),
        };
        assert_eq!(size as f64, c.get("size"), "{}: size", c.name);
    }
}

#[test]
fn lengths_and_perimeters_equal_imagej() {
    let mut problems = Vec::new();
    for c in cases() {
        let roi = c.roi();
        if roi.is_points() {
            continue;
        }
        let (key, cal) = if roi.is_area() {
            ("Perim", "cPerim")
        } else if matches!(roi, Roi::Angle { .. }) {
            // An angle is measured as an angle; "Perim" still holds the
            // length of its two arms.
            ("Perim", "cPerim")
        } else {
            ("Length", "cLength")
        };
        let ours = roi.length();
        if !close(ours, c.get(key)) {
            problems.push(format!("{}: {key} {ours} vs {}", c.name, c.get(key)));
        }
        let ours = roi.length_scaled(0.5, 0.25);
        if !close(ours, c.get(cal)) {
            problems.push(format!("{}: {cal} {ours} vs {}", c.name, c.get(cal)));
        }
    }
    report(problems);
}

#[test]
fn feret_diameters_equal_imagej() {
    let mut problems = Vec::new();
    for c in cases() {
        let roi = c.roi();
        if !roi.is_area() {
            continue;
        }
        let feret = roi.feret().unwrap();
        for (ours, key) in [
            (feret.max, "Feret"),
            (feret.angle, "FeretAngle"),
            (feret.min, "MinFeret"),
            (feret.x, "FeretX"),
            (feret.y, "FeretY"),
        ] {
            if !close(ours, c.get(key)) {
                problems.push(format!("{}: {key} {ours} vs {}", c.name, c.get(key)));
            }
        }
        let feret = roi.feret_scaled(0.5, 0.25).unwrap();
        for (ours, key) in [
            (feret.max, "cFeret"),
            (feret.angle, "cFeretAngle"),
            (feret.min, "cMinFeret"),
            (feret.x, "cFeretX"),
            (feret.y, "cFeretY"),
        ] {
            if !close(ours, c.get(key)) {
                problems.push(format!("{}: {key} {ours} vs {}", c.name, c.get(key)));
            }
        }
    }
    report(problems);
}

#[test]
fn line_angles_equal_imagej() {
    for c in cases() {
        if let Some(angle) = c.roi().angle() {
            assert!(
                close(angle, c.get("Angle")),
                "{}: angle {angle} vs {}",
                c.name,
                c.get("Angle")
            );
        }
    }
}

#[test]
fn fitted_ellipses_and_centroids_equal_imagej() {
    for c in cases() {
        let roi = c.roi();
        if !roi.is_area() {
            continue;
        }
        let fit = roi.ellipse_fit(WIDTH, HEIGHT).unwrap();
        for (ours, key) in [(fit.major, "Major"), (fit.minor, "Minor"), (fit.angle, "Angle")] {
            assert!(close(ours, c.get(key)), "{}: {key} {ours} vs {}", c.name, c.get(key));
        }
        let (x, y) = roi.scale_center(WIDTH, HEIGHT).unwrap();
        assert!(close(x, c.get("X")) && close(y, c.get("Y")), "{}: centroid", c.name);
    }
}

/// ImageJ builds an ellipse's 72 vertices from the numbers it was given in
/// double precision but stores those numbers in single precision, so
/// reading its own file back gives slightly different vertices. These
/// cases have parameters that single precision cannot hold.
const INEXACT_ELLIPSES: [&str; 6] = [
    "ellipse_sub",
    "ellipse_cross",
    "fit_ellipse_poly",
    "fit_ellipse_rect",
    "fit_ellipse_oval",
    "fit_ellipse_comp",
];

#[test]
fn reencoding_gives_imagej_bytes() {
    let mut different = Vec::new();
    for c in cases() {
        let ours = encode(&c.entry).unwrap();
        if ours != c.bytes && !INEXACT_ELLIPSES.contains(&c.name.as_str()) {
            let at = ours
                .iter()
                .zip(&c.bytes)
                .position(|(a, b)| a != b)
                .unwrap_or(ours.len().min(c.bytes.len()));
            different.push(format!(
                "{} ({} vs {} bytes, first difference at {at})",
                c.name,
                ours.len(),
                c.bytes.len()
            ));
        }
        // Whatever the bytes, nothing is lost on the way.
        if decode(&ours).as_ref() != Ok(&c.entry) {
            different.push(format!("{}: decoding our bytes gives another selection", c.name));
        }
    }
    assert_eq!(different, Vec::<String>::new());
}

#[test]
fn ellipses_built_from_the_same_numbers_are_saved_as_imagej_saves_them() {
    let or = parse("rectangle 5 5 20 15")
        .combined(&parse("oval 15 10 25 25"), calipix_roi::Combine::Or)
        .unwrap();
    let fitted = |roi: Roi| roi.fitted_ellipse(WIDTH, HEIGHT).unwrap();
    for (name, roi) in [
        ("ellipse_sub", parse("ellipse 20.5 40.2 40.1 5.7 0.3")),
        ("ellipse_cross", parse("ellipse -10 10 40 60 0.6")),
        ("fit_ellipse_poly", fitted(parse("polygon 5,5 30,8 18,20 32,40 4,30 12,18"))),
        ("fit_ellipse_rect", fitted(parse("rectangle 10 8 20 12"))),
        ("fit_ellipse_oval", fitted(parse("oval 10 10 40 13"))),
        ("fit_ellipse_comp", fitted(or)),
    ] {
        assert!(INEXACT_ELLIPSES.contains(&name));
        let ours = encode(&RoiEntry::new(roi)).unwrap();
        assert!(ours == case(name).bytes, "{name}: bytes differ from ImageJ's");
    }
}

/// Compares two selections through their text forms, numbers to within
/// what single precision holds.
fn assert_same_shape(name: &str, ours: &Roi, theirs: &Roi) {
    let (a, b) = (ours.to_string(), theirs.to_string());
    let words = |s: &str| -> Vec<String> {
        s.split([' ', ','])
            .map(str::to_string)
            .collect()
    };
    let (wa, wb) = (words(&a), words(&b));
    let same = wa.len() == wb.len()
        && wa.iter().zip(&wb).all(|(x, y)| {
            match (x.parse::<f64>(), y.parse::<f64>()) {
                (Ok(x), Ok(y)) => (x - y).abs() <= 2e-5 * x.abs().max(y.abs()).max(1.0),
                _ => x == y,
            }
        });
    assert!(same, "{name}:\n  ours   {a}\n  ImageJ {b}");
}

/// The selections the macro makes directly, in Calipix's text form.
const DIRECT: &[(&str, &str)] = &[
    ("rect_int", "rectangle 10 8 20 12"),
    ("rect_sub", "rectangle 10.5 8.25 20.3 12.6"),
    ("rect_tiny", "rectangle 5 5 1 1"),
    ("rect_cross_tl", "rectangle -5 -4 20 12"),
    ("rect_cross_br", "rectangle 50 40 30 20"),
    ("rect_all", "rectangle 0 0 64 48"),
    ("rounded", "rounded-rectangle 10 8 30 20 10"),
    ("rounded_small", "rounded-rectangle 3 3 7 5 4"),
    ("rounded_big_arc", "rounded-rectangle 10 10 20 10 30"),
    ("rounded_cross", "rounded-rectangle 40 30 40 30 16"),
    ("rounded_large", "rounded-rectangle 2 2 60 44 23"),
    ("oval_int", "oval 20 10 15 20"),
    ("oval_circle_odd", "oval 5 5 21 21"),
    ("oval_circle_even", "oval 8 6 30 30"),
    ("oval_thin", "oval 10 10 40 3"),
    ("oval_2", "oval 3 3 2 2"),
    ("oval_1", "oval 3 3 1 1"),
    ("oval_small", "oval 3 3 5 4"),
    ("oval_sub", "oval 10.5 8.25 20.3 12.6"),
    ("oval_cross_tl", "oval -10 -8 30 25"),
    ("oval_cross_br", "oval 45 30 40 40"),
    ("oval_all", "oval 0 0 64 48"),
    ("poly_tri", "polygon 40,5 60,10 50,40"),
    ("poly_concave", "polygon 5,5 30,8 18,20 32,40 4,30 12,18"),
    ("poly_bowtie", "polygon 10,10 50,40 50,10 10,40"),
    ("poly_star", "polygon 32,4 44,42 12,18 52,18 20,42"),
    ("poly_cross", "polygon -10,5 30,-8 70,30 20,60"),
    ("poly_thin", "polygon 5,5 60,6 5,7"),
    ("poly_rectilinear", "polygon 5,5 25,5 25,15 15,15 15,30 5,30"),
    ("poly_small_box", "polygon 10,10 14,10 14,13 10,13"),
    ("poly_tiny", "polygon 3,3 5,3 4,5"),
    ("poly_all", "polygon 0,0 64,0 64,48 0,48"),
    ("poly_parallelogram", "polygon 8,40 20,4 56,4 44,40"),
    ("polyf_tri", "polygon 10.5,5.25 40.75,8.5 22.3,30.9"),
    ("polyf_cross", "polygon -6.5,5.5 30.25,-8.25 70.4,30.7 20.6,60.1"),
    ("polyf_small", "polygon 3.2,3.2 5.9,3.4 4.1,6.8"),
    ("polyf_bowtie", "polygon 10.3,10.2 50.6,40.7 50.1,10.4 10.9,40.3"),
    ("polyf_half_box", "polygon 10.5,10.5 20.5,10.5 20.5,20.5 10.5,20.5"),
    ("polyf_mixed", "polygon 10,10 20.5,10 20,20 10,20"),
    (
        "free_int",
        "freehand 10,12 14,6 22,4 30,8 36,16 34,28 26,36 15,34 8,22",
    ),
    (
        "free_sub",
        "freehand 10.5,12.1 14.2,6.6 22.8,4.3 30.1,8.8 36.6,16.2 34.3,28.5 26.9,36.4 15.4,34.9 8.7,22.3",
    ),
    ("traced_int", "traced 5,5 9,5 9,7 12,7 12,12 8,12 8,10 5,10"),
    (
        "traced_stairs",
        "traced 20,20 21,20 21,21 22,21 22,22 23,22 23,23 20,23",
    ),
    ("traced_1px", "traced 30,30 31,30 31,31 30,31"),
    ("line_int", "line 2 40 60 44"),
    ("line_sub", "line 2.5 3.25 40.75 30.5"),
    ("line_vertical", "line 10 2 10 40"),
    ("line_backwards", "line 50 30 12 6"),
    ("line_cross", "line -5 10 70 20"),
    ("line_wide", "line 5 5 50 30 width 7"),
    ("line_wide_horizontal", "line 8 40 56 40 width 4"),
    ("polyline_int", "polyline 5,5 20,30 40,10 60,40"),
    ("polyline_sub", "polyline 5.5,5.5 20.25,30.25 40.75,10.5 60.1,40.9"),
    ("polyline_wide", "polyline 5,5 20,30 40,10 60,40 width 6"),
    ("freeline_int", "freeline 5,40 9,30 15,24 22,22 30,26 36,34 44,38"),
    (
        "freeline_sub",
        "freeline 5.5,40.1 9.2,30.6 15.8,24.3 22.1,22.8 30.6,26.2 36.3,34.5 44.9,38.4",
    ),
    ("angle_int", "angle 10,40 30,10 50,35"),
    ("angle_sub", "angle 10.5,40.5 30.25,10.25 50.75,35.5"),
    ("point_int", "points 33,17"),
    ("point_sub", "points 10.5,20.25"),
    ("points_int", "points 5,5 20,30 40,12 63,47"),
    ("points_sub", "points 5.5,5.5 20.25,30.25 40.75,12.75 62.4,46.6"),
    ("points_cross", "points -3,5 20,30 70,50"),
    ("ellipse", "ellipse 10 10 50 35 0.5"),
    ("ellipse_sub", "ellipse 20.5 40.2 40.1 5.7 0.3"),
    ("ellipse_circle", "ellipse 10 24 50 24 1"),
    ("ellipse_cross", "ellipse -10 10 40 60 0.6"),
    ("ellipse_small", "ellipse 30 20 34 23 0.8"),
    ("rotrect", "rotated-rectangle 10 10 50 35 12"),
    ("rotrect_sub", "rotated-rectangle 20.5 40 40 5.5 7.3"),
    ("rotrect_axis", "rotated-rectangle 10 24 50 24 10"),
    ("rotrect_cross", "rotated-rectangle -10 10 40 60 15"),
];

#[test]
fn building_a_selection_directly_equals_reading_imagej_s_file() {
    for (name, text) in DIRECT {
        let c = case(name);
        let direct: Roi = text.parse().unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_same_shape(name, &direct, c.roi());
        if let Some(filled) = filled(&direct) {
            assert_mask(name, &filled, &c.mask);
        }
        assert_eq!(direct.int_bounds(), c.roi().int_bounds(), "{name}");
        let (a, b) = (direct.length(), c.roi().length());
        assert!(close(a, b), "{name}: length {a} vs {b}");
    }
}

fn parse(text: &str) -> Roi {
    text.parse().unwrap()
}

/// A selection made from others the way the macro makes it, compared with
/// ImageJ's result by the pixels it covers.
fn assert_covers(name: &str, ours: Option<Roi>) -> Roi {
    let c = case(name);
    let ours = ours.unwrap_or_else(|| panic!("{name}: no selection"));
    assert_mask(name, &filled(&ours).unwrap(), &c.mask);
    ours
}

#[test]
fn inverses_cover_what_imagej_s_cover() {
    for (name, source) in [
        ("inverse_oval", "oval 10 8 25 20"),
        ("inverse_rect", "rectangle 10 8 25 20"),
        ("inverse_rect_side", "rectangle 0 0 20 48"),
        ("inverse_rect_corner", "rectangle 0 0 20 30"),
        ("inverse_polyf", "polygon 10.5,5.25 40.75,8.5 22.3,30.9"),
        ("inverse_rounded", "rounded-rectangle 10 8 30 20 10"),
        ("inverse_poly_tri", "polygon 40,5 60,10 50,40"),
        ("inverse_parallelogram", "polygon 8,40 20,4 56,4 44,40"),
        ("inverse_bowtie", "polygon 10,10 50,40 50,10 10,40"),
        ("inverse_oval_cross", "oval 45 30 40 40"),
        ("inverse_ellipse", "ellipse 10 10 50 35 0.5"),
    ] {
        let inverse = assert_covers(name, parse(source).inverse(WIDTH, HEIGHT));
        let theirs = case(name);
        assert_eq!(inverse.kind(), theirs.roi().kind(), "{name}");
        assert_eq!(inverse.int_bounds(), theirs.roi().int_bounds(), "{name}");
        // Inverting twice gives back the pixels of the original.
        let twice = inverse.inverse(WIDTH, HEIGHT).unwrap();
        assert_mask(name, &mask_bits(&twice), &mask_bits(&parse(source)));
    }
    assert_eq!(
        case("inverse_rect_side").roi(),
        &parse("rectangle 0 0 20 48").inverse(WIDTH, HEIGHT).unwrap()
    );
    let hole = case("comp_hole").entry.roi;
    assert_covers("inverse_hole", hole.inverse(WIDTH, HEIGHT));
}

/// ImageJ inverts with `java.awt.geom.Area`, which cuts the edges of a
/// polygon where they cross the image border. A cut edge is rounded
/// anew, so a pixel whose centre lies exactly on it can end up in both
/// the selection and its inverse. Calipix keeps the edges whole and the
/// inverse is exactly what the selection leaves out.
#[test]
fn the_inverse_of_a_polygon_crossing_the_edge_differs_only_on_its_edges() {
    let source = parse("polygon -10,5 30,-8 70,30 20,60");
    let ours = mask_bits(&source.inverse(WIDTH, HEIGHT).unwrap());
    let theirs = case("inverse_poly_cross").mask;
    let original = case("poly_cross").mask;
    assert_mask("poly_cross", &mask_bits(&source), &original);
    let mut differing = 0;
    for i in 0..WIDTH * HEIGHT {
        assert_eq!(ours[i], !original[i]);
        if ours[i] != theirs[i] {
            // ImageJ selects the pixel in the polygon and in its inverse.
            assert!(theirs[i] && original[i]);
            differing += 1;
        }
    }
    assert_eq!(differing, 5);
}

#[test]
fn combined_areas_cover_what_imagej_s_cover() {
    use calipix_roi::Combine::{And, Not, Or, Xor};
    for (name, a, b, how) in [
        ("comp_or", "rectangle 5 5 20 15", "oval 15 10 25 25", Or),
        ("comp_hole", "rectangle 5 5 40 30", "oval 15 10 20 15", Not),
        ("comp_two", "rectangle 2 2 12 10", "rectangle 30 20 20 15", Or),
        ("comp_and", "rectangle 5 5 30 20", "oval 20 12 30 25", And),
        ("comp_xor", "rectangle 5 5 30 20", "oval 20 12 30 25", Xor),
        ("comp_combine", "rectangle 5 5 30 20", "oval 20 12 30 25", Or),
    ] {
        let ours = assert_covers(name, parse(a).combined(&parse(b), how));
        // Both sides traced pixel edges here, so the kinds agree too.
        assert_eq!(ours.kind(), case(name).roi().kind(), "{name}");
    }
}

#[test]
fn enlarged_selections_equal_imagej_s() {
    for (name, source, pixels) in [
        ("enlarge_poly_3", "polygon 5,5 30,8 18,20 32,40 4,30 12,18", 3),
        ("enlarge_poly_m2", "polygon 5,5 30,8 18,20 32,40 4,30 12,18", -2),
        ("enlarge_oval_4", "oval 20 10 15 20", 4),
        ("enlarge_rect_m3", "rectangle 10 8 20 12", -3),
        ("enlarge_polyf_5", "polygon 10.5,5.25 40.75,8.5 22.3,30.9", 5),
        ("enlarge_ellipse_m4", "ellipse 10 10 50 35 0.5", -4),
    ] {
        let ours = assert_covers(name, parse(source).enlarged(pixels));
        let theirs = case(name);
        // Not only the same pixels: the same selection, vertex for vertex.
        assert_eq!(&ours, theirs.roi(), "{name}");
        assert!(close(ours.length(), theirs.get("Perim")), "{name}: perimeter");
    }
    let hole = parse("rectangle 5 5 40 30")
        .combined(&parse("oval 15 10 20 15"), calipix_roi::Combine::Not)
        .unwrap();
    assert_covers("enlarge_hole_2", hole.enlarged(2));
    let enlarged = case("comp_hole").entry.roi.enlarged(2).unwrap();
    assert_eq!(&enlarged, case("enlarge_hole_2").roi());
}

#[test]
fn scaled_selections_equal_imagej_s() {
    let centre = |roi: &Roi| roi.scale_center(WIDTH, HEIGHT);
    for (name, source, x, y, centered) in [
        ("scale_poly", "polygon 40,5 60,10 50,40", 0.5, 0.75, false),
        ("scale_poly_centered", "polygon 40,5 60,10 50,40", 0.5, 0.75, true),
        ("scale_oval_centered", "oval 20 10 15 20", 1.5, 1.25, true),
        ("scale_rect", "rectangle 10 8 20 12", 1.5, 0.5, false),
        ("scale_line", "line 2 40 60 44", 0.5, 0.5, false),
        ("scale_line_centered", "line 2 40 60 44", 0.5, 0.5, true),
    ] {
        let source = parse(source);
        let scaled = source.scaled(x, y, if centered { centre(&source) } else { None });
        assert_same_shape(name, &scaled, case(name).roi());
        assert_covers(name, Some(scaled));
    }
    // Sources ImageJ made itself, read from its files so that only the
    // scaling is compared: a traced outline and a composite with a hole.
    let traced = case("comp_or").entry.roi;
    let scaled = traced.scaled(0.75, 0.5, None);
    assert_same_shape("scale_comp", &scaled, case("scale_comp").roi());
    assert_covers("scale_comp", Some(scaled));
    let scaled = traced.scaled(0.75, 0.5, centre(&traced));
    assert_same_shape("scale_comp_centered", &scaled, case("scale_comp_centered").roi());
    assert_covers("scale_comp_centered", Some(scaled));
    let hole = case("comp_hole").entry.roi;
    assert_covers("scale_hole", Some(hole.scaled(0.75, 0.5, None)));
    assert_covers("scale_hole_centered", Some(hole.scaled(0.75, 0.5, centre(&hole))));
}

#[test]
fn rotated_selections_equal_imagej_s() {
    for (name, source, angle) in [
        ("rotate_rect_30", "rectangle 20 14 24 12", 30.0),
        ("rotate_oval_m45", "oval 20 10 15 20", -45.0),
        ("rotate_poly_90", "polygon 40,5 60,10 50,40", 90.0),
        ("rotate_line_15", "line 2 40 60 44", 15.0),
        ("rotate_rotrect_20", "rotated-rectangle 10 10 50 35 12", 20.0),
        ("rotate_points_10", "points 5,5 20,30 40,12 60,40", 10.0),
    ] {
        let source = parse(source);
        let (xc, yc) = source.rotation_center();
        let rotated = source.rotated(angle, xc, yc);
        assert_same_shape(name, &rotated, case(name).roi());
        assert_covers(name, Some(rotated));
    }
    for (name, source) in [("rotate_comp_25", "comp_or"), ("rotate_hole_25", "comp_hole")] {
        let source = case(source).entry.roi;
        let (xc, yc) = source.rotation_center();
        assert_covers(name, Some(source.rotated(25.0, xc, yc)));
    }
}

#[test]
fn fitted_ellipses_hulls_and_boxes_equal_imagej_s() {
    let or = || {
        parse("rectangle 5 5 20 15")
            .combined(&parse("oval 15 10 25 25"), calipix_roi::Combine::Or)
            .unwrap()
    };
    for (name, source) in [
        ("fit_ellipse_poly", parse("polygon 5,5 30,8 18,20 32,40 4,30 12,18")),
        ("fit_ellipse_rect", parse("rectangle 10 8 20 12")),
        ("fit_ellipse_oval", parse("oval 10 10 40 13")),
        ("fit_ellipse_comp", or()),
    ] {
        let fitted = source.fitted_ellipse(WIDTH, HEIGHT);
        assert_same_shape(name, fitted.as_ref().unwrap(), case(name).roi());
        assert_covers(name, fitted);
    }
    let hole = parse("rectangle 5 5 40 30")
        .combined(&parse("oval 15 10 20 15"), calipix_roi::Combine::Not)
        .unwrap();
    for (name, source) in [
        ("hull_poly", parse("polygon 5,5 30,8 18,20 32,40 4,30 12,18")),
        ("hull_oval", parse("oval 20 10 15 20")),
        ("hull_points", parse("points 5,5 20,30 40,12 60,40 30,20")),
        ("hull_comp", hole),
    ] {
        let hull = source.convex_hull_roi();
        assert_same_shape(name, hull.as_ref().unwrap(), case(name).roi());
        assert_covers(name, hull);
    }
    assert_eq!(parse("line 1 2 3 4").convex_hull_roi(), None);
    for (name, source) in [
        ("box_oval_sub", "oval 20.5 10.25 15.3 20.6"),
        ("box_polyf", "polygon 10.5,5.25 40.75,8.5 22.3,30.9"),
        ("box_line", "line 2 40 60 44"),
    ] {
        let boxed = parse(source).bounding_box();
        assert_eq!(&boxed, case(name).roi(), "{name}");
        assert_covers(name, Some(boxed));
    }
}

#[test]
fn lines_become_the_areas_imagej_makes() {
    for (name, source) in [
        ("area_line_wide", "line 5 5 50 30 width 7"),
        ("area_line_thin", "line 2 40 60 44"),
        ("area_polyline_wide", "polyline 5,5 20,30 40,10 60,40 width 6"),
        ("area_polyline_thin", "polyline 5,5 20,30 40,10 60,40"),
        (
            "area_freeline_sub",
            "freeline 5.5,40.1 9.2,30.6 15.8,24.3 22.1,22.8 30.6,26.2 36.3,34.5 44.9,38.4 width 3",
        ),
    ] {
        let area = assert_covers(name, parse(source).to_area());
        let theirs = case(name);
        assert_same_shape(name, &area, theirs.roi());
        assert_eq!(area.int_bounds(), theirs.roi().int_bounds(), "{name}");
        assert!(close(area.length(), theirs.get("Perim")), "{name}: perimeter");
    }
    assert_eq!(parse("oval 1 2 3 4").to_area(), None);
}

#[test]
fn thresholded_shapes_trace_to_the_composite_imagej_makes() {
    // The same picture the macro thresholds.
    let mut bits = vec![false; WIDTH * HEIGHT];
    let mut paint = |roi: &str, value: bool| {
        for (bit, inside) in bits.iter_mut().zip(mask_bits(&parse(roi))) {
            if inside {
                *bit = value;
            }
        }
    };
    paint("oval 6 6 24 20", true);
    paint("rectangle 40 10 10 10", true);
    paint("rectangle 50 20 6 6", true);
    paint("rectangle 44 36 1 1", true);
    paint("rectangle 14 12 6 5", false);
    let c = case("comp_threshold");
    assert_eq!(bits, c.mask);
    assert_eq!(c.roi().kind(), Kind::Composite);
    // ImageJ's outline has four sub-paths: the oval, its hole, the two
    // squares that touch at a corner, and the single pixel.
    let Roi::Composite { path } = c.roi() else {
        unreachable!()
    };
    assert_eq!(path.polygons().len(), 4);
}

#[test]
fn stored_properties_are_read() {
    let stroke = case("props_stroke").entry.props;
    assert_eq!(stroke.name.as_deref(), Some("nucleus"));
    assert_eq!(stroke.stroke_color, Some(Color::rgb(255, 0, 0)));
    assert_eq!(stroke.stroke_width, 2.5);
    assert_eq!(stroke.fill_color, None);

    let fill = case("props_fill").entry.props;
    assert_eq!(fill.fill_color, Some(Color(0x4000_ff00)));
    assert_eq!(fill.fill_color.unwrap().rgba(), [0, 255, 0, 0x40]);

    let group = case("props_group").entry.props;
    assert_eq!(group.group, 7);
    assert_eq!(group.position, 2);
    assert_eq!(group.properties.as_deref(), Some("kind: test\n"));

    let line = case("props_line").entry.props;
    assert_eq!(line.stroke_color, Some(Color(0x8000_ffff)));
    assert_eq!((line.channel, line.slice, line.frame), (2, 3, 4));
    assert_eq!(line.position, 0);

    assert_eq!(case("line_wide").roi().line_width(), 7.0);
    assert_eq!(case("polyline_wide").roi().line_width(), 6.0);
    assert_eq!(case("rect_int").entry.props.name, None);
}

#[test]
fn thin_lines_are_drawn_as_imagej_draws_them() {
    let line = parse("line 2 40 60 44");
    assert_eq!(line.drawn_pixels().unwrap(), draw_line_pixels(2, 40, 60, 44));
    assert_eq!(draw_line_pixels(3, 3, 3, 3), [(3, 3)]);
    assert_eq!(draw_line_pixels(0, 0, 3, 1), [(0, 0), (1, 0), (2, 1), (3, 1)]);
    // Line.PointIterator visits each pixel once, end points included.
    let pixels = line.line_pixels().unwrap();
    assert_eq!(pixels.first(), Some(&(2, 40)));
    assert_eq!(pixels.last(), Some(&(60, 44)));
    assert_eq!(pixels.len(), 59);
    // ImageProcessor.getLine reads round(length) + 1 samples.
    let positions = line.profile_positions().unwrap();
    assert_eq!(positions.len(), 59);
    assert_eq!(positions[0], (2.0, 40.0));
    assert_eq!(parse("oval 1 2 3 4").line_pixels(), None);
}
