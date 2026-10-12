//! Measurements compared with ImageJ 1.54p.
//!
//! `fixtures/imagej/out/measure_cases.txt` holds, at full precision, what
//! ImageJ measures on the base images with every measurement on, for no
//! selection, rectangles, ovals, polygons, a freehand outline, lines and
//! points (written by `fixtures/imagej/gen/70_measure.ijm`). For the
//! shapes `calipix-roi` does not have yet, the pixels they cover come from
//! masks ImageJ saved and their outlines from the vertices it listed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use calipix_core::{AxisKind, Buffer, Cell, ColorModel, Image, Rect};
use calipix_ops::measure::{
    Calibration, MeasureSettings, Measured, Measurements, Outline, PlaneContext, Region, Row,
    Samples, measure,
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

/// One block of `measure_cases.txt`.
#[derive(Debug, Default)]
struct Case {
    name: String,
    /// The `@key=value` lines.
    about: BTreeMap<String, String>,
    /// Heading and ImageJ's value.
    expected: Vec<(String, f64)>,
}

impl Case {
    fn about(&self, key: &str) -> Option<&str> {
        self.about.get(key).map(String::as_str)
    }

    fn numbers(&self, key: &str) -> Vec<f64> {
        self.about(key)
            .unwrap_or_else(|| panic!("{}: no @{key}", self.name))
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect()
    }

    fn expected(&self, heading: &str) -> f64 {
        self.expected
            .iter()
            .find(|(h, _)| h == heading)
            .unwrap_or_else(|| panic!("{}: no {heading}", self.name))
            .1
    }
}

fn parse_java_double(text: &str) -> f64 {
    match text {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        other => other
            .parse()
            .unwrap_or_else(|_| panic!("not a number: {other}")),
    }
}

fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(fixture("measure_cases.txt")).unwrap();
    let mut cases: Vec<Case> = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        if let Some(name) = line.strip_prefix("# ") {
            cases.push(Case {
                name: name.to_string(),
                ..Case::default()
            });
            continue;
        }
        let case = cases.last_mut().unwrap();
        let (key, value) = line.split_once('=').unwrap();
        match key.strip_prefix('@') {
            Some(key) => {
                case.about.insert(key.to_string(), value.to_string());
            }
            None => case
                .expected
                .push((key.to_string(), parse_java_double(value))),
        }
    }
    cases
}

/// The planes ImageJ would hold in one processor at a position: one
/// plane, or the three of an RGB image.
struct Plane {
    buffers: Vec<Buffer>,
    width: usize,
    height: usize,
}

impl Plane {
    fn read(image: &Image, channel: usize, slice: usize, frame: usize) -> Plane {
        let meta = image.meta();
        let at = |c: usize| {
            let position: Vec<usize> = meta
                .axes()
                .iter()
                .skip(2)
                .map(|axis| match axis.kind {
                    AxisKind::Channel => c,
                    AxisKind::Z => slice - 1,
                    AxisKind::Time => frame - 1,
                    _ => 0,
                })
                .collect();
            image
                .read_plane(meta.plane_index(&position).unwrap())
                .unwrap()
        };
        let buffers = if meta.color == ColorModel::Rgb {
            vec![at(0), at(1), at(2)]
        } else {
            vec![at(channel - 1)]
        };
        Plane {
            buffers,
            width: meta.width(),
            height: meta.height(),
        }
    }

    fn samples(&self) -> Samples<'_> {
        match self.buffers.as_slice() {
            [gray] => Samples::Gray(gray),
            [red, green, blue] => Samples::Rgb { red, green, blue },
            _ => unreachable!(),
        }
    }
}

/// The part of a mask image inside `rect`, as a selection's mask.
fn mask_within(name: &str, rect: Rect) -> Vec<u8> {
    let image = calipix_io::open(fixture(name)).unwrap();
    let width = image.meta().width();
    let plane = image.read_plane(0).unwrap();
    let pixels = plane.as_slice::<u8>().unwrap();
    // Nothing outside the bounding rectangle may be selected.
    for (i, &v) in pixels.iter().enumerate() {
        let (x, y) = (i % width, i / width);
        let inside = x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
        assert!(inside || v == 0, "{name}: ({x}, {y}) outside {rect:?}");
    }
    let mut mask = Vec::with_capacity(rect.w * rect.h);
    for y in rect.y..rect.y + rect.h {
        mask.extend_from_slice(&pixels[y * width + rect.x..y * width + rect.x + rect.w]);
    }
    mask
}

/// Headings whose values are counts or sample values, which must match
/// exactly when ImageJ's value is a whole number.
const EXACT: [&str; 13] = [
    "Area",
    "Min",
    "Max",
    "Mode",
    "Median",
    "BX",
    "BY",
    "Width",
    "Height",
    "Ch",
    "Slice",
    "Frame",
    "RawIntDen",
];

fn close(heading: &str, ours: f64, theirs: f64) -> bool {
    if theirs.is_nan() {
        return ours.is_nan();
    }
    if theirs.is_infinite() || (EXACT.contains(&heading) && theirs.fract() == 0.0) {
        return ours == theirs;
    }
    let scale = theirs.abs().max(1e-300);
    // Values that are zero up to rounding (a centred shape's skewness, an
    // angle of a symmetric shape) cannot be held to a relative error.
    (ours - theirs).abs() <= 1e-5 * scale || (ours - theirs).abs() < 1e-9
}

struct Outcome {
    rows: Vec<Row>,
    /// True if every expected heading must be present and no others.
    complete: bool,
}

