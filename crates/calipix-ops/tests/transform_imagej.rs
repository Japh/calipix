//! Geometric transforms of single planes against ImageJ 1.54p
//! (`fixtures/imagej/gen/63_geometry.ijm` and `64_rotate_scale.ijm`).
//! Stacks, hyperstacks, Crop and Duplicate are rules of the commands and
//! are checked in `calipix-commands`.

#[path = "math_support/mod.rs"]
mod support;

use std::collections::BTreeMap;

use calipix_core::{Buffer, Rect};
use calipix_ops::transform::{
    Interpolation, canvas, flip_horizontal, flip_vertical, resize, rotate, rotate_left,
    rotate_right, scale_in_place, translate,
};
use support::{Difference, Raw, assert_matches, base, crop, csv, raw_planes, report, with_samples};

/// A source image of the macros: its channel planes, size and the display
/// range it was saved with.
struct Source {
    planes: Vec<Buffer>,
    width: usize,
    height: usize,
    range: Option<(f64, f64)>,
}

fn source(name: &str) -> Option<Source> {
    let (file, rect) = match name {
        "gray8" | "gray16" | "gray32f" | "rgb" => (name, Rect::new(16, 12, 31, 23)),
        "row" => ("gray8", Rect::new(16, 12, 31, 1)),
        _ => return None,
    };
    let image = base(file);
    let meta = image.meta();
    let planes = (0..meta.plane_count())
        .map(|p| crop(&image.read_plane(p).unwrap(), meta.width(), rect))
        .map(|plane| match name {
            "gray32f" => with_samples(&plane, &[(31 + 2, f32::NAN)]),
            _ => plane,
        })
        .collect();
    Some(Source {
        planes,
        width: rect.w,
        height: rect.h,
        range: meta.channels.first().and_then(|c| c.display_range),
    })
}

/// `key=value` and bare words of an ImageJ options string.
fn options(text: &str) -> BTreeMap<String, String> {
    text.split_whitespace()
        .map(|word| match word.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (word.to_string(), String::new()),
        })
        .collect()
}

fn number(options: &BTreeMap<String, String>, key: &str) -> Option<f64> {
    options.get(key).and_then(|v| v.parse().ok())
}

fn interpolation(options: &BTreeMap<String, String>) -> Interpolation {
    // A macro that names no method gets none (Rotator.showDialog, Scaler.showDialog).
    options
        .get("interpolation")
        .map_or(Interpolation::None, |name| {
            Interpolation::from_name(name).expect("interpolation")
        })
}

/// Math.round.
fn round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

/// The canvas ImageJ's Rotate makes with "Enlarge image": the bounding box
/// of the rotated outline (`Rotator.enlargeCanvas`, `RoiRotator.rotate`).
fn enlarged(width: usize, height: usize, angle: f64) -> (usize, usize) {
    let (w, h) = (width as f64, height as f64);
    let (xc, yc) = (w / 2.0, h / 2.0);
    let theta = -angle * std::f64::consts::PI / 180.0;
    let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y): (f64, f64)| {
        let (dx, dy) = (x - xc, yc - y);
        let radius = (dx * dx + dy * dy).sqrt();
        let a = dy.atan2(dx);
        (
            (xc + radius * (a + theta).cos()) as f32,
            (yc - radius * (a + theta).sin()) as f32,
        )
    });
    let span = |values: [f32; 4]| {
        let min = values.iter().copied().fold(f32::MAX, f32::min);
        let max = values.iter().copied().fold(f32::MIN_POSITIVE, f32::max);
        round((max - min) as f64) as usize
    };
    (
        span(corners.map(|c| c.0)).max(width),
        span(corners.map(|c| c.1)).max(height),
    )
}

