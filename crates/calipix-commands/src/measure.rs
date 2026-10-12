//! Measurement commands (Analyze) and the results table.
//!
//! # Where measurement settings live
//!
//! What Measure measures is chosen with Set Measurements, as in ImageJ.
//! ImageJ keeps that choice in a global preference; here it is part of the
//! session: Set Measurements writes it to a one-row table in the
//! workspace, [`SETTINGS_TABLE`], whose columns are ImageJ's macro
//! keywords. That makes the choice
//!
//! * per session, and absent from any file on the user's machine, so a
//!   recorded workflow replays the same anywhere;
//! * visible in History, as the Set Measurements step with every option
//!   spelled out, and undone and redone with it like any other table;
//! * usable headlessly: with no such table ImageJ's defaults apply (area,
//!   mean, minimum and maximum, three decimal places), and Measure can be
//!   given `measurements` to choose for one run without it.

use std::path::PathBuf;

use calipix_command::{
    CommandCtx, CommandSpec, DatasetId, Effects, Error, ImageEffect, Outputs, ParamType, Params,
    Registry, Result, Workspace, typed,
};
use calipix_core::{AxisKind, Buffer, Cell, ColorModel, Image, ImageMeta, Table};
use calipix_ops::measure::{
    self, Calibration, HistogramSpec, MeasureSettings, Measured, Measurements, Outline,
    OutlineKind, PlaneContext, Region, Samples, TableFormat,
};
use calipix_render::{HistogramOptions, stack_histogram};
use calipix_roi::Roi;

use crate::view::plane_position;

/// The workspace table that holds the session's measurement settings: one
/// row, one column per ImageJ keyword (`area`, `mean`, ..., `decimal`),
/// with 1 for a chosen option.
pub const SETTINGS_TABLE: &str = "Measurement Settings";

/// Keywords of the Set Measurements options that are not measurements.
const DISPLAY: &str = "display";
const INVERT: &str = "invert";
const SCIENTIFIC: &str = "scientific";
const ADD: &str = "add";
const NAN: &str = "nan";
const DECIMAL: &str = "decimal";

/// The session's measurement settings: those last chosen with Set
/// Measurements, or ImageJ's defaults.
pub fn measure_settings(workspace: &Workspace) -> MeasureSettings {
    let mut settings = MeasureSettings::default();
    let Some(table) = workspace.table(SETTINGS_TABLE).filter(|t| !t.is_empty()) else {
        return settings;
    };
    let on = |key: &str, default: bool| match table.get(0, key) {
        Cell::Number(v) => *v != 0.0,
        _ => default,
    };
    for (key, flag) in Measurements::FLAGS {
        let default = *flag(&mut settings.measurements);
        *flag(&mut settings.measurements) = on(key, default);
    }
    settings.display_label = on(DISPLAY, settings.display_label);
    settings.invert_y = on(INVERT, settings.invert_y);
    settings.scientific_notation = on(SCIENTIFIC, settings.scientific_notation);
    settings.add_to_overlay = on(ADD, settings.add_to_overlay);
    settings.nan_empty_cells = on(NAN, settings.nan_empty_cells);
    if let Cell::Number(v) = table.get(0, DECIMAL) {
        settings.decimal_places = v.clamp(0.0, 9.0) as u8;
    }
    settings
}

fn settings_table(settings: &MeasureSettings) -> Table {
    let mut measurements = settings.measurements;
    let flag = |on: bool| Cell::from(if on { 1.0 } else { 0.0 });
    let mut cells: Vec<(&str, Cell)> = Measurements::FLAGS
        .iter()
        .map(|(key, at)| (*key, flag(*at(&mut measurements))))
        .collect();
    cells.extend([
        (DISPLAY, flag(settings.display_label)),
        (INVERT, flag(settings.invert_y)),
        (SCIENTIFIC, flag(settings.scientific_notation)),
        (ADD, flag(settings.add_to_overlay)),
        (NAN, flag(settings.nan_empty_cells)),
        (DECIMAL, Cell::from(settings.decimal_places as f64)),
    ]);
    let mut table = Table::new();
    table.push_row(cells);
    table
}

/// The table called `name` as ImageJ would save it under the session's
/// settings, with `delimiter` between cells; `None` if there is no such
/// table.
pub fn results_text(workspace: &Workspace, name: &str, delimiter: char) -> Option<String> {
    let table = workspace.table(name)?;
    let format = TableFormat::results(table, &measure_settings(workspace), delimiter);
    Some(measure::format_table(table, &format))
}

/// What one processor of ImageJ would hold for a plane of an image: the
/// plane itself, or the three channels of an RGB image.
struct PlaneData {
    buffers: Vec<Buffer>,
    width: usize,
    height: usize,
    calibration: Calibration,
    context: PlaneContext,
}

impl PlaneData {
    /// Reads the plane at `channel` (from 1; 0, which stands for every
    /// channel shown together, takes the first), `slice` and `frame`.
    fn read(image: &Image, channel: i64, slice: i64, frame: i64) -> Result<PlaneData> {
        let meta = image.meta();
        if meta.dtype().is_complex() {
            return Err(Error::failed(
                "complex images cannot be measured; take the magnitude or a component first",
            ));
        }
        let rgb = meta.color == ColorModel::Rgb;
        let channels = meta.size(&AxisKind::Channel);
        let channel = if rgb { 1 } else { channel.max(1) };
        if channel as u64 > channels as u64 {
            return Err(Error::InvalidArg {
                param: "channel".to_string(),
                reason: format!("{channel} is outside 1 to {channels}"),
            });
        }
        let channel = channel as usize;
        let read = |c: usize| -> Result<Buffer> {
            let position = plane_position(meta, c, slice, frame)?;
            Ok(image.read_plane(meta.plane_index(&position)?)?)
        };
        let buffers = if rgb {
            vec![read(0)?, read(1)?, read(2)?]
        } else {
            vec![read(channel - 1)?]
        };

        // ImageJ sees an RGB image as one channel.
        let channels = if rgb { 1 } else { channels };
        let (slices, frames) = (meta.size(&AxisKind::Z), meta.size(&AxisKind::Time));
        // An image without slices or frames has only position 1 there.
        let slice = if slices == 1 { 1 } else { slice as usize };
        let frame = if frames == 1 { 1 } else { frame as usize };
        let planes = channels * slices * frames;
        let stack_index = ((frame - 1) * slices + (slice - 1)) * channels + channel;
        let slice_label = meta
            .properties
            .get(&format!("imagej.label.{}", stack_index - 1));
        Ok(PlaneData {
            buffers,
            width: meta.width(),
            height: meta.height(),
            calibration: Calibration {
                // ImageJ's per-image "Invert Y coordinates" (Image > Properties),
                // kept by the TIFF reader.
                invert_y: meta
                    .properties
                    .get("imagej.inverty")
                    .is_some_and(|v| v == "true"),
                ..Calibration::from_meta(meta)
            },
            context: PlaneContext {
                label: measure::row_label(
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
                // ImageJ's isHyperStack() || isComposite().
                hyperstack: channels > 1 || (slices > 1 && frames > 1),
                stack_index,
            },
        })
    }

    fn samples(&self) -> Samples<'_> {
        match self.buffers.as_slice() {
            [red, green, blue] => Samples::Rgb { red, green, blue },
            other => Samples::Gray(&other[0]),
        }
    }
}

