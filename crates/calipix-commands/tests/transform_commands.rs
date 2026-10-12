//! The geometry commands: defaults on every pixel type, the plane scope,
//! selections, new images, calibration and undo.

use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Executor, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::{Axis, AxisKind, Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut};
use calipix_ops::convert::convert;
use calipix_roi::Roi;

const REAL: [DType; 10] = [
    DType::U8,
    DType::I8,
    DType::U16,
    DType::I16,
    DType::U32,
    DType::I32,
    DType::U64,
    DType::I64,
    DType::F32,
    DType::F64,
];

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

/// A 6 x 4 image of `dtype` with `c` channels, `z` slices and `t` frames;
/// the samples of plane `p` are `10 * p + 1, 10 * p + 2, ...` (mod 256).
fn image(dtype: DType, c: usize, z: usize, t: usize) -> Image {
    let meta = ImageMeta::xyczt(dtype, 6, 4, c, z, t).unwrap();
    let planes = (0..meta.plane_count())
        .map(|p| {
            let values: Vec<u8> = (1..=24).map(|i| (10 * p + i) as u8).collect();
            convert(&Buffer::from_vec(values), dtype).unwrap()
        })
        .collect();
    Image::from_planes(meta.with_name("a.tif"), planes).unwrap()
}

fn rgb() -> Image {
    let source = image(DType::U8, 3, 2, 1);
    let mut meta = source.meta().clone();
    meta.color = ColorModel::Rgb;
    source.with_meta(meta).unwrap()
}

/// `image` with pixels of 0.5 x 0.25 µm and physical zero at pixel (2, 1).
fn calibrated(image: Image) -> Image {
    let mut meta = image.meta().clone();
    for (index, scale, origin) in [(0, 0.5, 2.0), (1, 0.25, 1.0)] {
        let axis = meta.axis_mut(index).unwrap();
        axis.scale = scale;
        axis.unit = Some("µm".to_string());
        axis.origin = -origin * scale;
    }
    image.with_meta(meta).unwrap()
}

/// Pixel size and the pixel index of physical zero along an axis.
fn calibration(session: &Session, id: DatasetId, axis: usize) -> (f64, f64) {
    let image = session.workspace.image(id).unwrap();
    let axis = &image.meta().axes()[axis];
    (axis.scale, -axis.origin / axis.scale + 0.0)
}

fn values(session: &Session, id: DatasetId, plane: usize) -> Vec<f64> {
    let buffer = session
        .workspace
        .image(id)
        .unwrap()
        .read_plane(plane)
        .unwrap();
    (0..buffer.len())
        .map(|i| buffer.get_f64(i).unwrap())
        .collect()
}

fn size(session: &Session, id: DatasetId) -> (usize, usize) {
    let meta = session.workspace.image(id).unwrap().meta().clone();
    (meta.width(), meta.height())
}

fn created(outputs: &Args) -> Option<DatasetId> {
    match outputs.get("image") {
        Some(&Value::Image(id)) => Some(id),
        _ => None,
    }
}

