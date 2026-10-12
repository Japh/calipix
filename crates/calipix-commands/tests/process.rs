//! The rules every sample-changing command shares, exercised through
//! Invert: which planes are in scope, and confinement to the selection.

use std::sync::Arc;

use calipix_command::{
    Args, CommandSpec, DatasetId, EffectsPolicy, Error, Executor, Outputs, Params, Session, Value,
    typed,
};
use calipix_commands::process::PlaneScope;
use calipix_commands::standard_registry;
use calipix_core::{Buffer, Cell, ColorModel, DType, Image, ImageMeta, Table};
use calipix_roi::Roi;

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

/// A 4 x 3 image with `c` channels, `z` slices and `t` frames whose every
/// sample is its plane number.
fn stack(c: usize, z: usize, t: usize) -> Image {
    let meta = ImageMeta::xyczt(DType::U8, 4, 3, c, z, t).unwrap();
    let planes = (0..meta.plane_count())
        .map(|p| Buffer::from_vec(vec![p as u8; 12]))
        .collect();
    Image::from_planes(meta, planes).unwrap()
}

fn first_samples(session: &Session, id: DatasetId) -> Vec<u8> {
    let image = session.workspace.image(id).unwrap();
    (0..image.meta().plane_count())
        .map(|p| image.read_plane(p).unwrap().as_slice::<u8>().unwrap()[0])
        .collect()
}

#[test]
fn only_the_plane_on_view_is_processed_unless_the_stack_is_asked_for() {
    let (exec, mut session) = setup();
    let id = session.workspace.insert(stack(1, 3, 2), None);

    // Slice 2 of frame 2 is plane 4.
    let args = Args::new().with("slice", 2).with("frame", 2);
    exec.run(&mut session, "image.invert", &args).unwrap();
    assert_eq!(first_samples(&session, id), [0, 1, 2, 3, 251, 5]);

    // The untouched planes are still the very same buffers.
    exec.undo(&mut session).unwrap();
    let before = session.workspace.image(id).unwrap().clone();
    exec.run(&mut session, "image.invert", &args).unwrap();
    let after = session.workspace.image(id).unwrap();
    for plane in [0, 1, 2, 3, 5] {
        assert!(
            after
                .read_plane(plane)
                .unwrap()
                .shares_storage(&before.read_plane(plane).unwrap())
        );
    }

    exec.undo(&mut session).unwrap();
    exec.run(
        &mut session,
        "image.invert",
        &Args::new().with("stack", true),
    )
    .unwrap();
    assert_eq!(first_samples(&session, id), [255, 254, 253, 252, 251, 250]);
}

#[test]
fn channels_are_chosen_by_number_and_rgb_always_goes_together() {
    let (exec, mut session) = setup();
    let id = session.workspace.insert(stack(3, 2, 1), None);

    // Channel 2 of slice 2 is plane 4; channel 0 means all three.
    exec.run(
        &mut session,
        "image.invert",
        &Args::new().with("channel", 2).with("slice", 2),
    )
    .unwrap();
    assert_eq!(first_samples(&session, id), [0, 1, 2, 3, 251, 5]);
    exec.undo(&mut session).unwrap();
    exec.run(&mut session, "image.invert", &Args::new().with("slice", 2))
        .unwrap();
    assert_eq!(first_samples(&session, id), [0, 1, 2, 252, 251, 250]);

    let out_of_range = exec.run(
        &mut session,
        "image.invert",
        &Args::new().with("channel", 4),
    );
    assert!(matches!(out_of_range, Err(Error::InvalidArg { .. })));
    let no_such_slice = exec.run(&mut session, "image.invert", &Args::new().with("slice", 3));
    assert!(matches!(no_such_slice, Err(Error::InvalidArg { .. })));

    // An RGB image is one picture: naming a channel still inverts all three.
    let mut meta = ImageMeta::xyczt(DType::U8, 4, 3, 3, 1, 1).unwrap();
    meta.color = ColorModel::Rgb;
    let planes = (0..3)
        .map(|p| Buffer::from_vec(vec![p as u8; 12]))
        .collect();
    let rgb = session
        .workspace
        .insert(Image::from_planes(meta, planes).unwrap(), None);
    exec.run(
        &mut session,
        "image.invert",
        &Args::new().with("channel", 1),
    )
    .unwrap();
    assert_eq!(first_samples(&session, rgb), [255, 254, 253]);
}