/// The selection of an image as measurement code takes it: the pixels an
/// area covers and the outline's quantities.
struct Selected {
    mask: Option<calipix_roi::Mask>,
    rectangular: bool,
    outline: Outline,
}

impl Selected {
    fn of(ctx: &CommandCtx<'_>, input: DatasetId, plane: &PlaneData) -> Result<Selected> {
        let (width, height) = (plane.width, plane.height);
        let Some(roi) = &ctx.workspace().get(input)?.roi else {
            return Ok(Selected {
                mask: None,
                rectangular: true,
                outline: Outline::whole(width, height),
            });
        };
        let outline = measure::outline_of(roi, &plane.calibration).map_err(Error::failed)?;
        let mask = if roi.is_area() {
            Some(crate::process::selection_mask(roi, width, height)?)
        } else {
            None
        };
        Ok(Selected {
            mask,
            rectangular: roi.is_rectangular(),
            outline,
        })
    }

    fn region(&self, plane: &PlaneData) -> Region<'_> {
        match &self.mask {
            None => Region::whole(plane.width, plane.height),
            Some(mask) if self.rectangular => Region::rect(mask.rect),
            Some(mask) => Region::masked(mask),
        }
    }
}

/// The parameters that say which plane a command reads, filled from the
/// view by interfaces.
#[derive(Params, Clone, Debug)]
struct PlaneOf {
    /// The image to measure.
    #[param(label = "Image")]
    input: DatasetId,
    /// Channel to measure, counted from 1. 0, which interfaces give when
    /// every channel is shown together, measures the first.
    #[param(default = 0, min = 0, advanced, no_persist, position = "channel")]
    channel: i64,
    /// Slice to measure, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "slice")]
    slice: i64,
    /// Frame to measure, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "frame")]
    frame: i64,
}

impl PlaneOf {
    fn read(&self, ctx: &CommandCtx<'_>) -> Result<PlaneData> {
        PlaneData::read(ctx.image(self.input)?, self.channel, self.slice, self.frame)
    }
}

#[derive(Params)]
struct MeasureParams {
    #[param(flatten)]
    plane: PlaneOf,
    /// Measurements to make this once instead of those chosen with Set
    /// Measurements, as ImageJ's keywords separated by spaces, for example
    /// `area mean centroid`; `all` for every one.
    #[param(label = "Measurements", advanced, no_persist)]
    measurements: Option<String>,
    /// The table the row is added to.
    #[param(label = "Table", default = "Results", advanced, no_persist)]
    table: String,
}

/// Results-table headings and the names of the outputs that carry them.
const OUTPUTS: [(&str, &str); 37] = [
    ("Area", "area"),
    ("Mean", "mean"),
    ("StdDev", "std_dev"),
    ("Mode", "mode"),
    ("Min", "min"),
    ("Max", "max"),
    ("X", "x"),
    ("Y", "y"),
    ("XM", "xm"),
    ("YM", "ym"),
    ("Perim.", "perimeter"),
    ("BX", "bx"),
    ("BY", "by"),
    ("Width", "width"),
    ("Height", "height"),
    ("Major", "major"),
    ("Minor", "minor"),
    ("Angle", "angle"),
    ("Circ.", "circularity"),
    ("Feret", "feret"),
    ("IntDen", "int_den"),
    ("Median", "median"),
    ("Skew", "skewness"),
    ("Kurt", "kurtosis"),
    ("%Area", "area_fraction"),
    ("RawIntDen", "raw_int_den"),
    ("Ch", "channel"),
    ("Slice", "slice"),
    ("Frame", "frame"),
    ("FeretX", "feret_x"),
    ("FeretY", "feret_y"),
    ("FeretAngle", "feret_angle"),
    ("MinFeret", "min_feret"),
    ("AR", "aspect_ratio"),
    ("Round", "roundness"),
    ("Solidity", "solidity"),
    ("Length", "length"),
];

fn run_measure(ctx: &mut CommandCtx<'_>, p: MeasureParams) -> Result<Outputs> {
    let mut settings = measure_settings(ctx.workspace());
    if let Some(keywords) = &p.measurements {
        settings.measurements =
            Measurements::from_keywords(keywords).map_err(|e| Error::InvalidArg {
                param: "measurements".to_string(),
                reason: e.to_string(),
            })?;
    }
    let plane = p.plane.read(ctx)?;
    let selected = Selected::of(ctx, p.plane.input, &plane)?;
    let rows = measure::measure(
        Measured {
            samples: plane.samples(),
            width: plane.width,
            height: plane.height,
            calibration: &plane.calibration,
            context: &plane.context,
        },
        selected.region(&plane),
        &selected.outline,
        &settings,
    )
    .map_err(Error::failed)?;

    let mut table = ctx.workspace().table(&p.table).cloned().unwrap_or_default();
    // As in ImageJ, measuring again discards a summary.
    measure::clear_summary(&mut table);
    for row in &rows {
        measure::append_row(&mut table, row);
    }
    let mut outputs = Outputs::new()
        .with("table", p.table.as_str())
        .with("row", table.len());
    if let Some(row) = rows.last() {
        for (heading, name) in OUTPUTS {
            if let Some(v) = row.number(heading) {
                outputs.insert(name, v);
            }
        }
        if let Some(label) = row.get(measure::LABEL).and_then(Cell::as_text) {
            outputs.insert("label", label);
        }
    }
    ctx.workspace_mut().set_table(p.table, table);

    if settings.add_to_overlay {
        let dataset = ctx.workspace().get(p.plane.input)?;
        if let Some(roi) = dataset.roi.clone() {
            let mut overlay = dataset.overlay.clone();
            overlay.push(roi);
            ctx.workspace_mut().set_overlay(p.plane.input, overlay)?;
        }
    }
    Ok(outputs)
}

