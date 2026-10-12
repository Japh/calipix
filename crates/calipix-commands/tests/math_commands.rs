//! Process > Math and the image calculator as commands: defaults on every
//! pixel type, the plane scope, selections, new images and undo.

use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Error, Executor, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::{
    Buffer, ChannelMeta, ColorModel, Complex32, DType, Image, ImageMeta, RealPixel,
};
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

const MATH: [&str; 18] = [
    "math.add",
    "math.subtract",
    "math.multiply",
    "math.divide",
    "math.and",
    "math.or",
    "math.xor",
    "math.min",
    "math.max",
    "math.gamma",
    "math.set",
    "math.log",
    "math.exp",
    "math.square",
    "math.square_root",
    "math.reciprocal",
    "math.nan_background",
    "math.abs",
];

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

/// A 4 x 3 image of `dtype` with `c` channels and `z` slices; the samples
/// of plane `p` are `10 * p + 1, 10 * p + 2, ...`.
fn image(dtype: DType, c: usize, z: usize) -> Image {
    let meta = ImageMeta::xyczt(dtype, 4, 3, c, z, 1).unwrap();
    let planes = (0..meta.plane_count())
        .map(|p| {
            let values: Vec<u8> = (1..=12).map(|i| (10 * p + i) as u8).collect();
            convert(&Buffer::from_vec(values), dtype).unwrap()
        })
        .collect();
    Image::from_planes(meta.with_name("a.tif"), planes).unwrap()
}

fn rgb() -> Image {
    let source = image(DType::U8, 3, 1);
    let mut meta = source.meta().clone();
    meta.color = ColorModel::Rgb;
    source.with_meta(meta).unwrap()
}

fn values(session: &Session, id: DatasetId, plane: usize) -> Vec<f64> {
    let buffer = session.workspace.image(id).unwrap().read_plane(plane).unwrap();
    (0..buffer.len()).map(|i| buffer.get_f64(i).unwrap()).collect()
}

fn typed<T: RealPixel>(session: &Session, id: DatasetId, plane: usize) -> Vec<T> {
    let buffer = session.workspace.image(id).unwrap().read_plane(plane).unwrap();
    buffer.as_slice::<T>().unwrap().to_vec()
}

fn extra(command: &str) -> Args {
    match command {
        "math.nan_background" => Args::new().with("lower", 2.0).with("upper", 9.0),
        _ => Args::new(),
    }
}

#[test]
fn every_command_runs_with_its_defaults_on_every_pixel_type() {
    let (exec, mut session) = setup();
    let mut images: Vec<(String, Image)> = REAL
        .iter()
        .map(|&dtype| (dtype.name().to_string(), image(dtype, 1, 2)))
        .collect();
    images.push(("rgb".to_string(), rgb()));
    for (kind, original) in &images {
        let id = session.workspace.insert(original.clone(), None);
        for command in MATH {
            let what = format!("{command} on {kind}");
            let float_only = matches!(command, "math.reciprocal" | "math.nan_background");
            let is_float = original.meta().dtype().is_float();
            match exec.run(&mut session, command, &extra(command)) {
                Err(error) => {
                    assert!(float_only && !is_float, "{what}: {error}");
                    assert!(error.to_string().contains("needs a float image"), "{what}: {error}");
                    assert!(session.workspace.image(id).unwrap().ptr_eq(original), "{what}");
                    continue;
                }
                Ok(_) => assert!(!float_only || is_float, "{what} should be refused"),
            }
            let changed = session.workspace.image(id).unwrap().clone();
            assert_eq!(changed.meta().dtype(), original.meta().dtype(), "{what}");
            assert_eq!(changed.meta().shape(), original.meta().shape(), "{what}");
            assert_eq!(changed.meta().color, original.meta().color, "{what}");
            // Only the plane on view was touched; RGB channels go together.
            if kind != "rgb" {
                let untouched = changed.read_plane(1).unwrap();
                assert!(untouched.shares_storage(&original.read_plane(1).unwrap()), "{what}");
            }
            assert_eq!(exec.undo(&mut session).unwrap(), exec.registry().get(command).unwrap().spec().title);
            assert!(session.workspace.image(id).unwrap().ptr_eq(original), "{what}: undo");
        }
        session.workspace.remove(id).unwrap();
    }
}

