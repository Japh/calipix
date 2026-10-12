//! The filter commands, run the way every frontend runs them.

use std::path::PathBuf;
use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Error, Executor, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::{Axis, AxisKind, Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut};
use calipix_ops::filter::{self, Frame, Outliers};

const FILTERS: [&str; 16] = [
    "filter.gaussian_blur",
    "filter.median",
    "filter.mean",
    "filter.minimum",
    "filter.maximum",
    "filter.variance",
    "filter.top_hat",
    "filter.convolve",
    "filter.unsharp_mask",
    "filter.smooth",
    "filter.sharpen",
    "filter.find_edges",
    "filter.shadows",
    "filter.despeckle",
    "filter.remove_outliers",
    "filter.remove_nans",
];

const W: usize = 12;
const H: usize = 9;

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

/// A value with structure at every scale: steps, a ramp and a spike.
fn sample(i: usize, plane: usize) -> f64 {
    let (x, y) = (i % W, i / W);
    let step = if (x / 4 + y / 3) % 2 == 0 { 60.0 } else { 0.0 };
    let spike = if i == 40 + plane { 150.0 } else { 0.0 };
    10.0 + 3.0 * x as f64 + 2.0 * y as f64 + step + spike + 7.0 * plane as f64
}

/// One plane of the given type; float planes also hold a NaN.
fn plane(dtype: DType, plane: usize) -> Buffer {
    let values = (0..W * H).map(|i| sample(i, plane));
    match dtype {
        DType::U8 => Buffer::from_vec(values.map(|v| v as u8).collect::<Vec<_>>()),
        DType::U16 => Buffer::from_vec(values.map(|v| (v * 200.0) as u16).collect::<Vec<_>>()),
        DType::I16 => Buffer::from_vec(
            values
                .map(|v| (v * 100.0 - 9000.0) as i16)
                .collect::<Vec<_>>(),
        ),
        DType::F32 => {
            let mut v: Vec<f32> = values.map(|v| v as f32 - 100.5).collect();
            v[43] = f32::NAN;
            Buffer::from_vec(v)
        }
        DType::F64 => {
            let mut v: Vec<f64> = values.map(|v| v - 100.5).collect();
            v[43] = f64::NAN;
            Buffer::from_vec(v)
        }
        other => panic!("no test plane for {other}"),
    }
}

fn image(dtype: DType, channels: usize, slices: usize, rgb: bool) -> Image {
    let mut meta = ImageMeta::xyczt(dtype, W, H, channels, slices, 1).unwrap();
    if rgb {
        meta.color = ColorModel::Rgb;
    }
    let planes = (0..meta.plane_count()).map(|p| plane(dtype, p)).collect();
    Image::from_planes(meta, planes).unwrap()
}

fn planes(session: &Session, id: DatasetId) -> Vec<Buffer> {
    let image = session.workspace.image(id).unwrap();
    (0..image.meta().plane_count())
        .map(|p| image.read_plane(p).unwrap())
        .collect()
}

/// Equal, with NaN equal to NaN.
fn same(a: &Buffer, b: &Buffer) -> bool {
    a.len() == b.len()
        && a.dtype() == b.dtype()
        && (0..a.len()).all(|i| {
            let (x, y) = (a.get_f64(i).unwrap(), b.get_f64(i).unwrap());
            x == y || (x.is_nan() && y.is_nan())
        })
}

