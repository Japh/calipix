//! The results tables of the session as the Results and Plot panels show
//! them: which tables there are, the one on show, its text, the rows picked
//! in it, and the tables that are also charts.
//!
//! Tables themselves belong to the workspace (commands write them, undo
//! restores them). What is kept here is only how they are being looked at,
//! plus their text, formatted once per change so that a table of a hundred
//! thousand rows costs nothing to scroll.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

use calipix_command::Workspace;
use calipix_commands::measure::{SETTINGS_TABLE, results_text};
use calipix_core::{Cell, Table};

/// The names of the tables a person can look at: the default Results table
/// first, then the others in alphabetical order. The table that holds the
/// measurement settings is not a result and is left out.
pub fn table_names(workspace: &Workspace) -> Vec<String> {
    let mut names: Vec<String> = workspace
        .table_names()
        .filter(|name| *name != SETTINGS_TABLE && *name != Workspace::RESULTS)
        .map(str::to_string)
        .collect();
    if workspace.table(Workspace::RESULTS).is_some() {
        names.insert(0, Workspace::RESULTS.to_string());
    }
    names
}

/// A table as text, exactly as ImageJ shows and saves it: the cells are
/// those of `measure::results_text`, which is what Save Results writes.
#[derive(Debug)]
pub struct TableText {
    pub name: String,
    /// Column headings. The first is the row number, which ImageJ heads
    /// with a single space.
    pub headings: Vec<String>,
    /// For each column, the longest cell in it, for sizing the column
    /// without measuring every row.
    pub widest: Vec<String>,
    /// For each column, whether its cells are numbers (and so read best
    /// aligned on the right).
    pub numeric: Vec<bool>,
    text: String,
    /// Where each row's line starts and ends in `text`.
    lines: Vec<Range<usize>>,
    /// What the text was made from, to know when it is out of date.
    table: Table,
    settings: Option<Table>,
}

impl TableText {
    fn new(workspace: &Workspace, name: &str) -> Option<TableText> {
        let table = workspace.table(name)?.clone();
        let text = results_text(workspace, name, '\t')?;
        let mut lines = Vec::with_capacity(table.len() + 1);
        let mut start = 0;
        for (end, _) in text.match_indices('\n') {
            lines.push(start..end);
            start = end + 1;
        }
        // An empty table without columns is written as no text at all.
        let heading_line = if lines.is_empty() {
            0..0
        } else {
            lines.remove(0)
        };
        let headings: Vec<String> = text[heading_line].split('\t').map(str::to_string).collect();
        let mut widest: Vec<&str> = headings.iter().map(String::as_str).collect();
        for line in &lines {
            for (column, cell) in text[line.clone()].split('\t').enumerate() {
                if let Some(longest) = widest.get_mut(column)
                    && cell.len() > longest.len()
                {
                    *longest = cell;
                }
            }
        }
        let widest = widest.into_iter().map(str::to_string).collect();
        // The row number comes first and the labels, if any, second; every
        // other column is numeric unless it holds text.
        let numeric = headings
            .iter()
            .enumerate()
            .map(|(index, heading)| {
                index == 0
                    || !(0..table.len().min(50))
                        .any(|row| matches!(table.get(row, heading), Cell::Text(_)))
            })
            .collect();
        Some(TableText {
            name: name.to_string(),
            headings,
            widest,
            numeric,
            lines,
            settings: workspace.table(SETTINGS_TABLE).cloned(),
            table,
            text,
        })
    }

    fn current(&self, workspace: &Workspace, name: &str) -> bool {
        let same = |a: Option<&Table>, b: Option<&Table>| match (a, b) {
            (Some(a), Some(b)) => a.ptr_eq(b),
            (None, None) => true,
            _ => false,
        };
        self.name == name
            && same(workspace.table(name), Some(&self.table))
            && same(workspace.table(SETTINGS_TABLE), self.settings.as_ref())
    }

