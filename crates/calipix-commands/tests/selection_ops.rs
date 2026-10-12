//! Commands run with a selection that is not a rectangle, against ImageJ
//! 1.54p (`fixtures/imagej/gen/82_selection_ops.ijm`):
//!
//! * filters and arithmetic change exactly the samples ImageJ changes, and
//!   only inside the selection;
//! * the flips and Translate act on the selection's bounding rectangle;
//! * with a line or points selected the whole image is processed;
//! * Crop, Duplicate, the quarter turns, Scale, Size, Canvas Size, the
//!   type conversions and the Image Calculator leave the selection ImageJ
//!   leaves.
//!
//! Every case runs through the executor: `select.set`, then the command.

#[path = "../../calipix-ops/tests/math_support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Executor, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::{Buffer, ColorModel, Image, Rect};
use calipix_roi::{Combine, Roi};
use support::{Difference, Raw, assert_matches, cropped_base, csv, raw_planes, report};

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

/// The source images of the macro: the base planes cropped to 31x23.
fn source(name: &str) -> Image {
    cropped_base(name, Rect::new(16, 12, 31, 23), Buffer::clone)
}

/// The selection the macro made, from the text in the table (`;` stands
/// for the commas of Calipix's text form).
fn selection(text: &str) -> Roi {
    let text = text.replace(';', ",");
    match text.split_once(" minus ") {
        Some((a, b)) => {
            let (a, b): (Roi, Roi) = (a.parse().unwrap(), b.parse().unwrap());
            a.combined(&b, Combine::Not).unwrap()
        }
        None => text.parse().unwrap_or_else(|e| panic!("{text}: {e}")),
    }
}

fn options(text: &str) -> BTreeMap<String, String> {
    text.split_whitespace()
        .map(|word| match word.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (word.to_string(), String::new()),
        })
        .collect()
}

/// The Calipix command and arguments for an ImageJ command and options.
fn translate(command: &str, opts: &BTreeMap<String, String>) -> (&'static str, Args) {
    let mut args = Args::new();
    let flag = |key: &str| opts.contains_key(key);
    let number = |args: &mut Args, key: &str, name: &str| {
        args.insert(name, opts[key].parse::<f64>().unwrap());
    };
    let integer = |args: &mut Args, key: &str, name: &str| {
        args.insert(name, opts[key].parse::<i64>().unwrap());
    };
    let method = |args: &mut Args| {
        args.insert(
            "interpolation",
            opts.get("interpolation").map_or("None", String::as_str),
        );
    };
    let id = match command {
        "Gaussian Blur..." => {
            number(&mut args, "sigma", "sigma");
            "filter.gaussian_blur"
        }
        "Median..." => {
            number(&mut args, "radius", "radius");
            "filter.median"
        }
        "Mean..." => {
            number(&mut args, "radius", "radius");
            "filter.mean"
        }
        "Unsharp Mask..." => {
            number(&mut args, "radius", "sigma");
            number(&mut args, "mask", "weight");
            "filter.unsharp_mask"
        }
        "Smooth" => "filter.smooth",
        "Find Edges" => "filter.find_edges",
        "Invert" => "image.invert",
        "Add..." => {
            number(&mut args, "value", "value");
            "math.add"
        }
        "Multiply..." => {
            number(&mut args, "value", "value");
            "math.multiply"
        }
        "Rotate... " => {
            number(&mut args, "angle", "angle");
            integer(&mut args, "grid", "grid");
            method(&mut args);
            "image.rotate"
        }
        "Flip Horizontally" => "image.flip_horizontal",
        "Flip Vertically" => "image.flip_vertical",
        "Translate..." => {
            number(&mut args, "x", "x");
            number(&mut args, "y", "y");
            method(&mut args);
            "image.translate"
        }
        "Crop" => "image.crop",
        "Duplicate..." => {
            args.insert("title", opts["title"].as_str());
            args.insert("ignore_selection", flag("ignore"));
            "image.duplicate"
        }
        "Rotate 90 Degrees Right" => "image.rotate_right",
        "Rotate 90 Degrees Left" => "image.rotate_left",
        "Scale..." => {
            number(&mut args, "x", "x_scale");
            number(&mut args, "y", "y_scale");
            method(&mut args);
            args.insert("average", flag("average"));
            args.insert("create", flag("create"));
            "image.scale"
        }
        "Size..." => {
            integer(&mut args, "width", "width");
            integer(&mut args, "height", "height");
            method(&mut args);
            args.insert("average", flag("average"));
            args.insert("constrain", flag("constrain"));
            "image.resize"
        }
        "Canvas Size..." => {
            integer(&mut args, "width", "width");
            integer(&mut args, "height", "height");
            args.insert("position", opts["position"].as_str());
            args.insert("zero", flag("zero"));
            "image.canvas_size"
        }
        "8-bit" => "image.to_8bit",
        "16-bit" => "image.to_16bit",
        "32-bit" => "image.to_32bit",
        "RGB Color" => "image.to_rgb",
        "calculator" => {
            let operation = if flag("add") { "Add" } else { "Subtract" };
            args.insert("operation", operation);
            args.insert("create", flag("create"));
            "math.image_calculator"
        }
        other => panic!("unknown command {other:?}"),
    };
    (id, args)
}