/// What one plane of the source becomes, or `None` for a command this test
/// leaves to the command tests.
fn transform(
    plane: &Buffer,
    src: &Source,
    range: Option<(f64, f64)>,
    selection: Option<Rect>,
    command: &str,
    opts: &BTreeMap<String, String>,
) -> Option<Buffer> {
    let (w, h) = (src.width, src.height);
    let rect = selection.unwrap_or(Rect::new(0, 0, w, h));
    let method = interpolation(opts);
    // ImageJ's "white" for images that are not 8-bit is the top of the
    // display range.
    let display = range.unwrap_or((0.0, 255.0));
    let is_byte = plane.as_slice::<u8>().is_some();
    Some(match command {
        "Flip Horizontally" => flip_horizontal(plane, w, rect).unwrap(),
        "Flip Vertically" => flip_vertical(plane, w, rect).unwrap(),
        "Rotate 90 Degrees Right" => rotate_right(plane, w).unwrap(),
        "Rotate 90 Degrees Left" => rotate_left(plane, w).unwrap(),
        "Canvas Size..." => {
            let new_w = number(opts, "width").unwrap() as usize;
            let new_h = number(opts, "height").unwrap() as usize;
            let (xc, xr) = ((new_w as i64 - w as i64) / 2, new_w as i64 - w as i64);
            let (yc, yb) = ((new_h as i64 - h as i64) / 2, new_h as i64 - h as i64);
            let (x, y) = match opts["position"].as_str() {
                "Top-Left" => (0, 0),
                "Top-Center" => (xc, 0),
                "Top-Right" => (xr, 0),
                "Center-Left" => (0, yc),
                "Center" => (xc, yc),
                "Center-Right" => (xr, yc),
                "Bottom-Left" => (0, yb),
                "Bottom-Center" => (xc, yb),
                "Bottom-Right" => (xr, yb),
                other => panic!("position {other}"),
            };
            let fill = if opts.contains_key("zero") {
                0.0
            } else if is_byte {
                255.0
            } else {
                display.1
            };
            canvas(plane, w, new_w, new_h, x, y, fill).unwrap()
        }
        "Translate..." => translate(
            plane,
            w,
            rect,
            number(opts, "x").unwrap(),
            number(opts, "y").unwrap(),
            method,
        )
        .unwrap(),
        "Rotate... " => {
            let angle = number(opts, "angle").unwrap();
            if opts.contains_key("enlarge") {
                let (new_w, new_h) = enlarged(w, h, angle);
                let (x, y) = ((new_w - w) as i64 / 2, (new_h - h) as i64 / 2);
                let larger = canvas(plane, w, new_w, new_h, x, y, 0.0).unwrap();
                let whole = Rect::new(0, 0, new_w, new_h);
                rotate(&larger, new_w, whole, angle, method, 0.0).unwrap()
            } else {
                rotate(plane, w, rect, angle, method, 0.0).unwrap()
            }
        }
        "Scale..." => {
            let (mut xs, mut ys) = (number(opts, "x"), number(opts, "y"));
            let (new_w, new_h) = match (xs, ys) {
                (Some(x), Some(y)) => (
                    round(rect.w as f64 * x) as usize,
                    round(rect.h as f64 * y) as usize,
                ),
                _ => {
                    let new_w = number(opts, "width").unwrap() as usize;
                    let new_h = number(opts, "height").unwrap() as usize;
                    xs = Some(new_w as f64 / rect.w as f64);
                    ys = Some(new_h as f64 / rect.h as f64);
                    (new_w, new_h)
                }
            };
            if opts.contains_key("create") {
                resize(
                    plane,
                    w,
                    rect,
                    new_w,
                    new_h,
                    method,
                    opts.contains_key("average"),
                )
                .unwrap()
            } else {
                // ByteProcessor fills with its background value, which
                // Scaler sets to 0; the others fill with the display minimum.
                let background = if is_byte { 0.0 } else { display.0 };
                let method = if w > 1 && h > 1 {
                    method
                } else {
                    Interpolation::None
                };
                scale_in_place(plane, w, rect, xs.unwrap(), ys.unwrap(), method, background)
                    .unwrap()
            }
        }
        "Size..." => {
            let new_w = number(opts, "width").unwrap() as usize;
            let mut new_h = number(opts, "height").unwrap() as usize;
            if opts.contains_key("constrain") {
                new_h = round(new_w as f64 * (rect.h as f64 / rect.w as f64)) as usize;
            }
            resize(
                plane,
                w,
                rect,
                new_w,
                new_h,
                method,
                opts.contains_key("average"),
            )
            .unwrap()
        }
        _ => return None,
    })
}

fn check(table: &str, expected_cases: usize) {
    let mut by_op: BTreeMap<String, Difference> = BTreeMap::new();
    let mut cases = 0;
    let mut groups: BTreeMap<String, Vec<Vec<Buffer>>> = BTreeMap::new();
    for row in csv(table) {
        let (group, slice, name, setup, command, opts) = (
            &row[0],
            row[1].parse::<usize>().unwrap(),
            &row[3],
            &row[4],
            &row[5],
            options(&row[6]),
        );
        let Some(src) = source(name) else { continue };
        let mut selection = None;
        let mut range = src.range;
        for word in setup.split(' ') {
            if let Some(sel) = word.strip_prefix("sel=") {
                let v: Vec<i64> = sel.split(':').map(|n| n.parse().unwrap()).collect();
                // Only selections inside the image are used here.
                if v[0] >= 0 && v[1] >= 0 {
                    selection = Some(Rect::new(
                        v[0] as usize,
                        v[1] as usize,
                        v[2] as usize,
                        v[3] as usize,
                    ));
                }
            } else if let Some(r) = word.strip_prefix("range=") {
                let (lo, hi) = r.split_once(':').unwrap();
                range = Some((lo.parse().unwrap(), hi.parse().unwrap()));
            }
        }
        let ours: Option<Vec<Buffer>> = src
            .planes
            .iter()
            .map(|plane| transform(plane, &src, range, selection, command, &opts))
            .collect();
        let Some(ours) = ours else { continue };

        let (width, height) = (
            row[7].parse::<usize>().unwrap(),
            row[8].parse::<usize>().unwrap(),
        );
        let expected = groups.entry(group.clone()).or_insert_with(|| {
            raw_planes(
                &format!("{group}.raw"),
                Raw::from_bit_depth(&row[9]),
                width,
                height,
            )
        });
        let what = format!("{name} [{setup}] {command} {}", row[6]);
        let interpolation = opts.get("interpolation").map_or("None", String::as_str);
        for (ours, theirs) in ours.iter().zip(&expected[slice - 1]) {
            let difference = assert_matches(ours, theirs, &what);
            by_op
                .entry(format!(
                    "{} {} ({interpolation})",
                    Raw::from_bit_depth(&row[9]).name(),
                    command.trim().trim_end_matches("...")
                ))
                .or_default()
                .merge(difference);
        }
        cases += 1;
    }
    for (operation, difference) in &by_op {
        report(operation, *difference);
    }
    assert_eq!(cases, expected_cases, "{table}");
}

#[test]
fn moving_samples_matches_imagej() {
    check("geometry.csv", 48);
}

#[test]
fn interpolating_transforms_match_imagej() {
    check("rotate_scale.csv", 157);
}
