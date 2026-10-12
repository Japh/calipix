//! Analyze > Measure on every kind of selection, run end to end as a
//! frontend runs it: `file.open`, `image.properties`,
//! `measure.set_measurements`, `select.set` with the selection's text
//! form, then `measure.measure`, all through the executor. Every column of
//! the row (or rows) added to the Results table is compared with what
//! ImageJ 1.54p measured for the same selection.
//!
//! References, in `fixtures/imagej/out`:
//!
//! * `measure_cases.txt` (`gen/70_measure.ijm`): no selection, rectangles,
//!   ovals, polygons, a freehand outline, straight lines and single points
//!   on each kind of image, with calibrations, Invert Y and a hyperstack.
//! * `measure_shape_cases.txt` (`gen/80_measure_shapes.ijm`): each of the
//!   selections saved by `gen/40_roi_shapes.ijm` (`roi_*.roi`), which
//!   include rounded rectangles, ellipses, rotated rectangles, composites,
//!   traced outlines, wide lines, segmented and freehand lines, angles and
//!   multi-point selections, selections with fractional coordinates and
//!   selections that cross the image's edge.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Executor, Session, Value, Workspace};
use calipix_commands::standard_registry;
use calipix_core::Cell;
use calipix_roi::Roi;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

/// One block of a cases file: `# name`, `@key=value` lines describing the
/// case, then `Heading=value` as ImageJ measured it.
#[derive(Debug, Default)]
struct Case {
    name: String,
    about: BTreeMap<String, String>,
    expected: Vec<(String, f64)>,
}

fn java_double(text: &str) -> f64 {
    match text {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        other => other
            .parse()
            .unwrap_or_else(|_| panic!("not a number: {other}")),
    }
}

fn cases(file: &str) -> Vec<Case> {
    let text = std::fs::read_to_string(fixture(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
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
            Some(key) => {
                case.about.insert(key.to_string(), value.to_string());
            }
            None => case.expected.push((key.to_string(), java_double(value))),
        }
    }
    cases
}

/// Headings whose values are counts, sample values or whole pixels, which
/// must match exactly when ImageJ's value is a whole number.
const EXACT: [&str; 13] = [
    "Area",
    "Min",
    "Max",
    "Mode",
    "Median",
    "BX",
    "BY",
    "Width",
    "Height",
    "Ch",
    "Slice",
    "Frame",
    "RawIntDen",
];

/// Integers exactly; everything else within 1e-5 relative. Values that are
/// zero but for rounding (the skewness of a symmetric region) cannot be
/// held to a relative error and get an absolute one.
fn matches_imagej(heading: &str, ours: f64, theirs: f64) -> bool {
    if theirs.is_nan() {
        return ours.is_nan();
    }
    if theirs.is_infinite() {
        return ours == theirs;
    }
    if EXACT.contains(&heading) && theirs.fract() == 0.0 {
        return ours == theirs;
    }
    (ours - theirs).abs() <= 1e-5 * theirs.abs() || (ours - theirs).abs() < 1e-9
}

const ALL: &str = "area mean standard modal min centroid center perimeter bounding fit shape \
                   feret's integrated median skewness kurtosis area_fraction stack";

/// Set Measurements arguments from ImageJ's keywords; anything not named
/// is off, as in an ImageJ macro.
fn measurement_args(keywords: &str, invert_y: bool) -> Args {
    let keys = [
        ("area", "area"),
        ("mean", "mean"),
        ("standard", "std_dev"),
        ("modal", "mode"),
        ("min", "min_max"),
        ("centroid", "centroid"),
        ("center", "center_of_mass"),
        ("perimeter", "perimeter"),
        ("bounding", "bounding_rect"),
        ("fit", "fit_ellipse"),
        ("shape", "shape_descriptors"),
        ("feret's", "feret"),
        ("integrated", "integrated_density"),
        ("median", "median"),
        ("skewness", "skewness"),
        ("kurtosis", "kurtosis"),
        ("area_fraction", "area_fraction"),
        ("stack", "stack_position"),
    ];
    let words: Vec<&str> = keywords.split_whitespace().collect();
    let mut args = Args::new().with("invert_y", invert_y);
    for (keyword, name) in keys {
        args.insert(name, words.contains(&keyword));
    }
    args
}

/// The selection a block of `measure_cases.txt` describes, as the macro
/// made it.
fn roi_of(text: &str) -> Option<Roi> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let n: Vec<f64> = words[1..].iter().map(|w| w.parse().unwrap()).collect();
    let pairs = || -> Vec<(f64, f64)> { n.chunks(2).map(|p| (p[0], p[1])).collect() };
    Some(match words[0] {
        "none" => return None,
        "rect" => Roi::rectangle(n[0], n[1], n[2], n[3]),
        "oval" => Roi::oval(n[0], n[1], n[2], n[3]),
        "polygon" => Roi::polygon(pairs()),
        "freehand" => Roi::Freehand { points: pairs() },
        "line" => Roi::line(n[0], n[1], n[2], n[3]),
        "point" => Roi::point(n[0], n[1]),
        other => panic!("unknown selection {other}"),
    })
}