#[test]
fn every_command_runs_with_its_defaults_on_every_pixel_type() {
    let (exec, mut session) = setup();
    let mut sources: Vec<(String, Image)> = REAL
        .iter()
        .map(|&dtype| (dtype.name().to_string(), image(dtype, 1, 2, 1)))
        .collect();
    sources.push(("rgb".to_string(), rgb()));
    // Each command, whether it makes a new image, and the size it gives a
    // 6 x 4 image with the rectangle (1, 1, 4, 2) selected.
    let commands: [(&str, bool, (usize, usize)); 12] = [
        ("image.crop", false, (4, 2)),
        ("image.duplicate", true, (4, 2)),
        ("image.scale", true, (2, 1)),
        ("image.resize", false, (2, 1)),
        ("image.canvas_size", false, (6, 4)),
        ("image.flip_horizontal", false, (6, 4)),
        ("image.flip_vertical", false, (6, 4)),
        ("image.flip_z", false, (6, 4)),
        ("image.rotate_right", false, (4, 6)),
        ("image.rotate_left", false, (4, 6)),
        ("image.rotate", false, (6, 4)),
        ("image.translate", false, (6, 4)),
    ];
    let selection = Roi::rectangle(1.0, 1.0, 4.0, 2.0);
    for (kind, original) in &sources {
        let id = session.workspace.insert(original.clone(), None);
        session
            .workspace
            .set_roi(id, Some(selection.clone()))
            .unwrap();
        for (command, makes_new, expected_size) in commands {
            let what = format!("{command} on {kind}");
            let outputs = exec
                .run(&mut session, command, &Args::new().with("input", id))
                .unwrap_or_else(|e| panic!("{what}: {e}"));
            let new = created(&outputs);
            assert_eq!(new.is_some(), makes_new, "{what}");
            let result = new.unwrap_or(id);
            let meta = session.workspace.image(result).unwrap().meta().clone();
            assert_eq!(meta.dtype(), original.meta().dtype(), "{what}");
            assert_eq!(meta.color, original.meta().color, "{what}");
            assert_eq!((meta.width(), meta.height()), expected_size, "{what}");
            // Every plane can be read and has the new size.
            for plane in 0..meta.plane_count() {
                let samples = session.workspace.image(result).unwrap().read_plane(plane);
                assert_eq!(samples.unwrap().len(), meta.plane_len(), "{what}");
            }
            if makes_new {
                assert!(
                    session.workspace.image(id).unwrap().ptr_eq(original),
                    "{what}"
                );
            }
            exec.undo(&mut session).unwrap();
            assert!(
                session.workspace.image(id).unwrap().ptr_eq(original),
                "{what}: undo"
            );
            assert_eq!(
                session.workspace.get(id).unwrap().roi,
                Some(selection.clone()),
                "{what}"
            );
            assert_eq!(
                session.workspace.len(),
                1,
                "{what}: undo removes the new image"
            );
        }
        session.workspace.remove(id).unwrap();
    }
}

#[test]
fn crop_needs_a_selection_and_moves_a_calibration_origin() {
    let (exec, mut session) = setup();
    let id = session
        .workspace
        .insert(calibrated(image(DType::U16, 1, 2, 1)), None);
    let error = exec
        .run(&mut session, "image.crop", &Args::new())
        .unwrap_err();
    assert!(
        error.to_string().contains("needs an area selection"),
        "{error}"
    );

    // A selection hanging over the edge is cut to the image.
    session
        .workspace
        .set_roi(id, Some(Roi::rectangle(3.0, 2.0, 10.0, 10.0)))
        .unwrap();
    exec.run(&mut session, "image.crop", &Args::new()).unwrap();
    assert_eq!(size(&session, id), (3, 2));
    assert_eq!(
        values(&session, id, 0),
        [16.0, 17.0, 18.0, 22.0, 23.0, 24.0]
    );
    assert_eq!(values(&session, id, 1)[0], 26.0, "every plane is cropped");
    assert_eq!(calibration(&session, id, 0), (0.5, -1.0));
    assert_eq!(calibration(&session, id, 1), (0.25, -1.0));
    assert_eq!(session.workspace.get(id).unwrap().roi, None);

    // Without a calibration origin none appears.
    let plain = session.workspace.insert(image(DType::U8, 1, 1, 1), None);
    session
        .workspace
        .set_roi(plain, Some(Roi::rectangle(3.0, 2.0, 2.0, 2.0)))
        .unwrap();
    exec.run(&mut session, "image.crop", &Args::new()).unwrap();
    assert_eq!(
        session.workspace.image(plain).unwrap().meta().axes()[0].origin,
        0.0
    );
    session
        .workspace
        .set_roi(plain, Some(Roi::rectangle(30.0, 2.0, 2.0, 2.0)))
        .unwrap();
    let error = exec
        .run(&mut session, "image.crop", &Args::new())
        .unwrap_err();
    assert!(error.to_string().contains("outside the image"), "{error}");
}

