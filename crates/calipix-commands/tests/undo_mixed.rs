//! Undo and redo across a long, mixed session: every step must come back
//! exactly, in both directions.

use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Executor, Session};
use calipix_commands::standard_registry;
use calipix_core::{Buffer, DType, Image, ImageMeta};

/// Everything undo is responsible for, as comparable values.
#[derive(Debug, PartialEq)]
struct Snapshot {
    images: Vec<(DatasetId, ImageMeta, Vec<Buffer>)>,
    selections: Vec<Option<String>>,
    regions: Vec<usize>,
    tables: Vec<(String, usize)>,
    active: Option<DatasetId>,
}

fn snapshot(session: &Session) -> Snapshot {
    let ws = &session.workspace;
    let datasets: Vec<_> = ws.datasets().collect();
    Snapshot {
        images: datasets
            .iter()
            .map(|d| {
                let meta = d.image.meta().clone();
                let planes = (0..meta.plane_count())
                    .map(|p| d.image.read_plane(p).unwrap())
                    .collect();
                (d.id, meta, planes)
            })
            .collect(),
        selections: datasets
            .iter()
            .map(|d| d.roi.as_ref().map(ToString::to_string))
            .collect(),
        regions: datasets.iter().map(|d| d.overlay.len()).collect(),
        tables: ws
            .table_names()
            .map(|name| (name.to_string(), ws.table(name).unwrap().len()))
            .collect(),
        active: ws.active(),
    }
}

/// A 3-slice 16-bit stack with structure, so that every operation changes
/// something.
fn stack() -> Image {
    let meta = ImageMeta::xyczt(DType::U16, 32, 24, 1, 3, 1).unwrap();
    let planes = (0..3)
        .map(|z| {
            Buffer::from_vec(
                (0..32 * 24)
                    .map(|i| ((i * 37 + z * 1000) % 4096) as u16)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    Image::from_planes(meta, planes).unwrap()
}

#[test]
fn twenty_mixed_steps_undo_and_redo_exactly() {
    let exec = Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all());
    let mut session = Session::new();
    session.workspace.insert(stack(), None);

    let args = |pairs: &[(&str, &str)]| {
        let mut args = Args::new();
        for (name, value) in pairs {
            args.insert(*name, *value);
        }
        args
    };
    let steps: Vec<(&str, Args)> = vec![
        ("filter.gaussian_blur", args(&[]).with("sigma", 1.5)),
        ("select.set", args(&[("selection", "oval 4 3 20 15")])),
        ("measure.measure", args(&[])),
        ("math.add", args(&[]).with("value", 100.0)),
        ("regions.add", args(&[])),
        (
            "filter.median",
            args(&[]).with("radius", 1.0).with("stack", true),
        ),
        ("measure.measure", args(&[]).with("slice", 2)),
        (
            "select.set",
            args(&[("selection", "polygon 2,2 28,4 16,20")]),
        ),
        ("image.invert", args(&[]).with("stack", true)),
        ("regions.add", args(&[])),
        ("view.set_lut", args(&[("lut", "fire")])),
        ("select.invert", args(&[])),
        ("math.multiply", args(&[]).with("value", 1.25)),
        (
            "measure.set_measurements",
            args(&[]).with("perimeter", true),
        ),
        ("measure.measure", args(&[])),
        ("select.none", args(&[])),
        ("image.flip_horizontal", args(&[]).with("stack", true)),
        ("image.to_32bit", args(&[])),
        ("select.set", args(&[("selection", "rectangle 5 5 12 10")])),
        ("image.crop", args(&[])),
        ("image.rotate_right", args(&[])),
        ("measure.clear_results", args(&[])),
    ];
    assert!(steps.len() >= 20);

    // Forward, remembering the state after every step.
    let mut states = vec![snapshot(&session)];
    for (id, step_args) in &steps {
        exec.run(&mut session, id, step_args)
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        states.push(snapshot(&session));
    }
    // Every step is meant to change something undo cares about.
    for (i, pair) in states.windows(2).enumerate() {
        assert_ne!(
            pair[0],
            pair[1],
            "step {} ({}) changed nothing",
            i + 1,
            steps[i].0
        );
    }

    // All the way back, then all the way forward again, twice.
    for _ in 0..2 {
        for expected in states.iter().rev().skip(1) {
            exec.undo(&mut session).unwrap();
            assert_eq!(&snapshot(&session), expected);
        }
        assert!(exec.undo(&mut session).is_err(), "nothing is left to undo");
        for expected in states.iter().skip(1) {
            exec.redo(&mut session).unwrap();
            assert_eq!(&snapshot(&session), expected);
        }
        assert!(exec.redo(&mut session).is_err(), "nothing is left to redo");
    }
}

/// Undo keeps earlier states by sharing what did not change, so a step
/// that touches one plane of a stack must not copy the others.
#[test]
fn history_shares_the_planes_a_step_did_not_touch() {
    let exec = Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all());
    let mut session = Session::new();
    let id = session.workspace.insert(stack(), None);
    let before = session.workspace.image(id).unwrap().clone();

    for slice in [2, 2, 2, 2, 2] {
        exec.run(
            &mut session,
            "math.add",
            &Args::new().with("value", 1.0).with("slice", slice),
        )
        .unwrap();
    }
    let after = session.workspace.image(id).unwrap();
    for untouched in [0, 2] {
        assert!(
            after
                .read_plane(untouched)
                .unwrap()
                .shares_storage(&before.read_plane(untouched).unwrap()),
            "plane {untouched} was copied"
        );
    }
    assert!(
        !after
            .read_plane(1)
            .unwrap()
            .shares_storage(&before.read_plane(1).unwrap())
    );
}