#[derive(Params)]
struct SetMeasurementsParams {
    /// Area of the selection, in calibrated units squared.
    #[param(label = "Area", default = true)]
    area: bool,
    /// Mean of the samples.
    #[param(label = "Mean gray value", default = true)]
    mean: bool,
    /// Standard deviation of the samples.
    #[param(label = "Standard deviation", default = false, ijm = "standard")]
    std_dev: bool,
    /// The most frequent sample value.
    #[param(label = "Modal gray value", default = false, ijm = "modal")]
    mode: bool,
    /// Smallest and largest sample.
    #[param(label = "Min and max gray value", default = true, ijm = "min")]
    min_max: bool,
    /// Centre of the selection (X, Y).
    #[param(label = "Centroid", default = false)]
    centroid: bool,
    /// Brightness-weighted centre of the selection (XM, YM).
    #[param(label = "Center of mass", default = false, ijm = "center")]
    center_of_mass: bool,
    /// Length of the selection's outline.
    #[param(label = "Perimeter", default = false)]
    perimeter: bool,
    /// The smallest rectangle around the selection (BX, BY, Width, Height).
    #[param(label = "Bounding rectangle", default = false, ijm = "bounding")]
    bounding_rect: bool,
    /// Axes and angle of the best-fitting ellipse (Major, Minor, Angle).
    #[param(label = "Fit ellipse", default = false, ijm = "fit")]
    fit_ellipse: bool,
    /// Circularity, aspect ratio, roundness and solidity.
    #[param(label = "Shape descriptors", default = false, ijm = "shape")]
    shape_descriptors: bool,
    /// The longest and shortest caliper widths of the selection.
    #[param(label = "Feret's diameter", default = false, ijm = "feret's")]
    feret: bool,
    /// Area times mean (IntDen) and the sum of the samples (RawIntDen).
    #[param(label = "Integrated density", default = false, ijm = "integrated")]
    integrated_density: bool,
    /// Median of the samples.
    #[param(label = "Median", default = false)]
    median: bool,
    /// Skewness of the samples.
    #[param(label = "Skewness", default = false)]
    skewness: bool,
    /// Kurtosis of the samples.
    #[param(label = "Kurtosis", default = false)]
    kurtosis: bool,
    /// Percentage of samples that are not zero.
    #[param(label = "Area fraction", default = false)]
    area_fraction: bool,
    /// Channel, slice and frame of the measured plane.
    #[param(label = "Stack position", default = false, ijm = "stack")]
    stack_position: bool,
    /// Start each row with the image's name.
    #[param(label = "Display label", default = false, ijm = "display")]
    display_label: bool,
    /// Report Y coordinates counted from the bottom of the image.
    #[param(label = "Invert Y coordinates", default = false, ijm = "invert")]
    invert_y: bool,
    /// Write results in scientific notation.
    #[param(label = "Scientific notation", default = false, ijm = "scientific")]
    scientific_notation: bool,
    /// Keep each measured selection on the image as part of its overlay.
    #[param(label = "Add to overlay", default = false, ijm = "add")]
    add_to_overlay: bool,
    /// Show cells that were never filled as NaN instead of 0.
    #[param(label = "NaN empty cells", default = false, ijm = "nan")]
    nan_empty_cells: bool,
    /// Digits after the decimal point in the results table.
    #[param(
        label = "Decimal places",
        default = 3,
        min = 0,
        max = 9,
        ijm = "decimal"
    )]
    decimal_places: i64,
}

impl SetMeasurementsParams {
    fn settings(&self) -> MeasureSettings {
        MeasureSettings {
            measurements: Measurements {
                area: self.area,
                mean: self.mean,
                std_dev: self.std_dev,
                mode: self.mode,
                min_max: self.min_max,
                centroid: self.centroid,
                center_of_mass: self.center_of_mass,
                perimeter: self.perimeter,
                bounding_rect: self.bounding_rect,
                fit_ellipse: self.fit_ellipse,
                shape_descriptors: self.shape_descriptors,
                feret: self.feret,
                integrated_density: self.integrated_density,
                median: self.median,
                skewness: self.skewness,
                kurtosis: self.kurtosis,
                area_fraction: self.area_fraction,
                stack_position: self.stack_position,
            },
            display_label: self.display_label,
            invert_y: self.invert_y,
            scientific_notation: self.scientific_notation,
            nan_empty_cells: self.nan_empty_cells,
            add_to_overlay: self.add_to_overlay,
            decimal_places: self.decimal_places as u8,
        }
    }
}

#[derive(Params)]
struct TableParams {
    /// The results table to act on.
    #[param(label = "Table", default = "Results", advanced, no_persist)]
    table: String,
}

#[derive(Params)]
struct SaveResultsParams {
    /// Where to save the table. A name ending in `.csv` gives
    /// comma-separated values; any other gives tab-separated text.
    #[param(label = "File", path = "save", no_persist, ijm = "save")]
    path: PathBuf,
    /// The results table to save.
    #[param(label = "Table", default = "Results", advanced, no_persist)]
    table: String,
}

fn existing_table<'a>(workspace: &'a Workspace, name: &str) -> Result<&'a Table> {
    workspace
        .table(name)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            Error::failed(format!(
                "the {name} table is empty; measure something first"
            ))
        })
}