    /// Number of rows, not counting the headings.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The cells of one row, the row number first.
    pub fn row(&self, row: usize) -> impl Iterator<Item = &str> {
        let line = self.lines.get(row).cloned().unwrap_or(0..0);
        self.text[line].split('\t')
    }

    /// One row as a line of tab-separated text.
    pub fn line(&self, row: usize) -> &str {
        &self.text[self.lines.get(row).cloned().unwrap_or(0..0)]
    }

    /// The headings and the given rows as tab-separated text, for the
    /// clipboard. A spreadsheet pasted into keeps the columns apart.
    pub fn copy(&self, rows: impl IntoIterator<Item = usize>) -> String {
        let mut out = self.headings.join("\t");
        out.push('\n');
        for row in rows {
            out.push_str(self.line(row));
            out.push('\n');
        }
        out
    }
}

/// How a row was clicked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowClick {
    /// With Shift: extend the selection from the row clicked before.
    pub extend: bool,
    /// With Ctrl: add or remove this row, keeping the others.
    pub toggle: bool,
}

/// The rows of `rows` as ImageJ-style ranges counted from 1: `2-4,9`.
pub fn row_ranges(rows: &BTreeSet<usize>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    let flush = |run: Option<(usize, usize)>, parts: &mut Vec<String>| {
        if let Some((first, last)) = run {
            parts.push(if first == last {
                (first + 1).to_string()
            } else {
                format!("{}-{}", first + 1, last + 1)
            });
        }
    };
    for &row in rows {
        run = match run {
            Some((first, last)) if row == last + 1 => Some((first, row)),
            other => {
                flush(other, &mut parts);
                Some((row, row))
            }
        };
    }
    flush(run, &mut parts);
    parts.join(",")
}

/// What a chart draws its points as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlotKind {
    /// Values along a line, joined up: a profile.
    Line,
    /// Counts per bin: a histogram or distribution.
    Bars,
}

/// What the command that made a plot's table said about it, which the
/// table itself does not record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlotNote {
    /// Unit of the horizontal axis.
    pub x_unit: Option<String>,
}

/// A table read as a chart.
#[derive(Clone, Debug, PartialEq)]
pub struct PlotData {
    pub name: String,
    pub kind: PlotKind,
    /// What the horizontal axis measures, with its unit if known.
    pub x_label: String,
    pub y_label: String,
    /// One point per row: where it is along the axis and its value. For
    /// bars, the place is where the bin starts.
    pub points: Vec<(f64, f64)>,
    /// For bars: how wide a bin is.
    pub bin_width: f64,
}

/// The columns that make a table a chart, if it is one: the horizontal
/// one, then the vertical one.
fn plot_columns(table: &Table) -> Option<(PlotKind, &'static str, &'static str)> {
    let has = |name: &str| table.has_column(name);
    if has("Distance") && has("Value") {
        Some((PlotKind::Line, "Distance", "Value"))
    } else if has("count") && has("bin start") {
        Some((PlotKind::Bars, "bin start", "count"))
    } else if has("count") && has("value") {
        Some((PlotKind::Bars, "value", "count"))
    } else {
        None
    }
}

/// True if a table holds a profile, a histogram or a distribution: the
/// tables Plot Profile, Histogram and Distribution write.
pub fn is_plot(table: &Table) -> bool {
    !table.is_empty() && plot_columns(table).is_some()
}

