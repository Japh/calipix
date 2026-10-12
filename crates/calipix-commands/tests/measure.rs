//! The measurement commands run through the executor, as every frontend
//! runs them, and compared with what ImageJ 1.54p gives for the same steps
//! (`fixtures/imagej/out/results_*.csv` and `analyze_cases.txt`, written
//! by `fixtures/imagej/gen/7?_*.ijm`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use calipix_command::{
    Args, DatasetId, EffectsPolicy, Error, Executor, Outputs, Session, Value, Workspace,
};
use calipix_commands::standard_registry;
use calipix_core::{Axis, AxisKind, Buffer, Cell, DType, Image, ImageMeta};
use calipix_roi::Roi;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn fixture_text(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-measure-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

fn open(exec: &Executor, session: &mut Session, file: &str) -> DatasetId {
    let out = exec
        .run(
            session,
            "file.open",
            &Args::new().with("path", fixture(file)),
        )
        .unwrap();
    match out.get("image") {
        Some(&Value::Image(id)) => id,
        other => panic!("file.open returned {other:?}"),
    }
}

/// A 4x3 8-bit image with the values 0 to 11.
fn ramp(session: &mut Session) -> DatasetId {
    let image = Image::from_vec(4, 3, (0u8..12).collect()).unwrap();
    session.workspace.insert(image, None)
}

fn number(outputs: &Outputs, name: &str) -> f64 {
    match outputs.get(name) {
        Some(&Value::Float(v)) => v,
        Some(&Value::Int(v)) => v as f64,
        other => panic!("output `{name}` is {other:?}"),
    }
}

fn results(session: &Session) -> &calipix_core::Table {
    session
        .workspace
        .table(Workspace::RESULTS)
        .expect("there is a Results table")
}

fn headings(session: &Session) -> Vec<String> {
    results(session)
        .column_names()
        .map(str::to_string)
        .collect()
}

fn close(ours: f64, theirs: f64) -> bool {
    if theirs.is_nan() {
        return ours.is_nan();
    }
    if theirs.is_infinite() {
        return ours == theirs;
    }
    (ours - theirs).abs() <= 1e-5 * theirs.abs() || (ours - theirs).abs() < 1e-9
}

/// Set Measurements arguments from ImageJ's option string, such as
/// `area mean min display redirect=None decimal=3`. Options not named are
/// off, as in an ImageJ macro.
fn measurement_args(options: &str) -> Args {
    let keys = [
        ("area", "area"),
        ("mean", "mean"),
        ("standard", "std_dev"),
        ("modal", "mode"),
        ("min", "min_max"),
        ("centroid", "centroid"),
        ("center", "center_of_mass"),
        ("perimeter", "perimeter"),
        ("bounding", "bounding_rect"),
        ("fit", "fit_ellipse"),
        ("shape", "shape_descriptors"),
        ("feret's", "feret"),
        ("integrated", "integrated_density"),
        ("median", "median"),
        ("skewness", "skewness"),
        ("kurtosis", "kurtosis"),
        ("area_fraction", "area_fraction"),
        ("stack", "stack_position"),
        ("display", "display_label"),
        ("invert", "invert_y"),
        ("scientific", "scientific_notation"),
        ("add", "add_to_overlay"),
        ("nan", "nan_empty_cells"),
    ];
    let words: Vec<&str> = options.split_whitespace().collect();
    let mut args = Args::new();
    for (keyword, name) in keys {
        args.insert(name, words.contains(&keyword));
    }
    for word in &words {
        if let Some(n) = word.strip_prefix("decimal=") {
            args.insert("decimal_places", n.parse::<i64>().unwrap());
        }
    }
    args
}

#[test]
fn measure_adds_a_row_and_undo_removes_it() {
    let (exec, mut session) = setup();
    ramp(&mut session);
    assert!(session.workspace.table(Workspace::RESULTS).is_none());

    let out = exec
        .run(&mut session, "measure.measure", &Args::new())
        .unwrap();
    // The measured values come back as outputs as well.
    assert_eq!(out.get("table"), Some(&Value::from("Results")));
    assert_eq!(out.get("row"), Some(&Value::Int(1)));
    assert_eq!(number(&out, "area"), 12.0);
    assert_eq!(number(&out, "mean"), 5.5);
    assert_eq!(number(&out, "min"), 0.0);
    assert_eq!(number(&out, "max"), 11.0);
    assert!(out.get("median").is_none(), "not measured by default");
    assert_eq!(headings(&session), ["Area", "Mean", "Min", "Max"]);
    assert_eq!(results(&session).len(), 1);

    // A selection confines the next measurement.
    exec.run(
        &mut session,
        "select.set",
        &Args::new().with("selection", Roi::rectangle(1.0, 1.0, 2.0, 2.0)),
    )
    .unwrap();
    let out = exec
        .run(&mut session, "measure.measure", &Args::new())
        .unwrap();
    assert_eq!(out.get("row"), Some(&Value::Int(2)));
    assert_eq!(number(&out, "area"), 4.0);
    assert_eq!(number(&out, "mean"), 7.5);
    assert_eq!(results(&session).len(), 2);
    assert_eq!(results(&session).get(1, "Min"), &Cell::Number(5.0));

    assert_eq!(exec.undo(&mut session).unwrap(), "Measure");
    assert_eq!(results(&session).len(), 1);
    assert_eq!(results(&session).get(0, "Mean"), &Cell::Number(5.5));
    exec.undo(&mut session).unwrap(); // the selection
    assert_eq!(exec.undo(&mut session).unwrap(), "Measure");
    assert!(session.workspace.table(Workspace::RESULTS).is_none());
    assert_eq!(exec.redo(&mut session).unwrap(), "Measure");
    assert_eq!(results(&session).len(), 1);
}

#[test]
fn settings_choose_the_columns_and_belong_to_the_session() {
    let (exec, mut session) = setup();
    ramp(&mut session);
    let out = exec
        .run(
            &mut session,
            "measure.set_measurements",
            &measurement_args("area centroid median display decimal=2"),
        )
        .unwrap();
    assert_eq!(
        out.get("measurements"),
        Some(&Value::from("area centroid median"))
    );
    // The choice is in History with every option spelled out.
    let entry = session.history().entries().last().unwrap();
    assert_eq!(entry.command, "measure.set_measurements");
    assert_eq!(entry.args.get("median"), Some(&Value::Bool(true)));
    assert_eq!(entry.args.get("mean"), Some(&Value::Bool(false)));
    assert_eq!(entry.args.get("decimal_places"), Some(&Value::Int(2)));
    assert!(entry.changed_workspace());

    exec.run(&mut session, "measure.measure", &Args::new())
        .unwrap();
    assert_eq!(headings(&session), ["Label", "Area", "X", "Y", "Median"]);
    assert_eq!(results(&session).get(0, "X"), &Cell::Number(2.0));

    // Different settings add their columns in ImageJ's order; earlier
    // rows are empty there.
    exec.run(
        &mut session,
        "measure.set_measurements",
        &measurement_args("mean centroid display decimal=2"),
    )
    .unwrap();
    exec.run(&mut session, "measure.measure", &Args::new())
        .unwrap();
    assert_eq!(
        headings(&session),
        ["Label", "Area", "Mean", "X", "Y", "Median"]
    );
    assert_eq!(results(&session).get(0, "Mean"), &Cell::Empty);
    assert_eq!(results(&session).get(1, "Mean"), &Cell::Number(5.5));
    assert_eq!(results(&session).get(1, "Area"), &Cell::Empty);

    // One run can name its own measurements without touching the choice.
    let out = exec
        .run(
            &mut session,
            "measure.measure",
            &Args::new()
                .with("measurements", "min kurtosis")
                .with("table", "Other"),
        )
        .unwrap();
    assert_eq!(number(&out, "max"), 11.0);
    assert!(out.get("kurtosis").is_some());
    assert!(out.get("mean").is_none());
    assert_eq!(results(&session).len(), 2, "the row went to another table");
    assert_eq!(session.workspace.table("Other").unwrap().len(), 1);
    let err = exec
        .run(
            &mut session,
            "measure.measure",
            &Args::new().with("measurements", "area volume"),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("`volume` is not a measurement"),
        "{err}"
    );

    // Undoing Set Measurements brings the previous choice back; a new
    // session starts from ImageJ's defaults.
    exec.undo(&mut session).unwrap(); // Measure into Other
    exec.undo(&mut session).unwrap(); // Measure
    assert_eq!(exec.undo(&mut session).unwrap(), "Set Measurements");
    exec.run(&mut session, "measure.clear_results", &Args::new())
        .unwrap();
    exec.run(&mut session, "measure.measure", &Args::new())
        .unwrap();
    assert_eq!(headings(&session), ["Label", "Area", "X", "Y", "Median"]);

    let (exec, mut fresh) = setup();
    ramp(&mut fresh);
    exec.run(&mut fresh, "measure.measure", &Args::new())
        .unwrap();
    assert_eq!(headings(&fresh), ["Area", "Mean", "Min", "Max"]);
}

#[test]
fn display_label_off_removes_labels_and_overlay_keeps_selections() {
    let (exec, mut session) = setup();
    let id = ramp(&mut session);
    let roi = Roi::rectangle(0.0, 0.0, 2.0, 2.0);
    exec.run(
        &mut session,
        "select.set",
        &Args::new().with("selection", roi.clone()),
    )
    .unwrap();
    exec.run(
        &mut session,
        "measure.set_measurements",
        &measurement_args("area display add decimal=3"),
    )
    .unwrap();
    exec.run(&mut session, "measure.measure", &Args::new())
        .unwrap();
    assert_eq!(headings(&session), ["Label", "Area"]);
    assert_eq!(session.workspace.get(id).unwrap().overlay, vec![roi]);

    exec.run(
        &mut session,
        "measure.set_measurements",
        &measurement_args("area decimal=3"),
    )
    .unwrap();
    assert_eq!(headings(&session), ["Area"]);
    exec.run(&mut session, "measure.measure", &Args::new())
        .unwrap();
    assert_eq!(session.workspace.get(id).unwrap().overlay.len(), 1);
    exec.undo(&mut session).unwrap();
    exec.undo(&mut session).unwrap();
    assert_eq!(headings(&session), ["Label", "Area"]);
}

/// Runs the steps of one line of `results_cases.txt` and saves the table.
fn saved_results(line: &str) -> (String, String) {
    let fields: Vec<&str> = line.split('\t').collect();
    let [file, image, position, roi, options] = fields[..] else {
        panic!("bad line: {line}");
    };
    let (exec, mut session) = setup();
    open(&exec, &mut session, image);
    exec.run(
        &mut session,
        "measure.set_measurements",
        &measurement_args(options),
    )
    .unwrap();
    let words: Vec<&str> = roi.split(' ').collect();
    if words[0] == "rect" {
        let n: Vec<f64> = words[1..].iter().map(|w| w.parse().unwrap()).collect();
        exec.run(
            &mut session,
            "select.set",
            &Args::new().with("selection", Roi::rectangle(n[0], n[1], n[2], n[3])),
        )
        .unwrap();
    }
    let mut args = Args::new();
    if position != "-" {
        let p: Vec<i64> = position.split(' ').map(|n| n.parse().unwrap()).collect();
        args = args
            .with("channel", p[0])
            .with("slice", p[1])
            .with("frame", p[2]);
    }
    exec.run(&mut session, "measure.measure", &args).unwrap();
    let path = temp(&format!("saved_{file}"));
    let out = exec
        .run(
            &mut session,
            "measure.save_results",
            &Args::new().with("path", path.clone()),
        )
        .unwrap();
    assert_eq!(out.get("rows"), Some(&Value::Int(1)));
    (file.to_string(), std::fs::read_to_string(path).unwrap())
}

#[test]
fn saved_results_are_byte_identical_to_imagej() {
    let index = fixture_text("results_cases.txt");
    let mut checked = Vec::new();
    for line in index.lines() {
        // Lines and points wait for calipix-roi; their tables are checked
        // in calipix-ops.
        let roi = line.split('\t').nth(3).unwrap();
        if !(roi == "none" || roi.starts_with("rect")) {
            continue;
        }
        let (file, ours) = saved_results(line);
        assert_eq!(ours, fixture_text(&file), "{file}");
        checked.push(file);
    }
    assert!(checked.len() >= 14, "only {checked:?}");
    for needed in [
        "results_all_label.csv",
        "results_all_label.txt",
        "results_all_sci4.csv",
        "results_hyper.csv",
        "results_all_rgb.csv",
        "results_inverty.csv",
    ] {
        assert!(checked.iter().any(|f| f == needed), "{needed} not checked");
    }
}

#[test]
fn saving_needs_results_and_permission() {
    let (mut exec, mut session) = setup();
    ramp(&mut session);
    let path = temp("nothing.csv");
    let err = exec
        .run(
            &mut session,
            "measure.save_results",
            &Args::new().with("path", path.clone()),
        )
        .unwrap_err();
    assert!(err.to_string().contains("Results table is empty"), "{err}");
    assert!(!path.exists());

    exec.run(&mut session, "measure.measure", &Args::new())
        .unwrap();
    exec.policy = EffectsPolicy::read_only();
    let err = exec
        .run(
            &mut session,
            "measure.save_results",
            &Args::new().with("path", path.clone()),
        )
        .unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err}");
    assert!(!path.exists());
}