#[derive(Params)]
struct DistributionParams {
    /// The column whose values are counted.
    #[param(label = "Parameter", default = "Area", no_persist)]
    parameter: String,
    /// Choose the number of bins from the data (Scott's rule).
    #[param(label = "Automatic binning", default = true, ijm = "automatic")]
    automatic: bool,
    /// Number of bins when not automatic.
    #[param(label = "Bins", default = 10, min = 1, ijm = "or")]
    bins: i64,
    /// Smallest value counted when not automatic; with the largest also 0,
    /// the range of the data.
    #[param(label = "Range from", default = 0.0)]
    range_min: f64,
    /// Largest value counted when not automatic.
    #[param(label = "Range to", default = 0.0)]
    range_max: f64,
    /// The results table the column is in.
    #[param(label = "Table", default = "Results", advanced, no_persist)]
    table: String,
}

/// `ij.plugin.Distribution`: the values of one column counted into bins,
/// as a table of bin starts and counts.
fn run_distribution(ctx: &mut CommandCtx<'_>, p: DistributionParams) -> Result<Outputs> {
    let table = existing_table(ctx.workspace(), &p.table)?;
    let Some(column) = table.numbers(&p.parameter) else {
        let names: Vec<&str> = table.column_names().collect();
        return Err(Error::InvalidArg {
            param: "parameter".to_string(),
            reason: format!(
                "the {} table has no column `{}`; it has: {}",
                p.table,
                p.parameter,
                names.join(", ")
            ),
        });
    };
    // ImageJ works on the column in single precision, empty cells as 0.
    let data: Vec<f32> = column.iter().map(|v| v.unwrap_or(0.0) as f32).collect();
    let count = data.len();
    let (mut bins, mut range) = (p.bins as usize, (p.range_min, p.range_max));
    if p.automatic {
        // Scott's rule, 3.49 * sd * n^(-1/3), with the single-precision
        // arithmetic of Distribution.stats.
        let (mut total, mut min, mut max) = (0f32, f32::MAX, -f32::MAX);
        for &v in &data {
            total += v;
            min = min.min(v);
            max = max.max(v);
        }
        let mean = total / count as f32;
        let mut variance = 0f32;
        for &v in &data {
            variance += (v - mean) * (v - mean);
        }
        variance /= count as f32 - 1.0;
        let sd = variance.sqrt();
        let width = (3.49 * sd as f64 * (count as f32 as f64).powf(-1.0 / 3.0)) as f32;
        let wanted = (((max - min) / width) as f64 + 0.5).floor();
        bins = if wanted >= 2.0 { wanted as usize } else { 2 };
        range = (0.0, 0.0);
    }
    let image = Image::from_vec(count, 1, data)?;
    let options = HistogramOptions {
        bins,
        range: Some(range),
    };
    let histogram = stack_histogram(&image, &options).map_err(Error::failed)?;
    let mut out = Table::new();
    for (i, &n) in histogram.counts.iter().enumerate() {
        out.push_row([
            ("index", Cell::from(i as f64)),
            ("bin start", Cell::from(histogram.bin_start(i))),
            ("count", Cell::from(n as f64)),
        ]);
    }
    let name = format!("{} Distribution", p.parameter);
    ctx.workspace_mut().set_table(name.clone(), out);
    Ok(Outputs::new()
        .with("table", name)
        .with("bins", histogram.counts.len())
        .with("count", histogram.count as usize)
        .with("mean", histogram.mean)
        .with("std_dev", histogram.std_dev)
        .with("min", histogram.min)
        .with("max", histogram.max))
}

#[derive(Params)]
struct SetScaleParams {
    /// The image to calibrate.
    #[param(label = "Image")]
    input: DatasetId,
    /// A length measured on the image, in pixels.
    #[param(
        label = "Distance in pixels",
        default = 0.0,
        no_persist,
        ijm = "distance"
    )]
    distance: f64,
    /// The real length of that distance.
    #[param(label = "Known distance", default = 0.0, no_persist, ijm = "known")]
    known: f64,
    /// Height of a pixel divided by its width.
    #[param(label = "Pixel aspect ratio", default = 1.0, ijm = "pixel")]
    pixel_aspect_ratio: f64,
    /// The unit of the known distance, such as `um` or `mm`. `pixel`
    /// removes the scale.
    #[param(label = "Unit of length", default = "pixel", ijm = "unit")]
    unit: String,
    /// Give every open image this scale.
    #[param(label = "Global", default = false, ijm = "global")]
    global: bool,
}

/// `Calibration.sanitizeUnit`, with the units that mean "not calibrated"
/// as `None`.
fn length_unit(unit: &str) -> Option<String> {
    match unit.trim() {
        "" | "pixel" | "pixels" | "Pixel" | "Pixels" => None,
        "um" => Some("\u{b5}m".to_string()),
        "A" => Some("\u{c5}".to_string()),
        other => Some(other.to_string()),
    }
}

/// Sets the size of a sample along the axis of `kind`, if the image has
/// one. The axis keeps its origin in samples, as an ImageJ calibration
/// does.
fn set_axis(meta: &mut ImageMeta, kind: AxisKind, scale: f64, unit: &Option<String>) {
    let Some(index) = meta.axis_index(&kind) else {
        return;
    };
    let axis = meta.axis_mut(index).expect("the index was just found");
    let origin_in_samples = axis.origin / axis.scale;
    axis.scale = scale;
    axis.origin = if origin_in_samples == 0.0 {
        0.0
    } else {
        origin_in_samples * scale
    };
    axis.unit = unit.clone();
}

fn axis_scale(meta: &ImageMeta, kind: AxisKind) -> f64 {
    meta.axis_index(&kind).map_or(1.0, |i| meta.axes()[i].scale)
}

fn axis_unit(meta: &ImageMeta, kind: AxisKind) -> String {
    meta.axis_index(&kind)
        .and_then(|i| meta.axes()[i].unit.clone())
        .unwrap_or_else(|| "pixel".to_string())
}

fn calibration_outputs(meta: &ImageMeta) -> Outputs {
    Outputs::new()
        .with("pixel_width", axis_scale(meta, AxisKind::X))
        .with("pixel_height", axis_scale(meta, AxisKind::Y))
        .with("voxel_depth", axis_scale(meta, AxisKind::Z))
        .with("unit", axis_unit(meta, AxisKind::X))
}