/// Reads a table as a chart, or `None` if it is not one. `note` adds what
/// only the command knew.
pub fn plot_of(name: &str, table: &Table, note: Option<&PlotNote>) -> Option<PlotData> {
    let (kind, x_column, y_column) = plot_columns(table)?;
    if table.is_empty() {
        return None;
    }
    let (xs, ys) = (table.numbers(x_column)?, table.numbers(y_column)?);
    let points: Vec<(f64, f64)> = xs
        .into_iter()
        .zip(ys)
        .map(|(x, y)| (x.unwrap_or(f64::NAN), y.unwrap_or(f64::NAN)))
        .collect();
    let bin_width = match points.as_slice() {
        [first, second, ..] => second.0 - first.0,
        _ => 1.0,
    };
    let unit = note.and_then(|note| note.x_unit.as_deref());
    let (x_label, y_label) = match kind {
        PlotKind::Line => {
            let unit = match unit {
                Some("pixel") | None => "pixels",
                Some(unit) => unit,
            };
            (format!("Distance ({unit})"), "Gray value".to_string())
        }
        PlotKind::Bars => {
            // "Area Distribution" counts the Area column.
            let counted = name.strip_suffix(" Distribution").unwrap_or("Value");
            let x_label = match unit {
                Some(unit) => format!("{counted} ({unit})"),
                None => counted.to_string(),
            };
            (x_label, "Count".to_string())
        }
    };
    Some(PlotData {
        name: name.to_string(),
        kind,
        x_label,
        y_label,
        points,
        bin_width: if bin_width.is_finite() && bin_width > 0.0 {
            bin_width
        } else {
            1.0
        },
    })
}

impl PlotData {
    /// The smallest and largest place along the horizontal axis, counting
    /// the width of the last bar.
    pub fn x_span(&self) -> (f64, f64) {
        let finite = || self.points.iter().map(|p| p.0).filter(|x| x.is_finite());
        let low = finite().fold(f64::INFINITY, f64::min);
        let mut high = finite().fold(f64::NEG_INFINITY, f64::max);
        if self.kind == PlotKind::Bars {
            high += self.bin_width;
        }
        if low.is_finite() && high > low {
            (low, high)
        } else if low.is_finite() {
            (low, low + 1.0)
        } else {
            (0.0, 1.0)
        }
    }

    /// The smallest and largest value. Bars stand on zero.
    pub fn y_span(&self) -> (f64, f64) {
        let finite = || self.points.iter().map(|p| p.1).filter(|y| y.is_finite());
        let mut low = finite().fold(f64::INFINITY, f64::min);
        let high = finite().fold(f64::NEG_INFINITY, f64::max);
        if self.kind == PlotKind::Bars {
            low = low.min(0.0);
        }
        if low.is_finite() && high > low {
            (low, high)
        } else if low.is_finite() {
            (low, low + 1.0)
        } else {
            (0.0, 1.0)
        }
    }

    /// The point a place along the horizontal axis belongs to: the bar it
    /// falls in, or the nearest point of a line.
    pub fn index_at(&self, x: f64) -> Option<usize> {
        if self.points.is_empty() || !x.is_finite() {
            return None;
        }
        match self.kind {
            PlotKind::Bars => self
                .points
                .iter()
                .position(|p| x >= p.0 && x < p.0 + self.bin_width),
            PlotKind::Line => self
                .points
                .iter()
                .enumerate()
                .filter(|(_, p)| p.0.is_finite())
                .min_by(|a, b| (a.1.0 - x).abs().total_cmp(&(b.1.0 - x).abs()))
                .map(|(index, _)| index),
        }
    }
}

/// Positions for the tick marks of an axis from `low` to `high`: round
/// numbers (1, 2 or 5 times a power of ten), about `wanted` of them.
pub fn ticks(low: f64, high: f64, wanted: usize) -> Vec<f64> {
    if !(low.is_finite() && high.is_finite()) || high <= low || wanted == 0 {
        return Vec::new();
    }
    let rough = (high - low) / wanted as f64;
    let power = 10f64.powf(rough.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * power)
        .find(|step| *step >= rough)
        .unwrap_or(10.0 * power);
    let first = (low / step).ceil() as i64;
    let last = (high / step).floor() as i64;
    // Rounded to the step's own precision, so that 3 x 0.2 is 0.6.
    let scale = 10f64.powf((1.0 - power.log10()).max(0.0));
    (first..=last)
        .map(|i| {
            let tick = (i as f64 * step * scale).round() / scale;
            if tick == 0.0 { 0.0 } else { tick }
        })
        .collect()
}

