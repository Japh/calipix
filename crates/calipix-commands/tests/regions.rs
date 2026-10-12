//! The region commands, run the way every frontend runs them.

use std::path::PathBuf;
use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Error, Executor, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::{Cell, Image};
use calipix_roi::{Color, Kind, Roi, RoiEntry};

const WIDTH: usize = 64;
const HEIGHT: usize = 48;

/// An image whose samples are their column number.
fn setup() -> (Executor, Session, DatasetId) {
    let exec = Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all());
    let mut session = Session::new();
    let pixels: Vec<u8> = (0..WIDTH * HEIGHT).map(|i| (i % WIDTH) as u8).collect();
    let image = Image::from_vec(WIDTH, HEIGHT, pixels).unwrap();
    let id = session.workspace.insert(image, None);
    (exec, session, id)
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

/// A temporary file that no other test uses.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-regions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn regions(session: &Session, id: DatasetId) -> Vec<RoiEntry> {
    session.workspace.get(id).unwrap().overlay.clone()
}

fn names(session: &Session, id: DatasetId) -> Vec<Option<String>> {
    regions(session, id)
        .into_iter()
        .map(|entry| entry.props.name)
        .collect()
}

fn run(exec: &Executor, session: &mut Session, id: &str, args: Args) -> calipix_command::Outputs {
    exec.run(session, id, &args)
        .unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// Makes `text` the selection and adds it as a region called `name`.
fn add(exec: &Executor, session: &mut Session, text: &str, name: Option<&str>) {
    run(
        exec,
        session,
        "select.set",
        Args::new().with("selection", Value::from(text)),
    );
    let mut args = Args::new();
    if let Some(name) = name {
        args.insert("name", name);
    }
    run(exec, session, "regions.add", args);
}

/// One of every kind of shape a tool can draw.
const SHAPES: [(&str, &str); 6] = [
    ("rectangle 4 4 10 8", "Square"),
    ("oval 21 6 16 12", "Blob"),
    ("polygon 26,31 40,30 42,40 30,41", "Patch"),
    ("line 5 36 24 28", "Width"),
    ("polyline 5,40 10,44 20,40", "Path"),
    ("points 52,8 57,12", "Marks"),
];

fn with_shapes() -> (Executor, Session, DatasetId) {
    let (exec, mut session, id) = setup();
    for (text, name) in SHAPES {
        add(&exec, &mut session, text, Some(name));
    }
    (exec, session, id)
}

#[test]
fn add_keeps_the_selection_as_a_region_and_undo_takes_it_back() {
    let (exec, mut session, id) = setup();
    let err = exec
        .run(&mut session, "regions.add", &Args::new())
        .unwrap_err();
    assert!(err.to_string().contains("no selection to add"), "{err}");

    add(&exec, &mut session, "rectangle 4 4 10 8", None);
    let out = run(
        &exec,
        &mut session,
        "regions.add",
        Args::new().with("name", " Again "),
    );
    assert_eq!(out.get("region"), Some(&Value::Int(2)));
    assert_eq!(out.get("count"), Some(&Value::Int(2)));
    let listed = regions(&session, id);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].roi, Roi::rectangle(4.0, 4.0, 10.0, 8.0));
    assert_eq!(names(&session, id), [None, Some("Again".to_string())]);
    // The selection is still there.
    assert!(session.workspace.get(id).unwrap().roi.is_some());

    assert_eq!(exec.undo(&mut session).unwrap(), "Add to Regions");
    assert_eq!(regions(&session, id).len(), 1);
    assert_eq!(exec.redo(&mut session).unwrap(), "Add to Regions");
    assert_eq!(regions(&session, id).len(), 2);
}