#[test]
fn every_filter_is_under_process_filters_with_its_imagej_name() {
    let registry = standard_registry();
    let ids: Vec<&str> = registry
        .specs()
        .filter(|s| s.id.starts_with("filter."))
        .map(|s| s.id.as_str())
        .collect();
    let mut expected = FILTERS.to_vec();
    expected.sort_unstable();
    let mut found = ids.clone();
    found.sort_unstable();
    assert_eq!(found, expected);

    let imagej = [
        ("filter.gaussian_blur", "Gaussian Blur...", true),
        ("filter.median", "Median...", true),
        ("filter.mean", "Mean...", true),
        ("filter.minimum", "Minimum...", true),
        ("filter.maximum", "Maximum...", true),
        ("filter.variance", "Variance...", true),
        ("filter.top_hat", "Top Hat...", true),
        ("filter.convolve", "Convolve...", true),
        ("filter.unsharp_mask", "Unsharp Mask...", true),
        ("filter.smooth", "Smooth", false),
        ("filter.sharpen", "Sharpen", false),
        ("filter.find_edges", "Find Edges", false),
        ("filter.despeckle", "Despeckle", false),
        ("filter.remove_outliers", "Remove Outliers...", true),
        ("filter.remove_nans", "Remove NaNs...", true),
    ];
    for (id, name, previewable) in imagej {
        let spec = registry.get(id).unwrap().spec().clone();
        assert_eq!(spec.ijm_name.as_deref(), Some(name), "{id}");
        // ImageJ shows a dialog with a preview exactly for these.
        assert_eq!(spec.flags.previewable, previewable, "{id}");
        assert_eq!(spec.menu.unwrap().path, ["Process", "Filters"], "{id}");
        assert!(spec.flags.cancellable && spec.flags.headless_ok, "{id}");
    }
    assert_eq!(
        registry.get("filter.shadows").unwrap().spec().ijm_name,
        None
    );

    // ImageJ's macro keywords, defaults and units.
    let param = |id: &str, name: &str| {
        registry
            .get(id)
            .unwrap()
            .spec()
            .param(name)
            .unwrap_or_else(|| panic!("{id} has no {name}"))
            .clone()
    };
    let keys = [
        ("filter.gaussian_blur", "sigma", "sigma", Value::Float(2.0)),
        (
            "filter.gaussian_blur",
            "scaled",
            "scaled",
            Value::Bool(false),
        ),
        ("filter.median", "radius", "radius", Value::Float(2.0)),
        ("filter.mean", "radius", "radius", Value::Float(2.0)),
        (
            "filter.top_hat",
            "light_background",
            "light",
            Value::Bool(false),
        ),
        (
            "filter.top_hat",
            "dont_subtract",
            "don't",
            Value::Bool(false),
        ),
        (
            "filter.convolve",
            "normalize",
            "normalize",
            Value::Bool(true),
        ),
        ("filter.unsharp_mask", "sigma", "radius", Value::Float(1.0)),
        ("filter.unsharp_mask", "weight", "mask", Value::Float(0.6)),
        (
            "filter.remove_outliers",
            "threshold",
            "threshold",
            Value::Float(50.0),
        ),
        (
            "filter.remove_outliers",
            "which",
            "which",
            Value::from("bright"),
        ),
        ("filter.remove_nans", "radius", "radius", Value::Float(2.0)),
    ];
    for (id, name, key, default) in keys {
        let p = param(id, name);
        assert_eq!(p.ijm_key.as_deref(), Some(key), "{id} {name}");
        assert_eq!(p.default, Some(default), "{id} {name}");
    }
    assert_eq!(
        param("filter.convolve", "kernel").ijm_key.as_deref(),
        Some("text1")
    );
    assert_eq!(
        param("filter.gaussian_blur", "sigma").unit.as_deref(),
        Some("pixels")
    );
    assert_eq!(param("filter.median", "radius").max, Some(100.0));
    assert_eq!(param("filter.mean", "radius").max, Some(1000.0));
    for id in FILTERS {
        let stack = param(id, "stack");
        assert_eq!(stack.ijm_key.as_deref(), Some("stack"), "{id}");
    }
}

#[test]
fn every_filter_runs_with_defaults_on_every_pixel_type() {
    let kinds = [
        (DType::U8, false),
        (DType::U8, true),
        (DType::U16, false),
        (DType::I16, false),
        (DType::F32, false),
        (DType::F64, false),
    ];
    for (dtype, rgb) in kinds {
        for id in FILTERS {
            let (exec, mut session) = setup();
            let channels = if rgb { 3 } else { 1 };
            let dataset = session
                .workspace
                .insert(image(dtype, channels, 1, rgb), None);
            let before = planes(&session, dataset);
            let result = exec.run(&mut session, id, &Args::new());
            if id == "filter.remove_nans" && !dtype.is_float() {
                let message = result.unwrap_err().to_string();
                assert!(message.contains("needs a float image"), "{message}");
                continue;
            }
            result.unwrap_or_else(|e| panic!("{id} on {dtype} (rgb {rgb}): {e}"));
            let after = planes(&session, dataset);
            assert_eq!(after.len(), before.len());
            for (a, b) in after.iter().zip(&before) {
                assert_eq!(a.dtype(), dtype, "{id}");
                assert!(
                    !same(a, b),
                    "{id} left a {dtype} plane (rgb {rgb}) unchanged"
                );
            }
            // One step of undo brings every sample back.
            exec.undo(&mut session).unwrap();
            for (a, b) in planes(&session, dataset).iter().zip(&before) {
                assert!(same(a, b), "{id}: undo did not restore {dtype}");
            }
        }
    }
}