/// `ij.plugin.filter.ScaleDialog`.
fn run_set_scale(ctx: &mut CommandCtx<'_>, p: SetScaleParams) -> Result<Outputs> {
    // ImageJ offers "unit" when a length is typed without one; equal
    // distances with it mean there is no scale.
    let unscaled = p.distance <= 0.0
        || p.known <= 0.0
        || (p.distance == p.known && p.unit == "unit")
        || p.unit.is_empty()
        || p.unit.starts_with("pixel")
        || p.unit.starts_with("Pixel");
    let unit = length_unit(&p.unit);
    let rescale = |meta: &ImageMeta| -> ImageMeta {
        let mut meta = meta.clone();
        if unscaled {
            for kind in [AxisKind::X, AxisKind::Y, AxisKind::Z] {
                set_axis(&mut meta, kind, 1.0, &None);
            }
            return meta;
        }
        let width = p.known / p.distance;
        let height = if p.pixel_aspect_ratio != 0.0 {
            width * p.pixel_aspect_ratio
        } else {
            width
        };
        // A depth that was never set follows the width.
        let depth = match axis_scale(&meta, AxisKind::Z) {
            1.0 => width,
            depth => depth,
        };
        set_axis(&mut meta, AxisKind::X, width, &unit);
        set_axis(&mut meta, AxisKind::Y, height, &unit);
        set_axis(&mut meta, AxisKind::Z, depth, &unit);
        meta
    };
    let targets: Vec<DatasetId> = if p.global {
        ctx.workspace().datasets().map(|d| d.id).collect()
    } else {
        vec![p.input]
    };
    for id in targets {
        let image = ctx.image(id)?.clone();
        let meta = rescale(image.meta());
        if &meta != image.meta() {
            ctx.workspace_mut().set_image(id, image.with_meta(meta)?)?;
        }
    }
    Ok(calibration_outputs(ctx.image(p.input)?.meta()))
}

#[derive(Params)]
struct PropertiesParams {
    /// The image whose calibration to change.
    #[param(label = "Image")]
    input: DatasetId,
    /// Width of a pixel. 0 removes the calibration.
    #[param(label = "Pixel width", min = 0.0, no_persist)]
    pixel_width: Option<f64>,
    /// Height of a pixel.
    #[param(label = "Pixel height", min = 0.0, no_persist)]
    pixel_height: Option<f64>,
    /// Distance between slices.
    #[param(label = "Voxel depth", min = 0.0, no_persist)]
    voxel_depth: Option<f64>,
    /// Unit of the three sizes, such as `um` or `mm`. `none` removes the
    /// calibration.
    #[param(label = "Unit of length", no_persist)]
    unit: Option<String>,
    /// Time between frames, optionally with its unit: `0.5` or `3 sec`.
    #[param(label = "Frame interval", no_persist, ijm = "frame")]
    frame_interval: Option<String>,
    /// The pixel position reported as (0, 0), as `x,y` or `x,y,z`.
    #[param(label = "Origin (pixels)", no_persist)]
    origin: Option<String>,
}

/// `ImageProperties.validateInterval` and the parsing that follows it: a
/// number, optionally followed by a unit (`sec` if absent).
fn parse_interval(text: &str) -> Result<(f64, String)> {
    let text = text.trim();
    let split = text
        .find(|c: char| c.is_alphabetic() || c == ' ')
        .unwrap_or(text.len());
    let (number, unit) = (text[..split].trim(), text[split..].trim());
    let interval: f64 = number.parse().map_err(|_| Error::InvalidArg {
        param: "frame_interval".to_string(),
        reason: format!("`{text}` is not a number followed by an optional unit"),
    })?;
    let unit = match unit {
        "" => "sec".to_string(),
        "usec" => "\u{b5}sec".to_string(),
        other => other.to_string(),
    };
    Ok((interval, unit))
}

/// `ij.plugin.filter.ImageProperties`, for the calibration it holds.
fn run_properties(ctx: &mut CommandCtx<'_>, p: PropertiesParams) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let mut meta = image.meta().clone();
    let width = p.pixel_width.unwrap_or(axis_scale(&meta, AxisKind::X));
    let unit = match &p.unit {
        Some(unit) if unit.eq_ignore_ascii_case("none") => None,
        Some(unit) => length_unit(unit),
        None => meta.axes()[0].unit.clone(),
    };
    let sizing = p.pixel_width.is_some()
        || p.pixel_height.is_some()
        || p.voxel_depth.is_some()
        || p.unit.is_some();
    let no_unit = p
        .unit
        .as_deref()
        .is_some_and(|u| u.is_empty() || u.eq_ignore_ascii_case("none"));
    if sizing && (width == 0.0 || no_unit) {
        // No unit or no width: ImageJ resets the calibration.
        for kind in [AxisKind::X, AxisKind::Y, AxisKind::Z] {
            set_axis(&mut meta, kind, 1.0, &None);
        }
    } else if sizing {
        let height = p.pixel_height.unwrap_or(axis_scale(&meta, AxisKind::Y));
        let depth = p.voxel_depth.unwrap_or(axis_scale(&meta, AxisKind::Z));
        for (kind, scale, name) in [
            (AxisKind::X, width, "pixel_width"),
            (AxisKind::Y, height, "pixel_height"),
            (AxisKind::Z, depth, "voxel_depth"),
        ] {
            if scale == 0.0 {
                return Err(Error::InvalidArg {
                    param: name.to_string(),
                    reason: "a size of 0 is only meaningful for the pixel width, where it \
                             removes the calibration"
                        .to_string(),
                });
            }
            set_axis(&mut meta, kind, scale, &unit);
        }
    }
    if let Some(text) = &p.frame_interval {
        let (interval, time_unit) = parse_interval(text)?;
        if let Some(index) = meta.axis_index(&AxisKind::Time) {
            let axis = meta.axis_mut(index).expect("the index was just found");
            // An interval of 0 means "not known", which an axis says with
            // a scale of 1 and no unit.
            (axis.scale, axis.unit) = if interval == 0.0 {
                (1.0, None)
            } else {
                (interval, Some(time_unit))
            };
        }
    }
    if let Some(text) = &p.origin {
        let numbers: Vec<f64> = text
            .split([',', ' '])
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse().map_err(|_| Error::InvalidArg {
                    param: "origin".to_string(),
                    reason: format!("`{text}` is not `x,y` or `x,y,z` in pixels"),
                })
            })
            .collect::<Result<_>>()?;
        // As ImageJ: a missing Y is the X, a missing Z is 0.
        let x = numbers.first().copied().unwrap_or(0.0);
        let y = numbers.get(1).copied().unwrap_or(x);
        let z = numbers.get(2).copied().unwrap_or(0.0);
        for (kind, origin) in [(AxisKind::X, x), (AxisKind::Y, y), (AxisKind::Z, z)] {
            if let Some(index) = meta.axis_index(&kind) {
                let axis = meta.axis_mut(index).expect("the index was just found");
                axis.origin = if origin == 0.0 {
                    0.0
                } else {
                    -origin * axis.scale
                };
            }
        }
    }
    if &meta != image.meta() {
        ctx.workspace_mut()
            .set_image(p.input, image.with_meta(meta.clone())?)?;
    }
    let interval = meta
        .axis_index(&AxisKind::Time)
        .map(|i| &meta.axes()[i])
        .filter(|axis| axis.unit.is_some())
        .map_or(0.0, |axis| axis.scale);
    Ok(calibration_outputs(&meta).with("frame_interval", interval))
}

