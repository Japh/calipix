//! The geometry commands against ImageJ 1.54p
//! (`fixtures/imagej/gen/63_geometry.ijm`, `64_rotate_scale.ijm`): samples,
//! size, dimensions, display range, calibration and selection of every
//! case, run through the executor.

#[path = "../../calipix-ops/tests/math_support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Executor, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::{AxisKind, Buffer, ColorModel, DType, Image, Rect};
use calipix_roi::Roi;
use support::{
    Difference, Raw, assert_matches, base, close, cropped_base, csv, ij_number, raw_planes, report,
    with_samples,
};

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

/// A source image of the two macros.
fn source(name: &str) -> Image {
    let axis = |image: &Image, kind: AxisKind| image.meta().axis_index(&kind).unwrap();
    match name {
        "stack" => base("stack_z6_8bit"),
        "hyper" => {
            let image = base("hyperstack_c3z4t5");
            let image = image.range(axis(&image, AxisKind::Z), 0, 2).unwrap();
            let image = image.range(axis(&image, AxisKind::Time), 1, 2).unwrap();
            image.materialize().unwrap()
        }
        "row" => cropped_base("gray8", Rect::new(16, 12, 31, 1), Buffer::clone),
        "gray32f" => cropped_base(name, Rect::new(16, 12, 31, 23), |plane| {
            with_samples(plane, &[(31 + 2, f32::NAN)])
        }),
        _ => cropped_base(name, Rect::new(16, 12, 31, 23), Buffer::clone),
    }
}

/// `key=value` pairs and bare words of an ImageJ options string.
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
        if let Some(value) = opts.get(key).and_then(|v| v.parse::<f64>().ok()) {
            args.insert(name, value);
        }
    };
    let integer = |args: &mut Args, key: &str, name: &str| {
        if let Some(value) = opts.get(key).and_then(|v| v.parse::<i64>().ok()) {
            args.insert(name, value);
        }
    };
    let text = |args: &mut Args, key: &str, name: &str| {
        if let Some(value) = opts.get(key) {
            args.insert(name, value.as_str());
        }
    };
    // A macro that names no interpolation gets none.
    let method = |args: &mut Args| {
        let name = opts.get("interpolation").map_or("None", String::as_str);
        args.insert("interpolation", name);
    };
    let id = match command {
        "Flip Horizontally" => "image.flip_horizontal",
        "Flip Vertically" => "image.flip_vertical",
        "Rotate 90 Degrees Right" => "image.rotate_right",
        "Rotate 90 Degrees Left" => "image.rotate_left",
        "Flip Z" => "image.flip_z",
        "Crop" => "image.crop",
        "Duplicate..." => {
            args.insert("ignore_selection", flag("ignore"));
            args.insert("stack", flag("duplicate"));
            for key in ["title", "range", "channels", "slices", "frames"] {
                text(&mut args, key, key);
            }
            "image.duplicate"
        }
        "Canvas Size..." => {
            integer(&mut args, "width", "width");
            integer(&mut args, "height", "height");
            text(&mut args, "position", "position");
            args.insert("zero", flag("zero"));
            "image.canvas_size"
        }
        "Translate..." => {
            number(&mut args, "x", "x");
            number(&mut args, "y", "y");
            method(&mut args);
            "image.translate"
        }
        "Rotate... " => {
            number(&mut args, "angle", "angle");
            integer(&mut args, "grid", "grid");
            method(&mut args);
            args.insert("enlarge", flag("enlarge"));
            "image.rotate"
        }
        "Scale..." => {
            number(&mut args, "x", "x_scale");
            number(&mut args, "y", "y_scale");
            number(&mut args, "z", "z_scale");
            integer(&mut args, "width", "width");
            integer(&mut args, "height", "height");
            integer(&mut args, "depth", "depth");
            method(&mut args);
            args.insert("average", flag("average"));
            args.insert("process", flag("process"));
            args.insert("create", flag("create"));
            "image.scale"
        }
        "Size..." => {
            integer(&mut args, "width", "width");
            integer(&mut args, "height", "height");
            integer(&mut args, "depth", "depth");
            integer(&mut args, "time", "frames");
            method(&mut args);
            args.insert("average", flag("average"));
            args.insert("constrain", flag("constrain"));
            "image.resize"
        }
        other => panic!("unknown command {other:?}"),
    };
    if flag("stack") && command != "Duplicate..." {
        args.insert("stack", true);
    }
    (id, args)
}

/// ImageJ's `xOrigin`: the pixel index of physical zero.
fn pixel_origin(image: &Image, axis: usize) -> f64 {
    let axis = &image.meta().axes()[axis];
    if axis.origin == 0.0 {
        0.0
    } else {
        -axis.origin / axis.scale
    }
}