#[test]
fn only_the_plane_on_view_is_filtered_unless_the_whole_stack_is_asked_for() {
    for id in FILTERS {
        let (exec, mut session) = setup();
        let dataset = session
            .workspace
            .insert(image(DType::F32, 1, 3, false), None);
        let before = planes(&session, dataset);

        exec.run(&mut session, id, &Args::new().with("slice", 2))
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let one = planes(&session, dataset);
        assert!(
            same(&one[0], &before[0]) && same(&one[2], &before[2]),
            "{id}"
        );
        assert!(!same(&one[1], &before[1]), "{id}");
        exec.undo(&mut session).unwrap();

        exec.run(&mut session, id, &Args::new().with("stack", true))
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let all = planes(&session, dataset);
        assert!(
            same(&all[1], &one[1]),
            "{id}: a slice is filtered the same either way"
        );
        for (a, b) in all.iter().zip(&before) {
            assert!(!same(a, b), "{id}");
        }
        assert_eq!(exec.undo(&mut session).unwrap(), {
            let registry = standard_registry();
            registry.get(id).unwrap().spec().title.clone()
        });
        for (a, b) in planes(&session, dataset).iter().zip(&before) {
            assert!(same(a, b), "{id}");
        }
    }
}

#[test]
fn a_selection_confines_every_filter() {
    let (x0, y0, w, h) = (3, 2, 6, 5);
    for id in FILTERS {
        let (exec, mut session) = setup();
        let dataset = session
            .workspace
            .insert(image(DType::F32, 1, 1, false), None);
        let before = planes(&session, dataset).remove(0);
        exec.run(
            &mut session,
            "select.set",
            &Args::new().with("selection", format!("rectangle {x0} {y0} {w} {h}")),
        )
        .unwrap();
        exec.run(&mut session, id, &Args::new())
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let after = planes(&session, dataset).remove(0);
        let mut changed = 0;
        for i in 0..W * H {
            let (x, y) = (i % W, i / W);
            let inside = x >= x0 && x < x0 + w && y >= y0 && y < y0 + h;
            let (a, b) = (after.get_f64(i).unwrap(), before.get_f64(i).unwrap());
            let equal = a == b || (a.is_nan() && b.is_nan());
            assert!(
                inside || equal,
                "{id} changed ({x}, {y}) outside the selection"
            );
            changed += usize::from(!equal);
        }
        assert!(changed > 0, "{id} changed nothing inside the selection");
        exec.undo(&mut session).unwrap();
        assert!(same(&planes(&session, dataset)[0], &before), "{id}");
    }
}

/// Inside a rectangular selection the samples are the ones the algorithm
/// computes for that rectangle, reading neighbours from outside it.
#[test]
fn a_rectangle_gets_the_algorithms_result_for_that_rectangle() {
    let (exec, mut session) = setup();
    let dataset = session
        .workspace
        .insert(image(DType::U16, 1, 1, false), None);
    let before = planes(&session, dataset).remove(0);
    exec.run(
        &mut session,
        "select.set",
        &Args::new().with("selection", "rectangle 3 2 6 5"),
    )
    .unwrap();
    exec.run(
        &mut session,
        "filter.mean",
        &Args::new().with("radius", 3.0),
    )
    .unwrap();
    let frame = Frame::new(W, H, calipix_core::Rect::new(3, 2, 6, 5));
    let expected = filter::rank_filter(&before, frame, 3.0, filter::Rank::Mean).unwrap();
    assert_eq!(planes(&session, dataset).remove(0), expected);
}

