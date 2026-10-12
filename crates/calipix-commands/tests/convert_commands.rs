//! Image > Type as commands: every source and target type, the pixel types
//! ImageJ lacks, what is refused, and undo.

use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Executor, Session};
use calipix_commands::standard_registry;
use calipix_core::{
    AxisKind, Buffer, ChannelMeta, ColorModel, Complex32, DType, Image, ImageMeta, RealPixel,
};
use calipix_ops::convert::convert;

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
    let source = image(DType::U8, 3, 2);
    let mut meta = source.meta().clone();
    meta.color = ColorModel::Rgb;
    source.with_meta(meta).unwrap()
}

fn typed<T: RealPixel>(session: &Session, id: DatasetId, plane: usize) -> Vec<T> {
    let buffer = session
        .workspace
        .image(id)
        .unwrap()
        .read_plane(plane)
        .unwrap();
    buffer.as_slice::<T>().unwrap().to_vec()
}

fn dtype_of(session: &Session, id: DatasetId) -> DType {
    session.workspace.image(id).unwrap().meta().dtype()
}

#[test]
fn every_conversion_runs_with_its_defaults_on_every_pixel_type() {
    let (exec, mut session) = setup();
    let mut sources: Vec<(String, Image)> = REAL
        .iter()
        .map(|&dtype| (dtype.name().to_string(), image(dtype, 1, 2)))
        .collect();
    sources.push(("rgb".to_string(), rgb()));
    let mut targets: Vec<(&str, Args, DType)> = vec![
        ("image.to_8bit", Args::new(), DType::U8),
        ("image.to_16bit", Args::new(), DType::U16),
        ("image.to_32bit", Args::new(), DType::F32),
        ("image.to_rgb", Args::new(), DType::U8),
    ];
    for dtype in REAL {
        let args = Args::new().with("pixel_type", dtype.name());
        targets.push(("image.convert_type", args, dtype));
    }
    for (kind, original) in &sources {
        let id = session.workspace.insert(original.clone(), None);
        for (command, args, target) in &targets {
            let what = format!("{command} {:?} on {kind}", args.get("pixel_type"));
            let result = exec.run(&mut session, command, args);
            // An RGB image is 8-bit by definition and has no other type.
            if kind == "rgb" && *command == "image.convert_type" {
                if *target == DType::U8 {
                    result.unwrap();
                    assert!(
                        session.workspace.image(id).unwrap().ptr_eq(original),
                        "{what}"
                    );
                } else {
                    let error = result.unwrap_err().to_string();
                    assert!(error.contains("RGB Stack"), "{what}: {error}");
                }
                continue;
            }
            result.unwrap_or_else(|e| panic!("{what}: {e}"));
            let converted = session.workspace.image(id).unwrap().clone();
            let meta = converted.meta();
            assert_eq!(meta.dtype(), *target, "{what}");
            assert_eq!((meta.width(), meta.height()), (4, 3), "{what}");
            assert_eq!(meta.size(&AxisKind::Z), 2, "{what}");
            assert_eq!(meta.name, "a.tif", "{what}");
            let to_rgb = *command == "image.to_rgb";
            assert_eq!(meta.color == ColorModel::Rgb, to_rgb, "{what}");
            assert_eq!(meta.plane_count(), if to_rgb { 6 } else { 2 }, "{what}");
            // Converting to the type it already has leaves the image alone;
            // otherwise one undo step brings it back.
            if !converted.ptr_eq(original) {
                exec.undo(&mut session).unwrap();
            }
            assert!(
                session.workspace.image(id).unwrap().ptr_eq(original),
                "{what}: undo"
            );
        }
        session.workspace.remove(id).unwrap();
    }
}

#[test]
fn pixel_types_imagej_lacks_convert_by_documented_rules() {
    let (exec, mut session) = setup();
    let signed = Image::from_vec(4, 1, vec![-300i16, -1, 100, 300]).unwrap();
    let id = session.workspace.insert(signed, None);

    // To 8-bit with scaling: through the display range, here the data
    // range, by ImageJ's 16-bit formula.
    exec.run(&mut session, "image.to_8bit", &Args::new())
        .unwrap();
    assert_eq!(typed::<u8>(&session, id, 0), [0, 127, 170, 255]);
    exec.undo(&mut session).unwrap();
    // Without scaling: limited to 0..=255.
    exec.run(
        &mut session,
        "image.to_8bit",
        &Args::new().with("scale", false),
    )
    .unwrap();
    assert_eq!(typed::<u8>(&session, id, 0), [0, 0, 100, 255]);
    exec.undo(&mut session).unwrap();
    // To 16-bit: integers are never scaled, only limited.
    exec.run(&mut session, "image.to_16bit", &Args::new())
        .unwrap();
    assert_eq!(typed::<u16>(&session, id, 0), [0, 0, 100, 300]);
    exec.undo(&mut session).unwrap();
    exec.run(&mut session, "image.to_32bit", &Args::new())
        .unwrap();
    assert_eq!(typed::<f32>(&session, id, 0), [-300.0, -1.0, 100.0, 300.0]);
    exec.undo(&mut session).unwrap();

    // Any type and back, where the values fit.
    for dtype in [DType::I32, DType::I64, DType::F32, DType::F64] {
        let to = Args::new().with("pixel_type", dtype.name());
        exec.run(&mut session, "image.convert_type", &to).unwrap();
        assert_eq!(dtype_of(&session, id), dtype);
        let back = Args::new().with("pixel_type", "i16");
        exec.run(&mut session, "image.convert_type", &back).unwrap();
        assert_eq!(
            typed::<i16>(&session, id, 0),
            [-300, -1, 100, 300],
            "{dtype}"
        );
    }

    // 64-bit float to 16-bit scales through its display range like 32-bit.
    let double = Image::from_vec(3, 1, vec![-1.0f64, 0.0, 1.0]).unwrap();
    let id = session.workspace.insert(double, None);
    exec.run(&mut session, "image.to_16bit", &Args::new())
        .unwrap();
    assert_eq!(typed::<u16>(&session, id, 0), [0, 32768, 65535]);
    exec.undo(&mut session).unwrap();
    exec.run(
        &mut session,
        "image.to_16bit",
        &Args::new().with("scale", false),
    )
    .unwrap();
    assert_eq!(typed::<u16>(&session, id, 0), [0, 0, 1]);

    let complex = Image::from_vec(1, 1, vec![Complex32::new(1.0, 2.0)]).unwrap();
    session.workspace.insert(complex, None);
    for command in [
        "image.to_8bit",
        "image.to_16bit",
        "image.to_32bit",
        "image.to_rgb",
    ] {
        let error = exec.run(&mut session, command, &Args::new()).unwrap_err();
        assert!(error.to_string().contains("complex"), "{command}: {error}");
    }
}

