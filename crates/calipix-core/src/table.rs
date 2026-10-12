use std::sync::Arc;

/// One cell of a [`Table`].
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Cell {
    #[default]
    Empty,
    Number(f64),
    Text(String),
}

impl Cell {
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Cell::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Cell::Text(s) => Some(s),
            _ => None,
        }
    }
}

impl From<f64> for Cell {
    fn from(v: f64) -> Cell {
        Cell::Number(v)
    }
}

impl From<&str> for Cell {
    fn from(v: &str) -> Cell {
        Cell::Text(v.to_string())
    }
}

impl From<String> for Cell {
    fn from(v: String) -> Cell {
        Cell::Text(v)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Column {
    name: String,
    cells: Vec<Cell>,
}

/// A table of results: named columns of numbers or text, in the order the
/// columns were first used. Cloning is cheap; the contents are shared until
/// one of the copies is changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    columns: Arc<Vec<Column>>,
    rows: usize,
}

impl Table {
    pub fn new() -> Table {
        Table::default()
    }

    /// Number of rows.
    pub fn len(&self) -> usize {
        self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    pub fn column_names(&self) -> impl Iterator<Item = &str> {
        self.columns.iter().map(|c| c.name.as_str())
    }

    pub fn has_column(&self, name: &str) -> bool {
        self.columns.iter().any(|c| c.name == name)
    }

    /// The cell at `row` in column `name`; [`Cell::Empty`] if either does
    /// not exist.
    pub fn get(&self, row: usize, name: &str) -> &Cell {
        static EMPTY: Cell = Cell::Empty;
        self.columns
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| c.cells.get(row))
            .unwrap_or(&EMPTY)
    }

    /// The numbers in column `name`, with `None` where a cell is empty or
    /// text. `None` if there is no such column.
    pub fn numbers(&self, name: &str) -> Option<Vec<Option<f64>>> {
        let column = self.columns.iter().find(|c| c.name == name)?;
        Some(column.cells.iter().map(Cell::as_number).collect())
    }

    /// Appends a row. Columns not seen before are added at the right and
    /// are empty in earlier rows; columns not mentioned are empty in the
    /// new row.
    pub fn push_row<S: Into<String>>(&mut self, values: impl IntoIterator<Item = (S, Cell)>) {
        let row = self.rows;
        self.rows += 1;
        let columns = Arc::make_mut(&mut self.columns);
        for column in columns.iter_mut() {
            column.cells.push(Cell::Empty);
        }
        for (name, cell) in values {
            let name = name.into();
            let index = columns
                .iter()
                .position(|c| c.name == name)
                .unwrap_or_else(|| {
                    columns.push(Column {
                        name,
                        cells: vec![Cell::Empty; row + 1],
                    });
                    columns.len() - 1
                });
            columns[index].cells[row] = cell;
        }
    }

    /// Sets one cell, adding the column if needed. Does nothing if `row`
    /// is past the end.
    pub fn set(&mut self, row: usize, name: &str, cell: Cell) {
        if row >= self.rows {
            return;
        }
        let rows = self.rows;
        let columns = Arc::make_mut(&mut self.columns);
        let index = columns
            .iter()
            .position(|c| c.name == name)
            .unwrap_or_else(|| {
                columns.push(Column {
                    name: name.to_string(),
                    cells: vec![Cell::Empty; rows],
                });
                columns.len() - 1
            });
        columns[index].cells[row] = cell;
    }

    /// Removes one row. Does nothing if `row` is past the end.
    pub fn remove_row(&mut self, row: usize) {
        if row >= self.rows {
            return;
        }
        self.rows -= 1;
        for column in Arc::make_mut(&mut self.columns).iter_mut() {
            column.cells.remove(row);
        }
    }

    /// Removes a column. Returns false if there was none of that name.
    pub fn remove_column(&mut self, name: &str) -> bool {
        let columns = Arc::make_mut(&mut self.columns);
        let before = columns.len();
        columns.retain(|c| c.name != name);
        columns.len() != before
    }

    /// Puts the columns in the order given by `names`. Columns not named
    /// keep their relative order after the named ones; names that match no
    /// column are ignored.
    pub fn reorder_columns(&mut self, names: &[&str]) {
        let columns = Arc::make_mut(&mut self.columns);
        let rank = |c: &Column| {
            names
                .iter()
                .position(|n| *n == c.name)
                .unwrap_or(names.len())
        };
        columns.sort_by_key(rank);
    }

    /// Removes every row but keeps the columns.
    pub fn clear_rows(&mut self) {
        self.rows = 0;
        for column in Arc::make_mut(&mut self.columns).iter_mut() {
            column.cells.clear();
        }
    }

    /// True if both tables share the same contents without comparing them.
    pub fn ptr_eq(&self, other: &Table) -> bool {
        self.rows == other.rows && Arc::ptr_eq(&self.columns, &other.columns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_columns_grow_as_used() {
        let mut t = Table::new();
        t.push_row([("Area", Cell::from(12.0)), ("Mean", Cell::from(3.5))]);
        t.push_row([("Mean", Cell::from(4.0)), ("Label", Cell::from("cell 2"))]);
        assert_eq!(t.len(), 2);
        assert_eq!(
            t.column_names().collect::<Vec<_>>(),
            ["Area", "Mean", "Label"]
        );
        assert_eq!(t.get(0, "Area"), &Cell::Number(12.0));
        assert_eq!(t.get(1, "Area"), &Cell::Empty);
        assert_eq!(t.get(0, "Label"), &Cell::Empty);
        assert_eq!(t.get(1, "Label").as_text(), Some("cell 2"));
        assert_eq!(t.get(5, "Area"), &Cell::Empty);
        assert_eq!(t.get(0, "Nope"), &Cell::Empty);
        assert_eq!(t.numbers("Mean"), Some(vec![Some(3.5), Some(4.0)]));
        assert_eq!(t.numbers("Label"), Some(vec![None, None]));
        assert_eq!(t.numbers("Nope"), None);
    }

    #[test]
    fn rows_and_columns_can_be_removed_and_reordered() {
        let mut t = Table::new();
        for i in 0..3 {
            t.push_row([
                ("A", Cell::from(i as f64)),
                ("B", Cell::from(10.0 + i as f64)),
                ("C", Cell::from("x")),
            ]);
        }
        t.remove_row(1);
        t.remove_row(9);
        assert_eq!(t.numbers("A"), Some(vec![Some(0.0), Some(2.0)]));
        t.reorder_columns(&["C", "Nope", "A"]);
        assert_eq!(t.column_names().collect::<Vec<_>>(), ["C", "A", "B"]);
        assert!(t.remove_column("A"));
        assert!(!t.remove_column("A"));
        assert_eq!(t.column_names().collect::<Vec<_>>(), ["C", "B"]);
        assert_eq!(t.get(1, "B"), &Cell::Number(12.0));
    }

    #[test]
    fn copies_are_independent() {
        let mut a = Table::new();
        a.push_row([("X", Cell::from(1.0))]);
        let b = a.clone();
        assert!(a.ptr_eq(&b));
        a.set(0, "X", Cell::from(2.0));
        a.set(0, "Y", Cell::from("new"));
        a.set(9, "X", Cell::from(9.0));
        assert!(!a.ptr_eq(&b));
        assert_eq!(b.get(0, "X"), &Cell::Number(1.0));
        assert!(!b.has_column("Y"));
        assert_eq!(a.get(0, "Y").as_text(), Some("new"));
        a.clear_rows();
        assert!(a.is_empty());
        assert!(a.has_column("X"));
        assert_eq!(b.len(), 1);
    }
}
