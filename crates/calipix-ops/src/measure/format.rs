//! Results tables as text, written the way ImageJ's
//! `ij.measure.ResultsTable` writes them.

use calipix_core::{Cell, Table};

use super::results::{LABEL, MeasureSettings, SUMMARY_LABELS};

/// How to turn a table into text.
#[derive(Clone, Debug, PartialEq)]
pub struct TableFormat {
    /// Between cells: a comma for CSV, a tab otherwise.
    pub delimiter: char,
    /// Digits after the decimal point; negative for scientific notation
    /// with that many digits (ImageJ's convention).
    pub precision: i32,
    /// Start each row with its number, under a heading of one space.
    pub row_numbers: bool,
    /// Write the line of column headings.
    pub headings: bool,
    /// Show cells that were never filled as NaN rather than 0.
    pub nan_empty_cells: bool,
    /// Columns always written with a fixed number of decimal places,
    /// whatever their values, and that number (negative as above).
    pub fixed: Vec<(String, i32)>,
    /// Written after every line.
    pub line_ending: String,
}

impl TableFormat {
    /// The format ImageJ saves a file called `file_name` in: comma
    /// separated if it ends in `.csv`, tab separated otherwise.
    pub fn delimiter_for(file_name: &str) -> char {
        if file_name.ends_with(".csv") || file_name.ends_with(".CSV") {
            ','
        } else {
            '\t'
        }
    }

    /// The format of the Results table under `settings`, as Analyze >
    /// Measure leaves it: row numbers, the chosen decimal places and
    /// notation, and the shape descriptors always with decimals.
    ///
    /// ImageJ fixes the decimal places of the shape descriptors when the
    /// first row is measured, ignoring "Scientific notation", and only
    /// brings them into line at the next measurement; so a table of one
    /// row shows them in plain notation.
    pub fn results(table: &Table, settings: &MeasureSettings, delimiter: char) -> TableFormat {
        let places = settings.decimal_places.min(9) as i32;
        let precision = if settings.scientific_notation {
            -places
        } else {
            places
        };
        let shape = if table.len() > 1 { precision } else { places };
        TableFormat {
            delimiter,
            precision,
            row_numbers: true,
            headings: true,
            nan_empty_cells: settings.nan_empty_cells,
            fixed: ["Circ.", "AR", "Round", "Solidity"]
                .iter()
                .map(|h| (h.to_string(), shape))
                .collect(),
            line_ending: "\n".to_string(),
        }
    }

    /// A plain table of numbers: no row numbers, `precision` decimals.
    pub fn plain(delimiter: char, precision: i32) -> TableFormat {
        TableFormat {
            delimiter,
            precision,
            row_numbers: false,
            headings: true,
            nan_empty_cells: false,
            fixed: Vec::new(),
            line_ending: "\n".to_string(),
        }
    }
}

/// `ResultsTable.d2s`: `n` with `decimal_places` digits after the point,
/// switching to scientific notation for numbers that would show as zero,
/// numbers of more than 12 digits, and whenever `decimal_places` is
/// negative (then with that many digits).
///
/// Rounding is Java's `DecimalFormat`: half to even on the exact value,
/// except with no decimals, where halves round away from zero.
pub fn d2s(n: f64, decimal_places: i32) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if n == f32::MAX as f64 {
        return "3.4e38".to_string(); // a division by zero in a float image
    }
    let np = n.abs();
    if (np != 0.0 && np < 1.0 / 10f64.powi(decimal_places))
        || np > 999999999999.0
        || decimal_places < 0
    {
        let digits = if decimal_places < 0 {
            (-decimal_places).min(9) as usize
        } else {
            3
        };
        return format!("{n:.digits$e}").replace('e', "E");
    }
    match decimal_places.min(9) as usize {
        0 => format!("{:.0}", n.round()),
        places => format!("{n:.places$}"),
    }
}

/// True for a value that makes ImageJ give its whole column decimals:
/// anything that is not a whole number an `int` can hold.
fn needs_decimals(v: f64) -> bool {
    !v.is_nan() && (v as i32) as f64 != v
}

/// The decimal places of each column, or `None` for a column whose values
/// are formatted one by one (`ResultsTable.AUTO_FORMAT`).
///
/// ImageJ switches a column to fixed decimals when a value that is not a
/// whole number is added to one of its first 24 rows, or set afterwards
/// (which is how Summarize writes its rows).
fn column_places(table: &Table, heading: &str, format: &TableFormat) -> Option<i32> {
    if let Some((_, places)) = format.fixed.iter().find(|(h, _)| h == heading) {
        return Some(*places);
    }
    let rows = table.len();
    let summary = if super::results::is_summarized(table) {
        rows - SUMMARY_LABELS.len()..rows
    } else {
        rows..rows
    };
    (0..rows.min(24))
        .chain(summary)
        .any(|row| {
            table
                .get(row, heading)
                .as_number()
                .is_some_and(needs_decimals)
        })
        .then_some(format.precision)
}

