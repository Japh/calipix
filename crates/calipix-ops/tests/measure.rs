//! Measurements, the Results table, histograms and profiles compared with
//! ImageJ 1.54p.
//!
//! The references are in `fixtures/imagej/out`, written by ImageJ from the
//! macros `fixtures/imagej/gen/7?_*.ijm`:
//!
//! * `measure_cases.txt`: every measurement at full precision for no
//!   selection, rectangles, ovals, polygons, a freehand outline, lines and
//!   points on each base image.
//! * `results_*.csv` and `.txt`: the Results table as ImageJ saves it.
//! * `analyze_cases.txt`: histograms within a selection and profiles.
//!
//! Selections are built with `calipix-roi`; the pixels they cover and the
//! vertices of their outlines are compared with the masks ImageJ saved and
//! the vertices it listed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use calipix_core::{AxisKind, Buffer, Cell, ColorModel, Image, Rect, Table};
use calipix_ops::measure::{
    Calibration, HistogramSpec, MeasureSettings, Measured, Measurements, Outline, PlaneContext,
    Region, Row, Samples, TableFormat, append_row, column_average_profile, format_table,
    line_profile, measure, outline_of, row_average_profile, row_label, statistics,
};
use calipix_roi::Roi;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn fixture_text(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// One block of `measure_cases.txt` or `analyze_cases.txt`.
#[derive(Debug, Default)]
struct Case {
    name: String,
    /// The `@key=value` lines.
    about: BTreeMap<String, String>,
    /// The other lines: key and ImageJ's value or values.
    expected: Vec<(String, String)>,
}

impl Case {
    fn about(&self, key: &str) -> Option<&str> {
        self.about.get(key).map(String::as_str)
    }

    fn numbers(&self, key: &str) -> Vec<f64> {
        self.about(key)
            .unwrap_or_else(|| panic!("{}: no @{key}", self.name))
            .split_whitespace()
            .map(parse_java_double)
            .collect()
    }

    fn expected(&self, key: &str) -> f64 {
        parse_java_double(self.text(key))
    }

    fn list(&self, key: &str) -> Vec<f64> {
        self.text(key)
            .split_whitespace()
            .map(parse_java_double)
            .collect()
    }

    fn text(&self, key: &str) -> &str {
        &self
            .expected
            .iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("{}: no {key}", self.name))
            .1
    }
}

fn parse_java_double(text: &str) -> f64 {
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
    let text = fixture_text(file);
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
            None => case.expected.push((key.to_string(), value.to_string())),
        }
    }
    cases
}

/// The planes ImageJ would hold in one processor at a position (one
/// plane, or the three of an RGB image), with what the Results table says
/// about where they are.
struct Plane {
    buffers: Vec<Buffer>,
    width: usize,
    height: usize,
    calibration: Calibration,
    context: PlaneContext,
}

impl Plane {
    /// The plane of `file` at a channel, slice and frame counted from 1.
    fn open(file: &str, position: Option<(usize, usize, usize)>) -> Plane {
        let image: Image = calipix_io::open(fixture(file)).unwrap();
        let meta = image.meta();
        let (channel, slice, frame) = position.unwrap_or((1, 1, 1));
        let at = |c: usize| {
            let position: Vec<usize> = meta
                .axes()
                .iter()
                .skip(2)
                .map(|axis| match axis.kind {
                    AxisKind::Channel => c,
                    AxisKind::Z => slice - 1,
                    AxisKind::Time => frame - 1,
                    _ => 0,
                })
                .collect();
            image
                .read_plane(meta.plane_index(&position).unwrap())
                .unwrap()
        };
        let rgb = meta.color == ColorModel::Rgb;
        let buffers = if rgb {
            vec![at(0), at(1), at(2)]
        } else {
            vec![at(channel - 1)]
        };
        let (channels, slices, frames) = if rgb {
            (1, 1, 1)
        } else {
            (
                meta.size(&AxisKind::Channel),
                meta.size(&AxisKind::Z),
                meta.size(&AxisKind::Time),
            )
        };
        let planes = channels * slices * frames;
        let stack_index = ((frame - 1) * slices + (slice - 1)) * channels + channel;
        let slice_label = meta
            .properties
            .get(&format!("imagej.label.{}", stack_index - 1));
        Plane {
            buffers,
            width: meta.width(),
            height: meta.height(),
            calibration: Calibration::from_meta(meta),
            context: PlaneContext {
                label: row_label(
                    &meta.name,
                    planes,
                    stack_index,
                    slice_label.map(String::as_str),
                ),
                channel,
                slice,
                frame,
                channels,
                slices,
                frames,
                hyperstack: planes > 1,
                stack_index,
            },
        }
    }

