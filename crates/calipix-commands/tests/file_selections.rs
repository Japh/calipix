//! The selection and overlay ImageJ stores in a TIFF file, through
//! `file.open`, `file.save` and `file.save_as`.
//!
//! `fixtures/imagej/out/gray8_roi_overlay.tif` was saved by ImageJ 1.54p
//! with an oval selected and an overlay of a red rectangle and a green
//! line two pixels wide (`fixtures/imagej/gen/04_luts_calibration.ijm`).

use std::path::PathBuf;
use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Executor, Outputs, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::Image;
use calipix_roi::{Roi, decode_hex};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

/// A temporary file that no other test uses.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-file-selections-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

fn open(exec: &Executor, session: &mut Session, path: PathBuf) -> (DatasetId, Outputs) {
    let out = exec
        .run(session, "file.open", &Args::new().with("path", path))
        .unwrap();
    match out.get("image") {
        Some(&Value::Image(id)) => (id, out),
        other => panic!("file.open returned {other:?}"),
    }
}

fn roi(text: &str) -> Roi {
    text.parse().unwrap()
}

/// The selection properties of a file as the TIFF reader gives them.
fn stored(path: &PathBuf) -> (Option<String>, Vec<String>) {
    let image = calipix_io::open(path).unwrap();
    let properties = &image.meta().properties;
    let overlay = (0..)
        .map_while(|i| properties.get(&format!("imagej.overlay.{i}")).cloned())
        .collect();
    (properties.get("imagej.roi").cloned(), overlay)
}

#[test]
fn an_imagej_file_opens_with_its_selection_and_overlay_and_saves_them_back() {
    let (exec, mut session) = setup();
    let source = fixture("gray8_roi_overlay.tif");
    let (id, out) = open(&exec, &mut session, source.clone());
    assert!(out.get("warning").is_none(), "{out:?}");
    let dataset = session.workspace.get(id).unwrap();
    assert_eq!(dataset.roi, Some(roi("oval 20 10 15 20")));
    assert_eq!(
        dataset.overlay,
        [roi("rectangle 4 4 10 10"), roi("line 2 40 60 44 width 2")]
    );

    // Undo closes the image again; redo brings it back with its selection.
    assert_eq!(exec.undo(&mut session).unwrap(), "Open");
    assert!(session.workspace.get(id).is_err());
    exec.redo(&mut session).unwrap();
    assert_eq!(
        session.workspace.get(id).unwrap().roi,
        Some(roi("oval 20 10 15 20"))
    );

    // Saved and opened again, they are still there, and the file holds
    // them byte for byte as ImageJ wrote them: the colours and the stroke
    // width of the overlay are kept although the workspace holds shapes.
    let copy = temp("round-trip.tif");
    let out = exec
        .run(
            &mut session,
            "file.save_as",
            &Args::new().with("path", copy.clone()),
        )
        .unwrap();
    assert!(out.get("warning").is_none(), "{out:?}");
    let imagej = stored(&source);
    assert!(imagej.0.is_some() && imagej.1.len() == 2);
    assert_eq!(stored(&copy), imagej);

    let (again, _) = open(&exec, &mut session, copy.clone());
    let reopened = session.workspace.get(again).unwrap();
    assert_eq!(reopened.roi, Some(roi("oval 20 10 15 20")));
    assert_eq!(reopened.overlay.len(), 2);
    let colours: Vec<_> = imagej
        .1
        .iter()
        .map(|hex| decode_hex(hex).unwrap().props.stroke_color)
        .collect();
    assert!(colours.iter().all(Option::is_some));

    // Save (to the file it now belongs to) does the same.
    exec.run(&mut session, "file.save", &Args::new().with("input", again))
        .unwrap();
    assert_eq!(stored(&copy), imagej);
}

