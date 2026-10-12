//! Process > Math and the image calculator, run as commands, against
//! ImageJ 1.54p (`fixtures/imagej/gen/60_math.ijm`, `61_calculator.ijm`):
//! samples, pixel type, size and display range of every case.

#[path = "../../calipix-ops/tests/math_support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Executor, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::{Buffer, ColorModel, DType, Image, ImageMeta, Rect};
use support::{
    Difference, Raw, assert_matches, base, close, cropped_base, csv, generated, ij_number,
    raw_planes, report, with_math_specials,
};

const CROP: Rect = Rect {
    x: 16,
    y: 12,
    w: 32,
    h: 24,
};

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

/// The display range the viewer would use for plane `plane`.
fn shown_range(session: &Session, id: DatasetId, plane: usize) -> (f64, f64) {
    let image = session.workspace.image(id).unwrap();
    calipix_render::effective_display_range(image, plane).unwrap()
}

fn planes(session: &Session, id: DatasetId) -> Vec<Buffer> {
    let image = session.workspace.image(id).unwrap();
    (0..image.meta().plane_count())
        .map(|p| image.read_plane(p).unwrap())
        .collect()
}

/// The command and arguments for a row of `math.csv`.
fn math_command(command: &str, options: &str, range: &str) -> (&'static str, Args) {
    let value = options.strip_prefix("value=");
    let number = || Value::Float(value.expect("value").parse().unwrap());
    let text = || Value::from(value.expect("value"));
    let (id, args) = match command {
        "Add..." => ("math.add", Args::new().with("value", number())),
        "Subtract..." => ("math.subtract", Args::new().with("value", number())),
        "Multiply..." => ("math.multiply", Args::new().with("value", number())),
        "Divide..." => ("math.divide", Args::new().with("value", number())),
        "AND..." => ("math.and", Args::new().with("value", text())),
        "OR..." => ("math.or", Args::new().with("value", text())),
        "XOR..." => ("math.xor", Args::new().with("value", text())),
        "Min..." => ("math.min", Args::new().with("value", number())),
        "Max..." => ("math.max", Args::new().with("value", number())),
        "Gamma..." => ("math.gamma", Args::new().with("value", number())),
        "Set..." => ("math.set", Args::new().with("value", number())),
        "Log" => ("math.log", Args::new()),
        "Exp" => ("math.exp", Args::new()),
        "Square" => ("math.square", Args::new()),
        "Square Root" => ("math.square_root", Args::new()),
        "Abs" => ("math.abs", Args::new()),
        "Reciprocal" => ("math.reciprocal", Args::new()),
        "NaN Background" => {
            let limits: Vec<f64> = range.split(' ').skip(1).map(ij_number).collect();
            (
                "math.nan_background",
                Args::new().with("lower", limits[0]).with("upper", limits[1]),
            )
        }
        other => panic!("unknown command {other}"),
    };
    (id, args)
}