    fn of(case: &Case) -> Plane {
        let position = case.about("position").map(|_| {
            let p = case.numbers("position");
            (p[0] as usize, p[1] as usize, p[2] as usize)
        });
        let mut plane = Plane::open(case.about("image").unwrap(), position);
        if let Some(text) = case.about("cal") {
            let parts: Vec<&str> = text.split_whitespace().collect();
            let n = |i: usize| parts[i].parse::<f64>().unwrap();
            plane.calibration = Calibration {
                pixel_width: n(0),
                pixel_height: n(1),
                unit: Some(parts[2].to_string()),
                x_origin: n(3),
                y_origin: n(4),
                invert_y: false,
            };
        }
        plane
    }

    fn samples(&self) -> Samples<'_> {
        match self.buffers.as_slice() {
            [gray] => Samples::Gray(gray),
            [red, green, blue] => Samples::Rgb { red, green, blue },
            _ => unreachable!(),
        }
    }

    fn measured(&self) -> Measured<'_> {
        Measured {
            samples: self.samples(),
            width: self.width,
            height: self.height,
            calibration: &self.calibration,
            context: &self.context,
        }
    }
}

/// The part of a mask image inside `rect`, as a selection's mask.
fn mask_within(name: &str, rect: Rect) -> Vec<u8> {
    let image = calipix_io::open(fixture(name)).unwrap();
    let width = image.meta().width();
    let plane = image.read_plane(0).unwrap();
    let pixels = plane.as_slice::<u8>().unwrap();
    // Nothing outside the bounding rectangle may be selected.
    for (i, &v) in pixels.iter().enumerate() {
        let (x, y) = (i % width, i / width);
        let inside = x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
        assert!(inside || v == 0, "{name}: ({x}, {y}) outside {rect:?}");
    }
    let mut mask = Vec::with_capacity(rect.w * rect.h);
    for y in rect.y..rect.y + rect.h {
        mask.extend_from_slice(&pixels[y * width + rect.x..y * width + rect.x + rect.w]);
    }
    mask
}

/// A selection as the measurement code takes it: the pixels covered and
/// the outline's quantities.
struct Selection {
    kind: String,
    rect: Rect,
    mask: Option<Vec<u8>>,
    outline: Outline,
    /// The selection's numbers as written in the fixture.
    args: Vec<f64>,
}