#[test]
fn what_changed_is_written_and_what_was_removed_is_not() {
    let (exec, mut session) = setup();
    let source = fixture("gray8_roi_overlay.tif");
    let imagej = stored(&source);
    let (id, _) = open(&exec, &mut session, source);

    // Another selection, and one more element in the overlay.
    exec.run(
        &mut session,
        "select.set",
        &Args::new().with("selection", "polygon 5,5 40,8 20,30"),
    )
    .unwrap();
    let mut overlay = session.workspace.get(id).unwrap().overlay.clone();
    overlay.push(roi("ellipse 10 10 50 35 0.5"));
    session.workspace.set_overlay(id, overlay).unwrap();
    let changed = temp("changed.tif");
    exec.run(
        &mut session,
        "file.save_as",
        &Args::new().with("path", changed.clone()),
    )
    .unwrap();
    let (selection, overlay) = stored(&changed);
    assert_eq!(
        decode_hex(&selection.unwrap()).unwrap().roi,
        roi("polygon 5,5 40,8 20,30")
    );
    assert_eq!(overlay.len(), 3);
    // The two elements ImageJ wrote are unchanged, colours and all.
    assert_eq!(overlay[..2], imagej.1[..]);
    assert_eq!(
        decode_hex(&overlay[2]).unwrap().roi,
        roi("ellipse 10 10 50 35 0.5")
    );
    let (again, _) = open(&exec, &mut session, changed);
    let reopened = session.workspace.get(again).unwrap();
    assert_eq!(reopened.roi, Some(roi("polygon 5,5 40,8 20,30")));
    assert_eq!(reopened.overlay.len(), 3);

    // No selection, and only the second element left.
    exec.run(&mut session, "select.none", &Args::new().with("input", id))
        .unwrap();
    session
        .workspace
        .set_overlay(id, vec![roi("line 2 40 60 44 width 2")])
        .unwrap();
    let fewer = temp("fewer.tif");
    exec.run(
        &mut session,
        "file.save_as",
        &Args::new().with("input", id).with("path", fewer.clone()),
    )
    .unwrap();
    let (selection, overlay) = stored(&fewer);
    assert_eq!(selection, None);
    assert_eq!(overlay, [imagej.1[1].clone()]);
    let (last, _) = open(&exec, &mut session, fewer);
    let reopened = session.workspace.get(last).unwrap();
    assert_eq!(reopened.roi, None);
    assert_eq!(reopened.overlay, [roi("line 2 40 60 44 width 2")]);
}

#[test]
fn an_image_made_here_is_saved_with_its_selection_and_overlay() {
    let (exec, mut session) = setup();
    let image = Image::from_vec(64, 48, vec![7u8; 64 * 48]).unwrap();
    let id = session.workspace.insert(image, None);
    let shapes = [
        "rounded-rectangle 10 8 30 20 10",
        "polyline 5,5 20,30 40,10 60,40 width 6",
        "points 5,5 20,30 40,12",
        "rotated-rectangle 10 10 50 35 12",
    ];
    session
        .workspace
        .set_roi(id, Some(roi("freehand 10,12 14,6 22,4 30,8 36,16 34,28")))
        .unwrap();
    session
        .workspace
        .set_overlay(id, shapes.iter().map(|s| roi(s)).collect())
        .unwrap();
    let path = temp("made-here.tif");
    exec.run(
        &mut session,
        "file.save_as",
        &Args::new().with("path", path.clone()),
    )
    .unwrap();
    let (again, out) = open(&exec, &mut session, path);
    assert!(out.get("warning").is_none(), "{out:?}");
    let reopened = session.workspace.get(again).unwrap();
    assert_eq!(
        reopened.roi,
        Some(roi("freehand 10,12 14,6 22,4 30,8 36,16 34,28"))
    );
    let texts: Vec<String> = reopened.overlay.iter().map(Roi::to_string).collect();
    assert_eq!(texts, shapes);
}