#[test]
fn gaussian_blur_through_the_executor_matches_imagej() {
    for base in ["gray8", "gray16", "gray32f", "rgb"] {
        let (exec, mut session) = setup();
        let open = Args::new().with("path", fixture(&format!("{base}.tif")));
        let out = exec.run(&mut session, "file.open", &open).unwrap();
        let Some(&Value::Image(id)) = out.get("image") else {
            panic!("file.open returned no image");
        };
        exec.run(
            &mut session,
            "filter.gaussian_blur",
            &Args::new().with("sigma", 2.0),
        )
        .unwrap();
        let expected = calipix_io::open(fixture(&format!("{base}_gauss2.tif"))).unwrap();
        for (p, ours) in planes(&session, id).iter().enumerate() {
            assert_eq!(ours, &expected.read_plane(p).unwrap(), "{base} plane {p}");
        }
    }
}

#[test]
fn scaled_units_divide_sigma_by_the_pixel_size() {
    let source = plane(DType::U16, 0);
    let calibrated = |unit: Option<&str>| {
        let axis = |kind, scale: f64| Axis {
            kind,
            scale,
            origin: 0.0,
            unit: unit.map(str::to_string),
        };
        let meta = ImageMeta::new(
            DType::U16,
            vec![(axis(AxisKind::X, 0.5), W), (axis(AxisKind::Y, 0.25), H)],
        )
        .unwrap();
        Image::from_planes(meta, vec![source.clone()]).unwrap()
    };
    let blur = |image: Image, scaled: bool| {
        let (exec, mut session) = setup();
        let id = session.workspace.insert(image, None);
        let args = Args::new().with("sigma", 0.5).with("scaled", scaled);
        exec.run(&mut session, "filter.gaussian_blur", &args)
            .unwrap();
        planes(&session, id).remove(0)
    };
    let direct = |sx: f64, sy: f64| {
        filter::gaussian_blur(&source, Frame::whole(W, H), sx, sy, 0.0002).unwrap()
    };
    assert_eq!(blur(calibrated(Some("um")), true), direct(1.0, 2.0));
    assert_eq!(blur(calibrated(Some("um")), false), direct(0.5, 0.5));
    // Without a unit of length there is nothing to scale by, as in ImageJ.
    assert_eq!(blur(calibrated(None), true), direct(0.5, 0.5));
    assert_eq!(blur(calibrated(Some("pixel")), true), direct(0.5, 0.5));
}

#[test]
fn an_inverting_lookup_table_swaps_bright_and_dark() {
    let source = plane(DType::U8, 0);
    let run = |id: &str, args: Args, inverted: bool| {
        let (exec, mut session) = setup();
        let mut meta = ImageMeta::plane(DType::U8, W, H).unwrap();
        let lut = if inverted {
            Lut::grays().inverted()
        } else {
            Lut::grays()
        };
        meta.channels = vec![ChannelMeta {
            lut: Some(lut),
            ..ChannelMeta::default()
        }];
        let image = Image::from_planes(meta, vec![source.clone()]).unwrap();
        let dataset = session.workspace.insert(image, None);
        exec.run(&mut session, id, &args).unwrap();
        planes(&session, dataset).remove(0)
    };
    let frame = Frame::whole(W, H);

    let args = || Args::new().with("radius", 1.0).with("threshold", 20.0);
    let outliers = |which, inverted| {
        filter::remove_outliers(&source, frame, 1.0, 20.0, which, inverted).unwrap()
    };
    let bright = run("filter.remove_outliers", args(), false);
    assert_eq!(bright, outliers(Outliers::Bright, false));
    // The spike is gone.
    assert!(bright.as_slice::<u8>().unwrap()[40] < source.as_slice::<u8>().unwrap()[40] - 100);
    let dark = run(
        "filter.remove_outliers",
        args().with("which", "dark"),
        false,
    );
    assert_eq!(dark, outliers(Outliers::Dark, false));
    assert_ne!(dark, bright);
    // Shown inverted, the spike looks dark.
    assert_eq!(run("filter.remove_outliers", args(), true), dark);
    assert_eq!(
        run("filter.remove_outliers", args().with("which", "dark"), true),
        bright
    );

    let hat = |light: bool, inverted: bool| {
        run(
            "filter.top_hat",
            Args::new().with("light_background", light),
            inverted,
        )
    };
    assert_eq!(
        hat(false, false),
        filter::top_hat(&source, frame, 2.0, false, false, false).unwrap()
    );
    assert_eq!(
        hat(true, false),
        filter::top_hat(&source, frame, 2.0, true, false, false).unwrap()
    );
    assert_ne!(hat(false, false), hat(true, false));
    assert_eq!(hat(false, true), hat(true, false));
    assert_eq!(hat(true, true), hat(false, false));
}

