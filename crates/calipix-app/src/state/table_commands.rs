//! Commands on results tables that the application adds to the standard
//! ones. Unlike the application commands in `action`, these change data:
//! they run through the executor like any other command, are recorded in
//! History, undone with it, and can be used headlessly and in macros.
//!
//! They live here because the Results panel is what needs them. They
//! belong with the measurement commands in `calipix-commands` and should
//! move there.

use std::collections::BTreeSet;

use calipix_command::{CommandSpec, Error, Outputs, ParamType, Params, Registry, Result, typed};
use calipix_core::Table;

/// The command that removes rows from a results table.
pub const DELETE_ROWS: &str = "measure.delete_rows";

#[derive(Params)]
struct DeleteRowsParams {
    /// The rows to remove, counted from 1 as the table numbers them:
    /// single rows and ranges separated by commas, such as `2-4,9`.
    #[param(label = "Rows", no_persist)]
    rows: String,
    /// The results table to remove them from.
    #[param(label = "Table", default = "Results", advanced, no_persist)]
    table: String,
}

/// Reads `2-4,9` as row indices counted from 0. Rows past the end of a
/// table of `len` rows are an error rather than ignored: a recorded step
/// that no longer fits the table should say so.
pub fn parse_rows(text: &str, len: usize) -> std::result::Result<BTreeSet<usize>, String> {
    let mut rows = BTreeSet::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        let number = |text: &str| {
            text.trim()
                .parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| format!("`{part}` is not a row number or a range such as 2-4"))
        };
        let (first, last) = (number(first)?, number(last)?);
        if first > last {
            return Err(format!("`{part}` runs backwards"));
        }
        if last > len {
            return Err(format!("row {last} is past the end; the table has {len}"));
        }
        rows.extend(first - 1..last);
    }
    if rows.is_empty() {
        return Err("no rows were given".to_string());
    }
    Ok(rows)
}

/// `table` without the rows in `remove`. The columns stay, in order, even
/// if no row is left.
fn without_rows(table: &Table, remove: &BTreeSet<usize>) -> Table {
    let headings: Vec<&str> = table.column_names().collect();
    let mut kept = table.clone();
    kept.clear_rows();
    for row in (0..table.len()).filter(|row| !remove.contains(row)) {
        kept.push_row(headings.iter().map(|h| (*h, table.get(row, h).clone())));
    }
    kept
}

/// Adds the table commands to `registry`.
pub fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        CommandSpec::new(DELETE_ROWS, "Delete Rows")
            .summary("Removes rows from the Results table.")
            .doc(
                "Rows are given by the numbers the table shows, as single rows and ranges \
                 separated by commas. The rows that remain are numbered again from 1.",
            )
            .output("rows", ParamType::Int),
        |ctx, p: DeleteRowsParams| {
            let Some(table) = ctx.workspace().table(&p.table) else {
                return Err(Error::failed(format!("there is no {} table", p.table)));
            };
            let remove = parse_rows(&p.rows, table.len()).map_err(|reason| Error::InvalidArg {
                param: "rows".to_string(),
                reason,
            })?;
            let kept = without_rows(table, &remove);
            let rows = kept.len();
            ctx.workspace_mut().set_table(p.table, kept);
            Ok(Outputs::new().with("rows", rows))
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_parse_as_the_table_numbers_them() {
        let rows = |text: &str| parse_rows(text, 10).map(|rows| rows.into_iter().collect());
        assert_eq!(rows("1"), Ok(vec![0]));
        assert_eq!(rows("2-4, 9"), Ok(vec![1, 2, 3, 8]));
        assert_eq!(rows("3,3,2-3"), Ok(vec![1, 2]));
        assert!(rows("").is_err());
        assert!(rows("0").is_err());
        assert!(rows("4-2").unwrap_err().contains("backwards"));
        assert!(rows("11").unwrap_err().contains("past the end"));
        assert!(rows("two").unwrap_err().contains("not a row number"));
    }
}