#[test]
fn delete_rename_select_update_and_clear_are_each_one_undoable_step() {
    let (exec, mut session, id) = with_shapes();
    assert_eq!(regions(&session, id).len(), 6);

    // Delete by number and range.
    let out = run(
        &exec,
        &mut session,
        "regions.delete",
        Args::new().with("regions", "2-3,6"),
    );
    assert_eq!(out.get("count"), Some(&Value::Int(3)));
    let left: Vec<String> = names(&session, id).into_iter().flatten().collect();
    assert_eq!(left, ["Square", "Width", "Path"]);
    exec.undo(&mut session).unwrap();
    assert_eq!(regions(&session, id).len(), 6);
    exec.redo(&mut session).unwrap();
    for (bad, why) in [
        ("9", "no region 9"),
        ("x", "not a region number"),
        ("", "no regions were given"),
    ] {
        let err = exec
            .run(
                &mut session,
                "regions.delete",
                &Args::new().with("regions", bad),
            )
            .unwrap_err();
        assert!(matches!(err, Error::InvalidArg { .. }), "{err}");
        assert!(err.to_string().contains(why), "{err}");
    }
    assert_eq!(regions(&session, id).len(), 3);

    // Rename; an empty name takes the name away.
    let rename = |region: i64, name: &str| Args::new().with("region", region).with("name", name);
    run(&exec, &mut session, "regions.rename", rename(2, "Diameter"));
    assert_eq!(names(&session, id)[1].as_deref(), Some("Diameter"));
    run(&exec, &mut session, "regions.rename", rename(1, "  "));
    assert_eq!(names(&session, id)[0], None);
    exec.undo(&mut session).unwrap();
    exec.undo(&mut session).unwrap();
    assert_eq!(names(&session, id)[1].as_deref(), Some("Width"));
    let err = exec
        .run(&mut session, "regions.rename", &rename(7, "x"))
        .unwrap_err();
    assert!(err.to_string().contains("outside 1 to 3"), "{err}");

    // Select makes a region the selection; the list is unchanged.
    run(
        &exec,
        &mut session,
        "regions.select",
        Args::new().with("region", 2),
    );
    let dataset = session.workspace.get(id).unwrap();
    assert_eq!(dataset.roi.as_ref().map(Roi::kind), Some(Kind::Line));
    assert_eq!(dataset.overlay.len(), 3);
    assert_eq!(exec.undo(&mut session).unwrap(), "Select Region");
    assert_eq!(
        session
            .workspace
            .get(id)
            .unwrap()
            .roi
            .as_ref()
            .map(Roi::kind),
        Some(Kind::Points)
    );

    // Update gives a region the selection's shape and keeps its name.
    run(
        &exec,
        &mut session,
        "select.set",
        Args::new().with("selection", Value::from("oval 1 2 3 4")),
    );
    run(
        &exec,
        &mut session,
        "regions.update",
        Args::new().with("region", 3),
    );
    let third = &regions(&session, id)[2];
    assert_eq!(third.roi, Roi::oval(1.0, 2.0, 3.0, 4.0));
    assert_eq!(third.props.name.as_deref(), Some("Path"));
    exec.undo(&mut session).unwrap();
    assert_eq!(regions(&session, id)[2].roi.kind(), Kind::Polyline);

    // Clear removes them all, and undo brings them back.
    run(&exec, &mut session, "regions.clear", Args::new());
    assert!(regions(&session, id).is_empty());
    let err = exec
        .run(&mut session, "regions.clear", &Args::new())
        .unwrap_err();
    assert!(err.to_string().contains("no regions"), "{err}");
    exec.undo(&mut session).unwrap();
    assert_eq!(regions(&session, id).len(), 3);
}

#[test]
fn colours_are_set_on_the_regions_named() {
    let (exec, mut session, id) = with_shapes();
    let out = run(
        &exec,
        &mut session,
        "regions.set_color",
        Args::new()
            .with("color", "red")
            .with("fill", "#4000ff00")
            .with("regions", "1-2"),
    );
    assert_eq!(out.get("count"), Some(&Value::Int(2)));
    let listed = regions(&session, id);
    assert_eq!(listed[0].props.stroke_color, Some(Color::rgb(255, 0, 0)));
    assert_eq!(listed[1].props.fill_color, Some(Color(0x4000_ff00)));
    assert_eq!(listed[2].props.stroke_color, None);
    // Without regions named, all of them; `none` is the default colour.
    run(
        &exec,
        &mut session,
        "regions.set_color",
        Args::new().with("color", "none"),
    );
    assert!(
        regions(&session, id)
            .iter()
            .all(|entry| entry.props.stroke_color.is_none() && entry.props.fill_color.is_none())
    );
    exec.undo(&mut session).unwrap();
    assert_eq!(
        regions(&session, id)[0].props.stroke_color,
        Some(Color::rgb(255, 0, 0))
    );
    let err = exec
        .run(
            &mut session,
            "regions.set_color",
            &Args::new().with("color", "mauve"),
        )
        .unwrap_err();
    assert!(err.to_string().contains("not a colour"), "{err}");
}