fn check(table: &str, expected_cases: usize) {
    let (exec, mut session) = setup();
    let mut by_op: BTreeMap<String, Difference> = BTreeMap::new();
    let mut expected: BTreeMap<String, Vec<Vec<Buffer>>> = BTreeMap::new();
    let mut cases = 0;
    for row in csv(table) {
        let (group, first, count, name, setup_words, command) = (
            &row[0],
            row[1].parse::<usize>().unwrap(),
            row[2].parse::<usize>().unwrap(),
            row[3].as_str(),
            row[4].as_str(),
            row[5].as_str(),
        );
        let opts = options(&row[6]);
        let what = format!("{name} [{setup_words}] {command}{}", row[6]);
        let (command_id, mut args) = translate(command, &opts);
        // The plane on view, for the commands that ask which it is.
        let spec = exec.registry().get(command_id).unwrap().spec().clone();
        let on_view = |args: &mut Args, name: &str, value: i64| {
            if spec.param(name).is_some() {
                args.insert(name, value);
            }
        };

        // The source, with the setup the macro gives it.
        let mut image = source(name);
        let mut selection = None;
        let (mut channel_on_view, mut slice_on_view) = (1, 1);
        let mut calibrated = false;
        for word in setup_words.split(' ') {
            if let Some(sel) = word.strip_prefix("sel=") {
                let v: Vec<f64> = sel.split(':').map(ij_number).collect();
                selection = Some(Roi::rectangle(v[0], v[1], v[2], v[3]));
            } else if let Some(range) = word.strip_prefix("range=") {
                let (lo, hi) = range.split_once(':').unwrap();
                let mut meta = image.meta().clone();
                meta.channels = vec![Default::default(); meta.size(&AxisKind::Channel)];
                for channel in &mut meta.channels {
                    channel.display_range = Some((ij_number(lo), ij_number(hi)));
                }
                image = image.with_meta(meta).unwrap();
            } else if let Some(slice) = word.strip_prefix("slice=") {
                slice_on_view = slice.parse().unwrap();
                on_view(&mut args, "slice", slice_on_view as i64);
            } else if let Some(position) = word.strip_prefix("pos=") {
                let v: Vec<i64> = position.split(':').map(|n| n.parse().unwrap()).collect();
                channel_on_view = v[0] as usize;
                slice_on_view = v[1] as usize;
                // ImageJ duplicates all the channels of a composite image
                // on view; other commands act on the current channel.
                if command != "Duplicate..." {
                    on_view(&mut args, "channel", v[0]);
                }
                on_view(&mut args, "slice", v[1]);
                on_view(&mut args, "frame", v[2]);
            } else if word == "cal" {
                calibrated = true;
                let mut meta = image.meta().clone();
                for (axis, scale, origin) in [(0, 0.5, 3.0), (1, 0.25, 5.0)] {
                    let axis = meta.axis_mut(axis).unwrap();
                    axis.scale = scale;
                    axis.unit = Some("µm".to_string());
                    axis.origin = -origin * scale;
                }
                image = image.with_meta(meta).unwrap();
            }
        }
        let original = image.clone();
        let (old_width, old_height) = (image.meta().width(), image.meta().height());
        let id = session.workspace.insert(image, None);
        session.workspace.set_roi(id, selection.clone()).unwrap();

        let outputs = exec
            .run(&mut session, command_id, &args)
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        let created = matches!(outputs.get("image"), Some(Value::Image(_)));
        let result_id = match outputs.get("image") {
            Some(&Value::Image(new)) => new,
            _ => id,
        };
        assert_eq!(
            created,
            command == "Duplicate..." || opts.contains_key("create"),
            "{what}: new image"
        );
        if created {
            assert!(session.workspace.image(id).unwrap().ptr_eq(&original));
        }

        // Shape and pixel type.
        let result = session.workspace.image(result_id).unwrap().clone();
        let meta = result.meta();
        let (width, height) = (
            row[7].parse::<usize>().unwrap(),
            row[8].parse::<usize>().unwrap(),
        );
        let kind = Raw::from_bit_depth(&row[9]);
        let rgb = kind == Raw::Rgb;
        assert_eq!((meta.width(), meta.height()), (width, height), "{what}");
        assert_eq!(meta.dtype(), original.meta().dtype(), "{what}");
        assert_eq!(meta.color == ColorModel::Rgb, rgb, "{what}");
        let channels = if rgb {
            1
        } else {
            meta.size(&AxisKind::Channel)
        };
        assert_eq!(
            [
                channels,
                meta.size(&AxisKind::Z),
                meta.size(&AxisKind::Time)
            ],
            [&row[10], &row[11], &row[12]].map(|n| n.parse::<usize>().unwrap()),
            "{what}: channels, slices, frames"
        );

        // Samples.
        let step = if rgb { 3 } else { 1 };
        assert_eq!(meta.plane_count(), count * step, "{what}: planes");
        let theirs = expected
            .entry(group.clone())
            .or_insert_with(|| raw_planes(&format!("{group}.raw"), kind, width, height));
        let method = opts.get("interpolation").map_or("None", String::as_str);
        for k in 0..count {
            for (c, theirs) in theirs[first - 1 + k].iter().enumerate() {
                let ours = result.read_plane(k * step + c).unwrap();
                let difference = assert_matches(&ours, theirs, &what);
                by_op
                    .entry(format!(
                        "{} {} ({method})",
                        kind.name(),
                        command.trim().trim_end_matches("...")
                    ))
                    .or_default()
                    .merge(difference);
            }
        }

        // Display range of the plane ImageJ leaves current.
        if meta.dtype() != DType::U8
            || original
                .meta()
                .channels
                .iter()
                .any(|c| c.display_range.is_some())
        {
            let mut position = vec![0; meta.ndim() - 2];
            for (index, axis) in meta.axes().iter().enumerate().skip(2) {
                let size = meta.shape()[index];
                position[index - 2] = match axis.kind {
                    AxisKind::Channel if !rgb => (channel_on_view - 1).min(size - 1),
                    AxisKind::Z if size == original.meta().size(&AxisKind::Z) => {
                        (slice_on_view - 1).min(size - 1)
                    }
                    _ => 0,
                };
            }
            let plane = meta.plane_index(&position).unwrap();
            let (min, max) = calipix_render::effective_display_range(&result, plane).unwrap();
            let (their_min, their_max) = (ij_number(&row[13]), ij_number(&row[14]));
            // A new or resized 16-bit or float image without a display
            // range is shown over its own data range by both programs, but
            // ImageJ's is that of the slice it had on view.
            let auto = original
                .meta()
                .channels
                .iter()
                .all(|c| c.display_range.is_none());
            if !auto {
                assert!(
                    close(min, their_min) && close(max, their_max),
                    "{what}: shown over {min}..{max}, ImageJ {their_min}..{their_max}"
                );
            }
        }

        // Calibration.
        let x = &meta.axes()[0];
        let y = &meta.axes()[1];
        assert!(
            close(x.scale, ij_number(&row[15])),
            "{what}: pixel width {}",
            x.scale
        );
        assert!(
            close(y.scale, ij_number(&row[16])),
            "{what}: pixel height {}",
            y.scale
        );
        if let Some(z) = meta.axis_index(&AxisKind::Z) {
            let depth = meta.axes()[z].scale;
            assert!(
                close(depth, ij_number(&row[17])),
                "{what}: pixel depth {depth}"
            );
        }
        assert_eq!(
            x.unit.clone().unwrap_or("pixels".to_string()),
            row[18],
            "{what}"
        );
        let (mut their_x, mut their_y) = (ij_number(&row[19]), ij_number(&row[20]));
        if calibrated {
            // Where ImageJ leaves the origin behind, Calipix moves it with
            // the picture: on a larger canvas by the border added, and in
            // Image > Size in proportion, as ImageJ's own Scale does.
            if command == "Canvas Size..." || opts.contains_key("enlarge") {
                their_x += ((width as i64 - old_width as i64) / 2) as f64;
                their_y += ((height as i64 - old_height as i64) / 2) as f64;
            } else if command == "Size..." {
                their_x *= width as f64 / old_width as f64;
                their_y *= height as f64 / old_height as f64;
            }
        }
        let (our_x, our_y) = (pixel_origin(&result, 0), pixel_origin(&result, 1));
        assert!(
            close(our_x, their_x) && close(our_y, their_y),
            "{what}: origin at pixel ({our_x}, {our_y}), ImageJ ({their_x}, {their_y})"
        );

        // Selection.
        let roi = session.workspace.get(result_id).unwrap().roi.clone();
        match row[21].as_str() {
            "-1" => assert_eq!(roi, None, "{what}: selection"),
            _ => {
                let bounds = [&row[22], &row[23], &row[24], &row[25]].map(|n| ij_number(n));
                let expected = Roi::rectangle(bounds[0], bounds[1], bounds[2], bounds[3]);
                assert_eq!(roi, Some(expected), "{what}: selection");
            }
        }

        // Undo restores the image and its selection, or removes the copy.
        exec.undo(&mut session).unwrap();
        assert!(
            session.workspace.image(id).unwrap().ptr_eq(&original),
            "{what}"
        );
        assert_eq!(session.workspace.get(id).unwrap().roi, selection, "{what}");
        assert_eq!(
            session.workspace.image(result_id).is_ok(),
            !created,
            "{what}"
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
    assert_eq!(cases, expected_cases, "{table}");
}

#[test]
fn geometry_commands_match_imagej() {
    check("geometry.csv", 83);
}

#[test]
fn rotate_and_scale_commands_match_imagej() {
    check("rotate_scale.csv", 175);
}