impl Selection {
    /// Reads `text` (`rect 8 6 20 10`, `oval ...`, `line ...`). `case`
    /// supplies the mask, bounds and vertices of shapes that need them.
    fn parse(text: &str, case: &Case, plane: &Plane) -> Selection {
        let cal = &plane.calibration;
        let words: Vec<&str> = text.split_whitespace().collect();
        let args: Vec<f64> = words[1..].iter().map(|n| n.parse().unwrap()).collect();
        let whole = Rect::new(0, 0, plane.width, plane.height);
        let bounds = || {
            let b = case.numbers("bounds");
            Rect::new(b[0] as usize, b[1] as usize, b[2] as usize, b[3] as usize)
        };
        let coords = || -> Vec<(f64, f64)> {
            case.about("coords")
                .unwrap_or("")
                .split_whitespace()
                .map(|pair| {
                    let (x, y) = pair.split_once(',').unwrap();
                    (x.parse::<f64>().unwrap(), y.parse::<f64>().unwrap())
                })
                .collect()
        };
        let pairs = || -> Vec<(f64, f64)> { args.chunks(2).map(|p| (p[0], p[1])).collect() };
        let roi = match words[0] {
            "none" => None,
            "rect" => Some(Roi::rectangle(args[0], args[1], args[2], args[3])),
            "oval" => Some(Roi::oval(args[0], args[1], args[2], args[3])),
            "polygon" => Some(Roi::polygon(pairs())),
            "freehand" => Some(Roi::Freehand { points: pairs() }),
            "line" => Some(Roi::line(args[0], args[1], args[2], args[3])),
            "point" => Some(Roi::point(args[0], args[1])),
            other => panic!("{}: unknown selection {other}", case.name),
        };
        let (rect, mask, outline) = match &roi {
            None => (whole, None, Outline::whole(plane.width, plane.height)),
            Some(roi) if roi.is_area() => {
                let mask = roi.mask(plane.width, plane.height).unwrap();
                if let Some(file) = case.about("mask") {
                    // The selection covers the pixels ImageJ's did, lists
                    // the vertices ImageJ listed and has its bounds.
                    assert_eq!(mask.rect, bounds(), "{}", case.name);
                    assert_eq!(mask.data, mask_within(file, mask.rect), "{}", case.name);
                    // (The fixture lists vertices to nine decimal places.)
                    let (ours, theirs) = (roi.outline(), coords());
                    if case.about("coords").is_some() {
                        assert_eq!(ours.len(), theirs.len(), "{}", case.name);
                    }
                    for (o, t) in ours.iter().zip(&theirs) {
                        assert!(
                            (o.0 - t.0).abs() < 1e-8 && (o.1 - t.1).abs() < 1e-8,
                            "{}: vertex {o:?}, ImageJ {t:?}",
                            case.name
                        );
                    }
                }
                let data = (!roi.is_rectangular()).then_some(mask.data);
                (mask.rect, data, outline_of(roi, cal).unwrap())
            }
            Some(roi) => (whole, None, outline_of(roi, cal).unwrap()),
        };
        Selection {
            kind: words[0].to_string(),
            rect,
            mask,
            outline,
            args,
        }
    }

    fn of(case: &Case, plane: &Plane) -> Selection {
        Selection::parse(case.about("roi").unwrap(), case, plane)
    }

    fn region(&self) -> Region<'_> {
        Region {
            rect: self.rect,
            mask: self.mask.as_deref(),
        }
    }

    /// True if ImageJ's values came from `List.setMeasurements`, which
    /// lists exactly the columns the measurement has.
    fn complete(&self) -> bool {
        !matches!(self.kind.as_str(), "line" | "point")
    }
}

/// Headings whose values are counts or sample values, which must match
/// exactly when ImageJ's value is a whole number.
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

/// Within 1e-5 relative of ImageJ's value. Values that are zero but for
/// rounding (the skewness of a symmetric region) cannot be held to a
/// relative error and get an absolute one.
fn close(ours: f64, theirs: f64) -> bool {
    if theirs.is_nan() {
        return ours.is_nan();
    }
    if theirs.is_infinite() {
        return ours == theirs;
    }
    (ours - theirs).abs() <= 1e-5 * theirs.abs() || (ours - theirs).abs() < 1e-9
}

fn matches_imagej(heading: &str, ours: f64, theirs: f64) -> bool {
    if EXACT.contains(&heading) && theirs.is_finite() && theirs.fract() == 0.0 {
        ours == theirs
    } else {
        close(ours, theirs)
    }
}