#[test]
fn measure_adds_a_row_per_region_as_measure_would() {
    let (exec, mut session, id) = setup();
    add(&exec, &mut session, "rectangle 4 4 10 8", Some("Left"));
    add(&exec, &mut session, "rectangle 40 10 20 6", Some("Right"));
    add(&exec, &mut session, "rectangle 0 0 64 48", None);
    let selection = session.workspace.get(id).unwrap().roi.clone();
    let before = regions(&session, id);

    // What Measure itself gives for the second rectangle.
    let (exec2, mut alone, _) = setup();
    run(
        &exec2,
        &mut alone,
        "select.set",
        Args::new().with("selection", Value::from("rectangle 40 10 20 6")),
    );
    let single = run(&exec2, &mut alone, "measure.measure", Args::new());

    let out = run(&exec, &mut session, "regions.measure", Args::new());
    assert_eq!(out.get("rows"), Some(&Value::Int(3)));
    assert_eq!(out.get("count"), Some(&Value::Int(3)));
    assert_eq!(out.get("table"), Some(&Value::from("Results")));
    let table = session.workspace.table("Results").unwrap();
    assert_eq!(table.len(), 3);
    assert_eq!(table.get(0, "Area"), &Cell::from(80.0));
    // Columns 4 to 13 average 8.5.
    assert_eq!(table.get(0, "Mean"), &Cell::from(8.5));
    assert_eq!(table.get(1, "Area"), &Cell::from(120.0));
    assert_eq!(table.get(2, "Area"), &Cell::from((WIDTH * HEIGHT) as f64));
    for (heading, name) in [
        ("Area", "area"),
        ("Mean", "mean"),
        ("Min", "min"),
        ("Max", "max"),
    ] {
        let Some(Value::Float(expected)) = single.get(name) else {
            panic!("Measure returned no {name}");
        };
        assert_eq!(
            table.get(1, heading).as_number(),
            Some(*expected),
            "{heading}"
        );
    }
    // Measuring leaves the selection and the regions as they were.
    let dataset = session.workspace.get(id).unwrap();
    assert_eq!(dataset.roi, selection);
    assert_eq!(dataset.overlay, before);

    // Only the regions named; and it is one step to undo.
    let out = run(
        &exec,
        &mut session,
        "regions.measure",
        Args::new().with("regions", "2"),
    );
    assert_eq!(out.get("rows"), Some(&Value::Int(1)));
    assert_eq!(session.workspace.table("Results").unwrap().len(), 4);
    assert_eq!(exec.undo(&mut session).unwrap(), "Measure Regions");
    assert_eq!(session.workspace.table("Results").unwrap().len(), 3);
    assert_eq!(exec.undo(&mut session).unwrap(), "Measure Regions");
    assert!(session.workspace.table("Results").is_none());
}

#[test]
fn measured_rows_are_labelled_with_the_region_and_never_listed_twice() {
    let (exec, mut session, id) = setup();
    add(&exec, &mut session, "rectangle 4 4 10 8", Some("Left"));
    add(&exec, &mut session, "rectangle 40 10 20 6", None);
    // Labels on, and ImageJ's "Add to overlay", which would otherwise
    // add each measured selection to the regions again.
    run(
        &exec,
        &mut session,
        "measure.set_measurements",
        Args::new()
            .with("area", true)
            .with("display_label", true)
            .with("add_to_overlay", true),
    );
    run(&exec, &mut session, "regions.measure", Args::new());
    let table = session.workspace.table("Results").unwrap();
    let label = |row: usize| table.get(row, "Label").as_text().map(str::to_string);
    assert!(
        label(0).is_some_and(|l| l.ends_with(":Left")),
        "{:?}",
        label(0)
    );
    assert!(label(1).is_some_and(|l| !l.contains(':')), "{:?}", label(1));
    assert_eq!(regions(&session, id).len(), 2);
}

#[test]
fn measure_says_which_region_could_not_be_measured() {
    let (exec, mut session, _) = setup();
    add(&exec, &mut session, "rectangle 4 4 10 8", None);
    add(&exec, &mut session, "rectangle 200 200 10 8", None);
    let err = exec
        .run(&mut session, "regions.measure", &Args::new())
        .unwrap_err();
    assert!(err.to_string().contains("region 2"), "{err}");
    // Nothing of the failed run is kept.
    assert!(session.workspace.table("Results").is_none());
    let err = exec
        .run(
            &mut session,
            "regions.measure",
            &Args::new().with("regions", "5"),
        )
        .unwrap_err();
    assert!(err.to_string().contains("no region 5"), "{err}");
}