#[test]
fn defaults_are_imagejs() {
    let (exec, mut session) = setup();
    let id = session.workspace.insert(image(DType::F32, 1, 1), None);
    let mut expect = |command: &str, first: f64| {
        exec.run(&mut session, command, &Args::new()).unwrap();
        assert_eq!(values(&session, id, 0)[0], first, "{command}");
        exec.undo(&mut session).unwrap();
    };
    expect("math.add", 26.0);
    expect("math.subtract", -24.0);
    expect("math.multiply", 1.25);
    expect("math.divide", 0.8f32 as f64);
    expect("math.min", 1.0);
    expect("math.max", 1.0);
    expect("math.gamma", 1.0);
    expect("math.set", 25.0);
    let registry = standard_registry();
    for command in ["math.and", "math.or", "math.xor"] {
        let spec = registry.get(command).unwrap().spec().clone();
        assert_eq!(spec.param("value").unwrap().default, Some(Value::from("11110000")));
        assert_eq!(spec.param("value").unwrap().ijm_key.as_deref(), Some("value"));
    }
    let names: Vec<_> = MATH
        .iter()
        .map(|id| registry.get(id).unwrap().spec().ijm_name.clone().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Add...", "Subtract...", "Multiply...", "Divide...", "AND...", "OR...", "XOR...",
            "Min...", "Max...", "Gamma...", "Set...", "Log", "Exp", "Square", "Square Root",
            "Reciprocal", "NaN Background", "Abs"
        ]
    );
}

#[test]
fn the_plane_scope_chooses_planes_and_channels() {
    let (exec, mut session) = setup();
    let id = session.workspace.insert(image(DType::U16, 2, 3), None);
    let firsts = |session: &Session| -> Vec<f64> {
        (0..6).map(|p| values(session, id, p)[0]).collect()
    };
    assert_eq!(firsts(&session), [1.0, 11.0, 21.0, 31.0, 41.0, 51.0]);

    // Channel 2 of slice 2 is plane 3.
    let one = Args::new().with("value", 100.0).with("channel", 2).with("slice", 2);
    exec.run(&mut session, "math.add", &one).unwrap();
    assert_eq!(firsts(&session), [1.0, 11.0, 21.0, 131.0, 41.0, 51.0]);
    exec.undo(&mut session).unwrap();
    // Channel 0: both channels of the slice on view.
    exec.run(&mut session, "math.add", &Args::new().with("value", 100.0).with("slice", 3))
        .unwrap();
    assert_eq!(firsts(&session), [1.0, 11.0, 21.0, 31.0, 141.0, 151.0]);
    exec.undo(&mut session).unwrap();
    exec.run(&mut session, "math.add", &Args::new().with("value", 100.0).with("stack", true))
        .unwrap();
    assert_eq!(firsts(&session), [101.0, 111.0, 121.0, 131.0, 141.0, 151.0]);

    // RGB: Set takes a packed colour, on all three channels at once.
    let rgb_id = session.workspace.insert(rgb(), None);
    exec.run(&mut session, "math.set", &Args::new().with("value", 0x0a_14_1e as f64))
        .unwrap();
    for (plane, expected) in [10.0, 20.0, 30.0].into_iter().enumerate() {
        assert_eq!(values(&session, rgb_id, plane), [expected; 12]);
    }
}

#[test]
fn a_selection_confines_the_arithmetic() {
    let (exec, mut session) = setup();
    for dtype in [DType::U8, DType::I16, DType::F64] {
        let id = session.workspace.insert(image(dtype, 1, 1), None);
        session
            .workspace
            .set_roi(id, Some(Roi::rectangle(1.0, 1.0, 2.0, 2.0)))
            .unwrap();
        exec.run(&mut session, "math.add", &Args::new().with("value", 100.0))
            .unwrap();
        #[rustfmt::skip]
        assert_eq!(
            values(&session, id, 0),
            [1.0,   2.0,   3.0,  4.0,
             5.0, 106.0, 107.0,  8.0,
             9.0, 110.0, 111.0, 12.0],
            "{dtype}"
        );
        // The selection is still there, and undo keeps it too.
        assert!(session.workspace.get(id).unwrap().roi.is_some());
        exec.undo(&mut session).unwrap();
        assert!(session.workspace.get(id).unwrap().roi.is_some());
        assert_eq!(values(&session, id, 0)[5], 6.0);
    }
}

