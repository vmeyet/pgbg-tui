//! The results grid's state: the current row, the first row and column in view, and each column's width.
use crate::db::model::{ResultSet, Value};
use crate::render;
use ratatui::text::Span;

/// No column is drawn wider, so one long text never hides the others.
const MAX_COLUMN_WIDTH: usize = 40;
/// The rows a column's width is measured on: the first page, so a big result costs no more.
const WIDTH_SAMPLE_ROWS: usize = 500;

/// A move of the grid's cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Move {
    Down,
    Up,
    PageDown,
    PageUp,
    First,
    Last,
    Left,
    Right,
}

/// Where the grid's cursor stands in a result; the widths are measured once, when the result arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Grid {
    /// Each column's width in terminal columns: its header or its widest sampled cell, capped.
    pub(in crate::tui) widths: Vec<usize>,
    pub(in crate::tui) row: usize,
    /// The first row in view.
    pub(in crate::tui) top: usize,
    /// The first column in view.
    pub(in crate::tui) column: usize,
    row_count: usize,
}

impl Grid {
    /// The grid on `result`, its cursor on the first row.
    pub(in crate::tui) fn new(result: &ResultSet) -> Self {
        let widths = (0..result.columns.len()).map(|c| column_width(result, c)).collect();
        Self { widths, row: 0, top: 0, column: 0, row_count: result.rows.len() }
    }

    /// Moves the cursor by `step`, keeping it among the `visible` rows in view.
    pub(super) fn step(&mut self, step: Move, visible: usize) {
        let last_row = self.row_count.saturating_sub(1);
        let page = visible.max(1);
        match step {
            Move::Down => self.row = (self.row + 1).min(last_row),
            Move::Up => self.row = self.row.saturating_sub(1),
            Move::PageDown => self.row = (self.row + page).min(last_row),
            Move::PageUp => self.row = self.row.saturating_sub(page),
            Move::First => self.row = 0,
            Move::Last => self.row = last_row,
            Move::Left => self.column = self.column.saturating_sub(1),
            Move::Right => self.column = (self.column + 1).min(self.widths.len().saturating_sub(1)),
        }
        self.keep_in_view(visible);
    }

    /// Scrolls the least so the cursor's row is among the `visible` rows in view.
    pub(super) fn keep_in_view(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.row < self.top {
            self.top = self.row;
        }
        if self.row >= self.top + visible {
            self.top = self.row + 1 - visible;
        }
    }
}

/// The text a cell shows: its value, control characters escaped.
pub(in crate::tui) fn cell_text(value: &Value) -> String {
    render::printable(&value.to_string()).into_owned()
}

fn column_width(result: &ResultSet, column: usize) -> usize {
    let header = Span::raw(render::printable(&result.columns[column].name)).width();
    let cells = result.rows.iter().take(WIDTH_SAMPLE_ROWS).filter_map(|row| row.get(column));
    let widest_cell = cells.map(|value| Span::raw(cell_text(value)).width()).max().unwrap_or(0);
    header.max(widest_cell).min(MAX_COLUMN_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::{Column, ColumnKind};

    fn result(rows: usize) -> ResultSet {
        let column = |name: &str| Column { name: name.to_owned(), kind: ColumnKind::Int, type_name: String::new() };
        ResultSet {
            columns: vec![column("id"), column("total"), column("customer")],
            rows: (0..rows).map(|n| vec![Value::Int(n as i64), Value::Int(12), Value::Text("nina".into())]).collect(),
        }
    }

    fn stepped(rows: usize, steps: &[Move], visible: usize) -> Grid {
        let mut grid = Grid::new(&result(rows));
        for &step in steps {
            grid.step(step, visible);
        }
        grid
    }

    #[test]
    fn a_column_is_as_wide_as_its_header_or_its_widest_cell() {
        let mut result = result(1);
        result.rows[0][2] = Value::Text("Zoë Nakamura".into());
        assert_eq!(Grid::new(&result).widths, [2, 5, 12]);
    }

    #[test]
    fn a_long_cell_is_capped() {
        let mut result = result(1);
        result.rows[0][2] = Value::Text("n".repeat(300));
        assert_eq!(Grid::new(&result).widths[2], MAX_COLUMN_WIDTH);
    }

    #[test]
    fn down_and_up_stop_at_the_ends() {
        assert_eq!(stepped(3, &[Move::Up], 10).row, 0);
        assert_eq!(stepped(3, &[Move::Down, Move::Down, Move::Down, Move::Down], 10).row, 2);
    }

    #[test]
    fn a_page_moves_by_the_visible_rows_and_scrolls_the_least() {
        let grid = stepped(20, &[Move::PageDown], 5);
        assert_eq!((grid.row, grid.top), (5, 1));
    }

    #[test]
    fn g_and_shift_g_go_to_the_first_and_last_rows() {
        assert_eq!(stepped(20, &[Move::Last], 5).row, 19);
        assert_eq!(stepped(20, &[Move::Last, Move::First], 5).top, 0);
    }

    #[test]
    fn right_and_left_scroll_one_column_and_stop_at_the_ends() {
        assert_eq!(stepped(1, &[Move::Right, Move::Right, Move::Right], 5).column, 2);
        assert_eq!(stepped(1, &[Move::Right, Move::Left, Move::Left], 5).column, 0);
    }

    #[test]
    fn an_empty_result_keeps_the_cursor_on_the_first_row() {
        assert_eq!(stepped(0, &[Move::Down, Move::Last, Move::PageDown], 5).row, 0);
    }
}
