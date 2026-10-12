//! From statistics to rows of the Results table, as
//! `ij.plugin.filter.Analyzer` does it.

use calipix_core::{Cell, Error, Result, Table};

use super::outline::{Outline, OutlineKind};
use super::profile::selection_profile;
use super::stats::{Calibration, HistogramSpec, Region, Samples, Statistics, statistics};
use calipix_core::{Buffer, Rect};

/// The quantities Analyze > Set Measurements offers, in the order of its
/// checkboxes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measurements {
    pub area: bool,
    pub mean: bool,
    pub std_dev: bool,
    pub mode: bool,
    pub min_max: bool,
    pub centroid: bool,
    pub center_of_mass: bool,
    pub perimeter: bool,
    pub bounding_rect: bool,
    pub fit_ellipse: bool,
    pub shape_descriptors: bool,
    pub feret: bool,
    pub integrated_density: bool,
    pub median: bool,
    pub skewness: bool,
    pub kurtosis: bool,
    pub area_fraction: bool,
    pub stack_position: bool,
}

/// One checkbox of Set Measurements: its ImageJ macro keyword and where it
/// lives in [`Measurements`].
type Flag = (&'static str, fn(&mut Measurements) -> &mut bool);

impl Measurements {
    /// ImageJ's macro keyword for each measurement (the first word of its
    /// checkbox label), in dialog order.
    pub const FLAGS: [Flag; 18] = [
        ("area", |m| &mut m.area),
        ("mean", |m| &mut m.mean),
        ("standard", |m| &mut m.std_dev),
        ("modal", |m| &mut m.mode),
        ("min", |m| &mut m.min_max),
        ("centroid", |m| &mut m.centroid),
        ("center", |m| &mut m.center_of_mass),
        ("perimeter", |m| &mut m.perimeter),
        ("bounding", |m| &mut m.bounding_rect),
        ("fit", |m| &mut m.fit_ellipse),
        ("shape", |m| &mut m.shape_descriptors),
        ("feret's", |m| &mut m.feret),
        ("integrated", |m| &mut m.integrated_density),
        ("median", |m| &mut m.median),
        ("skewness", |m| &mut m.skewness),
        ("kurtosis", |m| &mut m.kurtosis),
        ("area_fraction", |m| &mut m.area_fraction),
        ("stack", |m| &mut m.stack_position),
    ];

    /// ImageJ's setting on a fresh installation: area, mean, minimum and
    /// maximum.
    pub fn imagej_default() -> Measurements {
        Measurements {
            area: true,
            mean: true,
            min_max: true,
            ..Measurements::default()
        }
    }

    /// Every measurement.
    pub fn all() -> Measurements {
        let mut all = Measurements::default();
        for (_, flag) in Measurements::FLAGS {
            *flag(&mut all) = true;
        }
        all
    }

    /// Reads a list of ImageJ keywords separated by spaces or commas, as in
    /// `run("Set Measurements...", "area mean min")`. `circularity` and
    /// `slice` are accepted for `shape` and `stack`, as ImageJ does, and
    /// `feret` for `feret's`; `all` turns everything on.
    pub fn from_keywords(text: &str) -> Result<Measurements> {
        let mut m = Measurements::default();
        for word in text
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|w| !w.is_empty())
        {
            let word = match word.to_ascii_lowercase().as_str() {
                "all" => {
                    m = Measurements::all();
                    continue;
                }
                "circularity" => "shape".to_string(),
                "slice" => "stack".to_string(),
                "feret" | "ferets" => "feret's".to_string(),
                other => other.to_string(),
            };
            let Some((_, flag)) = Measurements::FLAGS.iter().find(|(key, _)| *key == word) else {
                let known: Vec<&str> = Measurements::FLAGS.iter().map(|(k, _)| *k).collect();
                return Err(Error::Other(format!(
                    "`{word}` is not a measurement; choose from: {}",
                    known.join(", ")
                )));
            };
            *flag(&mut m) = true;
        }
        Ok(m)
    }

    /// The keywords of the measurements that are on, separated by spaces.
    pub fn keywords(&self) -> String {
        let mut copy = *self;
        let on: Vec<&str> = Measurements::FLAGS
            .iter()
            .filter(|(_, flag)| *flag(&mut copy))
            .map(|(key, _)| *key)
            .collect();
        on.join(" ")
    }
}

