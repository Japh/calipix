//! Measurements of an image or of the part of it inside a selection, and
//! the Results table they go into.
//!
//! Everything here reproduces ImageJ 1.x:
//!
//! * [`statistics`] is `ij.process.ImageStatistics` (area, mean, moments,
//!   median, centroid, fitted ellipse, histogram) for a plane and an
//!   optional mask.
//! * [`Outline`] holds what ImageJ asks a selection's outline for
//!   (perimeter, Feret diameters, convex hull, bounds); [`outline_of`]
//!   gets it for every kind of selection from `calipix-roi`, which owns
//!   that geometry.
//! * [`measure`] and [`results_row`] are `ij.plugin.filter.Analyzer`:
//!   how areas, lines, angles and points are measured, which columns a
//!   measurement gives, in which order.
//! * [`format_table`] and [`d2s`] are `ij.measure.ResultsTable`: the text
//!   of a table, byte for byte.
//! * [`column_average_profile`], [`line_profile`] and
//!   [`selection_profile`] are the numbers behind Analyze > Plot Profile
//!   (`ij.gui.ProfilePlot`, `ij.plugin.Straightener`).

mod ellipse;
mod format;
mod outline;
mod profile;
mod results;
mod stats;

pub use ellipse::{Ellipse, fit_ellipse};
pub use format::{TableFormat, d2s, format_table};
pub use outline::{Feret, Outline, OutlineKind, outline_of};
pub use profile::{
    column_average_profile, line_profile, row_average_profile, sample_values, selection_profile,
};
pub use results::{
    DEFAULT_HEADINGS, LABEL, MeasureSettings, Measured, Measurements, PlaneContext, Row,
    append_row, clear_summary, is_summarized, measure, results_row, row_label, short_slice_label,
    summarize, without_labels,
};
pub use stats::{Calibration, HistogramSpec, Region, Samples, Statistics, statistics};