#[test]
fn several_rows_and_a_summary() {
    let (exec, mut session) = setup();
    ramp(&mut session);
    for (x, w) in [(0.0, 1.0), (0.0, 2.0), (1.0, 3.0)] {
        exec.run(
            &mut session,
            "select.set",
            &Args::new().with("selection", Roi::rectangle(x, 0.0, w, 3.0)),
        )
        .unwrap();
        exec.run(&mut session, "measure.measure", &Args::new())
            .unwrap();
    }
    let path = temp("rows.csv");
    exec.run(
        &mut session,
        "measure.save_results",
        &Args::new().with("path", path.clone()),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        " ,Area,Mean,Min,Max\n1,3,4.000,0,8\n2,6,4.500,0,9\n3,9,6.000,1,11\n"
    );

    let out = exec
        .run(&mut session, "measure.summarize", &Args::new())
        .unwrap();
    assert_eq!(out.get("rows"), Some(&Value::Int(7)));
    exec.run(
        &mut session,
        "measure.save_results",
        &Args::new().with("path", path.clone()),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        " ,Label,Area,Mean,Min,Max\n\
         1,,3,4.000,0.000,8.000\n\
         2,,6,4.500,0.000,9.000\n\
         3,,9,6.000,1.000,11.000\n\
         4,Mean,6,4.833,0.333,9.333\n\
         5,SD,3,1.041,0.577,1.528\n\
         6,Min,3,4.000,0.000,8.000\n\
         7,Max,9,6.000,1.000,11.000\n"
    );
    // Summarizing twice changes nothing, and measuring again drops the
    // summary first.
    exec.run(&mut session, "measure.summarize", &Args::new())
        .unwrap();
    assert_eq!(results(&session).len(), 7);
    exec.run(&mut session, "measure.measure", &Args::new())
        .unwrap();
    assert_eq!(results(&session).len(), 4);

    let out = exec
        .run(&mut session, "measure.clear_results", &Args::new())
        .unwrap();
    assert_eq!(out.get("rows"), Some(&Value::Int(4)));
    assert!(session.workspace.table(Workspace::RESULTS).is_none());
    let err = exec
        .run(&mut session, "measure.summarize", &Args::new())
        .unwrap_err();
    assert!(err.to_string().contains("measure something first"), "{err}");
    assert_eq!(exec.undo(&mut session).unwrap(), "Clear Results");
    assert_eq!(results(&session).len(), 4);
}