#[derive(Params)]
struct HistogramParams {
    #[param(flatten)]
    plane: PlaneOf,
    /// Number of bins.
    #[param(label = "Bins", default = 256, min = 1, max = 65536)]
    bins: i64,
    /// Smallest value counted. Leave both ends out to use the range of
    /// the samples.
    #[param(label = "X min", no_persist)]
    x_min: Option<f64>,
    /// Largest value counted.
    #[param(label = "X max", no_persist)]
    x_max: Option<f64>,
    /// Count every plane of the image, not only the one on view.
    #[param(
        label = "Whole stack",
        default = false,
        ijm = "stack",
        position = "stack"
    )]
    stack: bool,
}

/// What a histogram table needs, whichever code counted it.
struct Counted {
    counts: Vec<u64>,
    hist_min: f64,
    bin_width: f64,
    count: u64,
    mean: f64,
    std_dev: f64,
    min: f64,
    max: f64,
    mode: f64,
}

/// The data of Analyze > Histogram (`ij.plugin.Histogram`,
/// `HistogramPlot.draw`).
fn run_histogram(ctx: &mut CommandCtx<'_>, p: HistogramParams) -> Result<Outputs> {
    let image = ctx.image(p.plane.input)?.clone();
    let meta = image.meta();
    let range = match (p.x_min, p.x_max) {
        (None, None) => None,
        (Some(a), Some(b)) if a < b => Some((a, b)),
        (Some(a), Some(b)) => {
            return Err(Error::InvalidArg {
                param: "x_max".to_string(),
                reason: format!("{b} is not above the minimum, {a}"),
            });
        }
        _ => {
            return Err(Error::InvalidArg {
                param: if p.x_min.is_none() { "x_min" } else { "x_max" }.to_string(),
                reason: "give both ends of the range or neither".to_string(),
            });
        }
    };
    let bins = p.bins as usize;
    let plane = p.plane.read(ctx)?;
    let selected = Selected::of(ctx, p.plane.input, &plane)?;
    let has_selection = !matches!(selected.outline.kind, OutlineKind::Whole);
    if has_selection && !selected.outline.is_area() {
        return Err(Error::failed("a histogram needs an area selection or none"));
    }
    let custom = bins != 256 || range.is_some();
    let eight_bit = plane.buffers.len() == 3 || matches!(plane.buffers[0], Buffer::U8(_));

    let counted = if p.stack && meta.plane_count() > plane.buffers.len() {
        if has_selection {
            return Err(Error::failed(
                "the histogram of a whole stack within a selection is not available yet; \
                 remove the selection or count one plane",
            ));
        }
        let h =
            stack_histogram(&image, &HistogramOptions { bins, range }).map_err(Error::failed)?;
        Counted {
            counts: h.counts,
            hist_min: h.hist_min,
            bin_width: h.bin_width,
            count: h.count,
            mean: h.mean,
            std_dev: h.std_dev,
            min: h.min,
            max: h.max,
            mode: h.mode,
        }
    } else {
        // ImageJ counts 8-bit and RGB planes with other bins or another
        // range as a stack of one image (StackStatistics.doCalculations):
        // the values are binned like floats.
        let as_floats;
        let samples = if eight_bit && custom {
            let values = measure::sample_values(plane.samples()).map_err(Error::failed)?;
            as_floats = Buffer::from_vec(values.into_iter().map(|v| v as f32).collect::<Vec<_>>());
            Samples::Gray(&as_floats)
        } else {
            plane.samples()
        };
        let stats = measure::statistics(
            samples,
            plane.width,
            plane.height,
            selected.region(&plane),
            &plane.calibration,
            Measurements::default(),
            HistogramSpec { bins, range },
        )
        .map_err(Error::failed)?;
        let (bin_width, mode) = if eight_bit && custom {
            let (lo, hi) = match range {
                // ImageJ counts 0 to 256 on 8-bit data as 0 to 255.
                Some((lo, hi)) if lo == 0.0 && hi == 256.0 => (lo, 255.0),
                _ => (stats.hist_min, stats.hist_max),
            };
            let width = (hi - lo + 1.0) / bins as f64;
            (width, stats.hist_min + stats.mode_bin as f64 * width)
        } else {
            (stats.bin_size, stats.mode)
        };
        Counted {
            counts: stats.histogram,
            hist_min: stats.hist_min,
            bin_width,
            count: stats.pixel_count,
            mean: stats.mean,
            std_dev: stats.std_dev,
            min: stats.min,
            max: stats.max,
            mode,
        }
    };

    // HistogramWindow.getResultsTable
    let mut table = Table::new();
    let value_heading = if counted.bin_width == 1.0 {
        "value"
    } else {
        "bin start"
    };
    for (i, &n) in counted.counts.iter().enumerate() {
        let start = counted.hist_min + i as f64 * counted.bin_width;
        let mut cells = Vec::with_capacity(3);
        if counted.bin_width != 1.0 {
            cells.push(("index", Cell::from(i as f64)));
        }
        cells.push((value_heading, Cell::from(start)));
        cells.push(("count", Cell::from(n as f64)));
        table.push_row(cells);
    }
    let name = format!("Histogram of {}", meta.name);
    ctx.workspace_mut().set_table(name.clone(), table);
    Ok(Outputs::new()
        .with("table", name)
        .with("bins", counted.counts.len())
        .with("bin_width", counted.bin_width)
        .with("count", counted.count as usize)
        .with("mean", counted.mean)
        .with("std_dev", counted.std_dev)
        .with("min", counted.min)
        .with("max", counted.max)
        .with("mode", counted.mode))
}

