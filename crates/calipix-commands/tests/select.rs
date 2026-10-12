//! The selection commands, run the way every frontend runs them.

use std::path::PathBuf;
use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Error, Executor, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::Image;
use calipix_roi::{Kind, Roi};

const WIDTH: usize = 64;
const HEIGHT: usize = 48;

fn setup() -> (Executor, Session, DatasetId) {
    let exec = Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all());
    let mut session = Session::new();
    let image = Image::from_vec(WIDTH, HEIGHT, vec![0u8; WIDTH * HEIGHT]).unwrap();
    let id = session.workspace.insert(image, None);
    (exec, session, id)
}

/// A file ImageJ wrote (`fixtures/imagej/gen/40_roi_shapes.ijm`).
fn fixture(case: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(format!("roi_{case}.roi"))
}

fn imagej(case: &str) -> Roi {
    calipix_roi::decode(&std::fs::read(fixture(case)).unwrap())
        .unwrap()
        .roi
}

/// A temporary file that no other test uses.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-select-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn roi(session: &Session, id: DatasetId) -> Option<Roi> {
    session.workspace.get(id).unwrap().roi.clone()
}

fn select(exec: &Executor, session: &mut Session, text: &str) {
    // As typed on the command line: the selection is text.
    exec.run(
        session,
        "select.set",
        &Args::new().with("selection", Value::from(text)),
    )
    .unwrap_or_else(|e| panic!("{text}: {e}"));
}

fn count(roi: &Roi) -> usize {
    roi.mask(WIDTH, HEIGHT).map_or(0, |m| m.count())
}

#[test]
fn every_kind_of_selection_can_be_specified_as_text() {
    let (exec, mut session, id) = setup();
    for (text, kind) in [
        ("rectangle 10 8 20 12", Kind::Rectangle),
        ("rounded-rectangle 10 8 30 20 10", Kind::RoundedRectangle),
        ("oval 20 10 15 20", Kind::Oval),
        ("polygon 40,5 60,10 50,40", Kind::Polygon),
        ("freehand 10,12 14,6 22,4 30,8", Kind::Freehand),
        ("traced 5,5 9,5 9,7 5,7", Kind::Traced),
        ("line 2 40 60 44 width 5", Kind::Line),
        ("polyline 5,5 20,30 40,10", Kind::Polyline),
        ("freeline 5,40 9,30 15,24", Kind::Freeline),
        ("angle 10,40 30,10 50,35", Kind::Angle),
        ("points 5,5 20,30", Kind::Points),
        ("ellipse 10 10 50 35 0.5", Kind::Ellipse),
        ("rotated-rectangle 10 10 50 35 12", Kind::RotatedRectangle),
        ("composite M 0,0 L 10,0 L 10,10 L 0,10 Z M 3,3 L 7,3 L 7,7 L 3,7 Z", Kind::Composite),
    ] {
        select(&exec, &mut session, text);
        let roi = roi(&session, id).unwrap();
        assert_eq!(roi.kind(), kind);
        // History shows the selection as it was typed.
        assert_eq!(roi.to_string(), text);
    }
    let err = exec
        .run(
            &mut session,
            "select.set",
            &Args::new().with("selection", Value::from("blob 1 2")),
        )
        .unwrap_err();
    assert!(err.to_string().contains("unknown shape"), "{err}");
}

#[test]
fn invert_follows_imagej_and_undoes() {
    let (exec, mut session, id) = setup();
    // Without a selection, Make Inverse selects everything.
    exec.run(&mut session, "select.invert", &Args::new()).unwrap();
    assert_eq!(
        roi(&session, id),
        Some(Roi::rectangle(0.0, 0.0, WIDTH as f64, HEIGHT as f64))
    );
    // The inverse of everything is nothing.
    exec.run(&mut session, "select.invert", &Args::new()).unwrap();
    assert_eq!(roi(&session, id), None);

    select(&exec, &mut session, "oval 10 8 25 20");
    let oval = roi(&session, id).unwrap();
    exec.run(&mut session, "select.invert", &Args::new()).unwrap();
    let inverse = roi(&session, id).unwrap();
    assert_eq!(count(&inverse), WIDTH * HEIGHT - count(&oval));
    assert_eq!(
        inverse.mask(WIDTH, HEIGHT),
        imagej("inverse_oval").mask(WIDTH, HEIGHT)
    );
    assert_eq!(exec.undo(&mut session).unwrap(), "Invert Selection");
    assert_eq!(roi(&session, id), Some(oval));

    select(&exec, &mut session, "line 2 40 60 44");
    let err = exec
        .run(&mut session, "select.invert", &Args::new())
        .unwrap_err();
    assert!(err.to_string().contains("only an area selection"), "{err}");
}