#[test]
fn a_stack_without_a_display_range_uses_that_of_the_plane_on_view() {
    let (exec, mut session) = setup();
    // Plane 0 spans 1..=12 and plane 1 spans 11..=22.
    let id = session.workspace.insert(image(DType::U16, 1, 2), None);
    exec.run(&mut session, "image.to_8bit", &Args::new().with("slice", 2))
        .unwrap();
    // Scaled over 11..=22: plane 0 is almost all below the range.
    assert_eq!(typed::<u8>(&session, id, 0)[9..], [0, 0, 21]);
    assert_eq!(typed::<u8>(&session, id, 1)[..2], [0, 21]);
    assert_eq!(typed::<u8>(&session, id, 1)[11], 235);
    exec.undo(&mut session).unwrap();

    // With a display range, that is what is used, on every plane.
    let image = session.workspace.image(id).unwrap().clone();
    let mut meta = image.meta().clone();
    meta.channels = vec![ChannelMeta {
        display_range: Some((0.0, 255.0)),
        ..ChannelMeta::default()
    }];
    session
        .workspace
        .set_image(id, image.with_meta(meta).unwrap())
        .unwrap();
    exec.run(&mut session, "image.to_8bit", &Args::new())
        .unwrap();
    assert_eq!(typed::<u8>(&session, id, 1)[..3], [11, 12, 13]);
    let range = session.workspace.image(id).unwrap().meta().channels[0].display_range;
    assert_eq!(range, None, "8-bit results are shown over 0 to 255");
}

#[test]
fn rgb_stack_and_back_share_the_samples() {
    let (exec, mut session) = setup();
    let original = rgb();
    let id = session.workspace.insert(original.clone(), None);
    exec.run(&mut session, "image.to_rgb_stack", &Args::new())
        .unwrap();
    let stack = session.workspace.image(id).unwrap().clone();
    assert_eq!(stack.meta().color, ColorModel::Grayscale);
    let names: Vec<_> = stack
        .meta()
        .channels
        .iter()
        .map(|c| c.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["Red", "Green", "Blue"]);
    exec.run(&mut session, "image.to_rgb", &Args::new())
        .unwrap();
    let back = session.workspace.image(id).unwrap().clone();
    assert_eq!(back.meta().color, ColorModel::Rgb);
    for plane in 0..6 {
        let samples = back.read_plane(plane).unwrap();
        assert!(samples.shares_storage(&original.read_plane(plane).unwrap()));
    }

    // Only an RGB image can become an RGB stack.
    let grey = session.workspace.insert(image(DType::U8, 1, 1), None);
    let error = exec
        .run(&mut session, "image.to_rgb_stack", &Args::new())
        .unwrap_err();
    assert!(error.to_string().contains("only an RGB image"), "{error}");
    // Grey to RGB: three equal channels.
    exec.run(&mut session, "image.to_rgb", &Args::new())
        .unwrap();
    for plane in 0..3 {
        assert_eq!(typed::<u8>(&session, grey, plane)[..3], [1, 2, 3]);
    }

    // Channels shown one at a time are not one colour picture.
    let channels = session.workspace.insert(image(DType::U16, 2, 1), None);
    let error = exec
        .run(&mut session, "image.to_rgb", &Args::new())
        .unwrap_err();
    assert!(error.to_string().contains("composite"), "{error}");
    exec.run(
        &mut session,
        "view.set_channel_mode",
        &Args::new().with("mode", "composite"),
    )
    .unwrap();
    exec.run(&mut session, "image.to_rgb", &Args::new())
        .unwrap();
    let meta = session.workspace.image(channels).unwrap().meta().clone();
    assert_eq!(
        (meta.color, meta.dtype(), meta.plane_count()),
        (ColorModel::Rgb, DType::U8, 3)
    );
}