/// Everything Analyze > Set Measurements decides.
#[derive(Clone, Debug, PartialEq)]
pub struct MeasureSettings {
    pub measurements: Measurements,
    /// "Display label": start each row with the image name.
    pub display_label: bool,
    /// "Invert Y coordinates": Y counts up from the bottom of the image.
    pub invert_y: bool,
    /// "Scientific notation" in the table text.
    pub scientific_notation: bool,
    /// "NaN empty cells": cells never filled read NaN rather than 0.
    pub nan_empty_cells: bool,
    /// "Add to overlay": keep each measured selection on the image.
    pub add_to_overlay: bool,
    /// "Decimal places", 0 to 9.
    pub decimal_places: u8,
}

impl Default for MeasureSettings {
    /// ImageJ's settings on a fresh installation.
    fn default() -> MeasureSettings {
        MeasureSettings {
            measurements: Measurements::imagej_default(),
            display_label: false,
            invert_y: false,
            scientific_notation: false,
            nan_empty_cells: false,
            add_to_overlay: false,
            decimal_places: 3,
        }
    }
}

/// The headings of ImageJ's Results table in its fixed column order
/// (`ResultsTable.defaultHeadings`). Any other column comes after these,
/// in the order it was first used.
pub const DEFAULT_HEADINGS: [&str; 38] = [
    "Area",
    "Mean",
    "StdDev",
    "Mode",
    "Min",
    "Max",
    "X",
    "Y",
    "XM",
    "YM",
    "Perim.",
    "BX",
    "BY",
    "Width",
    "Height",
    "Major",
    "Minor",
    "Angle",
    "Circ.",
    "Feret",
    "IntDen",
    "Median",
    "Skew",
    "Kurt",
    "%Area",
    "RawIntDen",
    "Ch",
    "Slice",
    "Frame",
    "FeretX",
    "FeretY",
    "FeretAngle",
    "MinFeret",
    "AR",
    "Round",
    "Solidity",
    "MinThr",
    "MaxThr",
];

/// Heading of the column of row labels.
pub const LABEL: &str = "Label";

/// Where the measured plane sits in its image, for the Label, Ch, Slice
/// and Frame columns.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneContext {
    /// Text for the Label column (`Analyzer.getFileName`).
    pub label: String,
    /// Channel, slice and frame of the plane, from 1.
    pub channel: usize,
    pub slice: usize,
    pub frame: usize,
    /// Number of channels, slices and frames of the image.
    pub channels: usize,
    pub slices: usize,
    pub frames: usize,
    /// ImageJ's `isHyperStack() || isComposite()`: report Ch, Slice and
    /// Frame separately rather than one stack index.
    pub hyperstack: bool,
    /// The plane's number in ImageJ's stack order, from 1.
    pub stack_index: usize,
}

impl PlaneContext {
    /// A single plane called `label`.
    pub fn single(label: impl Into<String>) -> PlaneContext {
        PlaneContext {
            label: label.into(),
            channel: 1,
            slice: 1,
            frame: 1,
            channels: 1,
            slices: 1,
            frames: 1,
            hyperstack: false,
            stack_index: 1,
        }
    }
}

/// One row of results: heading and value in the order ImageJ writes them.
/// A heading written twice keeps its first place and its last value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row(Vec<(String, Cell)>);

impl Row {
    pub fn set(&mut self, heading: &str, value: impl Into<Cell>) {
        let value = value.into();
        match self.0.iter_mut().find(|(h, _)| h == heading) {
            Some((_, cell)) => *cell = value,
            None => self.0.push((heading.to_string(), value)),
        }
    }

    pub fn get(&self, heading: &str) -> Option<&Cell> {
        self.0.iter().find(|(h, _)| h == heading).map(|(_, c)| c)
    }