fn report(failures: &[String], compared: usize) {
    assert!(
        failures.is_empty(),
        "{} of {compared} values differ:\n{}",
        failures.len(),
        failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn measurements_match_imagej() {
    let cases = cases("measure_cases.txt");
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for case in &cases {
        let plane = Plane::of(case);
        let selection = Selection::of(case, &plane);
        let settings = MeasureSettings {
            measurements: Measurements::all(),
            invert_y: case.about("invert_y") == Some("1"),
            ..MeasureSettings::default()
        };
        let rows = measure(
            plane.measured(),
            selection.region(),
            &selection.outline,
            &settings,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert_eq!(rows.len(), 1, "{}", case.name);
        let row = &rows[0];
        *kinds.entry(selection.kind.clone()).or_default() += 1;
        for (heading, theirs) in &case.expected {
            let theirs = parse_java_double(theirs);
            let ours = match row.get(heading) {
                Some(Cell::Number(v)) => *v,
                // getValue gives NaN for a column the measurement lacks.
                None if !selection.complete() && theirs.is_nan() => continue,
                other => {
                    failures.push(format!(
                        "{} {heading}: {other:?}, ImageJ {theirs}",
                        case.name
                    ));
                    continue;
                }
            };
            compared += 1;
            if !matches_imagej(heading, ours, theirs) {
                failures.push(format!("{} {heading}: {ours}, ImageJ {theirs}", case.name));
            }
        }
        if selection.complete() {
            for (heading, _) in row.iter() {
                if !case.expected.iter().any(|(h, _)| h == heading) {
                    failures.push(format!("{} has {heading}, ImageJ does not", case.name));
                }
            }
        }
    }
    report(&failures, compared);
    // Every kind of selection is represented.
    for kind in [
        "none", "rect", "oval", "polygon", "freehand", "line", "point",
    ] {
        assert!(
            kinds.get(kind).copied().unwrap_or(0) >= 5,
            "{kind}: {kinds:?}"
        );
    }
    assert!(cases.len() > 250, "only {} cases", cases.len());
    assert!(compared > 9000, "only {compared} values compared");
}

/// The Set Measurements options of a macro call, such as
/// `area mean min display redirect=None decimal=3`.
fn settings_from(options: &str) -> MeasureSettings {
    let mut settings = MeasureSettings::default();
    let mut keywords = Vec::new();
    for word in options.split_whitespace() {
        match word {
            "display" => settings.display_label = true,
            "invert" => settings.invert_y = true,
            "scientific" => settings.scientific_notation = true,
            "nan" => settings.nan_empty_cells = true,
            "add" => settings.add_to_overlay = true,
            w if w.starts_with("redirect=") => {}
            w if w.starts_with("decimal=") => {
                settings.decimal_places = w["decimal=".len()..].parse().unwrap()
            }
            w => keywords.push(w),
        }
    }
    settings.measurements = Measurements::from_keywords(&keywords.join(" ")).unwrap();
    settings
}

/// For a point, ImageJ first writes the area and centroid of the pixel
/// under it and then replaces them with 0 and the point's position; the
/// replaced values still decide that those columns show decimals. Calipix
/// formats the values that are in the table.
fn point_columns_differ_only_in_decimals(ours: &str, theirs: &str) -> bool {
    let rows = |text: &str| -> Vec<Vec<String>> {
        text.lines()
            .map(|l| l.split(',').map(str::to_string).collect())
            .collect()
    };
    let (ours, theirs) = (rows(ours), rows(theirs));
    if ours.len() != theirs.len() || ours[0] != theirs[0] {
        return false;
    }
    ours.iter().zip(&theirs).skip(1).all(|(a, b)| {
        a.len() == b.len()
            && a.iter().zip(b).enumerate().all(|(i, (x, y))| {
                x == y
                    || (["Area", "X", "Y"].contains(&ours[0][i].as_str())
                        && x.parse::<f64>().ok() == y.parse::<f64>().ok())
            })
    })
}

#[test]
fn results_tables_match_imagej_byte_for_byte() {
    let index = fixture_text("results_cases.txt");
    let mut checked = 0;
    for line in index.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [file, image, position, roi, options] = fields[..] else {
            panic!("bad line: {line}");
        };
        let position = (position != "-").then(|| {
            let p: Vec<usize> = position.split(' ').map(|n| n.parse().unwrap()).collect();
            (p[0], p[1], p[2])
        });
        let plane = Plane::open(image, position);
        let selection = Selection::parse(roi, &Case::default(), &plane);
        let settings = settings_from(options);
        let rows = measure(
            plane.measured(),
            selection.region(),
            &selection.outline,
            &settings,
        )
        .unwrap();
        let mut table = Table::new();
        for row in &rows {
            append_row(&mut table, row);
        }
        let format = TableFormat::results(&table, &settings, TableFormat::delimiter_for(file));
        let ours = format_table(&table, &format);
        let theirs = fixture_text(file);
        if file == "results_point_all.csv" {
            assert_ne!(
                ours, theirs,
                "the known difference has gone; update the test"
            );
            assert!(
                point_columns_differ_only_in_decimals(&ours, &theirs),
                "{file}\nours:\n{ours}\nImageJ:\n{theirs}"
            );
        } else {
            assert_eq!(ours, theirs, "{file} ({options})");
        }
        checked += 1;
    }
    assert!(checked >= 20, "only {checked} tables");
}

/// The table `71_results_format.ijm` builds with `setResult`.
fn numbers_table(labels: bool) -> Table {
    let a = [
        0.0,
        1.0,
        -1.0,
        0.5,
        0.0004,
        0.0005,
        0.00049999,
        1e-7,
        123456789012.0,
        999999999999.0,
        1234567890123.0,
        1e13,
        2147483647.0,
        2147483648.0,
        -2147483648.0,
        -2147483649.0,
        2.5,
        3.5,
        0.125,
        0.375,
        1.0005,
        1.0015,
        -0.0005,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        1.5e300,
        12345.6789,
        -98765.4321,
        0.001,
        0.0009995,
        99999.9995,
    ];
    let mut table = Table::new();
    for (i, &v) in a.iter().enumerate() {
        let c = match i {
            3 => 2147483648.0,
            4 => -5000000000.0,
            _ => i as f64,
        };
        let s = match i {
            1 => "with,comma".to_string(),
            2 => String::new(),
            3 => "two\nlines".to_string(),
            4 => "tab\there".to_string(),
            5 => "say \"hi\"".to_string(),
            _ => format!("t{i}"),
        };
        table.push_row([
            ("A", Cell::from(v)),
            ("B", Cell::from(i as f64 * 37.0 - 100.0)),
            ("C", Cell::from(c)),
            ("S", Cell::from(s)),
        ]);
    }
    if labels {
        table.set(0, "Label", Cell::from("first"));
        table.set(2, "Label", Cell::from("a,b"));
    }
    table
}

#[test]
fn number_formatting_matches_imagej_byte_for_byte() {
    // Running Set Measurements without "Display label" removes the row
    // labels, so the tables saved after it have none.
    let with = |decimal_places: u8, scientific_notation: bool| MeasureSettings {
        decimal_places,
        scientific_notation,
        ..MeasureSettings::default()
    };
    for (file, labels, settings) in [
        ("results_numbers.csv", true, with(3, false)),
        ("results_numbers.txt", true, with(3, false)),
        ("results_numbers_sci5.csv", false, with(5, true)),
        ("results_numbers_dec0.csv", false, with(0, false)),
    ] {
        let table = numbers_table(labels);
        let format = TableFormat::results(&table, &settings, TableFormat::delimiter_for(file));
        assert_eq!(format_table(&table, &format), fixture_text(file), "{file}");
    }
}

#[test]
fn unfilled_cells_match_imagej_byte_for_byte() {
    // A measured row followed by rows that fill only some columns.
    let settings = settings_from("area mean min display scientific redirect=None decimal=4");
    let plane = Plane::open("gray8.tif", None);
    let whole = Outline::whole(plane.width, plane.height);
    let rows = measure(
        plane.measured(),
        Region::whole(plane.width, plane.height),
        &whole,
        &settings,
    )
    .unwrap();
    let mut table = Table::new();
    append_row(&mut table, &rows[0]);
    let mut row = Row::default();
    row.set("Foo", 0.5);
    row.set("Label", "a,b");
    append_row(&mut table, &row);
    let mut row = Row::default();
    row.set("Foo", 1234567890123.0);
    append_row(&mut table, &row);
    let format = TableFormat::results(&table, &settings, ',');
    assert_eq!(
        format_table(&table, &format),
        fixture_text("results_rows.csv")
    );

    // "NaN empty cells", and a text column that starts late and ends early.
    let settings = settings_from("area mean min nan redirect=None decimal=3");
    let mut table = Table::new();
    for v in [1.5, 2.0, 7.0] {
        table.push_row([("A", Cell::from(v))]);
    }
    table.set(2, "B", Cell::from(5.0));
    table.set(1, "S", Cell::from("text"));
    let format = TableFormat::results(&table, &settings, ',');
    assert_eq!(
        format_table(&table, &format),
        fixture_text("results_nan_cells.csv")
    );
}

#[test]
fn histograms_within_a_selection_match_imagej() {
    let cases = cases("analyze_cases.txt");
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut seen = 0;
    for case in cases.iter().filter(|c| c.name.starts_with("hist_")) {
        seen += 1;
        let plane = Plane::of(case);
        let selection = Selection::of(case, &plane);
        let bins = case.numbers("bins")[0] as usize;
        let range = case.about("range").map(|_| {
            let r = case.numbers("range");
            (r[0], r[1])
        });
        let of = |spec: HistogramSpec| {
            statistics(
                plane.samples(),
                plane.width,
                plane.height,
                selection.region(),
                &plane.calibration,
                Measurements::default(),
                spec,
            )
            .unwrap()
        };
        // getHistogram with the bins asked for; getStatistics always with
        // the default histogram.
        let binned = of(HistogramSpec { bins, range });
        let plain = of(HistogramSpec::default());

        let counts = case.list("counts");
        compared += counts.len();
        let ours: Vec<f64> = binned.histogram.iter().map(|&c| c as f64).collect();
        if ours != counts {
            failures.push(format!("{} counts: {ours:?}, ImageJ {counts:?}", case.name));
        }
        // The macro function spaces the bin starts by (max - min) / bins
        // for 16-bit and float images and by 1 otherwise.
        let values = case.list("values");
        let wide = plane.buffers.len() == 1 && !matches!(plane.buffers[0], Buffer::U8(_));
        let step = if wide {
            (binned.hist_max - binned.hist_min) / bins as f64
        } else {
            1.0
        };
        for (i, &theirs) in values.iter().enumerate() {
            compared += 1;
            let ours = binned.hist_min + i as f64 * step;
            if !close(ours, theirs) {
                failures.push(format!("{} value {i}: {ours}, ImageJ {theirs}", case.name));
            }
        }
        // The total of the counts is exact; the moments are within 1e-5.
        compared += 5;
        if binned.pixel_count as f64 != case.expected("count") {
            failures.push(format!("{} count: {}", case.name, binned.pixel_count));
        }
        for (key, ours) in [
            ("mean", plain.mean),
            ("min", plain.min),
            ("max", plain.max),
            ("std", plain.std_dev),
        ] {
            let theirs = case.expected(key);
            let exact = matches!(key, "min" | "max") && theirs.fract() == 0.0;
            let same = if exact {
                ours == theirs
            } else {
                close(ours, theirs)
            };
            if !same {
                failures.push(format!("{} {key}: {ours}, ImageJ {theirs}", case.name));
            }
        }
    }
    report(&failures, compared);
    assert!(seen >= 16, "only {seen} histograms");
}

#[test]
fn profiles_match_imagej() {
    let cases = cases("analyze_cases.txt");
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut seen = 0;
    for case in cases.iter().filter(|c| c.name.starts_with("profile_")) {
        seen += 1;
        let plane = Plane::of(case);
        let selection = Selection::of(case, &plane);
        let (s, w, h) = (plane.samples(), plane.width, plane.height);
        let a = &selection.args;
        let ours = match selection.kind.as_str() {
            "rect" if case.about("vertical") == Some("1") => {
                row_average_profile(s, w, h, selection.rect)
            }
            "rect" => column_average_profile(s, w, h, selection.rect),
            "line" => line_profile(s, w, h, (a[0], a[1]), (a[2], a[3]), true),
            other => panic!("{}: no profile of {other}", case.name),
        }
        .unwrap();
        let theirs = case.list("profile");
        if ours.len() != theirs.len() {
            failures.push(format!(
                "{}: {} points, ImageJ {}",
                case.name,
                ours.len(),
                theirs.len()
            ));
            continue;
        }
        for (i, (&o, &t)) in ours.iter().zip(&theirs).enumerate() {
            compared += 1;
            if !close(o, t) {
                failures.push(format!("{} point {i}: {o}, ImageJ {t}", case.name));
            }
        }
    }
    report(&failures, compared);
    assert!(seen >= 32, "only {seen} profiles");
}