#[test]
fn duplicate_copies_a_plane_a_range_or_everything() {
    let (exec, mut session) = setup();
    let source = image(DType::U16, 2, 3, 2);
    let id = session.workspace.insert(source.clone(), None);
    let run = |session: &mut Session, args: Args| {
        let outputs = exec
            .run(session, "image.duplicate", &args.with("input", id))
            .unwrap();
        created(&outputs).expect("a new image")
    };
    let shape = |session: &Session, id: DatasetId| {
        let meta = session.workspace.image(id).unwrap().meta().clone();
        [AxisKind::Channel, AxisKind::Z, AxisKind::Time].map(|kind| meta.size(&kind))
    };

    // The plane on view: slice 2 of frame 2, both channels (planes 8, 9).
    let plane = run(&mut session, Args::new().with("slice", 2).with("frame", 2));
    assert_eq!(shape(&session, plane), [2, 1, 1]);
    assert_eq!(values(&session, plane, 1)[0], 91.0);
    assert_eq!(
        session.workspace.image(plane).unwrap().meta().name,
        "a-1.tif"
    );
    // One channel of it.
    let channel = run(
        &mut session,
        Args::new().with("channel", 2).with("slice", 3),
    );
    assert_eq!(shape(&session, channel), [1, 1, 1]);
    assert_eq!(values(&session, channel, 0)[0], 51.0);
    assert_eq!(
        session.workspace.image(channel).unwrap().meta().name,
        "a-2.tif"
    );

    // Everything: the planes are shared with the source, not copied.
    let all = run(
        &mut session,
        Args::new().with("stack", true).with("title", "all"),
    );
    assert_eq!(shape(&session, all), [2, 3, 2]);
    assert_eq!(session.workspace.image(all).unwrap().meta().name, "all");
    for p in 0..12 {
        let copy = session.workspace.image(all).unwrap().read_plane(p).unwrap();
        assert!(copy.shares_storage(&source.read_plane(p).unwrap()));
    }
    // Ranges.
    let args = Args::new()
        .with("stack", true)
        .with("channels", "2")
        .with("slices", "2-3")
        .with("frames", "1");
    let part = run(&mut session, args);
    assert_eq!(shape(&session, part), [1, 2, 1]);
    assert_eq!(values(&session, part, 0)[0], 31.0);
    assert_eq!(values(&session, part, 1)[0], 51.0);
    // Out of range or reversed means everything, as in ImageJ.
    let odd = run(
        &mut session,
        Args::new().with("stack", true).with("slices", "3-1"),
    );
    assert_eq!(shape(&session, odd), [2, 3, 2]);
    let bad = Args::new()
        .with("input", id)
        .with("stack", true)
        .with("slices", "two");
    assert!(exec.run(&mut session, "image.duplicate", &bad).is_err());

    // A selection crops the copy, unless told to ignore it; then the copy
    // gets the selection too.
    let selection = Roi::rectangle(1.0, 1.0, 2.0, 2.0);
    session
        .workspace
        .set_roi(id, Some(selection.clone()))
        .unwrap();
    let cropped = run(&mut session, Args::new());
    assert_eq!(size(&session, cropped), (2, 2));
    assert_eq!(session.workspace.get(cropped).unwrap().roi, None);
    let whole = run(&mut session, Args::new().with("ignore_selection", true));
    assert_eq!(size(&session, whole), (6, 4));
    assert_eq!(session.workspace.get(whole).unwrap().roi, Some(selection));

    // `range` is for an image with one stack axis.
    let stack = session.workspace.insert(image(DType::U8, 1, 5, 1), None);
    let args = Args::new()
        .with("input", stack)
        .with("stack", true)
        .with("range", "2-4");
    let outputs = exec.run(&mut session, "image.duplicate", &args).unwrap();
    let middle = created(&outputs).unwrap();
    assert_eq!(shape(&session, middle), [1, 3, 1]);
    assert_eq!(values(&session, middle, 0)[0], 11.0);
}