/// What ImageJ said of the selection left on the result.
struct Left {
    kind: i32,
    bounds: (i32, i32, i32, i32),
    /// Pixels covered and their centroid, for an area.
    area: Option<(f64, f64, f64)>,
    coords: Vec<(f64, f64)>,
}

impl Left {
    fn of(row: &[String]) -> Option<Left> {
        let kind: i32 = row[11].parse().unwrap();
        if kind < 0 {
            return None;
        }
        let n = |i: usize| row[i].parse::<f64>().unwrap();
        Some(Left {
            kind,
            bounds: (n(12) as i32, n(13) as i32, n(14) as i32, n(15) as i32),
            area: (row[16] != "NaN").then(|| (n(16), n(17), n(18))),
            coords: row[19]
                .split_whitespace()
                .map(|pair| {
                    let (x, y) = pair.split_once(':').unwrap();
                    (x.parse().unwrap(), y.parse().unwrap())
                })
                .collect(),
        })
    }
}

/// Checks the selection on the result against ImageJ's.
fn check_selection(what: &str, roi: Option<&Roi>, theirs: Option<&Left>, size: (usize, usize)) {
    let (Some(roi), Some(theirs)) = (roi, theirs) else {
        assert_eq!(
            roi.map(|r| r.to_string()),
            theirs.map(|t| format!("a selection of type {}", t.kind)),
            "{what}: selection"
        );
        return;
    };
    assert_eq!(roi.int_bounds(), theirs.bounds, "{what}: selection bounds");
    if let Some((area, cx, cy)) = theirs.area {
        // The pixels covered: their number and their centroid.
        let mask = roi
            .mask(size.0, size.1)
            .unwrap_or_else(|| panic!("{what}: the selection covers nothing"));
        let (mut n, mut sx, mut sy) = (0.0, 0.0, 0.0);
        for (i, _) in mask.data.iter().enumerate().filter(|(_, v)| **v != 0) {
            n += 1.0;
            sx += (mask.rect.x + i % mask.rect.w) as f64 + 0.5;
            sy += (mask.rect.y + i / mask.rect.w) as f64 + 0.5;
        }
        assert_eq!(n, area, "{what}: pixels in the selection");
        assert!(
            (sx / n - cx).abs() < 1e-5 && (sy / n - cy).abs() < 1e-5,
            "{what}: centroid ({}, {}), ImageJ ({cx}, {cy})",
            sx / n,
            sy / n
        );
    }
    // A selection Calipix made by combining masks follows pixel edges
    // where ImageJ's follows the original outlines; the two cover the same
    // pixels (checked above) but are not the same kind of outline.
    if theirs.kind == 9 {
        return;
    }
    assert_eq!(roi.ij_type(), theirs.kind, "{what}: kind of selection");
    let ours = roi.outline();
    assert_eq!(ours.len(), theirs.coords.len(), "{what}: vertices");
    for (o, t) in ours.iter().zip(&theirs.coords) {
        assert!(
            (o.0 - t.0).abs() < 1e-3 && (o.1 - t.1).abs() < 1e-3,
            "{what}: vertex {o:?}, ImageJ {t:?}"
        );
    }
}