#[test]
fn a_set_round_trips_through_a_zip_file() {
    let (exec, mut session, id) = with_shapes();
    run(
        &exec,
        &mut session,
        "regions.set_color",
        Args::new().with("color", "#ff8000").with("regions", "2"),
    );
    let saved = regions(&session, id);
    let path = temp("round-trip.zip");
    let out = run(
        &exec,
        &mut session,
        "regions.save",
        Args::new().with("path", path.clone()),
    );
    assert_eq!(out.get("count"), Some(&Value::Int(6)));

    // Into another image, where there are none.
    let (exec, mut other, other_id) = setup();
    let out = run(
        &exec,
        &mut other,
        "regions.open",
        Args::new().with("path", path.clone()),
    );
    assert_eq!(out.get("opened"), Some(&Value::Int(6)));
    assert_eq!(out.get("skipped"), Some(&Value::Int(0)));
    let opened = regions(&other, other_id);
    assert_eq!(opened.len(), 6);
    for (a, b) in saved.iter().zip(&opened) {
        assert_eq!(a.roi, b.roi);
        assert_eq!(a.props.name, b.props.name);
        assert_eq!(a.props.stroke_color, b.props.stroke_color);
    }

    // Opening again adds to the list; `replace` starts it afresh.
    run(
        &exec,
        &mut other,
        "regions.open",
        Args::new().with("path", path.clone()),
    );
    assert_eq!(regions(&other, other_id).len(), 12);
    assert_eq!(exec.undo(&mut other).unwrap(), "Open Regions");
    assert_eq!(regions(&other, other_id).len(), 6);
    run(
        &exec,
        &mut other,
        "regions.open",
        Args::new().with("path", path).with("replace", true),
    );
    assert_eq!(regions(&other, other_id).len(), 6);
}

#[test]
fn one_region_is_saved_and_opened_as_a_selection_file() {
    let (exec, mut session, id) = with_shapes();
    let path = temp("single.roi");
    run(
        &exec,
        &mut session,
        "regions.save",
        Args::new().with("path", path.clone()).with("regions", "2"),
    );
    // It is the file Save Selection writes: ImageJ's own format.
    let entry = calipix_roi::decode(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(entry.roi, Roi::oval(21.0, 6.0, 16.0, 12.0));
    assert_eq!(entry.props.name.as_deref(), Some("Blob"));
    let err = exec
        .run(
            &mut session,
            "regions.save",
            &Args::new().with("path", temp("several.roi")),
        )
        .unwrap_err();
    assert!(err.to_string().contains("holds one selection"), "{err}");

    run(
        &exec,
        &mut session,
        "regions.open",
        Args::new().with("path", path),
    );
    let listed = regions(&session, id);
    assert_eq!(listed.len(), 7);
    assert_eq!(listed[6].roi, listed[1].roi);
}

#[test]
fn sets_and_selections_written_by_imagej_open() {
    let (exec, mut session, id) = setup();
    let out = run(
        &exec,
        &mut session,
        "regions.open",
        Args::new().with("path", fixture("roiset.zip")),
    );
    let Some(Value::Int(opened)) = out.get("opened") else {
        panic!("no count of regions opened");
    };
    assert!(*opened > 1);
    let listed = regions(&session, id);
    assert_eq!(listed.len() as i64, *opened);
    // A set's entries are named after their files where they have no name.
    assert!(listed.iter().all(|entry| entry.props.name.is_some()));

    // A single selection file is called after the file.
    run(
        &exec,
        &mut session,
        "regions.open",
        Args::new().with("path", fixture("roi_angle_int.roi")),
    );
    let last = regions(&session, id).pop().unwrap();
    assert_eq!(last.roi.kind(), Kind::Angle);
    assert!(last.props.name.is_some());

    let err = exec
        .run(
            &mut session,
            "regions.open",
            &Args::new().with("path", fixture("gray8.png")),
        )
        .unwrap_err();
    assert!(err.to_string().contains("cannot open"), "{err}");
    let err = exec
        .run(
            &mut session,
            "regions.open",
            &Args::new().with("path", temp("missing.zip")),
        )
        .unwrap_err();
    assert!(err.to_string().contains("cannot read"), "{err}");
}

#[test]
fn commands_on_regions_need_regions() {
    let (exec, mut session, _) = setup();
    for (id, args) in [
        ("regions.delete", Args::new().with("regions", "1")),
        ("regions.select", Args::new()),
        ("regions.rename", Args::new().with("name", "x")),
        ("regions.measure", Args::new()),
        ("regions.save", Args::new().with("path", temp("none.zip"))),
        ("regions.set_color", Args::new()),
    ] {
        let err = exec.run(&mut session, id, &args).unwrap_err();
        assert!(err.to_string().contains("no regions"), "{id}: {err}");
    }
    assert!(session.history().entries().is_empty());
}
