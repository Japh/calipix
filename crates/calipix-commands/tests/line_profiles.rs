//! Analyze > Plot Profile along lines, run through the executor
//! (`select.set`, then `analyze.plot_profile`) and compared with ImageJ
//! 1.54p (`fixtures/imagej/out/profile_cases.txt`, written by
//! `gen/81_line_profiles.ijm`): straight lines one pixel wide, with and
//! without interpolation; straight lines of several widths; segmented and
//! freehand lines, thin and wide; rotated rectangles; on 8-bit, 16-bit,
//! float and RGB images and with pixels that are not square.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use calipix_command::{Args, EffectsPolicy, Executor, Session, Value};
use calipix_commands::standard_registry;
use calipix_core::Image;
use calipix_roi::Roi;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

#[derive(Debug, Default)]
struct Case {
    name: String,
    about: BTreeMap<String, String>,
    expected: BTreeMap<String, String>,
}

fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(fixture("profile_cases.txt")).unwrap();
    let mut cases: Vec<Case> = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        if let Some(name) = line.strip_prefix("# ") {
            cases.push(Case {
                name: name.to_string(),
                ..Case::default()
            });
            continue;
        }
        let case = cases.last_mut().unwrap();
        let (key, value) = line.split_once('=').unwrap();
        match key.strip_prefix('@') {
            Some(key) => case.about.insert(key.to_string(), value.to_string()),
            None => case.expected.insert(key.to_string(), value.to_string()),
        };
    }
    cases
}

fn java_double(text: &str) -> f64 {
    match text {
        "NaN" => f64::NAN,
        other => other.parse().unwrap(),
    }
}

fn close(ours: f64, theirs: f64, relative: f64) -> bool {
    if theirs.is_nan() {
        return ours.is_nan();
    }
    (ours - theirs).abs() <= relative * theirs.abs() || (ours - theirs).abs() < 1e-9
}

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

fn column(session: &Session, table: &str, name: &str) -> Vec<f64> {
    session
        .workspace
        .table(table)
        .unwrap()
        .numbers(name)
        .unwrap()
        .into_iter()
        .map(|v| v.unwrap())
        .collect()
}

#[test]
fn line_profiles_match_imagej() {
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let (mut checked, mut values) = (0, 0);
    let mut failures = Vec::new();
    for case in &cases() {
        let (exec, mut session) = setup();
        exec.run(
            &mut session,
            "file.open",
            &Args::new().with("path", fixture(&case.about["image"])),
        )
        .unwrap();
        if let Some(cal) = case.about.get("cal") {
            let w: Vec<&str> = cal.split_whitespace().collect();
            exec.run(
                &mut session,
                "image.properties",
                &Args::new()
                    .with("pixel_width", w[0].parse::<f64>().unwrap())
                    .with("pixel_height", w[1].parse::<f64>().unwrap())
                    .with("unit", w[2]),
            )
            .unwrap();
        }
        let roi: Roi = match (case.about.get("roi_file"), case.about.get("roi")) {
            (Some(file), _) => {
                calipix_roi::decode(&std::fs::read(fixture(file)).unwrap())
                    .unwrap()
                    .roi
            }
            (None, Some(text)) => text.parse().unwrap(),
            (None, None) => panic!("{}: no selection", case.name),
        };
        exec.run(
            &mut session,
            "select.set",
            &Args::new().with("selection", roi.to_string()),
        )
        .unwrap();
        let interpolate = case.about.get("interpolate").is_none_or(|v| v != "0");
        let out = exec
            .run(
                &mut session,
                "analyze.plot_profile",
                &Args::new().with("interpolate", interpolate),
            )
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let Some(Value::Str(table)) = out.get("table") else {
            panic!("{}: no table", case.name);
        };
        let theirs: Vec<f64> = case.expected["profile"]
            .split_whitespace()
            .map(java_double)
            .collect();
        let ours = column(&session, table, "Value");
        if ours.len() != theirs.len() {
            failures.push(format!(
                "{}: {} points, ImageJ {}",
                case.name,
                ours.len(),
                theirs.len()
            ));
            continue;
        }
        for (i, (o, t)) in ours.iter().zip(&theirs).enumerate() {
            values += 1;
            if !close(*o, *t, 1e-5) {
                failures.push(format!("{} point {i}: {o}, ImageJ {t}", case.name));
            }
        }
        // The plot's X axis: a multiple of the spacing, which ImageJ holds
        // in single precision.
        let spacing = java_double(&case.expected["spacing"]);
        let distance = column(&session, table, "Distance");
        assert_eq!(distance[0], 0.0, "{}", case.name);
        if !close(distance[1], spacing, 1e-6) {
            failures.push(format!(
                "{} spacing: {}, ImageJ {spacing}",
                case.name, distance[1]
            ));
        }
        let mut kind = roi.kind().name().to_string();
        if roi.line_width() > 1.0 {
            kind = format!("wide-{kind}");
        }
        *kinds.entry(kind).or_default() += 1;
        checked += 1;
    }
    println!("profile_cases.txt: {checked} profiles, {values} values compared, by kind {kinds:?}");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    for kind in [
        "line",
        "wide-line",
        "polyline",
        "wide-polyline",
        "freeline",
        "wide-freeline",
        "rotated-rectangle",
    ] {
        assert!(
            kinds.get(kind).copied().unwrap_or(0) >= 7,
            "{kind}: {kinds:?}"
        );
    }
    assert!(checked >= 160, "only {checked} profiles");
}

#[test]
fn profiles_need_a_line_or_a_rectangle() {
    let (exec, mut session) = setup();
    let image = Image::from_vec(8, 8, (0u8..64).collect()).unwrap();
    let id = session.workspace.insert(image, None);
    for (roi, part) in [
        (
            Roi::oval(1.0, 1.0, 5.0, 5.0),
            "needs a rectangular or line selection",
        ),
        (
            Roi::Angle {
                points: [(1.0, 1.0), (4.0, 4.0), (6.0, 1.0)],
            },
            "needs a rectangular or line selection",
        ),
        (
            Roi::point(2.0, 2.0),
            "needs a rectangular or line selection",
        ),
        // A wide line with no length cannot be straightened.
        (
            Roi::line(3.0, 3.0, 3.0, 3.0).with_line_width(4.0),
            "too short",
        ),
    ] {
        session.workspace.set_roi(id, Some(roi)).unwrap();
        let err = exec
            .run(&mut session, "analyze.plot_profile", &Args::new())
            .unwrap_err();
        assert!(err.to_string().contains(part), "{err}");
    }
    // A rounded rectangle is a rectangle to ImageJ: column averages of its
    // bounds.
    session
        .workspace
        .set_roi(id, Some(Roi::rounded_rectangle(1.0, 2.0, 4.0, 3.0, 2)))
        .unwrap();
    let out = exec
        .run(&mut session, "analyze.plot_profile", &Args::new())
        .unwrap();
    assert_eq!(out.get("points"), Some(&Value::Int(4)));
}