#[test]
fn flips_and_quarter_turns_undo_each_other() {
    let (exec, mut session) = setup();
    let original = calibrated(image(DType::I32, 2, 2, 2));
    let id = session.workspace.insert(original.clone(), None);
    let same = |session: &Session| {
        let now = session.workspace.image(id).unwrap();
        now.meta() == original.meta()
            && (0..8).all(|p| now.read_plane(p).unwrap() == original.read_plane(p).unwrap())
    };
    let whole = Args::new().with("stack", true);

    for command in ["image.flip_horizontal", "image.flip_vertical"] {
        exec.run(&mut session, command, &whole).unwrap();
        assert!(!same(&session), "{command}");
        exec.run(&mut session, command, &whole).unwrap();
        assert!(
            same(&session),
            "{command} twice, calibration origin included"
        );
    }
    exec.run(&mut session, "image.flip_horizontal", &whole)
        .unwrap();
    // Physical zero was at pixel 2 of 6 and is now at pixel 3.
    assert_eq!(calibration(&session, id, 0), (0.5, 3.0));
    assert_eq!(values(&session, id, 0)[..6], [6.0, 5.0, 4.0, 3.0, 2.0, 1.0]);
    exec.undo(&mut session).unwrap();

    for command in ["image.rotate_right", "image.rotate_left"] {
        for turn in 0..4 {
            assert_eq!(same(&session), turn == 0, "{command}");
            exec.run(&mut session, command, &Args::new()).unwrap();
        }
        assert!(same(&session), "four of {command}");
    }
    exec.run(&mut session, "image.rotate_right", &Args::new())
        .unwrap();
    assert_eq!(size(&session, id), (4, 6));
    // The pixel sizes change places, and the origin turns with the image.
    assert_eq!(calibration(&session, id, 0), (0.25, 2.0));
    assert_eq!(calibration(&session, id, 1), (0.5, 2.0));
    assert_eq!(values(&session, id, 0)[..4], [19.0, 13.0, 7.0, 1.0]);
    exec.run(&mut session, "image.rotate_left", &Args::new())
        .unwrap();
    assert!(same(&session));

    // Flip Z reverses slices within each channel and frame.
    exec.run(&mut session, "image.flip_z", &Args::new())
        .unwrap();
    let firsts: Vec<f64> = (0..8).map(|p| values(&session, id, p)[0]).collect();
    assert_eq!(firsts, [21.0, 31.0, 1.0, 11.0, 61.0, 71.0, 41.0, 51.0]);
    exec.run(&mut session, "image.flip_z", &Args::new())
        .unwrap();
    assert!(same(&session));

    // Without slices it reverses frames; with neither it has nothing to do.
    let frames = session.workspace.insert(image(DType::U8, 1, 1, 3), None);
    exec.run(&mut session, "image.flip_z", &Args::new())
        .unwrap();
    let firsts: Vec<f64> = (0..3).map(|p| values(&session, frames, p)[0]).collect();
    assert_eq!(firsts, [21.0, 11.0, 1.0]);
    session.workspace.insert(image(DType::U8, 1, 1, 1), None);
    let error = exec
        .run(&mut session, "image.flip_z", &Args::new())
        .unwrap_err();
    assert!(
        error.to_string().contains("several slices or frames"),
        "{error}"
    );
}