/// One block of `analyze_cases.txt`.
#[derive(Debug, Default)]
struct Case {
    name: String,
    about: BTreeMap<String, String>,
    expected: BTreeMap<String, String>,
}

fn cases() -> Vec<Case> {
    let text = fixture_text("analyze_cases.txt");
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
            Some(key) => case.about.insert(key.to_string(), value.to_string()),
            None => case.expected.insert(key.to_string(), value.to_string()),
        };
    }
    cases
}

fn java_double(text: &str) -> f64 {
    match text {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        other => other.parse().unwrap(),
    }
}

/// The arguments of Set Scale or Properties from ImageJ's option string.
fn calibration_args(command: &str, options: &str) -> Args {
    let mut args = Args::new();
    // `key=value` or `key=[value with spaces]`.
    let mut rest = options;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim();
        let after = &rest[eq + 1..];
        let (value, tail) = if let Some(bracketed) = after.strip_prefix('[') {
            let end = bracketed.find(']').unwrap();
            (&bracketed[..end], &bracketed[end + 1..])
        } else {
            let end = after.find(' ').unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        rest = tail;
        let float = || value.parse::<f64>().unwrap();
        match (command, key) {
            ("Set Scale...", "distance") => args.insert("distance", float()),
            ("Set Scale...", "known") => args.insert("known", float()),
            ("Set Scale...", "pixel") => args.insert("pixel_aspect_ratio", float()),
            (_, "unit") => args.insert("unit", value),
            ("Properties...", "pixel_width") => args.insert("pixel_width", float()),
            ("Properties...", "pixel_height") => args.insert("pixel_height", float()),
            ("Properties...", "voxel_depth") => args.insert("voxel_depth", float()),
            ("Properties...", "frame") => args.insert("frame_interval", value),
            ("Properties...", "origin") => args.insert("origin", value),
            ("Properties...", "channels" | "slices" | "frames") => {}
            other => panic!("unexpected option {other:?}"),
        }
    }
    args
}