#[derive(Params)]
struct ProfileParams {
    #[param(flatten)]
    plane: PlaneOf,
    /// For a rectangle: average along rows, giving a profile down the
    /// image, instead of along columns.
    #[param(label = "Vertical profile", default = false)]
    vertical: bool,
    /// Interpolate between pixels along a line.
    #[param(label = "Interpolate line profiles", default = true, advanced)]
    interpolate: bool,
}

/// The data of Analyze > Plot Profile (`ij.gui.ProfilePlot`).
fn run_plot_profile(ctx: &mut CommandCtx<'_>, p: ProfileParams) -> Result<Outputs> {
    let plane = p.plane.read(ctx)?;
    let dataset = ctx.workspace().get(p.plane.input)?;
    let needs = "Plot Profile needs a rectangular or line selection";
    let Some(roi) = &dataset.roi else {
        return Err(Error::failed(format!("{needs}; there is no selection")));
    };
    let cal = &plane.calibration;
    let (samples, w, h) = (plane.samples(), plane.width, plane.height);
    // A rounded rectangle is a rectangle to ImageJ (Roi.RECTANGLE).
    let rectangle = matches!(roi, Roi::Rectangle { .. } | Roi::RoundedRectangle { .. });
    let line = matches!(
        roi,
        Roi::Line { .. }
            | Roi::Polyline { .. }
            | Roi::Freeline { .. }
            | Roi::RotatedRectangle { .. }
    );
    let (values, spacing) = if rectangle {
        let rect = roi
            .pixel_bounds(w, h)
            .ok_or_else(|| Error::failed("the selection is outside the image"))?;
        if p.vertical {
            (
                measure::row_average_profile(samples, w, h, rect),
                cal.pixel_height,
            )
        } else {
            (
                measure::column_average_profile(samples, w, h, rect),
                cal.pixel_width,
            )
        }
    } else if line {
        // ProfilePlot.getStraightLineProfile: along a straight line (a
        // rotated rectangle is the wide line along its axis) with pixels
        // that are not square, points are spaced by the line's calibrated
        // length. Other lines are spaced by the pixel width.
        let spacing = match *roi {
            Roi::Line { x1, y1, x2, y2, .. } | Roi::RotatedRectangle { x1, y1, x2, y2, .. }
                if cal.pixel_width != cal.pixel_height =>
            {
                // Line.getFloatPoints: single precision.
                let dx = f64::from(x2 as f32) - f64::from(x1 as f32);
                let dy = f64::from(y2 as f32) - f64::from(y1 as f32);
                let pixels = (dx * dx + dy * dy).sqrt();
                let calibrated =
                    ((dx * cal.pixel_width).powi(2) + (dy * cal.pixel_height).powi(2)).sqrt();
                calibrated / pixels
            }
            _ => cal.pixel_width,
        };
        (
            measure::selection_profile(samples, w, h, roi, p.interpolate),
            spacing,
        )
    } else {
        return Err(Error::failed(format!("{needs}, not `{roi}`")));
    };
    let values = values.map_err(Error::failed)?;
    if values.is_empty() {
        return Err(Error::failed(
            "the line selection is too short to have a profile",
        ));
    }
    let mut table = Table::new();
    for (i, &v) in values.iter().enumerate() {
        table.push_row([
            ("Distance", Cell::from(i as f64 * spacing)),
            ("Value", Cell::from(v)),
        ]);
    }
    let name = format!("Profile of {}", ctx.image(p.plane.input)?.meta().name);
    ctx.workspace_mut().set_table(name.clone(), table);
    let finite = || values.iter().copied().filter(|v| !v.is_nan());
    Ok(Outputs::new()
        .with("table", name)
        .with("points", values.len())
        .with("spacing", spacing)
        .with(
            "unit",
            cal.unit.clone().unwrap_or_else(|| "pixel".to_string()),
        )
        .with("min", finite().fold(f64::NAN, f64::min))
        .with("max", finite().fold(f64::NAN, f64::max)))
}