#[test]
fn the_plane_scope_and_selection_confine_flip_rotate_and_translate() {
    let (exec, mut session) = setup();
    let original = image(DType::F32, 1, 2, 1);
    let id = session.workspace.insert(original.clone(), None);
    let rotate = |extra: Args| extra.with("angle", 180.0).with("interpolation", "None");
    for (command, args) in [
        ("image.flip_horizontal", Args::new()),
        ("image.flip_vertical", Args::new()),
        ("image.rotate", rotate(Args::new())),
        ("image.translate", Args::new().with("x", 1.0).with("y", 0.0)),
    ] {
        // Only the plane on view.
        exec.run(&mut session, command, &args.clone().with("slice", 2))
            .unwrap();
        assert_eq!(
            values(&session, id, 0),
            values_of(&original, 0),
            "{command}"
        );
        assert_ne!(
            values(&session, id, 1),
            values_of(&original, 1),
            "{command}"
        );
        exec.undo(&mut session).unwrap();
        // Only inside the selection.
        session
            .workspace
            .set_roi(id, Some(Roi::rectangle(1.0, 1.0, 3.0, 2.0)))
            .unwrap();
        exec.run(&mut session, command, &args.clone().with("stack", true))
            .unwrap();
        for plane in 0..2 {
            let (now, before) = (values(&session, id, plane), values_of(&original, plane));
            assert_ne!(now, before, "{command}");
            for i in 0..24 {
                let inside = (1..4).contains(&(i % 6)) && (1..3).contains(&(i / 6));
                assert!(
                    inside || now[i] == before[i],
                    "{command}: sample {i} changed"
                );
            }
        }
        exec.undo(&mut session).unwrap();
        session.workspace.set_roi(id, None).unwrap();
    }

    // Rotate can only enlarge the image when all of it is turned.
    session
        .workspace
        .set_roi(id, Some(Roi::rectangle(1.0, 1.0, 3.0, 2.0)))
        .unwrap();
    let enlarge = Args::new().with("angle", 90.0).with("enlarge", true);
    exec.run(&mut session, "image.rotate", &enlarge).unwrap();
    assert_eq!(size(&session, id), (6, 4));
    exec.undo(&mut session).unwrap();
    session.workspace.set_roi(id, None).unwrap();
    exec.run(
        &mut session,
        "image.rotate",
        &enlarge.clone().with("slice", 2),
    )
    .unwrap();
    assert_eq!(
        size(&session, id),
        (6, 6),
        "room for the image turned on its side"
    );
    // Every plane gets the larger canvas, but only the one on view turns.
    assert_eq!(
        values(&session, id, 0)[6..12],
        [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
    );
    assert_ne!(
        values(&session, id, 1)[6..12],
        [11.0, 12.0, 13.0, 14.0, 15.0, 16.0]
    );
    exec.undo(&mut session).unwrap();
    assert!(session.workspace.image(id).unwrap().ptr_eq(&original));
}

fn values_of(image: &Image, plane: usize) -> Vec<f64> {
    let buffer = image.read_plane(plane).unwrap();
    (0..buffer.len())
        .map(|i| buffer.get_f64(i).unwrap())
        .collect()
}

#[test]
fn scale_and_resize_keep_the_calibration_true() {
    let (exec, mut session) = setup();
    let mut source = calibrated(image(DType::U16, 1, 4, 1));
    // Slices 2 µm apart.
    let mut meta = source.meta().clone();
    let z = meta.axis_index(&AxisKind::Z).unwrap();
    *meta.axis_mut(z).unwrap() = Axis::calibrated(AxisKind::Z, 2.0, "µm");
    source = source.with_meta(meta).unwrap();
    let id = session.workspace.insert(source.clone(), None);

    // Scale: a new image, every plane, pixels twice the size.
    let outputs = exec
        .run(&mut session, "image.scale", &Args::new().with("input", id))
        .unwrap();
    let half = created(&outputs).unwrap();
    assert_eq!(size(&session, half), (3, 2));
    assert_eq!(
        session.workspace.image(half).unwrap().meta().plane_count(),
        4
    );
    assert_eq!(
        session.workspace.image(half).unwrap().meta().name,
        "a-1.tif"
    );
    assert_eq!(calibration(&session, half, 0), (1.0, 1.0));
    assert_eq!(calibration(&session, half, 1), (0.5, 0.5));
    // The plane on view only.
    let args = Args::new()
        .with("input", id)
        .with("process", false)
        .with("slice", 3)
        .with("interpolation", "None")
        .with("average", false);
    let outputs = exec
        .run(&mut session, "image.scale", &args.with("title", "one"))
        .unwrap();
    let one = created(&outputs).unwrap();
    let meta = session.workspace.image(one).unwrap().meta().clone();
    assert_eq!((meta.plane_count(), meta.name.as_str()), (1, "one"));
    // Every other sample of plane 2, whose samples start at 21.
    assert_eq!(
        values(&session, one, 0),
        [21.0, 23.0, 25.0, 33.0, 35.0, 37.0]
    );
    // By size instead of factors, and in Z: half as many slices, twice as
    // far apart.
    let args = Args::new()
        .with("input", id)
        .with("width", 12)
        .with("height", 4)
        .with("z_scale", 0.5);
    let outputs = exec.run(&mut session, "image.scale", &args).unwrap();
    let wide = created(&outputs).unwrap();
    let meta = session.workspace.image(wide).unwrap().meta().clone();
    assert_eq!(
        (meta.width(), meta.height(), meta.size(&AxisKind::Z)),
        (12, 4, 2)
    );
    assert_eq!(meta.axes()[z].scale, 4.0);
    assert_eq!(calibration(&session, wide, 0), (0.25, 4.0));
    assert!(session.workspace.image(id).unwrap().ptr_eq(&source));

    // In place: same size, contents reduced about the centre, selection
    // removed.
    session
        .workspace
        .set_roi(id, Some(Roi::rectangle(0.0, 0.0, 6.0, 4.0)))
        .unwrap();
    let args = Args::new().with("input", id).with("create", false);
    let outputs = exec.run(&mut session, "image.scale", &args).unwrap();
    assert_eq!(created(&outputs), None);
    assert_eq!(size(&session, id), (6, 4));
    assert_eq!(session.workspace.get(id).unwrap().roi, None);
    assert_ne!(
        values(&session, id, 3),
        values_of(&source, 3),
        "every plane by default"
    );
    exec.undo(&mut session).unwrap();

    // Resize: in place, every plane; half size by default, shape kept.
    exec.run(&mut session, "image.resize", &Args::new().with("input", id))
        .unwrap();
    assert_eq!(size(&session, id), (3, 2));
    assert_eq!(calibration(&session, id, 0), (1.0, 1.0));
    exec.undo(&mut session).unwrap();
    let args = Args::new()
        .with("input", id)
        .with("width", 12)
        .with("depth", 2);
    exec.run(&mut session, "image.resize", &args).unwrap();
    let meta = session.workspace.image(id).unwrap().meta().clone();
    assert_eq!(
        (meta.width(), meta.height(), meta.size(&AxisKind::Z)),
        (12, 8, 2)
    );
    exec.undo(&mut session).unwrap();
    let args = Args::new()
        .with("input", id)
        .with("width", 5)
        .with("height", 7)
        .with("constrain", false);
    exec.run(&mut session, "image.resize", &args).unwrap();
    assert_eq!(size(&session, id), (5, 7));
    exec.undo(&mut session).unwrap();
    assert!(session.workspace.image(id).unwrap().ptr_eq(&source));

    for (command, args) in [
        ("image.scale", Args::new().with("x_scale", 0.0)),
        (
            "image.scale",
            Args::new().with("x_scale", 0.01).with("y_scale", 0.01),
        ),
        ("image.scale", Args::new().with("interpolation", "Nearest")),
    ] {
        assert!(
            exec.run(&mut session, command, &args.with("input", id))
                .is_err()
        );
    }
}

#[test]
fn canvas_size_places_the_image_and_fills_the_border() {
    let (exec, mut session) = setup();
    let id = session.workspace.insert(image(DType::U8, 1, 2, 1), None);
    let larger = |position: &str| {
        Args::new()
            .with("width", 8)
            .with("height", 5)
            .with("position", position)
    };
    // White by default, as ImageJ's default background colour.
    exec.run(&mut session, "image.canvas_size", &larger("Bottom-Right"))
        .unwrap();
    assert_eq!(size(&session, id), (8, 5));
    assert_eq!(
        values(&session, id, 0)[..10],
        [
            255.0, 255.0, 255.0, 255.0, 255.0, 255.0, 255.0, 255.0, 255.0, 255.0
        ]
    );
    assert_eq!(
        values(&session, id, 0)[10..16],
        [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
    );
    assert_eq!(values(&session, id, 1)[10], 11.0, "every plane");
    exec.undo(&mut session).unwrap();
    exec.run(
        &mut session,
        "image.canvas_size",
        &larger("Top-Left").with("zero", true),
    )
    .unwrap();
    assert_eq!(
        values(&session, id, 0)[..8],
        [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 0.0, 0.0]
    );
    exec.undo(&mut session).unwrap();
    // Smaller: the edges are cut.
    let smaller = Args::new().with("width", 2).with("height", 2);
    exec.run(&mut session, "image.canvas_size", &smaller)
        .unwrap();
    assert_eq!(values(&session, id, 0), [9.0, 10.0, 15.0, 16.0]);
    exec.undo(&mut session).unwrap();

    // White is the first entry of an inverting lookup table, and the top
    // of the display range for other pixel types.
    let source = session.workspace.image(id).unwrap().clone();
    let mut meta = source.meta().clone();
    meta.channels = vec![ChannelMeta {
        lut: Some(Lut::grays().inverted()),
        ..ChannelMeta::default()
    }];
    session
        .workspace
        .set_image(id, source.with_meta(meta).unwrap())
        .unwrap();
    exec.run(&mut session, "image.canvas_size", &larger("Bottom-Right"))
        .unwrap();
    assert_eq!(values(&session, id, 0)[0], 0.0);

    let float = calibrated(image(DType::F32, 1, 1, 1));
    let float = session.workspace.insert(float, None);
    exec.run(&mut session, "image.canvas_size", &larger("Center"))
        .unwrap();
    assert_eq!(values(&session, float, 0)[0], 24.0, "the largest sample");
    // The image moved one pixel right; so did physical zero.
    assert_eq!(calibration(&session, float, 0), (0.5, 3.0));
    assert_eq!(calibration(&session, float, 1), (0.25, 1.0));
    let error = exec
        .run(&mut session, "image.canvas_size", &larger("Middle"))
        .unwrap_err();
    assert!(error.to_string().contains("Middle"), "{error}");
}