/// `Calibration.getUnits`, which is what the macro function reports.
fn plural(unit: &str) -> String {
    match unit {
        "pixel" => "pixels".to_string(),
        "micron" => "microns".to_string(),
        "inch" => "inches".to_string(),
        other => other.to_string(),
    }
}

#[test]
fn set_scale_and_properties_match_imagej() {
    let mut checked = 0;
    for case in cases() {
        let Some(command) = case.about.get("command") else {
            continue;
        };
        let id = match command.as_str() {
            "Set Scale..." => "measure.set_scale",
            "Properties..." => "image.properties",
            other => panic!("{}: unknown command {other}", case.name),
        };
        let (exec, mut session) = setup();
        let image = open(&exec, &mut session, &case.about["image"]);
        let before = session
            .workspace
            .image(image)
            .unwrap()
            .read_plane(0)
            .unwrap();
        let out = exec
            .run(
                &mut session,
                id,
                &calibration_args(command, &case.about["options"]),
            )
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        // A scale that is already there leaves nothing to undo.
        let changed = session
            .history()
            .entries()
            .last()
            .unwrap()
            .changed_workspace();

        // The calibration, as the command reports it and as the image
        // now carries it. (A single plane has no axis to keep a voxel
        // depth or a frame interval on, so those are not compared.)
        let voxel: Vec<&str> = case.about["voxel"].split(' ').collect();
        let (pw, ph) = (java_double(voxel[0]), java_double(voxel[1]));
        assert!(
            close(number(&out, "pixel_width"), pw),
            "{}: {out:?}",
            case.name
        );
        assert!(
            close(number(&out, "pixel_height"), ph),
            "{}: {out:?}",
            case.name
        );
        let Some(Value::Str(unit)) = out.get("unit") else {
            panic!("{}: no unit", case.name);
        };
        assert_eq!(plural(unit), voxel[3], "{}", case.name);
        let meta = session.workspace.image(image).unwrap().meta().clone();
        assert!(close(meta.axes()[0].scale, pw) && close(meta.axes()[1].scale, ph));
        // Only metadata changed.
        let after = session
            .workspace
            .image(image)
            .unwrap()
            .read_plane(0)
            .unwrap();
        assert!(before.shares_storage(&after), "{}", case.name);

        // Measurements are reported in the new calibration.
        exec.run(
            &mut session,
            "select.set",
            &Args::new().with("selection", Roi::rectangle(8.0, 6.0, 20.0, 10.0)),
        )
        .unwrap();
        let measured = exec
            .run(
                &mut session,
                "measure.measure",
                &Args::new().with("measurements", "all"),
            )
            .unwrap();
        let table = results(&session);
        for (heading, theirs) in &case.expected {
            let theirs = java_double(theirs);
            let Cell::Number(ours) = *table.get(0, heading) else {
                panic!("{}: no {heading}", case.name);
            };
            assert!(
                close(ours, theirs),
                "{} {heading}: {ours}, ImageJ {theirs}",
                case.name
            );
        }
        assert!(close(
            number(&measured, "area"),
            java_double(&case.expected["Area"])
        ));

        // And undo puts the old calibration back.
        exec.undo(&mut session).unwrap(); // Measure
        exec.undo(&mut session).unwrap(); // the selection
        if changed {
            exec.undo(&mut session).unwrap();
        }
        let restored = session.workspace.image(image).unwrap().meta();
        let original = calipix_io::open(fixture(&case.about["image"])).unwrap();
        assert_eq!(restored.axes(), original.meta().axes(), "{}", case.name);
        checked += 1;
    }
    assert!(checked >= 13, "only {checked} cases");
}