#[test]
fn process_math_commands_match_imagej() {
    let (exec, mut session) = setup();
    let mut by_op: BTreeMap<String, Difference> = BTreeMap::new();
    let mut cases = 0;
    let mut expected: BTreeMap<String, Vec<Vec<Buffer>>> = BTreeMap::new();
    for row in csv("math.csv") {
        let (name, slice, command, options, range_before) =
            (&row[0], row[1].parse::<usize>().unwrap(), &row[2], &row[3], &row[4]);
        let what = format!("{name} {command} {options} [{range_before}]");
        let image = cropped_base(name, CROP, |plane| with_math_specials(name, plane));
        let original = image.clone();
        let id = session.workspace.insert(image, None);
        if range_before != "-" && !range_before.starts_with("threshold") {
            let (lo, hi) = range_before.split_once(' ').unwrap();
            exec.run(
                &mut session,
                "view.set_display_range",
                &Args::new()
                    .with("min", ij_number(lo))
                    .with("max", ij_number(hi)),
            )
            .unwrap();
        }
        let (command_id, args) = math_command(command, options, range_before);
        let result = exec.run(&mut session, command_id, &args);
        // ImageJ silently does nothing where Calipix says why it cannot.
        if command == "Reciprocal" && name != "gray32f" {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("needs a float image"), "{what}: {error}");
        } else {
            result.unwrap_or_else(|e| panic!("{what}: {e}"));
        }

        let theirs = &expected.entry(name.clone()).or_insert_with(|| {
            raw_planes(
                &format!("math_{name}.raw"),
                Raw::for_base(name),
                CROP.w,
                CROP.h,
            )
        })[slice - 1];
        for (ours, theirs) in planes(&session, id).iter().zip(theirs) {
            let difference = assert_matches(ours, theirs, &what);
            by_op
                .entry(format!("{name} {command}"))
                .or_default()
                .merge(difference);
        }
        let (min, max) = shown_range(&session, id, 0);
        let (their_min, their_max) = (ij_number(&row[5]), ij_number(&row[6]));
        assert!(
            close(min, their_min) && close(max, their_max),
            "{what}: display range {min}..{max}, ImageJ {their_min}..{their_max}"
        );

        // Undo restores the very image that was there, display range
        // included.
        while session.workspace.image(id).is_ok_and(|i| !i.ptr_eq(&original)) {
            exec.undo(&mut session).unwrap();
        }
        session.workspace.remove(id).unwrap();
        cases += 1;
    }
    for (operation, difference) in &by_op {
        report(operation, *difference);
    }
    assert_eq!(cases, 118);
}

/// An image for the calculator: a cropped base image, the 8-bit stack, or
/// one of the generated operands.
fn operand(exec: &Executor, session: &mut Session, name: &str) -> DatasetId {
    let image = match name {
        "gray8" | "gray16" | "gray32f" | "rgb" => {
            cropped_base(name, CROP, |plane| with_math_specials(name, plane))
        }
        "stack_z6_8bit" | "bstack" => base("stack_z6_8bit"),
        _ => {
            let planes = generated(name);
            let (w, h) = if name == "small8" { (20, 10) } else { (32, 24) };
            let mut meta =
                ImageMeta::xyczt(planes[0].dtype(), w, h, planes.len(), 1, 1).unwrap();
            if planes.len() == 3 {
                meta.color = ColorModel::Rgb;
            }
            Image::from_planes(meta.with_name(name), planes).unwrap()
        }
    };
    let id = session.workspace.insert(image, None);
    if name == "bstack" {
        let args = Args::new().with("value", 0.5).with("stack", true);
        exec.run(session, "math.multiply", &args).unwrap();
    }
    id
}