#[test]
fn the_display_range_is_reset_where_the_extremes_move() {
    let (exec, mut session) = setup();
    let ranged = |dtype| {
        let source = image(dtype, 1, 1);
        let mut meta = source.meta().clone();
        meta.channels = vec![ChannelMeta {
            display_range: Some((2.0, 9.0)),
            ..ChannelMeta::default()
        }];
        source.with_meta(meta).unwrap()
    };
    let range = |session: &Session, id| {
        session.workspace.image(id).unwrap().meta().channels[0].display_range
    };
    for (dtype, command, kept) in [
        (DType::U16, "math.add", true),
        (DType::U16, "math.log", true),
        (DType::U16, "math.min", false),
        (DType::U16, "math.max", false),
        (DType::U16, "math.abs", true),
        (DType::I16, "math.abs", false),
        (DType::F32, "math.abs", false),
        (DType::F32, "math.nan_background", false),
        (DType::U8, "math.min", true),
        (DType::U8, "math.max", true),
    ] {
        let id = session.workspace.insert(ranged(dtype), None);
        exec.run(&mut session, command, &extra(command)).unwrap();
        let expected = kept.then_some((2.0, 9.0));
        assert_eq!(range(&session, id), expected, "{command} on {dtype}");
        exec.undo(&mut session).unwrap();
        assert_eq!(range(&session, id), Some((2.0, 9.0)), "{command} on {dtype}: undo");
    }

    // Integer Gamma, Log and Exp work over the display range.
    let id = session.workspace.insert(ranged(DType::U16), None);
    exec.run(&mut session, "math.log", &Args::new()).unwrap();
    // ln(v) * 9 / ln(9), truncated.
    assert_eq!(typed::<u16>(&session, id, 0)[..4], [0, 2, 4, 5]);
}

#[test]
fn bad_values_are_refused_with_a_reason() {
    let (exec, mut session) = setup();
    let id = session.workspace.insert(image(DType::U8, 1, 1), None);
    let before = session.workspace.image(id).unwrap().clone();
    for (command, args) in [
        ("math.and", Args::new().with("value", "12")),
        ("math.or", Args::new().with("value", "")),
        ("math.gamma", Args::new().with("value", 7.0)),
        ("math.gamma", Args::new().with("value", 0.01)),
        ("math.add", Args::new().with("value", f64::NAN)),
    ] {
        let error = exec.run(&mut session, command, &args).unwrap_err();
        assert!(matches!(error, Error::InvalidArg { .. }), "{command}: {error}");
        assert!(session.workspace.image(id).unwrap().ptr_eq(&before));
    }
    exec.run(&mut session, "math.and", &Args::new().with("value", "110"))
        .unwrap();
    assert_eq!(values(&session, id, 0)[..7], [0.0, 2.0, 2.0, 4.0, 4.0, 6.0, 6.0]);

    let complex = Image::from_vec(1, 1, vec![Complex32::new(1.0, 2.0)]).unwrap();
    session.workspace.insert(complex, None);
    let error = exec.run(&mut session, "math.add", &Args::new()).unwrap_err();
    assert!(error.to_string().contains("complex"), "{error}");
}

fn calculate(exec: &Executor, session: &mut Session, args: Args) -> Result<Option<DatasetId>, Error> {
    let outputs = exec.run(session, "math.image_calculator", &args)?;
    Ok(match outputs.get("image") {
        Some(&Value::Image(id)) => Some(id),
        _ => None,
    })
}

#[test]
fn the_calculator_makes_a_new_image_unless_told_otherwise() {
    let (exec, mut session) = setup();
    let a = session.workspace.insert(image(DType::U8, 1, 2), None);
    let b = session.workspace.insert(image(DType::U8, 1, 2), None);
    let before = session.workspace.image(a).unwrap().clone();
    session
        .workspace
        .set_roi(a, Some(Roi::rectangle(0.0, 0.0, 1.0, 1.0)))
        .unwrap();

    // Defaults: Add, into a new image, of the plane on view only.
    let args = || Args::new().with("input", a).with("image2", b);
    let result = calculate(&exec, &mut session, args()).unwrap().expect("a new image");
    let meta = session.workspace.image(result).unwrap().meta().clone();
    assert_eq!(meta.name, "Result of a.tif");
    assert_eq!((meta.plane_count(), meta.dtype()), (1, DType::U8));
    assert_eq!(values(&session, result, 0)[..3], [2.0, 4.0, 6.0]);
    assert!(session.workspace.image(a).unwrap().ptr_eq(&before));
    assert_eq!(session.workspace.get(result).unwrap().roi, None);
    assert_eq!(session.workspace.active(), Some(result));
    exec.undo(&mut session).unwrap();
    assert!(session.workspace.image(result).is_err());

    // In place: image 1 changes, everywhere (the selection is ignored), on
    // the plane on view; the other plane is not even copied.
    let none = calculate(&exec, &mut session, args().with("create", false).with("slice", 2));
    assert_eq!(none.unwrap(), None);
    assert_eq!(values(&session, a, 1)[..3], [22.0, 24.0, 26.0]);
    assert_eq!(values(&session, a, 0)[..3], [1.0, 2.0, 3.0]);
    let untouched = session.workspace.image(a).unwrap().read_plane(0).unwrap();
    assert!(untouched.shares_storage(&before.read_plane(0).unwrap()));
    assert!(session.workspace.get(a).unwrap().roi.is_some());
    exec.undo(&mut session).unwrap();
    assert!(session.workspace.image(a).unwrap().ptr_eq(&before));

    // Whole stack, as float: a new float stack, nothing clipped.
    let args = args().with("stack", true).with("float_result", true).with("create", false);
    let result = calculate(&exec, &mut session, args.with("operation", "Subtract"))
        .unwrap()
        .expect("float results need a new image");
    let meta = session.workspace.image(result).unwrap().meta().clone();
    assert_eq!((meta.plane_count(), meta.dtype()), (2, DType::F32));
    assert_eq!(values(&session, result, 1), [0.0; 12]);
}