/// A 3-slice, 2-frame image with every axis calibrated.
fn stack(session: &mut Session) -> DatasetId {
    let meta = ImageMeta::new(
        DType::U8,
        vec![
            (Axis::calibrated(AxisKind::X, 0.5, "mm"), 4),
            (Axis::calibrated(AxisKind::Y, 0.5, "mm"), 2),
            (Axis::calibrated(AxisKind::Z, 3.0, "mm"), 3),
            (Axis::new(AxisKind::Time), 2),
        ],
    )
    .unwrap()
    .with_name("stack");
    let planes = (0..6u8)
        .map(|p| Buffer::from_vec((0..8u8).map(|i| i + 10 * p).collect::<Vec<u8>>()))
        .collect();
    session
        .workspace
        .insert(Image::from_planes(meta, planes).unwrap(), None)
}

#[test]
fn calibration_of_stacks() {
    let (exec, mut session) = setup();
    let id = stack(&mut session);
    let scale_of = |session: &Session, kind: AxisKind| {
        let meta = session.workspace.image(id).unwrap().meta();
        let axis = &meta.axes()[meta.axis_index(&kind).unwrap()];
        (axis.scale, axis.unit.clone())
    };

    // Properties changes only what it is given.
    let out = exec
        .run(
            &mut session,
            "image.properties",
            &Args::new()
                .with("voxel_depth", 2.0)
                .with("frame_interval", "3 sec")
                .with("origin", "1,2"),
        )
        .unwrap();
    assert_eq!(number(&out, "pixel_width"), 0.5);
    assert_eq!(number(&out, "voxel_depth"), 2.0);
    assert_eq!(number(&out, "frame_interval"), 3.0);
    assert_eq!(scale_of(&session, AxisKind::Z), (2.0, Some("mm".into())));
    assert_eq!(
        scale_of(&session, AxisKind::Time),
        (3.0, Some("sec".into()))
    );
    let meta = session.workspace.image(id).unwrap().meta().clone();
    assert_eq!((meta.axes()[0].origin, meta.axes()[1].origin), (-0.5, -1.0));

    // Set Scale keeps a depth that was set and the origin in pixels.
    exec.run(
        &mut session,
        "measure.set_scale",
        &Args::new()
            .with("distance", 10.0)
            .with("known", 2.5)
            .with("unit", "um"),
    )
    .unwrap();
    assert_eq!(
        scale_of(&session, AxisKind::X),
        (0.25, Some("\u{b5}m".into()))
    );
    assert_eq!(
        scale_of(&session, AxisKind::Z),
        (2.0, Some("\u{b5}m".into()))
    );
    let meta = session.workspace.image(id).unwrap().meta().clone();
    assert_eq!(
        (meta.axes()[0].origin, meta.axes()[1].origin),
        (-0.25, -0.5)
    );

    // The slice on view is the one measured, and reported.
    exec.run(
        &mut session,
        "measure.set_measurements",
        &measurement_args("area mean stack display decimal=3"),
    )
    .unwrap();
    let out = exec
        .run(
            &mut session,
            "measure.measure",
            &Args::new().with("slice", 2).with("frame", 2),
        )
        .unwrap();
    assert_eq!(number(&out, "mean"), 43.5);
    assert_eq!(number(&out, "area"), 8.0 * 0.25 * 0.25);
    assert_eq!(number(&out, "slice"), 2.0);
    assert_eq!(number(&out, "frame"), 2.0);
    assert_eq!(out.get("label"), Some(&Value::from("stack:5")));
    assert!(out.get("channel").is_none());
    let err = exec
        .run(
            &mut session,
            "measure.measure",
            &Args::new().with("slice", 4),
        )
        .unwrap_err();
    assert!(err.to_string().contains("slice"), "{err}");

    // Removing the scale, and bad values.
    exec.run(
        &mut session,
        "image.properties",
        &Args::new().with("unit", "none"),
    )
    .unwrap();
    assert_eq!(scale_of(&session, AxisKind::X), (1.0, None));
    assert_eq!(scale_of(&session, AxisKind::Z), (1.0, None));
    for (name, value) in [("frame_interval", "soon"), ("origin", "left,top")] {
        let err = exec
            .run(
                &mut session,
                "image.properties",
                &Args::new().with(name, value),
            )
            .unwrap_err();
        assert!(matches!(err, Error::InvalidArg { .. }), "{err}");
    }
}