#[test]
fn image_calculator_matches_imagej() {
    let (exec, mut session) = setup();
    let mut by_group: BTreeMap<String, Difference> = BTreeMap::new();
    let mut expected: BTreeMap<String, Vec<Vec<Buffer>>> = BTreeMap::new();
    let mut cases = 0;
    for row in csv("calc.csv") {
        let (group, first, count, name1, name2, options) = (
            &row[0],
            row[1].parse::<usize>().unwrap(),
            row[2].parse::<usize>().unwrap(),
            &row[3],
            &row[4],
            &row[5],
        );
        let (width, height, bits) = (
            row[6].parse::<usize>().unwrap(),
            row[7].parse::<usize>().unwrap(),
            &row[8],
        );
        let what = format!("{name1} {options} {name2}");
        let words: Vec<&str> = options.split(' ').collect();
        let id2 = operand(&exec, &mut session, name2);
        let id1 = operand(&exec, &mut session, name1);
        let before = session.workspace.image(id1).unwrap().clone();
        let mut args = Args::new()
            .with("input", id1)
            .with("image2", id2)
            .with("operation", words[0])
            .with("create", words.contains(&"create"))
            .with("float_result", words.contains(&"32-bit"))
            .with("stack", words.contains(&"stack"));
        let mut slice = 1;
        for word in &words {
            if let Some(value) = word.strip_prefix("divide=") {
                args.insert("divide_by_zero", value);
            }
            if let Some(n) = word.strip_prefix("slice=") {
                slice = n.parse().unwrap();
                args.insert("slice", slice);
            }
        }
        let outputs = exec
            .run(&mut session, "math.image_calculator", &args)
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        let created = matches!(outputs.get("image"), Some(Value::Image(_)));
        let result = match outputs.get("image") {
            Some(&Value::Image(id)) => id,
            _ => id1,
        };
        // A float result that cannot be stored in image 1 makes a new image.
        let float_forces = words.contains(&"32-bit") && !(name1 == "gray32f" && name2 == "bf");
        assert_eq!(created, words.contains(&"create") || float_forces, "{what}");
        if created {
            let unchanged = session.workspace.image(id1).unwrap();
            assert!(unchanged.ptr_eq(&before), "{what}: image 1 was changed");
        }

        let image = session.workspace.image(result).unwrap().clone();
        let meta = image.meta();
        let kind = Raw::from_bit_depth(bits);
        assert_eq!((meta.width(), meta.height()), (width, height), "{what}");
        let (dtype, rgb) = match kind {
            Raw::U8 => (DType::U8, false),
            Raw::U16 => (DType::U16, false),
            Raw::F32 => (DType::F32, false),
            Raw::Rgb => (DType::U8, true),
        };
        assert_eq!(meta.dtype(), dtype, "{what}");
        assert_eq!(meta.color == ColorModel::Rgb, rgb, "{what}");
        if created {
            assert_eq!(meta.name, format!("Result of {}", before.meta().name));
        }

        let step = if rgb { 3 } else { 1 };
        assert_eq!(meta.plane_count(), count * step, "{what}: planes");
        let theirs = expected
            .entry(group.clone())
            .or_insert_with(|| raw_planes(&format!("calc_{group}.raw"), kind, width, height));
        // Without "stack" ImageJ pairs the current slice of each image and
        // Calipix the planes at the same position. They differ in the one
        // case that combines slice 3 of a stack with another stack, whose
        // current slice is its first.
        let pairs_differently = name2 == "bstack" && slice == 3 && !words.contains(&"stack");
        if pairs_differently {
            let b = session.workspace.image(id2).unwrap().read_plane(2).unwrap();
            let a = before.read_plane(2).unwrap();
            let sum = calipix_ops::math::calculate(&a, &b, calipix_ops::math::CalcOp::Add, 0.0);
            assert_eq!(image.read_plane(0).unwrap(), sum.unwrap(), "{what}");
        } else {
            for k in 0..count {
                for c in 0..step {
                    let ours = image.read_plane(k * step + c).unwrap();
                    let difference = assert_matches(&ours, &theirs[first - 1 + k][c], &what);
                    by_group
                        .entry(format!("{group} {}", words[0]))
                        .or_default()
                        .merge(difference);
                }
            }
        }

        // The display range: ImageJ's is that of the slice left current.
        // An 8-bit result of a calculation done in float carries the float
        // range in ImageJ, which shows it almost uniformly grey; Calipix
        // keeps the 8-bit default.
        let odd_range = bits == "8" && name2 == "bf" && matches!(words[0], "Multiply" | "Divide");
        if !rgb && !odd_range {
            let current = if created { 0 } else { slice - 1 };
            let (min, max) = calipix_render::effective_display_range(&image, current).unwrap();
            let (their_min, their_max) = (ij_number(&row[9]), ij_number(&row[10]));
            assert!(
                close(min, their_min) && close(max, their_max),
                "{what}: display range {min}..{max}, ImageJ {their_min}..{their_max}"
            );
        }

        // Undo removes a created image and restores a changed one.
        exec.undo(&mut session).unwrap();
        assert!(session.workspace.image(id1).unwrap().ptr_eq(&before), "{what}");
        assert_eq!(session.workspace.image(result).is_ok(), !created, "{what}");
        let ids: Vec<DatasetId> = session.workspace.datasets().map(|d| d.id).collect();
        for id in ids {
            session.workspace.remove(id).unwrap();
        }
        cases += 1;
    }
    for (operation, difference) in &by_group {
        report(operation, *difference);
    }
    assert_eq!(cases, 104);
}