#[test]
fn calculator_rules_for_mismatched_images() {
    let (exec, mut session) = setup();
    let stack3 = session.workspace.insert(image(DType::U8, 1, 3), None);
    let stack2 = session.workspace.insert(image(DType::U8, 1, 2), None);
    let single = session.workspace.insert(image(DType::U8, 1, 1), None);
    let args = |a, b| Args::new().with("input", a).with("image2", b).with("stack", true);

    let error = calculate(&exec, &mut session, args(stack3, stack2)).unwrap_err();
    assert!(error.to_string().contains("3 planes and image 2 has 2"), "{error}");
    // A single image 2 is used for every plane.
    let result = calculate(&exec, &mut session, args(stack3, single)).unwrap().unwrap();
    assert_eq!(values(&session, result, 2)[0], 22.0);
    assert_eq!(session.workspace.image(result).unwrap().meta().plane_count(), 3);

    // Types ImageJ lacks: image 2 is limited to image 1's type, and the
    // arithmetic saturates.
    let signed = session.workspace.insert(image(DType::I8, 1, 1), None);
    let wide = Image::from_vec(4, 3, vec![1000i32, -1000, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    let wide = session.workspace.insert(wide, None);
    let pair = |a, b| Args::new().with("input", a).with("image2", b);
    let result = calculate(&exec, &mut session, pair(signed, wide)).unwrap().unwrap();
    assert_eq!(typed::<i8>(&session, result, 0)[..4], [127, -126, 3, 9]);
    let result = calculate(&exec, &mut session, pair(wide, signed).with("operation", "Multiply"));
    let result = result.unwrap().unwrap();
    assert_eq!(typed::<i32>(&session, result, 0)[..4], [1000, -2000, 0, 20]);

    // RGB with RGB stays RGB; complex is refused.
    let colour = session.workspace.insert(rgb(), None);
    let result = calculate(&exec, &mut session, pair(colour, colour)).unwrap().unwrap();
    let meta = session.workspace.image(result).unwrap().meta().clone();
    assert_eq!((meta.color, meta.plane_count()), (ColorModel::Rgb, 3));
    assert_eq!(values(&session, result, 2)[0], 42.0);
    let complex = Image::from_vec(4, 3, vec![Complex32::new(1.0, 0.0); 12]).unwrap();
    let complex = session.workspace.insert(complex, None);
    let error = calculate(&exec, &mut session, pair(single, complex)).unwrap_err();
    assert!(error.to_string().contains("complex"), "{error}");

    // The divide-by-zero value.
    let zero = session.workspace.insert(Image::from_vec(4, 3, vec![0f32; 12]).unwrap(), None);
    let float = session.workspace.insert(image(DType::F32, 1, 1), None);
    let divide = |value: &str| {
        pair(float, zero)
            .with("operation", "Divide")
            .with("divide_by_zero", value)
    };
    for (text, expected) in [("infinity", f64::INFINITY), ("max", f32::MAX as f64), ("-2.5", -2.5)] {
        let result = calculate(&exec, &mut session, divide(text)).unwrap().unwrap();
        assert_eq!(values(&session, result, 0)[0], expected, "{text}");
    }
    let result = calculate(&exec, &mut session, divide("NaN")).unwrap().unwrap();
    assert!(values(&session, result, 0)[0].is_nan());
    let error = calculate(&exec, &mut session, divide("lots")).unwrap_err();
    assert!(matches!(error, Error::InvalidArg { .. }), "{error}");
}