#[test]
fn enlarge_scale_rotate_and_translate_match_imagej() {
    let (exec, mut session, id) = setup();
    let run = |session: &mut Session, command: &str, args: Args| {
        exec.run(session, command, &args)
            .unwrap_or_else(|e| panic!("{command}: {e}"));
    };

    select(&exec, &mut session, "polygon 5,5 30,8 18,20 32,40 4,30 12,18");
    run(&mut session, "select.enlarge", Args::new().with("pixels", 3));
    assert_eq!(roi(&session, id), Some(imagej("enlarge_poly_3")));

    select(&exec, &mut session, "rectangle 10 8 20 12");
    run(&mut session, "select.enlarge", Args::new().with("pixels", -3));
    assert_eq!(roi(&session, id), Some(imagej("enlarge_rect_m3")));
    let err = exec
        .run(
            &mut session,
            "select.enlarge",
            &Args::new().with("pixels", -9),
        )
        .unwrap_err();
    assert!(err.to_string().contains("would leave nothing"), "{err}");
    assert_eq!(roi(&session, id), Some(imagej("enlarge_rect_m3")));

    select(&exec, &mut session, "polygon 40,5 60,10 50,40");
    let scale = Args::new().with("x_scale", 0.5).with("y_scale", 0.75);
    run(&mut session, "select.scale", scale.clone());
    assert_eq!(roi(&session, id), Some(imagej("scale_poly")));
    select(&exec, &mut session, "polygon 40,5 60,10 50,40");
    run(&mut session, "select.scale", scale.with("centered", true));
    assert_eq!(roi(&session, id), Some(imagej("scale_poly_centered")));

    select(&exec, &mut session, "polygon 40,5 60,10 50,40");
    run(&mut session, "select.rotate", Args::new().with("angle", 90.0));
    assert_eq!(roi(&session, id), Some(imagej("rotate_poly_90")));
    select(&exec, &mut session, "rectangle 20 14 24 12");
    run(
        &mut session,
        "select.rotate",
        Args::new()
            .with("angle", 180.0)
            .with("around_image_center", true),
    );
    // Half a turn about the middle of a 64x48 image.
    assert_eq!(
        roi(&session, id).unwrap().int_bounds(),
        (20, 22, 24, 12),
        "{:?}",
        roi(&session, id)
    );

    select(&exec, &mut session, "oval 20 10 15 20");
    run(
        &mut session,
        "select.translate",
        Args::new().with("x_offset", 5.0).with("y_offset", -3.0),
    );
    assert_eq!(roi(&session, id), Some(Roi::oval(25.0, 7.0, 15.0, 20.0)));
}

#[test]
fn conversions_match_imagej() {
    let (exec, mut session, id) = setup();
    let run = |session: &mut Session, command: &str| {
        exec.run(session, command, &Args::new())
            .unwrap_or_else(|e| panic!("{command}: {e}"));
    };
    let same_pixels = |a: &Roi, b: &Roi| a.mask(WIDTH, HEIGHT) == b.mask(WIDTH, HEIGHT);

    select(&exec, &mut session, "polygon 5,5 30,8 18,20 32,40 4,30 12,18");
    run(&mut session, "select.fit_ellipse");
    let fitted = roi(&session, id).unwrap();
    assert_eq!(fitted.kind(), Kind::Ellipse);
    assert!(same_pixels(&fitted, &imagej("fit_ellipse_poly")));

    select(&exec, &mut session, "polygon 5,5 30,8 18,20 32,40 4,30 12,18");
    run(&mut session, "select.convex_hull");
    assert_eq!(roi(&session, id), Some(imagej("hull_poly")));

    select(&exec, &mut session, "oval 20.5 10.25 15.3 20.6");
    run(&mut session, "select.to_bounding_box");
    assert_eq!(roi(&session, id), Some(imagej("box_oval_sub")));

    select(&exec, &mut session, "polyline 5,5 20,30 40,10 60,40 width 6");
    run(&mut session, "select.line_to_area");
    assert_eq!(roi(&session, id), Some(imagej("area_polyline_wide")));

    select(&exec, &mut session, "line 5 5 50 30 width 7");
    run(&mut session, "select.line_to_area");
    assert!(same_pixels(
        &roi(&session, id).unwrap(),
        &imagej("area_line_wide")
    ));

    // Each refuses what it cannot work on, and says why.
    for (selection, command, reason) in [
        ("line 2 40 60 44", "select.fit_ellipse", "needs an area selection"),
        ("line 2 40 60 44", "select.enlarge", "needs an area selection"),
        ("line 2 40 60 44", "select.convex_hull", "a convex hull needs"),
        ("oval 20 10 15 20", "select.line_to_area", "needs a line selection"),
    ] {
        select(&exec, &mut session, selection);
        let err = exec.run(&mut session, command, &Args::new()).unwrap_err();
        assert!(err.to_string().contains(reason), "{command}: {err}");
    }
}