#[test]
fn global_scale_reaches_every_open_image() {
    let (exec, mut session) = setup();
    let first = ramp(&mut session);
    let second = ramp(&mut session);
    exec.run(
        &mut session,
        "measure.set_scale",
        &Args::new()
            .with("distance", 4.0)
            .with("known", 1.0)
            .with("unit", "mm")
            .with("global", true),
    )
    .unwrap();
    for id in [first, second] {
        let meta = session.workspace.image(id).unwrap().meta();
        assert_eq!(meta.axes()[0].scale, 0.25);
        assert_eq!(meta.axes()[1].unit.as_deref(), Some("mm"));
    }
    exec.undo(&mut session).unwrap();
    assert_eq!(
        session.workspace.image(first).unwrap().meta().axes()[0].scale,
        1.0
    );
}

fn list(text: &str) -> Vec<f64> {
    text.split_whitespace().map(java_double).collect()
}

fn column(session: &Session, table: &str, name: &str) -> Vec<f64> {
    session
        .workspace
        .table(table)
        .unwrap_or_else(|| panic!("no table {table}"))
        .numbers(name)
        .unwrap_or_else(|| panic!("no column {name} in {table}"))
        .into_iter()
        .map(|v| v.unwrap())
        .collect()
}

/// Selects the rectangle of a case, if it has one; false for shapes the
/// commands cannot be given yet.
fn select(exec: &Executor, session: &mut Session, case: &Case) -> bool {
    let words: Vec<&str> = case.about["roi"].split(' ').collect();
    match words[0] {
        "none" => true,
        "rect" => {
            let n: Vec<f64> = words[1..].iter().map(|w| w.parse().unwrap()).collect();
            exec.run(
                session,
                "select.set",
                &Args::new().with("selection", Roi::rectangle(n[0], n[1], n[2], n[3])),
            )
            .unwrap();
            true
        }
        _ => false,
    }
}