struct Measurement {
    /// The rows Measure added: heading and value of every filled cell.
    rows: Vec<Vec<(String, f64)>>,
}

/// Runs a case through the executor and returns what Measure added to
/// the Results table.
fn run_case(case: &Case, roi: Option<&Roi>) -> Measurement {
    let exec = Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all());
    let mut session = Session::new();
    let run = |session: &mut Session, id: &str, args: &Args| {
        exec.run(session, id, args)
            .unwrap_or_else(|e| panic!("{}: {id}: {e}", case.name))
    };
    let opened = run(
        &mut session,
        "file.open",
        &Args::new().with("path", fixture(&case.about["image"])),
    );
    let Some(&Value::Image(id)) = opened.get("image") else {
        panic!("{}: file.open returned no image", case.name);
    };
    let id: DatasetId = id;
    if let Some(cal) = case.about.get("cal") {
        let w: Vec<&str> = cal.split_whitespace().collect();
        run(
            &mut session,
            "image.properties",
            &Args::new()
                .with("pixel_width", w[0].parse::<f64>().unwrap())
                .with("pixel_height", w[1].parse::<f64>().unwrap())
                .with("unit", w[2])
                .with("origin", format!("{},{}", w[3], w[4])),
        );
    }
    let keywords = case.about.get("measurements").map_or(ALL, String::as_str);
    let invert_y = case.about.get("invert_y").is_some_and(|v| v == "1");
    run(
        &mut session,
        "measure.set_measurements",
        &measurement_args(keywords, invert_y),
    );
    if let Some(roi) = roi {
        // The text form is what the command line, macros and agents give.
        run(
            &mut session,
            "select.set",
            &Args::new().with("selection", roi.to_string()),
        );
        assert_eq!(
            session.workspace.get(id).unwrap().roi.as_ref(),
            Some(roi),
            "{}: the selection's text form reads back differently",
            case.name
        );
    }
    let mut args = Args::new();
    if let Some(position) = case.about.get("position") {
        let p: Vec<i64> = position
            .split_whitespace()
            .map(|v| v.parse().unwrap())
            .collect();
        args = args
            .with("channel", p[0])
            .with("slice", p[1])
            .with("frame", p[2]);
    }
    run(&mut session, "measure.measure", &args);
    let table = session
        .workspace
        .table(Workspace::RESULTS)
        .unwrap_or_else(|| panic!("{}: no Results table", case.name));
    let headings: Vec<String> = table.column_names().map(str::to_string).collect();
    let rows = (0..table.len())
        .map(|r| {
            headings
                .iter()
                .filter_map(|h| match table.get(r, h) {
                    Cell::Number(v) => Some((h.clone(), *v)),
                    Cell::Empty => None,
                    Cell::Text(t) => panic!("{}: text `{t}` in column {h}", case.name),
                })
                .collect()
        })
        .collect();
    Measurement { rows }
}

#[derive(Default)]
struct Tally {
    cases: usize,
    values: usize,
    kinds: BTreeMap<String, usize>,
    failures: Vec<String>,
}

impl Tally {
    /// Compares one row with ImageJ's. With `complete`, ImageJ listed
    /// exactly the columns of its row, so ours must have the same ones;
    /// otherwise a NaN stands for "no such column".
    fn compare(&mut self, case: &Case, row: &[(String, f64)], complete: bool) {
        for (heading, theirs) in &case.expected {
            let ours = row.iter().find(|(h, _)| h == heading).map(|(_, v)| *v);
            match ours {
                Some(ours) => {
                    self.values += 1;
                    if !matches_imagej(heading, ours, *theirs) {
                        self.failures
                            .push(format!("{} {heading}: {ours}, ImageJ {theirs}", case.name));
                    }
                }
                None if !complete && theirs.is_nan() => {}
                None => self.failures.push(format!(
                    "{} {heading}: no such column, ImageJ {theirs}",
                    case.name
                )),
            }
        }
        if complete {
            for (heading, value) in row {
                if !case.expected.iter().any(|(h, _)| h == heading) {
                    self.failures.push(format!(
                        "{} has {heading}={value}, ImageJ has no such column",
                        case.name
                    ));
                }
            }
        }
        self.cases += 1;
    }