#[test]
fn commands_that_need_a_selection_say_so() {
    let (exec, mut session, id) = setup();
    for command in [
        "select.enlarge",
        "select.scale",
        "select.rotate",
        "select.translate",
        "select.fit_ellipse",
        "select.convex_hull",
        "select.to_bounding_box",
        "select.line_to_area",
    ] {
        let err = exec.run(&mut session, command, &Args::new()).unwrap_err();
        assert!(matches!(err, Error::Failed(_)), "{command}: {err}");
        assert!(err.to_string().contains("no selection"), "{command}: {err}");
        assert_eq!(roi(&session, id), None);
    }
    let err = exec
        .run(
            &mut session,
            "select.save",
            &Args::new().with("path", temp("needs-a-selection.roi")),
        )
        .unwrap_err();
    assert!(err.to_string().contains("no selection"), "{err}");
    assert!(!temp("needs-a-selection.roi").exists());
}

#[test]
fn selections_open_from_and_save_to_imagej_files() {
    let (exec, mut session, id) = setup();
    for case in ["oval_int", "polyf_tri", "comp_hole", "line_wide", "points_sub", "ellipse"] {
        exec.run(
            &mut session,
            "select.open",
            &Args::new().with("path", fixture(case)),
        )
        .unwrap();
        assert_eq!(roi(&session, id), Some(imagej(case)), "{case}");

        let saved = temp(&format!("saved-{case}.roi"));
        exec.run(
            &mut session,
            "select.save",
            &Args::new().with("path", saved.clone()),
        )
        .unwrap();
        exec.run(&mut session, "select.none", &Args::new()).unwrap();
        exec.run(&mut session, "select.open", &Args::new().with("path", saved))
            .unwrap();
        assert_eq!(roi(&session, id), Some(imagej(case)), "{case}");
    }
    // Opening is undone like any other change of selection.
    exec.undo(&mut session).unwrap();
    assert_eq!(roi(&session, id), None);
}

#[test]
fn files_that_are_not_selections_are_refused() {
    let (exec, mut session, id) = setup();
    select(&exec, &mut session, "oval 20 10 15 20");
    let before = roi(&session, id);
    let open = |session: &mut Session, path: PathBuf| {
        exec.run(session, "select.open", &Args::new().with("path", path))
            .unwrap_err()
            .to_string()
    };

    let missing = open(&mut session, temp("no-such-file.roi"));
    assert!(missing.contains("cannot read"), "{missing}");

    let garbage = temp("garbage.roi");
    std::fs::write(&garbage, b"this is not a selection").unwrap();
    let message = open(&mut session, garbage);
    assert!(message.contains("not a valid ImageJ selection"), "{message}");

    let truncated = temp("truncated.roi");
    let bytes = std::fs::read(fixture("poly_concave")).unwrap();
    std::fs::write(&truncated, &bytes[..70]).unwrap();
    let message = open(&mut session, truncated);
    assert!(message.contains("not a valid ImageJ selection"), "{message}");

    let set = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/imagej/out/roiset.zip");
    let message = open(&mut session, set);
    assert!(message.contains("set of selections"), "{message}");

    assert_eq!(roi(&session, id), before);
}

#[test]
fn a_read_only_policy_allows_opening_but_not_saving() {
    let (mut exec, mut session, id) = setup();
    exec.policy = EffectsPolicy::read_only();
    exec.run(
        &mut session,
        "select.open",
        &Args::new().with("path", fixture("oval_int")),
    )
    .unwrap();
    assert_eq!(roi(&session, id), Some(imagej("oval_int")));
    let target = temp("blocked.roi");
    let err = exec
        .run(
            &mut session,
            "select.save",
            &Args::new().with("path", target.clone()),
        )
        .unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err}");
    assert!(!target.exists());
}