/// One number as a column with decimal places `places` shows it
/// (`ResultsTable.getValueAsString`).
fn number(v: f64, places: Option<i32>, precision: i32) -> String {
    match places {
        Some(places) => d2s(v, places),
        // ResultsTable.n: whole numbers without decimals, unless the
        // notation is scientific.
        None if (v as i32) as f64 == v && precision >= 0 => d2s(v, 0),
        None => d2s(v, precision),
    }
}

/// The table as delimited text: what `ResultsTable.saveAs` writes to a
/// file, and (with a tab as delimiter) what the Results window shows.
///
/// A column called `Label` holds the row labels: it comes straight after
/// the row numbers and, in CSV, commas in it become semicolons. Other text
/// containing a comma, and empty text, is quoted in CSV.
pub fn format_table(table: &Table, format: &TableFormat) -> String {
    let d = format.delimiter;
    let csv = d == ',';
    let headings: Vec<&str> = table.column_names().collect();
    let labels = headings.contains(&LABEL);
    // Each column's decimal places and the last row holding text: ImageJ
    // pads a text column with empty text up to there.
    let columns: Vec<(&str, Option<i32>, Option<usize>)> = headings
        .iter()
        .filter(|h| **h != LABEL)
        .map(|h| {
            let last_text = (0..table.len())
                .rev()
                .find(|&row| matches!(table.get(row, h), Cell::Text(_)));
            (*h, column_places(table, h, format), last_text)
        })
        .collect();
    let mut out = String::new();
    if format.headings && !(table.is_empty() && headings.is_empty()) {
        if format.row_numbers {
            out.push(' ');
            out.push(d);
        }
        if labels {
            out.push_str(LABEL);
            out.push(d);
        }
        let names: Vec<&str> = columns.iter().map(|(h, _, _)| *h).collect();
        out.push_str(&names.join(&d.to_string()));
        out.push_str(&format.line_ending);
    }
    let empty = if format.nan_empty_cells {
        f64::NAN
    } else {
        0.0
    };
    for row in 0..table.len() {
        if format.row_numbers {
            out.push_str(&(row + 1).to_string());
            out.push(d);
        }
        if labels {
            let label = match table.get(row, LABEL) {
                Cell::Text(text) if csv => text.replace(',', ";"),
                Cell::Text(text) => text.clone(),
                Cell::Number(v) => number(*v, None, format.precision),
                Cell::Empty => String::new(),
            };
            out.push_str(&label);
            out.push(d);
        }
        for (i, (heading, places, last_text)) in columns.iter().enumerate() {
            if i > 0 {
                out.push(d);
            }
            let value = match table.get(row, heading) {
                Cell::Number(v) => number(*v, *places, format.precision),
                Cell::Empty if last_text.is_some_and(|last| row < last) => {
                    if csv { "\"\"" } else { "" }.to_string()
                }
                Cell::Empty => number(empty, *places, format.precision),
                Cell::Text(text) => {
                    let text = text.replace('\n', "\\n");
                    if csv && (text.contains(',') || text.is_empty()) {
                        format!("\"{text}\"")
                    } else {
                        text
                    }
                }
            };
            out.push_str(&value);
        }
        out.push_str(&format.line_ending);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_imagej_writes_them() {
        assert_eq!(d2s(127.556_722_689, 3), "127.557");
        assert_eq!(d2s(476.0, 0), "476");
        assert_eq!(d2s(2.5, 0), "3");
        assert_eq!(d2s(-2.5, 0), "-3");
        assert_eq!(d2s(0.125, 2), "0.12");
        assert_eq!(d2s(0.0004, 3), "4.000E-4");
        assert_eq!(d2s(0.5, 0), "5.000E-1");
        assert_eq!(d2s(0.0, 3), "0.000");
        assert_eq!(d2s(1234567890123.0, 3), "1.235E12");
        assert_eq!(d2s(476.0, -4), "4.7600E2");
        assert_eq!(d2s(f64::NAN, 3), "NaN");
        assert_eq!(d2s(f64::NEG_INFINITY, 3), "-Infinity");
        assert_eq!(d2s(f32::MAX as f64, 3), "3.4e38");
    }

    #[test]
    fn empty_table_is_empty_text() {
        let format = TableFormat::plain(',', 3);
        assert_eq!(format_table(&Table::new(), &format), "");
    }
}