/// The bytes of a rectangle selection ImageJ saved, as hex.
fn rectangle_hex(edit: impl Fn(&mut Vec<u8>)) -> String {
    let mut bytes = std::fs::read(fixture("roi_rect_int.roi")).unwrap();
    edit(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn stored_selections_that_cannot_be_read_do_not_stop_the_image_opening() {
    let (exec, mut session) = setup();
    // A file whose selection is damaged, and whose overlay holds a
    // rectangle, a text element (subtype 1, which Calipix has no shape
    // for) and something that is not a selection at all.
    let mut image = Image::from_vec(32, 24, vec![3u8; 32 * 24]).unwrap();
    let mut meta = image.meta().clone();
    let text = rectangle_hex(|bytes| bytes[49] = 1);
    for (key, value) in [
        ("imagej.roi", rectangle_hex(|bytes| bytes.truncate(20))),
        ("imagej.overlay.0", rectangle_hex(|_| {})),
        ("imagej.overlay.1", text.clone()),
        ("imagej.overlay.2", "00112233".to_string()),
    ] {
        meta.properties.insert(key.to_string(), value);
    }
    image = image.with_meta(meta).unwrap();
    let path = temp("damaged.tif");
    calipix_io::save(&image, &path).unwrap();

    let (id, out) = open(&exec, &mut session, path);
    let dataset = session.workspace.get(id).unwrap();
    assert_eq!(dataset.image.meta().width(), 32);
    assert_eq!(dataset.roi, None);
    assert_eq!(dataset.overlay, [roi("rectangle 10 8 20 12")]);
    let Some(Value::Str(warning)) = out.get("warning") else {
        panic!("no warning: {out:?}");
    };
    assert!(
        warning.contains("the selection stored in the file was not read"),
        "{warning}"
    );
    assert!(
        warning
            .contains("element 2 of the overlay was left out: text selections are not supported"),
        "{warning}"
    );
    assert!(
        warning.contains("element 3 of the overlay was left out"),
        "{warning}"
    );

    // Saving keeps the text element, which could not be shown or changed,
    // after the rectangle it followed; what was damaged is gone.
    let saved = temp("damaged-saved.tif");
    exec.run(
        &mut session,
        "file.save_as",
        &Args::new().with("path", saved.clone()),
    )
    .unwrap();
    let (selection, overlay) = stored(&saved);
    assert_eq!(selection, None);
    assert_eq!(overlay, [rectangle_hex(|_| {}), text]);
}

/// What `tests/support/imagej_tiff_selection_check.ijm` prints for an
/// image with this selection and overlay.
fn report(name: &str, size: (usize, usize), selection: Option<&Roi>, overlay: &[Roi]) -> String {
    let bounds = |roi: &Roi| {
        let (x, y, w, h) = roi.int_bounds();
        format!("{x},{y},{w},{h}")
    };
    let mut text = format!("{name} {}x{} selection=", size.0, size.1);
    match selection {
        Some(roi) => text.push_str(&format!("{} bounds={}", roi.ij_type(), bounds(roi))),
        None => text.push_str("-1"),
    }
    text.push_str(&format!(" overlay={}\n", overlay.len()));
    for (i, roi) in overlay.iter().enumerate() {
        text.push_str(&format!(
            "{name} overlay {i} type={} bounds={}\n",
            roi.ij_type(),
            bounds(roi)
        ));
    }
    text
}

/// Saves images with selections and overlays through the commands into
/// `$CALIPIX_FILE_CHECK_DIR`, with the report ImageJ should print when it
/// opens them. Not part of `cargo test`; see
/// `tests/support/imagej_tiff_selection_check.ijm`.
#[test]
#[ignore = "writes files for a manual check with ImageJ"]
fn write_files_for_imagej_check() {
    let dir = PathBuf::from(
        std::env::var("CALIPIX_FILE_CHECK_DIR").expect("set CALIPIX_FILE_CHECK_DIR to a directory"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let (exec, mut session) = setup();
    let mut expected = String::new();
    let mut save = |session: &mut Session, id: DatasetId, name: &str| {
        exec.run(
            session,
            "file.save_as",
            &Args::new()
                .with("input", id)
                .with("path", dir.join(format!("{name}.tif"))),
        )
        .unwrap();
        let dataset = session.workspace.get(id).unwrap();
        let meta = dataset.image.meta();
        expected.push_str(&report(
            name,
            (meta.width(), meta.height()),
            dataset.roi.as_ref(),
            &dataset.overlay,
        ));
    };

    // An image made here, with a selection and an overlay of several kinds.
    let samples: Vec<u8> = (0..64 * 48).map(|i| (i % 251) as u8).collect();
    let made = session
        .workspace
        .insert(Image::from_vec(64, 48, samples.clone()).unwrap(), None);
    session
        .workspace
        .set_roi(made, Some(roi("polygon 5,5 40,8 20,30")))
        .unwrap();
    session
        .workspace
        .set_overlay(
            made,
            vec![
                roi("rectangle 4 4 10 10"),
                roi("oval 20 10 15 20"),
                roi("line 2 40 60 44 width 3"),
                roi("polyline 5,5 20,30 40,10 60,40"),
                roi("points 5,5 20,30 40,12"),
                roi("ellipse 10 10 50 35 0.5"),
                roi("rounded-rectangle 10 8 30 20 10"),
            ],
        )
        .unwrap();
    save(&mut session, made, "made_here");

    // ImageJ's own file, saved again unchanged, then changed.
    let (theirs, _) = open(&exec, &mut session, fixture("gray8_roi_overlay.tif"));
    save(&mut session, theirs, "imagej_resaved");
    exec.run(
        &mut session,
        "select.set",
        &Args::new()
            .with("input", theirs)
            .with("selection", "freehand 10,12 14,6 22,4 30,8 36,16 34,28"),
    )
    .unwrap();
    let mut overlay = session.workspace.get(theirs).unwrap().overlay.clone();
    overlay.push(roi("oval 30 20 12 9"));
    session.workspace.set_overlay(theirs, overlay).unwrap();
    save(&mut session, theirs, "changed");

    // Neither a selection nor an overlay.
    let plain = session
        .workspace
        .insert(Image::from_vec(64, 48, samples).unwrap(), None);
    save(&mut session, plain, "plain");

    expected.push_str("CHECK_DONE\n");
    std::fs::write(dir.join("expected.txt"), expected).unwrap();
}
