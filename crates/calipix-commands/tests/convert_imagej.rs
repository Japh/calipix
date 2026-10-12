//! Image > Type conversions, run as commands, against ImageJ 1.54p
//! (`fixtures/imagej/gen/62_convert.ijm`): samples, pixel type, dimensions,
//! display ranges, lookup tables and calibration of every case.

#[path = "../../calipix-ops/tests/math_support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use calipix_command::{Args, EffectsPolicy, Executor, Session};
use calipix_commands::standard_registry;
use calipix_commands::view::channel_mode;
use calipix_core::{AxisKind, Buffer, ColorModel, DType, Image, Rect};
use calipix_roi::Roi;
use support::{
    Difference, Raw, assert_matches, base, close, cropped_base, csv, ij_number, raw_planes,
    report, with_convert_specials,
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

/// A source image of `62_convert.ijm`.
fn source(name: &str) -> Image {
    let axis = |image: &Image, kind: AxisKind| image.meta().axis_index(&kind).unwrap();
    match name {
        "stack" => base("stack_z6_8bit"),
        "hyper" => {
            let image = base("hyperstack_c3z4t5");
            let image = image.range(axis(&image, AxisKind::Z), 0, 2).unwrap();
            let image = image.range(axis(&image, AxisKind::Time), 1, 2).unwrap();
            image.materialize().unwrap()
        }
        "comp" => {
            let image = base("hyperstack_c3z4t5");
            // One slice and one frame, keeping the calibration of both axes.
            let image = image.range(axis(&image, AxisKind::Time), 2, 1).unwrap();
            let image = image.range(axis(&image, AxisKind::Z), 1, 1).unwrap();
            image.materialize().unwrap()
        }
        _ => cropped_base(name, CROP, |plane| with_convert_specials(name, plane)),
    }
}

fn command(name: &str) -> &'static str {
    match name {
        "8-bit" => "image.to_8bit",
        "16-bit" => "image.to_16bit",
        "32-bit" => "image.to_32bit",
        "RGB Color" => "image.to_rgb",
        "RGB Stack" => "image.to_rgb_stack",
        other => panic!("unknown conversion {other}"),
    }
}