#[test]
fn rgb_images_use_imagejs_colour_rules_for_sharpen_and_shadows() {
    for (id, args) in [
        ("filter.sharpen", Args::new()),
        ("filter.shadows", Args::new().with("direction", "southeast")),
    ] {
        let (exec, mut session) = setup();
        let rgb = session.workspace.insert(image(DType::U8, 3, 1, true), None);
        let grey = session
            .workspace
            .insert(image(DType::U8, 3, 1, false), None);
        let before = planes(&session, rgb);
        exec.run(
            &mut session,
            id,
            &args.clone().with("input", Value::Image(rgb)),
        )
        .unwrap();
        exec.run(&mut session, id, &args.with("input", Value::Image(grey)))
            .unwrap();
        let (rgb, grey) = (planes(&session, rgb), planes(&session, grey));
        for ((colour, grey), before) in rgb.iter().zip(&grey).zip(&before) {
            let (colour, grey) = (
                colour.as_slice::<u8>().unwrap(),
                grey.as_slice::<u8>().unwrap(),
            );
            let before = before.as_slice::<u8>().unwrap();
            // ImageJ leaves the border of an RGB image as it is.
            assert_eq!(&colour[..W], &before[..W], "{id}");
            assert_eq!(&colour[(H - 1) * W..], &before[(H - 1) * W..], "{id}");
            assert_ne!(&grey[..W], &before[..W], "{id}");
            assert_ne!(colour, before, "{id}");
        }
    }
}

#[test]
fn bad_arguments_are_refused_with_a_reason_and_change_nothing() {
    let (exec, mut session) = setup();
    let dataset = session
        .workspace
        .insert(image(DType::U8, 1, 1, false), None);
    let before = planes(&session, dataset);
    let cases = [
        (
            "filter.convolve",
            Args::new().with("kernel", "1 2; 3 4"),
            "odd width and height",
        ),
        (
            "filter.convolve",
            Args::new().with("kernel", "1 2 3; 4 5"),
            "row 2",
        ),
        (
            "filter.convolve",
            Args::new().with("kernel", "1 two 3"),
            "`two`",
        ),
        (
            "filter.gaussian_blur",
            Args::new().with("sigma", -1.0),
            "sigma",
        ),
        ("filter.median", Args::new().with("radius", 101.0), "radius"),
        (
            "filter.unsharp_mask",
            Args::new().with("weight", 1.0),
            "weight",
        ),
        (
            "filter.remove_outliers",
            Args::new().with("which", "odd"),
            "which",
        ),
        (
            "filter.shadows",
            Args::new().with("direction", "up"),
            "direction",
        ),
    ];
    for (id, args, reason) in cases {
        let err = exec.run(&mut session, id, &args).unwrap_err();
        assert!(matches!(err, Error::InvalidArg { .. }), "{id}: {err:?}");
        assert!(err.to_string().contains(reason), "{id}: {err}");
    }
    assert_eq!(planes(&session, dataset), before);
    assert!(session.history().entries().is_empty());

    // A kernel may be typed on one line or several.
    for kernel in [
        "0 0 0; 0 2 0; 0 0 0",
        "0 0 0\n0 2 0\n0 0 0\n",
        "0,0,0,0,2,0,0,0,0",
    ] {
        exec.run(
            &mut session,
            "filter.convolve",
            &Args::new().with("kernel", kernel),
        )
        .unwrap();
        assert_eq!(planes(&session, dataset), before, "{kernel:?}");
    }
}