fn run(case: &Case) -> Outcome {
    let image = calipix_io::open(fixture(case.about("image").unwrap())).unwrap();
    let meta = image.meta();
    let position = case.about("position").map(|_| case.numbers("position"));
    let (channel, slice, frame) = match &position {
        Some(p) => (p[0] as usize, p[1] as usize, p[2] as usize),
        None => (1, 1, 1),
    };
    let plane = Plane::read(&image, channel, slice, frame);

    let mut cal = Calibration::from_meta(meta);
    if let Some(text) = case.about("cal") {
        let parts: Vec<&str> = text.split_whitespace().collect();
        let n = |i: usize| parts[i].parse::<f64>().unwrap();
        cal = Calibration {
            pixel_width: n(0),
            pixel_height: n(1),
            unit: Some(parts[2].to_string()),
            x_origin: n(3),
            y_origin: n(4),
            invert_y: false,
        };
    }

    let (channels, slices, frames) = (
        meta.size(&AxisKind::Channel),
        meta.size(&AxisKind::Z),
        meta.size(&AxisKind::Time),
    );
    let context = PlaneContext {
        label: meta.name.clone(),
        channel,
        slice,
        frame,
        channels,
        slices,
        frames,
        hyperstack: meta.color != ColorModel::Rgb && channels * slices * frames > 1,
        stack_index: 1,
    };
    let settings = MeasureSettings {
        measurements: Measurements::all(),
        invert_y: case.about("invert_y") == Some("1"),
        ..MeasureSettings::default()
    };

    let roi: Vec<&str> = case.about("roi").unwrap().split_whitespace().collect();
    let args: Vec<f64> = roi[1..].iter().map(|n| n.parse().unwrap()).collect();
    let bounds = || {
        let b = case.numbers("bounds");
        Rect::new(b[0] as usize, b[1] as usize, b[2] as usize, b[3] as usize)
    };
    let coords = || -> Vec<(f32, f32)> {
        case.about("coords")
            .unwrap()
            .split_whitespace()
            .map(|pair| {
                let (x, y) = pair.split_once(',').unwrap();
                (
                    x.parse::<f64>().unwrap() as f32,
                    y.parse::<f64>().unwrap() as f32,
                )
            })
            .collect()
    };
    let mask;
    let (region, outline, complete) = match roi[0] {
        "none" => (
            Region::whole(plane.width, plane.height),
            Outline::whole(plane.width, plane.height),
            true,
        ),
        "rect" => {
            let rect = Rect::new(
                args[0] as usize,
                args[1] as usize,
                args[2] as usize,
                args[3] as usize,
            );
            (
                Region::rect(rect),
                Outline::rectangle(args[0], args[1], args[2], args[3], &cal),
                true,
            )
        }
        "oval" | "polygon" | "freehand" => {
            let rect = bounds();
            mask = mask_within(case.about("mask").unwrap(), rect);
            let region = Region {
                rect,
                mask: Some(&mask),
            };
            let outline = if roi[0] == "oval" {
                Outline::oval(args[0], args[1], args[2], args[3], &coords(), &cal)
            } else {
                // The perimeter of a polygon or freehand outline is the
                // selection's own business; ImageJ's value is passed on.
                Outline::polygon(
                    &coords(),
                    case.expected("Perim."),
                    (rect.x as f64, rect.y as f64, rect.w as f64, rect.h as f64),
                    &cal,
                )
            };
            (region, outline, true)
        }
        "line" => (
            Region::whole(plane.width, plane.height),
            Outline::line(args[0], args[1], args[2], args[3], &cal),
            false,
        ),
        "point" => (
            Region::whole(plane.width, plane.height),
            Outline::point(args[0], args[1]),
            false,
        ),
        other => panic!("{}: unknown selection {other}", case.name),
    };
    let measured = Measured {
        samples: plane.samples(),
        width: plane.width,
        height: plane.height,
        calibration: &cal,
        context: &context,
    };
    let rows = measure(measured, region, &outline, &settings)
        .unwrap_or_else(|e| panic!("{}: {e}", case.name));
    Outcome { rows, complete }
}

#[test]
fn measurements_match_imagej() {
    let cases = cases();
    assert!(cases.len() > 250, "only {} cases", cases.len());
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for case in &cases {
        let outcome = run(case);
        assert_eq!(outcome.rows.len(), 1, "{}", case.name);
        let row = &outcome.rows[0];
        let kind = case
            .about("roi")
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .to_string();
        *kinds.entry(kind).or_default() += 1;
        for (heading, theirs) in &case.expected {
            let ours = match row.get(heading) {
                Some(Cell::Number(v)) => *v,
                // getValue gives NaN for a column the measurement lacks.
                None if !outcome.complete && theirs.is_nan() => continue,
                other => {
                    failures.push(format!(
                        "{} {heading}: {other:?}, ImageJ {theirs}",
                        case.name
                    ));
                    continue;
                }
            };
            compared += 1;
            if !close(heading, ours, *theirs) {
                failures.push(format!("{} {heading}: {ours}, ImageJ {theirs}", case.name));
            }
        }
        if outcome.complete {
            for (heading, _) in row.iter() {
                if !case.expected.iter().any(|(h, _)| h == heading) {
                    failures.push(format!("{} has {heading}, ImageJ does not", case.name));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {compared} values differ:\n{}",
        failures.len(),
        failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    eprintln!(
        "compared {compared} values in {} cases: {kinds:?}",
        cases.len()
    );
    // Every kind of selection is represented.
    for kind in [
        "none", "rect", "oval", "polygon", "freehand", "line", "point",
    ] {
        assert!(
            kinds.get(kind).copied().unwrap_or(0) >= 5,
            "{kind}: {kinds:?}"
        );
    }
    assert!(compared > 9000, "only {compared} values compared");
}