#[test]
fn commands_with_shaped_selections_match_imagej() {
    let (exec, mut session) = setup();
    let mut by_op: BTreeMap<String, Difference> = BTreeMap::new();
    let mut expected: BTreeMap<String, Vec<Vec<Buffer>>> = BTreeMap::new();
    let mut cases = 0;
    for row in csv("selection_ops.csv") {
        let (group, first, count, name, selection_name, command) = (
            &row[0],
            row[1].parse::<usize>().unwrap(),
            row[2].parse::<usize>().unwrap(),
            row[3].as_str(),
            row[4].as_str(),
            row[6].as_str(),
        );
        let opts = options(&row[7]);
        let what = format!("{name} [{selection_name}] {command}{}", row[7]);
        let (command_id, mut args) = translate(command, &opts);
        let roi = selection(&row[5]);

        let original = source(name);
        let id = session.workspace.insert(original.clone(), None);
        exec.run(
            &mut session,
            "select.set",
            &Args::new().with("selection", roi.clone()),
        )
        .unwrap_or_else(|e| panic!("{what}: select.set: {e}"));
        if command == "calculator" {
            // The second operand: the same picture flipped.
            let second = session.workspace.insert(original.clone(), None);
            exec.run(
                &mut session,
                "image.flip_horizontal",
                &Args::new().with("input", second),
            )
            .unwrap();
            session.workspace.set_active(id).unwrap();
            args.insert("image2", second);
        }
        args.insert("input", id);

        let outputs = exec
            .run(&mut session, command_id, &args)
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        let result_id = match outputs.get("image") {
            Some(&Value::Image(new)) => new,
            _ => id,
        };

        // Shape, pixel type and samples.
        let result = session.workspace.image(result_id).unwrap().clone();
        let meta = result.meta();
        let (width, height) = (
            row[8].parse::<usize>().unwrap(),
            row[9].parse::<usize>().unwrap(),
        );
        let kind = Raw::from_bit_depth(&row[10]);
        let rgb = kind == Raw::Rgb;
        assert_eq!((meta.width(), meta.height()), (width, height), "{what}");
        assert_eq!(meta.color == ColorModel::Rgb, rgb, "{what}");
        let step = if rgb { 3 } else { 1 };
        assert_eq!(meta.plane_count(), count * step, "{what}: planes");
        let theirs = expected
            .entry(group.clone())
            .or_insert_with(|| raw_planes(&format!("{group}.raw"), kind, width, height));
        for k in 0..count {
            for (c, theirs) in theirs[first - 1 + k].iter().enumerate() {
                let ours = result.read_plane(k * step + c).unwrap();
                let difference = assert_matches(&ours, theirs, &what);
                by_op
                    .entry(format!(
                        "{} {} [{selection_name}]",
                        kind.name(),
                        command.trim().trim_end_matches("...")
                    ))
                    .or_default()
                    .merge(difference);
            }
        }

        // The selection left on the result.
        let left = session.workspace.get(result_id).unwrap().roi.clone();
        check_selection(
            &what,
            left.as_ref(),
            Left::of(&row).as_ref(),
            (width, height),
        );

        let ids: Vec<DatasetId> = session.workspace.datasets().map(|d| d.id).collect();
        for id in ids {
            session.workspace.remove(id).unwrap();
        }
        cases += 1;
    }
    for (operation, difference) in &by_op {
        report(operation, *difference);
    }
    println!("selection_ops.csv: {cases} cases");
    assert_eq!(cases, 221);
}

#[test]
fn selections_that_cover_nothing_say_so() {
    let (exec, mut session) = setup();
    let image = Image::from_vec(8, 6, (0u8..48).collect()).unwrap();
    let id = session.workspace.insert(image.clone(), None);
    let samples = |session: &Session| {
        session
            .workspace
            .image(id)
            .unwrap()
            .read_plane(0)
            .unwrap()
            .as_slice::<u8>()
            .unwrap()
            .to_vec()
    };
    let before = samples(&session);

    // A triangle inside one pixel, away from its centre: it has bounds but
    // covers no pixel.
    let sliver: Roi = "polygon 2.1,2.1 2.4,2.1 2.2,2.3".parse().unwrap();
    assert!(sliver.mask(8, 6).is_none() && sliver.pixel_bounds(8, 6).is_some());
    session.workspace.set_roi(id, Some(sliver)).unwrap();
    for command in ["math.add", "filter.gaussian_blur", "measure.measure"] {
        let args = match command {
            "math.add" => Args::new().with("value", 5.0),
            _ => Args::new(),
        };
        let err = exec.run(&mut session, command, &args).unwrap_err();
        assert!(
            err.to_string().contains("covers no pixel"),
            "{command}: {err}"
        );
        assert!(!err.to_string().contains("outside"), "{command}: {err}");
    }
    assert_eq!(samples(&session), before);
    // The flips act on the bounding rectangle, which here is one pixel.
    exec.run(&mut session, "image.flip_horizontal", &Args::new())
        .unwrap();
    assert_eq!(samples(&session), before);

    // A selection beyond the image is said to be outside it.
    session
        .workspace
        .set_roi(id, Some(Roi::oval(20.0, 20.0, 5.0, 5.0)))
        .unwrap();
    for command in ["image.invert", "image.flip_vertical", "measure.measure"] {
        let err = exec.run(&mut session, command, &Args::new()).unwrap_err();
        assert!(
            err.to_string().contains("outside the image"),
            "{command}: {err}"
        );
    }

    // A thin line is not an area: it confines nothing, as in ImageJ.
    session
        .workspace
        .set_roi(id, Some(Roi::line(1.0, 1.0, 6.0, 4.0)))
        .unwrap();
    exec.run(&mut session, "image.invert", &Args::new())
        .unwrap();
    let inverted: Vec<u8> = before.iter().map(|v| 255 - v).collect();
    assert_eq!(samples(&session), inverted);
    // Neither does a wide line, although it covers pixels.
    session
        .workspace
        .set_roi(id, Some(Roi::line(1.0, 1.0, 6.0, 4.0).with_line_width(3.0)))
        .unwrap();
    exec.run(&mut session, "image.invert", &Args::new())
        .unwrap();
    assert_eq!(samples(&session), before);
}