fn modifies() -> Effects {
    Effects {
        image: ImageEffect::Modifies,
        ..Effects::default()
    }
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    let mut measure_spec = CommandSpec::new("measure.measure", "Measure")
        .summary(
            "Adds a row to the Results table with measurements of the image or of the part \
             inside its selection.",
        )
        .doc(
            "Measures the plane on view. Which quantities are measured is chosen with Set \
             Measurements; lengths and areas are in the image's calibrated units. The values of \
             the new row are also returned as outputs.",
        )
        .menu("Analyze", 10.0)
        .shortcut("Ctrl+M")
        .ijm_name("Measure")
        .output("table", ParamType::Str)
        .output("row", ParamType::Int)
        .output("label", ParamType::Str);
    for (_, name) in OUTPUTS {
        measure_spec = measure_spec.output(name, ParamType::Float);
    }
    registry.register(typed(measure_spec, run_measure))?;

    registry.register(typed(
        CommandSpec::new("measure.set_measurements", "Set Measurements")
            .summary("Chooses what Measure measures and how results are written.")
            .doc(
                "The choice belongs to the session: it is recorded in History, undone with it, \
                 and starts from area, mean, minimum and maximum at three decimal places.",
            )
            .menu("Analyze", 70.0)
            .ijm_name("Set Measurements...")
            .output("measurements", ParamType::Str),
        |ctx, p: SetMeasurementsParams| {
            let settings = p.settings();
            let workspace = ctx.workspace_mut();
            workspace.set_table(SETTINGS_TABLE, settings_table(&settings));
            // As in ImageJ, turning "Display label" off removes the labels
            // already in the Results table.
            if !settings.display_label
                && let Some(results) = workspace.table(Workspace::RESULTS)
                && results.has_column(measure::LABEL)
            {
                let stripped = measure::without_labels(results);
                workspace.set_table(Workspace::RESULTS, stripped);
            }
            Ok(Outputs::new().with("measurements", settings.measurements.keywords()))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("measure.clear_results", "Clear Results")
            .summary("Removes every row and column of the Results table.")
            .menu("Analyze", 60.0)
            .ijm_name("Clear Results")
            .output("rows", ParamType::Int),
        |ctx, p: TableParams| {
            let removed = ctx.workspace_mut().remove_table(&p.table);
            Ok(Outputs::new().with("rows", removed.map_or(0, |t| t.len())))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("measure.save_results", "Save Results")
            .summary("Saves the Results table as comma-separated or tab-separated text.")
            .doc(
                "A file name ending in .csv gives comma-separated values; any other gives \
                 tab-separated text. Numbers are written as the Results table shows them, with \
                 the decimal places chosen in Set Measurements.",
            )
            .menu("Analyze", 65.0)
            .ijm_name("Results...")
            .output("rows", ParamType::Int)
            .effects(Effects {
                writes_fs: true,
                ..Effects::default()
            }),
        |ctx, p: SaveResultsParams| {
            let rows = existing_table(ctx.workspace(), &p.table)?.len();
            let name = p.path.to_string_lossy();
            let delimiter = TableFormat::delimiter_for(&name);
            let text = results_text(ctx.workspace(), &p.table, delimiter).unwrap_or_default();
            std::fs::write(&p.path, text)
                .map_err(|e| Error::failed(format!("cannot write {}: {e}", p.path.display())))?;
            Ok(Outputs::new().with("rows", rows))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("measure.summarize", "Summarize")
            .summary(
                "Adds rows with the mean, standard deviation, minimum and maximum of each \
                 column of the Results table.",
            )
            .menu("Analyze", 30.0)
            .ijm_name("Summarize")
            .output("rows", ParamType::Int),
        |ctx, p: TableParams| {
            let mut table = existing_table(ctx.workspace(), &p.table)?.clone();
            if table.len() < 2 {
                return Err(Error::failed(format!(
                    "the {} table has one row; a summary needs at least two",
                    p.table
                )));
            }
            if measure::summarize(&mut table) {
                let rows = table.len();
                ctx.workspace_mut().set_table(p.table, table);
                Ok(Outputs::new().with("rows", rows))
            } else {
                Ok(Outputs::new().with("rows", table.len()))
            }
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("measure.distribution", "Distribution")
            .summary("Counts the values of one column of the Results table into bins.")
            .doc(
                "The counts go to a table named after the column. With automatic binning the \
                 number of bins follows Scott's rule.",
            )
            .menu("Analyze", 40.0)
            .ijm_name("Distribution...")
            .output("table", ParamType::Str)
            .output("bins", ParamType::Int)
            .output("count", ParamType::Int)
            .output("mean", ParamType::Float)
            .output("std_dev", ParamType::Float)
            .output("min", ParamType::Float)
            .output("max", ParamType::Float),
        run_distribution,
    ))?;

    let calibration = |spec: CommandSpec| {
        spec.output("pixel_width", ParamType::Float)
            .output("pixel_height", ParamType::Float)
            .output("voxel_depth", ParamType::Float)
            .output("unit", ParamType::Str)
            .effects(modifies())
    };

    registry.register(typed(
        calibration(
            CommandSpec::new("measure.set_scale", "Set Scale")
                .summary("Sets the size of a pixel from a distance in pixels and its real length.")
                .doc(
                    "Measurements are then reported in the unit given. A distance or known \
                     distance of 0, or the unit `pixel`, removes the scale. Sample values are \
                     not changed.",
                )
                .menu("Analyze", 110.0)
                .ijm_name("Set Scale..."),
        ),
        run_set_scale,
    ))?;

    registry.register(typed(
        calibration(
            CommandSpec::new("image.properties", "Properties")
                .summary(
                    "Sets the size of a pixel, the distance between slices, their unit and the \
                     time between frames.",
                )
                .doc(
                    "Only what is given changes. A pixel width of 0 or the unit `none` removes \
                     the spatial calibration. Sample values are not changed.",
                )
                .menu("Image", 15.0)
                .ijm_name("Properties...")
                .output("frame_interval", ParamType::Float),
        ),
        run_properties,
    ))?;

    registry.register(typed(
        CommandSpec::new("analyze.histogram", "Histogram")
            .summary(
                "Counts how often each sample value occurs in the image or inside its \
                 selection.",
            )
            .doc(
                "The counts go to a table named after the image, one row per bin, and the \
                 number of samples, their mean, standard deviation, minimum, maximum and mode \
                 are returned.",
            )
            .menu("Analyze", 130.0)
            .ijm_name("Histogram")
            .output("table", ParamType::Str)
            .output("bins", ParamType::Int)
            .output("bin_width", ParamType::Float)
            .output("count", ParamType::Int)
            .output("mean", ParamType::Float)
            .output("std_dev", ParamType::Float)
            .output("min", ParamType::Float)
            .output("max", ParamType::Float)
            .output("mode", ParamType::Float),
        run_histogram,
    ))?;

    registry.register(typed(
        CommandSpec::new("analyze.plot_profile", "Plot Profile")
            .summary(
                "Gives the intensity along a line selection, or the column averages of a \
                 rectangular one.",
            )
            .doc(
                "The profile goes to a table named after the image, with the distance along \
                 the profile in calibrated units and the value there.",
            )
            .menu("Analyze", 140.0)
            .ijm_name("Plot Profile")
            .output("table", ParamType::Str)
            .output("points", ParamType::Int)
            .output("spacing", ParamType::Float)
            .output("unit", ParamType::Str)
            .output("min", ParamType::Float)
            .output("max", ParamType::Float),
        run_plot_profile,
    ))?;

    Ok(())
}