#[test]
fn histogram_tables_match_imagej() {
    let mut checked = 0;
    for case in cases().iter().filter(|c| c.name.starts_with("hist_")) {
        let (exec, mut session) = setup();
        open(&exec, &mut session, &case.about["image"]);
        if !select(&exec, &mut session, case) {
            continue;
        }
        let mut args = Args::new().with("bins", case.about["bins"].parse::<i64>().unwrap());
        if let Some(range) = case.about.get("range") {
            let r = list(range);
            args = args.with("x_min", r[0]).with("x_max", r[1]);
        }
        let out = exec
            .run(&mut session, "analyze.histogram", &args)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let Some(Value::Str(table)) = out.get("table") else {
            panic!("{}: no table", case.name);
        };
        assert_eq!(*table, format!("Histogram of {}", case.about["image"]));
        let counts = list(&case.expected["counts"]);
        assert_eq!(column(&session, table, "count"), counts, "{}", case.name);
        assert_eq!(number(&out, "count"), java_double(&case.expected["count"]));
        assert_eq!(number(&out, "bins"), counts.len() as f64);
        // 8-bit and RGB tables list values; the others bin starts.
        let names: Vec<&str> = session
            .workspace
            .table(table)
            .unwrap()
            .column_names()
            .collect();
        let starts = if number(&out, "bin_width") == 1.0 {
            assert_eq!(names, ["value", "count"], "{}", case.name);
            column(&session, table, "value")
        } else {
            assert_eq!(names, ["index", "bin start", "count"], "{}", case.name);
            column(&session, table, "bin start")
        };
        assert!(close(starts[0], list(&case.expected["values"])[0]));
        // The statistics beside the histogram are those of the counted
        // samples; without a fixed range that is every sample.
        if !case.about.contains_key("range") {
            for (name, key) in [
                ("mean", "mean"),
                ("std_dev", "std"),
                ("min", "min"),
                ("max", "max"),
            ] {
                let (ours, theirs) = (number(&out, name), java_double(&case.expected[key]));
                assert!(
                    close(ours, theirs),
                    "{} {name}: {ours}, {theirs}",
                    case.name
                );
            }
        }
        // The table is a new one each time, and undo removes it.
        exec.undo(&mut session).unwrap();
        assert!(session.workspace.table(table).is_none());
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} histograms");
}

#[test]
fn histogram_of_a_stack_and_of_other_bins() {
    let (exec, mut session) = setup();
    let id = stack(&mut session);
    let out = exec
        .run(
            &mut session,
            "analyze.histogram",
            &Args::new().with("stack", true),
        )
        .unwrap();
    assert_eq!(number(&out, "count"), 48.0);
    assert_eq!(number(&out, "min"), 0.0);
    assert_eq!(number(&out, "max"), 57.0);
    let one = exec
        .run(
            &mut session,
            "analyze.histogram",
            &Args::new().with("slice", 3).with("frame", 1),
        )
        .unwrap();
    assert_eq!(number(&one, "count"), 8.0);
    assert_eq!((number(&one, "min"), number(&one, "max")), (20.0, 27.0));

    // 8-bit data in other bins is counted as ImageJ counts a stack.
    let out = exec
        .run(
            &mut session,
            "analyze.histogram",
            &Args::new()
                .with("bins", 4)
                .with("x_min", 0.0)
                .with("x_max", 256.0),
        )
        .unwrap();
    assert_eq!(number(&out, "bin_width"), 64.0);
    assert_eq!(
        column(&session, "Histogram of stack", "bin start"),
        [0.0, 64.0, 128.0, 192.0]
    );
    assert_eq!(
        column(&session, "Histogram of stack", "count"),
        [8.0, 0.0, 0.0, 0.0]
    );

    session
        .workspace
        .set_roi(id, Some(Roi::rectangle(0.0, 0.0, 2.0, 2.0)))
        .unwrap();
    let within = exec
        .run(&mut session, "analyze.histogram", &Args::new())
        .unwrap();
    assert_eq!(number(&within, "count"), 4.0);
    let err = exec
        .run(
            &mut session,
            "analyze.histogram",
            &Args::new().with("stack", true),
        )
        .unwrap_err();
    assert!(err.to_string().contains("not available yet"), "{err}");
    let err = exec
        .run(
            &mut session,
            "analyze.histogram",
            &Args::new().with("x_min", 5.0),
        )
        .unwrap_err();
    assert!(err.to_string().contains("both ends"), "{err}");
}