/// How the tables are being looked at.
#[derive(Default)]
pub struct Tables {
    /// The table the Results panel was last asked to show.
    pub(super) shown: Option<String>,
    /// The plot the Plot panel was last asked to show.
    pub(super) shown_plot: Option<String>,
    /// Rows picked in the table on show, counted from 0.
    pub(super) selected: BTreeSet<usize>,
    /// The row clicked last, which Shift+click extends from.
    pub(super) anchor: Option<usize>,
    /// True while the table is what the keyboard acts on: from a click in
    /// it until a click somewhere else.
    pub(super) attention: bool,
    /// Set when the table is drawn, cleared when that is asked about.
    pub(super) drawn: bool,
    pub(super) text: Option<Arc<TableText>>,
    pub(super) plot: Option<(Table, Arc<PlotData>)>,
    pub(super) notes: BTreeMap<String, PlotNote>,
    /// How often a table's text has been made, to check that it is kept.
    pub(super) formatted: u64,
}

impl Tables {
    /// The text of table `name`, made again only if the table or the
    /// measurement settings changed since last time.
    pub(super) fn text(&mut self, workspace: &Workspace, name: &str) -> Option<Arc<TableText>> {
        if let Some(text) = &self.text
            && text.current(workspace, name)
        {
            return Some(text.clone());
        }
        self.text = TableText::new(workspace, name).map(Arc::new);
        self.formatted += 1;
        // Row numbers mean something else in another table.
        self.selected.clear();
        self.anchor = None;
        self.text.clone()
    }

    pub(super) fn plot(&mut self, workspace: &Workspace, name: &str) -> Option<Arc<PlotData>> {
        let table = workspace.table(name)?;
        if let Some((made_from, plot)) = &self.plot
            && plot.name == name
            && made_from.ptr_eq(table)
        {
            return Some(plot.clone());
        }
        let plot = Arc::new(plot_of(name, table, self.notes.get(name))?);
        self.plot = Some((table.clone(), plot.clone()));
        Some(plot)
    }

    pub(super) fn click(&mut self, row: usize, rows: usize, click: RowClick) {
        if row >= rows {
            return;
        }
        match (click.extend, click.toggle, self.anchor) {
            (true, _, Some(anchor)) => {
                if !click.toggle {
                    self.selected.clear();
                }
                self.selected.extend(anchor.min(row)..=anchor.max(row));
            }
            (_, true, _) => {
                if !self.selected.remove(&row) {
                    self.selected.insert(row);
                }
                self.anchor = Some(row);
            }
            _ => {
                self.selected = BTreeSet::from([row]);
                self.anchor = Some(row);
            }
        }
        self.attention = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_written_as_ranges_from_one() {
        let rows = |list: &[usize]| row_ranges(&list.iter().copied().collect());
        assert_eq!(rows(&[]), "");
        assert_eq!(rows(&[0]), "1");
        assert_eq!(rows(&[1, 2, 3, 8]), "2-4,9");
        assert_eq!(rows(&[0, 2, 4, 5]), "1,3,5-6");
    }

    #[test]
    fn ticks_are_round_numbers_inside_the_axis() {
        assert_eq!(ticks(0.0, 255.0, 5), [0.0, 100.0, 200.0]);
        assert_eq!(ticks(0.0, 1.0, 5), [0.0, 0.2, 0.4, 0.6, 0.8, 1.0]);
        assert_eq!(ticks(-3.0, 3.0, 4), [-2.0, 0.0, 2.0]);
        assert_eq!(ticks(12.0, 13.0, 2), [12.0, 12.5, 13.0]);
        assert!(ticks(1.0, 1.0, 5).is_empty());
        assert!(ticks(f64::NAN, 1.0, 5).is_empty());
    }
}