    fn report(&self, what: &str) {
        println!(
            "{what}: {} cases, {} values compared, by kind {:?}",
            self.cases, self.values, self.kinds
        );
        assert!(
            self.failures.is_empty(),
            "{what}: {} of {} values differ in {} cases:\n{}",
            self.failures.len(),
            self.values,
            self.cases,
            self.failures
                .iter()
                .take(80)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[test]
fn every_case_of_the_measurement_fixtures_matches_imagej() {
    let mut tally = Tally::default();
    for case in &cases("measure_cases.txt") {
        let text = &case.about["roi"];
        let kind = text.split_whitespace().next().unwrap().to_string();
        let roi = roi_of(text);
        let measured = run_case(case, roi.as_ref());
        assert_eq!(measured.rows.len(), 1, "{}", case.name);
        // Lines and points were measured with getValue per heading, which
        // gives NaN for a column the row lacks; the rest with
        // List.setMeasurements, which lists the row's columns.
        let complete = !matches!(kind.as_str(), "line" | "point");
        tally.compare(case, &measured.rows[0], complete);
        *tally.kinds.entry(kind).or_default() += 1;
    }
    tally.report("measure_cases.txt");
    for kind in [
        "none", "rect", "oval", "polygon", "freehand", "line", "point",
    ] {
        assert!(tally.kinds.get(kind).copied().unwrap_or(0) >= 5, "{kind}");
    }
    assert_eq!(tally.cases, 303);
    assert!(tally.values > 9000, "only {} values", tally.values);
}

/// The name `calipix-roi` gives the kind of selection in a `.roi` file.
fn kind_name(roi: &Roi) -> String {
    let mut name = roi.kind().name().to_string();
    if roi.is_line() && roi.line_width() > 1.0 {
        name = format!("wide-{name}");
    }
    if matches!(roi, Roi::Points { points } if points.len() > 1) {
        name = "multi-point".to_string();
    }
    name
}

#[test]
fn every_kind_of_selection_measures_as_in_imagej() {
    let mut tally = Tally::default();
    let mut selections: BTreeMap<String, Roi> = BTreeMap::new();
    for case in &cases("measure_shape_cases.txt") {
        let roi = match (case.about.get("roi_file"), case.about.get("roi")) {
            (Some(file), _) => selections
                .entry(file.clone())
                .or_insert_with(|| {
                    let bytes = std::fs::read(fixture(file)).unwrap();
                    let roi = calipix_roi::decode(&bytes)
                        .unwrap_or_else(|e| panic!("{file}: {e}"))
                        .roi;
                    match roi {
                        // ImageJ 1.54p reads the file of a rotated
                        // rectangle back as a polygon with the same four
                        // vertices (RoiDecoder looks for the subtype only
                        // under the freehand type), and that is what it
                        // measured here. Calipix keeps the rotated
                        // rectangle, which the cases made by the macro
                        // cover.
                        Roi::RotatedRectangle { .. } => Roi::polygon(roi.outline()),
                        other => other,
                    }
                })
                .clone(),
            (None, Some(text)) => {
                let roi: Roi = text.parse().unwrap_or_else(|e| panic!("{text}: {e}"));
                if case.about.contains_key("to_area") {
                    roi.to_area().expect("a line has an area")
                } else {
                    roi
                }
            }
            (None, None) => panic!("{}: no selection", case.name),
        };
        let roi = &roi;
        assert_eq!(
            roi.ij_type().to_string(),
            case.about["type"],
            "{}: selection type",
            case.name
        );
        let measured = run_case(case, Some(roi));
        // One row per point of a point selection, otherwise one row.
        let points: usize = case.about.get("points").map_or(1, |n| n.parse().unwrap());
        assert_eq!(measured.rows.len(), points, "{}: rows", case.name);
        let row: usize = case.about.get("row").map_or(0, |n| n.parse().unwrap());
        let mut expected = Case {
            name: case.name.clone(),
            about: case.about.clone(),
            expected: case.expected.clone(),
        };
        // A selection's group number is not kept by Calipix.
        expected.expected.retain(|(h, _)| h != "Group");
        tally.compare(&expected, &measured.rows[row], true);
        *tally.kinds.entry(kind_name(roi)).or_default() += 1;
    }
    tally.report("measure_shape_cases.txt");
    for kind in [
        "rectangle",
        "rounded-rectangle",
        "oval",
        "polygon",
        "freehand",
        "traced",
        "line",
        "wide-line",
        "polyline",
        "wide-polyline",
        "freeline",
        "angle",
        "points",
        "multi-point",
        "ellipse",
        "rotated-rectangle",
        "composite",
    ] {
        assert!(tally.kinds.get(kind).copied().unwrap_or(0) >= 2, "{kind}");
    }
    assert!(tally.cases > 500, "only {} cases", tally.cases);
    assert!(tally.values > 15000, "only {} values", tally.values);
}