#[test]
fn conversions_match_imagej() {
    let (exec, mut session) = setup();
    let mut by_kind: BTreeMap<String, Difference> = BTreeMap::new();
    let mut expected: BTreeMap<String, Vec<Vec<Buffer>>> = BTreeMap::new();
    let mut cases = 0;
    for row in csv("convert.csv") {
        let (group, first, count, name, setup, conversion) = (
            &row[0],
            row[1].parse::<usize>().unwrap(),
            row[2].parse::<usize>().unwrap(),
            row[3].as_str(),
            row[4].as_str(),
            row[5].as_str(),
        );
        let what = format!("{name} [{setup}] {conversion}");
        let original = source(name);
        let id = session.workspace.insert(original.clone(), None);
        let selection = Roi::rectangle(2.0, 3.0, 5.0, 4.0);
        session
            .workspace
            .set_roi(id, Some(selection.clone()))
            .unwrap();

        let mut args = Args::new();
        let mut steps = 1;
        for word in setup.split(' ') {
            if let Some(range) = word.strip_prefix("range=") {
                let (lo, hi) = range.split_once(':').unwrap();
                let range = Args::new()
                    .with("min", ij_number(lo))
                    .with("max", ij_number(hi));
                exec.run(&mut session, "view.set_display_range", &range)
                    .unwrap();
                steps += 1;
            } else if let Some(lut) = word.strip_prefix("lut=") {
                let lut = Args::new().with("lut", lut.to_lowercase());
                exec.run(&mut session, "view.set_lut", &lut).unwrap();
                steps += 1;
            } else if word == "invertlut" {
                exec.run(&mut session, "view.invert_lut", &Args::new())
                    .unwrap();
                steps += 1;
            } else if word == "noscale" {
                args.insert("scale", false);
            } else if word == "weighted" {
                args.insert("weighted", true);
            } else if let Some(earlier) = word.strip_prefix("first=") {
                let earlier = command(&earlier.replace('_', " "));
                exec.run(&mut session, earlier, &Args::new()).unwrap();
                steps += 1;
            }
        }
        exec.run(&mut session, command(conversion), &args)
            .unwrap_or_else(|e| panic!("{what}: {e}"));

        let image = session.workspace.image(id).unwrap().clone();
        let meta = image.meta();
        let kind = Raw::from_bit_depth(&row[6]);
        let rgb = kind == Raw::Rgb;
        let dtype = match kind {
            Raw::U8 | Raw::Rgb => DType::U8,
            Raw::U16 => DType::U16,
            Raw::F32 => DType::F32,
        };
        assert_eq!(meta.dtype(), dtype, "{what}");
        assert_eq!(meta.color == ColorModel::Rgb, rgb, "{what}");
        let channels = if rgb { 1 } else { meta.size(&AxisKind::Channel) };
        assert_eq!(
            [
                channels,
                meta.size(&AxisKind::Z),
                meta.size(&AxisKind::Time)
            ],
            [&row[7], &row[8], &row[9]].map(|n| n.parse::<usize>().unwrap()),
            "{what}: channels, slices, frames"
        );
        assert_eq!((meta.width(), meta.height()), (CROP.w, CROP.h), "{what}");

        // Samples.
        let step = if rgb { 3 } else { 1 };
        assert_eq!(meta.plane_count(), count * step, "{what}: planes");
        let theirs = expected
            .entry(group.clone())
            .or_insert_with(|| raw_planes(&format!("conv_{group}.raw"), kind, CROP.w, CROP.h));
        for k in 0..count {
            for c in 0..step {
                let ours = image.read_plane(k * step + c).unwrap();
                let difference = assert_matches(&ours, &theirs[first - 1 + k][c], &what);
                by_kind
                    .entry(format!("{name} to {conversion}"))
                    .or_default()
                    .merge(difference);
            }
        }

        // Display range of each channel. Converting a composite to 8-bit
        // without scaling leaves ImageJ with the old 16-bit ranges on 8-bit
        // data; Calipix resets them.
        let stale = name == "hyper" && setup == "noscale";
        if !rgb && !stale {
            for (channel, range) in row[10].split(';').enumerate() {
                let (lo, hi) = range.split_once(' ').unwrap();
                let (min, max) = calipix_render::effective_display_range(&image, channel).unwrap();
                assert!(
                    close(min, ij_number(lo)) && close(max, ij_number(hi)),
                    "{what}: channel {channel} shown over {min}..{max}, ImageJ {range}"
                );
            }
        }
        // Lookup table of the first channel.
        if !rgb {
            let lut = calipix_render::channel_lut(&image, 0, channel_mode(meta));
            let entries: Vec<String> = [64, 255]
                .iter()
                .flat_map(|&i| lut.table[i].iter().map(|v| v.to_string()))
                .collect();
            assert_eq!(entries.join("/"), row[12], "{what}: lookup table");
            let inverting = lut.table[0] == [255; 3] && lut.table[255] == [0; 3];
            assert_eq!(inverting, row[11] == "1", "{what}: inverting");
        }
        // Calibration.
        let axis = |kind: AxisKind| meta.axis_index(&kind).map(|i| &meta.axes()[i]);
        let scale = |kind: AxisKind| axis(kind).map_or(1.0, |a| a.scale);
        assert!(close(scale(AxisKind::X), ij_number(&row[13])), "{what}");
        assert!(close(scale(AxisKind::Y), ij_number(&row[14])), "{what}");
        assert!(close(scale(AxisKind::Z), ij_number(&row[15])), "{what}");
        let unit = meta.axes()[0].unit.clone().unwrap_or("pixels".to_string());
        assert_eq!(unit, row[16], "{what}: unit");

        // The selection is kept, and undoing gives back the original image.
        assert_eq!(
            session.workspace.get(id).unwrap().roi,
            Some(selection),
            "{what}"
        );
        for _ in 0..steps {
            if session.workspace.image(id).unwrap().ptr_eq(&original) {
                break;
            }
            exec.undo(&mut session).unwrap();
        }
        assert!(session.workspace.image(id).unwrap().ptr_eq(&original), "{what}");
        session.workspace.remove(id).unwrap();
        cases += 1;
    }
    for (kind, difference) in &by_kind {
        report(kind, *difference);
    }
    assert_eq!(cases, 44);
}