#[test]
fn profiles_of_rectangles_match_imagej() {
    let mut checked = 0;
    for case in cases().iter().filter(|c| c.name.starts_with("profile_")) {
        let (exec, mut session) = setup();
        open(&exec, &mut session, &case.about["image"]);
        if !select(&exec, &mut session, case) {
            continue;
        }
        let vertical = case.about.get("vertical").is_some();
        let out = exec
            .run(
                &mut session,
                "analyze.plot_profile",
                &Args::new().with("vertical", vertical),
            )
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let Some(Value::Str(table)) = out.get("table") else {
            panic!("{}: no table", case.name);
        };
        let theirs = list(&case.expected["profile"]);
        let ours = column(&session, table, "Value");
        assert_eq!(ours.len(), theirs.len(), "{}", case.name);
        for (i, (o, t)) in ours.iter().zip(&theirs).enumerate() {
            assert!(close(*o, *t), "{} point {i}: {o}, ImageJ {t}", case.name);
        }
        assert_eq!(number(&out, "points"), theirs.len() as f64);
        let distance = column(&session, table, "Distance");
        assert_eq!(distance[0], 0.0);
        assert_eq!(distance.last().copied(), Some(theirs.len() as f64 - 1.0));
        checked += 1;
    }
    assert!(checked >= 16, "only {checked} profiles");
}

#[test]
fn profile_needs_a_selection_and_uses_the_calibration() {
    let (exec, mut session) = setup();
    stack(&mut session);
    let err = exec
        .run(&mut session, "analyze.plot_profile", &Args::new())
        .unwrap_err();
    assert!(err.to_string().contains("there is no selection"), "{err}");
    exec.run(&mut session, "select.all", &Args::new()).unwrap();
    let out = exec
        .run(
            &mut session,
            "analyze.plot_profile",
            &Args::new().with("slice", 2),
        )
        .unwrap();
    assert_eq!(number(&out, "spacing"), 0.5);
    assert_eq!(out.get("unit"), Some(&Value::from("mm")));
    assert_eq!(
        column(&session, "Profile of stack", "Distance"),
        [0.0, 0.5, 1.0, 1.5]
    );
    assert_eq!(
        column(&session, "Profile of stack", "Value"),
        [12.0, 13.0, 14.0, 15.0]
    );
    assert_eq!((number(&out, "min"), number(&out, "max")), (12.0, 15.0));
}

#[test]
fn distribution_counts_a_column() {
    let (exec, mut session) = setup();
    ramp(&mut session);
    for w in 1..=4 {
        exec.run(
            &mut session,
            "select.set",
            &Args::new().with("selection", Roi::rectangle(0.0, 0.0, w as f64, 3.0)),
        )
        .unwrap();
        exec.run(&mut session, "measure.measure", &Args::new())
            .unwrap();
    }
    // Areas 3, 6, 9 and 12 in three bins over 0 to 12.
    let out = exec
        .run(
            &mut session,
            "measure.distribution",
            &Args::new()
                .with("automatic", false)
                .with("bins", 3)
                .with("range_max", 12.0),
        )
        .unwrap();
    assert_eq!(out.get("table"), Some(&Value::from("Area Distribution")));
    assert_eq!(number(&out, "count"), 4.0);
    assert_eq!(
        column(&session, "Area Distribution", "bin start"),
        [0.0, 4.0, 8.0]
    );
    assert_eq!(
        column(&session, "Area Distribution", "count"),
        [1.0, 1.0, 2.0]
    );
    // Automatic binning always gives at least two bins.
    let out = exec
        .run(&mut session, "measure.distribution", &Args::new())
        .unwrap();
    assert!(number(&out, "bins") >= 2.0);
    assert_eq!(number(&out, "count"), 4.0);
    let err = exec
        .run(
            &mut session,
            "measure.distribution",
            &Args::new().with("parameter", "Volume"),
        )
        .unwrap_err();
    assert!(err.to_string().contains("no column `Volume`"), "{err}");
}

#[test]
fn measuring_what_cannot_be_measured_explains() {
    let (exec, mut session) = setup();
    let complex = Image::from_vec(1, 1, vec![calipix_core::Complex32::new(1.0, 0.0)]).unwrap();
    session.workspace.insert(complex, None);
    let err = exec
        .run(&mut session, "measure.measure", &Args::new())
        .unwrap_err();
    assert!(err.to_string().contains("complex images"), "{err}");
    assert!(session.workspace.table(Workspace::RESULTS).is_none());

    let id = ramp(&mut session);
    session
        .workspace
        .set_roi(id, Some(Roi::rectangle(10.0, 10.0, 2.0, 2.0)))
        .unwrap();
    let err = exec
        .run(&mut session, "measure.measure", &Args::new())
        .unwrap_err();
    assert!(err.to_string().contains("outside the image"), "{err}");
}