#[test]
fn a_selection_confines_the_change() {
    let (exec, mut session) = setup();
    let id = session
        .workspace
        .insert(Image::from_vec(4, 3, vec![10u8; 12]).unwrap(), None);

    exec.run(
        &mut session,
        "select.set",
        &Args::new().with("selection", "rectangle 1 1 2 1"),
    )
    .unwrap();
    assert_eq!(
        session.workspace.get(id).unwrap().roi,
        Some(Roi::rectangle(1.0, 1.0, 2.0, 1.0))
    );
    exec.run(&mut session, "image.invert", &Args::new())
        .unwrap();
    let plane = session.workspace.image(id).unwrap().read_plane(0).unwrap();
    #[rustfmt::skip]
    assert_eq!(
        plane.as_slice::<u8>().unwrap(),
        &[10,  10,  10, 10,
          10, 245, 245, 10,
          10,  10,  10, 10]
    );

    // A selection wholly outside the image is an error, not a no-op.
    exec.run(
        &mut session,
        "select.set",
        &Args::new().with("selection", "rectangle 50 50 5 5"),
    )
    .unwrap();
    let err = exec
        .run(&mut session, "image.invert", &Args::new())
        .unwrap_err();
    assert!(err.to_string().contains("outside the image"), "{err}");

    exec.run(&mut session, "select.none", &Args::new()).unwrap();
    assert_eq!(session.workspace.get(id).unwrap().roi, None);
    exec.run(&mut session, "select.all", &Args::new()).unwrap();
    assert_eq!(
        session.workspace.get(id).unwrap().roi,
        Some(Roi::rectangle(0.0, 0.0, 4.0, 3.0))
    );
    let bad = exec.run(
        &mut session,
        "select.set",
        &Args::new().with("selection", "rectangle 1 2 3"),
    );
    assert!(matches!(bad, Err(Error::InvalidArg { .. })), "{bad:?}");
}

#[test]
fn selections_are_undone_and_redone_like_any_other_change() {
    let (exec, mut session) = setup();
    let id = session.workspace.insert(stack(1, 1, 1), None);
    let roi = |session: &Session| session.workspace.get(id).unwrap().roi.clone();

    exec.run(&mut session, "select.all", &Args::new()).unwrap();
    exec.run(&mut session, "image.invert", &Args::new())
        .unwrap();
    exec.run(&mut session, "select.none", &Args::new()).unwrap();
    assert_eq!(roi(&session), None);

    assert_eq!(exec.undo(&mut session).unwrap(), "Select None");
    assert!(roi(&session).is_some());
    assert_eq!(exec.undo(&mut session).unwrap(), "Invert");
    assert!(
        roi(&session).is_some(),
        "undoing Invert keeps the selection"
    );
    assert_eq!(first_samples(&session, id), [0]);
    assert_eq!(exec.undo(&mut session).unwrap(), "Select All");
    assert_eq!(roi(&session), None);
    for _ in 0..3 {
        exec.redo(&mut session).unwrap();
    }
    assert_eq!(roi(&session), None);
    assert_eq!(first_samples(&session, id), [255]);
}

#[derive(Params)]
struct NoParams {}

#[test]
fn results_tables_are_part_of_what_undo_restores() {
    let mut registry = standard_registry();
    registry
        .register(typed(
            CommandSpec::new("test.measure", "Measure").summary("Adds a row to the results table."),
            |ctx, _: NoParams| {
                let name = calipix_command::Workspace::RESULTS;
                let mut table = ctx.workspace().table(name).cloned().unwrap_or_default();
                table.push_row([("Area", Cell::from(table.len() as f64))]);
                ctx.workspace_mut().set_table(name, table);
                Ok(Outputs::new())
            },
        ))
        .unwrap();
    let exec = Executor::new(Arc::new(registry), EffectsPolicy::allow_all());
    let mut session = Session::new();
    let rows = |session: &Session| session.workspace.table("Results").map(Table::len);

    exec.run(&mut session, "test.measure", &Args::new())
        .unwrap();
    exec.run(&mut session, "test.measure", &Args::new())
        .unwrap();
    assert_eq!(rows(&session), Some(2));
    assert_eq!(exec.undo(&mut session).unwrap(), "Measure");
    assert_eq!(rows(&session), Some(1));
    exec.undo(&mut session).unwrap();
    assert_eq!(rows(&session), None, "the table did not exist before");
    exec.redo(&mut session).unwrap();
    exec.redo(&mut session).unwrap();
    assert_eq!(rows(&session), Some(2));
    assert_eq!(
        session.workspace.table("Results").unwrap().get(1, "Area"),
        &Cell::Number(1.0)
    );
}

#[test]
fn the_scope_is_shared_by_flattening_it_into_a_commands_parameters() {
    let spec = standard_registry()
        .get("image.invert")
        .unwrap()
        .spec()
        .clone();
    let names: Vec<_> = spec.inputs.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["input", "channel", "slice", "frame", "stack"]);
    // The plane parameters are marked so that an interface fills them from
    // the view instead of asking.
    let marked: Vec<_> = spec
        .inputs
        .iter()
        .filter(|p| p.position.is_some())
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(marked, ["channel", "slice", "frame", "stack"]);
    assert_eq!(
        spec.param("stack").unwrap().ijm_key.as_deref(),
        Some("stack")
    );

    let whole = PlaneScope::whole(DatasetId(0));
    let meta = ImageMeta::xyczt(DType::U8, 2, 2, 2, 3, 1).unwrap();
    assert_eq!(whole.planes(&meta).unwrap(), (0..6).collect::<Vec<_>>());
    assert_eq!(
        Value::from(Roi::rectangle(0.0, 0.0, 1.0, 1.0)).to_text(),
        "rectangle 0 0 1 1"
    );
}