    pub fn number(&self, heading: &str) -> Option<f64> {
        self.get(heading).and_then(Cell::as_number)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Cell)> {
        self.0.iter().map(|(h, c)| (h.as_str(), c))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `Analyzer.saveResults`: the row for `stats` of the selection described
/// by `outline`, with the columns `settings` asks for.
pub fn results_row(
    stats: &Statistics,
    outline: &Outline,
    cal: &Calibration,
    image_height: usize,
    context: &PlaneContext,
    settings: &MeasureSettings,
) -> Row {
    let m = settings.measurements;
    let mut row = Row::default();
    if settings.display_label {
        row.set(LABEL, context.label.as_str());
    }
    if m.area {
        row.set("Area", stats.area);
    }
    if m.mean {
        row.set("Mean", stats.mean);
    }
    if m.std_dev {
        row.set("StdDev", stats.std_dev);
    }
    if m.mode {
        row.set("Mode", stats.mode);
    }
    if m.min_max {
        row.set("Min", stats.min);
        row.set("Max", stats.max);
    }
    if m.centroid {
        row.set("X", stats.x_centroid);
        row.set("Y", stats.y_centroid);
    }
    if m.center_of_mass {
        row.set("XM", stats.x_center_of_mass);
        row.set("YM", stats.y_center_of_mass);
    }
    if m.perimeter || m.shape_descriptors {
        let perimeter = outline.perimeter;
        if m.perimeter {
            row.set("Perim.", perimeter);
        }
        if m.shape_descriptors {
            let circularity = if perimeter == 0.0 {
                0.0
            } else {
                (4.0 * std::f64::consts::PI * (stats.area / (perimeter * perimeter))).min(1.0)
            };
            let area = outline.is_area();
            let pixels = stats.pixel_count as f64;
            let convex = outline.convex_area.unwrap_or(pixels);
            row.set("Circ.", circularity);
            row.set("AR", if area { stats.major / stats.minor } else { 0.0 });
            row.set(
                "Round",
                if area {
                    4.0 * stats.area / (std::f64::consts::PI * stats.major * stats.major)
                } else {
                    0.0
                },
            );
            row.set("Solidity", if area { pixels / convex } else { f64::NAN });
        }
    }
    if m.bounding_rect {
        if outline.is_line() {
            let (x, y, w, h) = outline.bounds;
            row.set("BX", cal.x(x));
            row.set("BY", cal.y(y, image_height));
            row.set("Width", w * cal.pixel_width);
            row.set("Height", h * cal.pixel_height);
        } else {
            row.set("BX", stats.roi_x);
            row.set("BY", stats.roi_y);
            row.set("Width", stats.roi_width);
            row.set("Height", stats.roi_height);
        }
    }
    if m.fit_ellipse {
        row.set("Major", stats.major);
        row.set("Minor", stats.minor);
        row.set("Angle", stats.angle);
    }
    if m.feret {
        let f = outline.feret;
        row.set("Feret", f.diameter);
        row.set("FeretX", f.x);
        row.set("FeretY", f.y);
        row.set("FeretAngle", f.angle);
        row.set("MinFeret", f.min);
    }
    if m.integrated_density {
        row.set("IntDen", stats.area * stats.mean);
        row.set("RawIntDen", stats.pixel_count as f64 * stats.raw_mean);
    }
    if m.median {
        row.set("Median", stats.median);
    }
    if m.skewness {
        row.set("Skew", stats.skewness);
    }
    if m.kurtosis {
        row.set("Kurt", stats.kurtosis);
    }
    if m.area_fraction {
        row.set("%Area", stats.area_fraction);
    }
    if m.stack_position {
        stack_position(&mut row, context);
    }
    match &outline.kind {
        // Prefs.reflexAngle is off on a fresh installation.
        OutlineKind::Angle(degrees) => row.set("Angle", *degrees),
        OutlineKind::Line => {
            row.set("Length", outline.perimeter);
            if let Some(angle) = outline.line_angle(cal) {
                row.set("Angle", angle);
            }
        }
        OutlineKind::Points(points) => {
            // Analyzer.savePoints
            if m.area {
                row.set("Area", 0.0);
            }
            if let Some(&(x, y)) = points.first() {
                row.set("X", cal.x(x));
                row.set("Y", cal.y(y, image_height));
            }
            if context.hyperstack {
                stack_position(&mut row, context);
            } else if context.channels * context.slices * context.frames > 1 {
                row.set("Slice", context.stack_index as f64);
            }
        }
        OutlineKind::Whole | OutlineKind::Area => {}
    }
    if let Some((length, width)) = outline.rotated_rectangle(cal) {
        row.set("RRLength", length);
        row.set("RRWidth", width);
    }
    row
}

fn stack_position(row: &mut Row, context: &PlaneContext) {
    if context.hyperstack {
        if context.channels > 1 {
            row.set("Ch", context.channel as f64);
        }
        if context.slices > 1 {
            row.set("Slice", context.slice as f64);
        }
        if context.frames > 1 {
            row.set("Frame", context.frame as f64);
        }
    } else {
        row.set("Slice", context.stack_index as f64);
    }
}

/// What to measure: a plane, its calibration and where it sits.
#[derive(Clone, Copy, Debug)]
pub struct Measured<'a> {
    pub samples: Samples<'a>,
    pub width: usize,
    pub height: usize,
    pub calibration: &'a Calibration,
    pub context: &'a PlaneContext,
}

/// Analyze > Measure (`Analyzer.measure`): the rows for one plane and
/// selection.
///
/// * An area or no selection gives one row of the statistics of the
///   samples inside it.
/// * A line gives one row (`Analyzer.measureLength`): its length, and the
///   statistics of the profile along it or, for a line wider than one
///   pixel, of the area it covers (`Roi.convertLineToArea`).
/// * An angle gives one row with the angle and no statistics
///   (`Analyzer.measureAngle`).
/// * Points give a row each (`Analyzer.measurePoint`).
///
/// `region` is the part of the plane an area selection covers; it is not
/// used for lines, angles and points.
pub fn measure(
    image: Measured<'_>,
    region: Region<'_>,
    outline: &Outline,
    settings: &MeasureSettings,
) -> Result<Vec<Row>> {
    let cal = Calibration {
        invert_y: settings.invert_y || image.calibration.invert_y,
        ..image.calibration.clone()
    };
    let m = settings.measurements;
    let (width, height) = (image.width, image.height);
    let row = |stats: &Statistics, outline: &Outline| {
        results_row(stats, outline, &cal, height, image.context, settings)
    };
    match &outline.kind {
        OutlineKind::Whole | OutlineKind::Area => {
            let stats = statistics(
                image.samples,
                width,
                height,
                region,
                &cal,
                m,
                HistogramSpec::default(),
            )?;
            Ok(vec![row(&stats, outline)])
        }
        // new ImageStatistics(): nothing is measured but the outline.
        OutlineKind::Angle(_) => Ok(vec![row(&Statistics::zeroed(), outline)]),
        OutlineKind::Line => {
            let basic =
                m.area || m.mean || m.std_dev || m.mode || m.min_max || m.centroid || m.median;
            if !basic {
                let mut row = Row::default();
                row.set("Length", outline.perimeter);
                if let Some(angle) = outline.line_angle(&cal) {
                    row.set("Angle", angle);
                }
                if settings.display_label {
                    row.set(LABEL, image.context.label.as_str());
                }
                return Ok(vec![row]);
            }
            let roi = outline
                .roi
                .as_ref()
                .expect("a line outline keeps its selection");
            let of_line = Measurements {
                area: true,
                mean: true,
                std_dev: true,
                mode: true,
                min_max: true,
                median: true,
                ..Measurements::default()
            };
            let full = if outline.line_width() > 1 {
                // The samples the line covers: ip.setRoi(convertLineToArea).
                let mask = roi
                    .to_area()
                    .and_then(|area| area.mask(width, height))
                    .ok_or_else(|| {
                        Error::OutOfBounds(
                            "the line selection covers no pixel of the image".to_string(),
                        )
                    })?;
                statistics(
                    image.samples,
                    width,
                    height,
                    Region::masked(&mask),
                    &cal,
                    of_line,
                    HistogramSpec::default(),
                )?
            } else {
                // The statistics of the profile along the line, taken as a
                // one-row float image with the image's calibration.
                let profile = selection_profile(image.samples, width, height, roi, true)?;
                let n = profile.len();
                if n == 0 {
                    return Err(Error::OutOfBounds(
                        "the line selection is too short to have a profile".to_string(),
                    ));
                }
                let plane =
                    Buffer::from_vec(profile.into_iter().map(|v| v as f32).collect::<Vec<f32>>());
                statistics(
                    Samples::Gray(&plane),
                    n,
                    1,
                    Region::whole(n, 1),
                    &cal,
                    of_line,
                    HistogramSpec::default(),
                )?
            };
            // ImageJ asks only for the basic statistics here; everything
            // else stays at zero.
            let mut stats = Statistics::zeroed();
            stats.pixel_count = full.pixel_count;
            stats.area = full.area;
            stats.mean = full.mean;
            stats.raw_mean = full.raw_mean;
            stats.std_dev = full.std_dev;
            stats.mode = full.mode;
            stats.min = full.min;
            stats.max = full.max;
            stats.median = full.median;
            stats.roi_x = full.roi_x;
            stats.roi_y = full.roi_y;
            stats.roi_width = full.roi_width;
            stats.roi_height = full.roi_height;
            if let (true, Some(((x1, y1), (x2, y2)))) = (m.centroid, outline.straight_line()) {
                // Line.getFloatPoints: the end points in single precision.
                let (x1, y1, x2, y2) = (
                    f64::from(x1 as f32),
                    f64::from(y1 as f32),
                    f64::from(x2 as f32),
                    f64::from(y2 as f32),
                );
                stats.x_centroid = cal.x(x1 + (x2 - x1) / 2.0);
                stats.y_centroid = cal.y(y1 + (y2 - y1) / 2.0, height);
            }
            Ok(vec![row(&stats, outline)])
        }
        OutlineKind::Points(points) => {
            // Analyzer.measurePoint: each point is the pixel it rounds to.
            let mut rows = Vec::with_capacity(points.len());
            for &(x, y) in points {
                let (px, py) = ((x + 0.5).floor(), (y + 0.5).floor());
                let inside = px >= 0.0 && py >= 0.0 && px < width as f64 && py < height as f64;
                // A point outside the image leaves the processor with an
                // empty region (ImageProcessor.setRoi), which is measured
                // all the same.
                let rect = if inside {
                    Rect::new(px as usize, py as usize, 1, 1)
                } else {
                    Rect::new(0, 0, 0, 0)
                };
                let mut stats = statistics(
                    image.samples,
                    width,
                    height,
                    Region::rect(rect),
                    &cal,
                    m,
                    HistogramSpec::default(),
                )?;
                stats.x_center_of_mass = x;
                stats.y_center_of_mass = y;
                rows.push(row(&stats, &Outline::point(x, y)));
            }
            Ok(rows)
        }
    }
}

/// Where `heading` sorts among the columns of a Results table: after the
/// label, by ImageJ's fixed order, with other headings last.
fn rank(heading: &str) -> usize {
    if heading == LABEL {
        return 0;
    }
    DEFAULT_HEADINGS
        .iter()
        .position(|h| *h == heading)
        .map_or(usize::MAX, |i| i + 1)
}

/// Adds `row` to `table`, keeping the columns in ImageJ's order: Label,
/// then the standard headings in their fixed order, then any others in
/// order of first use. Cells of earlier rows in a new column stay empty.
pub fn append_row(table: &mut Table, row: &Row) {
    let mut headings: Vec<String> = table.column_names().map(str::to_string).collect();
    for (h, _) in row.iter() {
        if !headings.iter().any(|x| x == h) {
            headings.push(h.to_string());
        }
    }
    let mut ordered = headings.clone();
    ordered.sort_by_key(|h| rank(h)); // stable: other headings keep their order
    if ordered == headings {
        // Any new columns belong on the right, which is where they go.
        table.push_row(row.iter().map(|(h, c)| (h, c.clone())));
        return;
    }
    let mut rebuilt = Table::new();
    for r in 0..table.len() {
        rebuilt.push_row(
            ordered
                .iter()
                .map(|h| (h.as_str(), table.get(r, h).clone())),
        );
    }
    rebuilt.push_row(ordered.iter().map(|h| {
        let cell = row.get(h).cloned().unwrap_or(Cell::Empty);
        (h.as_str(), cell)
    }));
    *table = rebuilt;
}

/// Analyze > Summarize (`Analyzer.summarize`): adds four rows labelled
/// Mean, SD, Min and Max with those statistics of every numeric column.
/// Does nothing to a table with fewer than two rows or one that is already
/// summarized. Empty cells count as 0, as in ImageJ.
pub fn summarize(table: &mut Table) -> bool {
    let n = table.len();
    if n < 2 || is_summarized(table) {
        return false;
    }
    let headings: Vec<String> = table
        .column_names()
        .filter(|h| *h != LABEL)
        .map(str::to_string)
        .collect();
    let mut summary: Vec<[f64; 4]> = Vec::with_capacity(headings.len());
    for heading in &headings {
        let (mut min, mut max, mut sum, mut sum2) = (f64::MAX, -f64::MAX, 0.0, 0.0);
        for row in 0..n {
            let v = match table.get(row, heading) {
                Cell::Number(v) => *v,
                Cell::Empty => 0.0,
                Cell::Text(_) => f64::NAN,
            };
            if v < min {
                min = v;
            }
            if v > max {
                max = v;
            }
            sum += v;
            sum2 += v * v;
        }
        let nf = n as f64;
        summary.push([
            sum / nf,
            ((sum2 - sum * sum / nf) / (nf - 1.0)).sqrt(),
            min,
            max,
        ]);
    }
    for (i, label) in SUMMARY_LABELS.iter().enumerate() {
        let mut row = Row::default();
        row.set(LABEL, *label);
        for (heading, values) in headings.iter().zip(&summary) {
            row.set(heading, values[i]);
        }
        append_row(table, &row);
    }
    true
}

pub(super) const SUMMARY_LABELS: [&str; 4] = ["Mean", "SD", "Min", "Max"];

/// True if the table ends with the rows [`summarize`] adds.
pub fn is_summarized(table: &Table) -> bool {
    let n = table.len();
    n >= 4 && table.get(n - 1, LABEL).as_text() == Some("Max")
}

/// `ImageStack.getShortSliceLabel`: a slice label cut at its first line
/// break and at 60 characters, without a three-letter file extension.
/// `None` if there is nothing to show.
pub fn short_slice_label(label: &str) -> Option<String> {
    let line = match label.find('\n') {
        Some(0) => return None,
        Some(end) => &label[..end],
        None => label,
    };
    let mut chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    if n > 4 && chars[n - 4] == '.' && !chars[n - 1].is_ascii_digit() {
        chars.truncate(n - 4);
    }
    chars.truncate(60);
    Some(chars.into_iter().collect())
}

/// The text of the Label column (`Analyzer.getFileName`): the image name,
/// then for an image of several planes a colon and the plane's label, or
/// its number in the stack if it has none.
pub fn row_label(
    image_name: &str,
    planes: usize,
    stack_index: usize,
    slice_label: Option<&str>,
) -> String {
    let mut label = image_name.to_string();
    if planes > 1 {
        if !label.is_empty() {
            label.push(':');
        }
        match slice_label
            .and_then(short_slice_label)
            .filter(|l| !l.is_empty())
        {
            Some(slice) => label.push_str(&slice),
            None => label.push_str(&stack_index.to_string()),
        }
    }
    label
}

/// The table without its column of row labels, which is what ImageJ's Set
/// Measurements leaves when "Display label" is off
/// (`ResultsTable.disableRowLabels`).
pub fn without_labels(table: &Table) -> Table {
    if !table.has_column(LABEL) {
        return table.clone();
    }
    let headings: Vec<&str> = table.column_names().filter(|h| *h != LABEL).collect();
    let mut out = Table::new();
    for row in 0..table.len() {
        out.push_row(headings.iter().map(|h| (*h, table.get(row, h).clone())));
    }
    out
}

/// Removes the rows [`summarize`] added, as ImageJ does before adding
/// another measurement (`Analyzer.clearSummary`). Returns whether there
/// were any.
pub fn clear_summary(table: &mut Table) -> bool {
    if !is_summarized(table) {
        return false;
    }
    let headings: Vec<String> = table.column_names().map(str::to_string).collect();
    let mut kept = Table::new();
    for row in 0..table.len() - SUMMARY_LABELS.len() {
        kept.push_row(
            headings
                .iter()
                .map(|h| (h.as_str(), table.get(row, h).clone())),
        );
    }
    *table = kept;
    true
}
