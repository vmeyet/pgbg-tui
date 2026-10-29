//! The results grid's state: the cursor's cell, the first row and column in view, and each column's format, width and
//! colouring.
use super::colouring::{Colouring, colourings};
use super::shown_rows::{Order, ShownRows, Sort};
use crate::db::model::{ColumnName, ResultSet, Value};
use crate::format::{self, Format, Mode};
use crate::keys::{ColumnKey, Target};
use crate::render;
use chrono::Local;

/// No column is drawn wider, so one long text never hides the others.
const MAX_COLUMN_WIDTH: usize = 40;
/// How many rows a column's width and colouring are measured on, so measuring a big result costs no more than a small one.
const SAMPLE_ROWS: usize = 500;
/// What follows the name of a column that points to another table's.
const POINTS_TO_MARK: &str = " →";
/// How much of each cell `/` searches: more than the grid shows, without reading a long JSON value whole.
const FILTER_CHARS: usize = 200;
/// The space drawn before each column.
pub(in crate::tui) const GAP: &str = "  ";

/// What the frame just drawn had room for in the grid: the rows under its header and the terminal columns for cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct Room {
    pub(in crate::tui) rows: usize,
    pub(in crate::tui) columns: usize,
}

/// The moment and the local time zone the loop read for the frame.
pub(in crate::tui) type Clock = format::Clock<Local>;

/// The clock used until the loop reads the real one: the app never reads the clock itself.
pub(in crate::tui) const fn clock_at_epoch() -> Clock {
    Clock { now: chrono::DateTime::UNIX_EPOCH, zone: Local }
}

/// How cells are shown: the format mode a new result opens in, and the clock of the frame.
#[derive(Clone, Copy, Debug)]
pub(in crate::tui) struct Reading {
    pub(in crate::tui) mode: Mode,
    pub(in crate::tui) clock: Clock,
}

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

/// The cursor and scroll position in a result, and how each column is shown.
/// Widths and colourings are measured on the first page, when the result arrives and when a format changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Grid {
    /// Each column's format, by index.
    pub(in crate::tui) formats: Vec<Format>,
    /// Each column's width in terminal columns: its header or its widest sampled cell, capped.
    pub(in crate::tui) widths: Vec<usize>,
    /// How each column's cells are coloured, by index.
    pub(in crate::tui) colourings: Vec<Colouring>,
    /// Each column's key in the table the result was read from, by index; none when that table is unknown.
    keys: Vec<ColumnKey>,
    /// The rows shown and their order, after `/` and `s`.
    shown: ShownRows,
    /// The cursor's row, by its index in the result.
    pub(in crate::tui) row: usize,
    /// The cursor's column.
    pub(in crate::tui) column: usize,
    /// The place of the first row in view among the rows shown.
    pub(in crate::tui) top: usize,
    /// The first column in view.
    pub(in crate::tui) left: usize,
}

impl Grid {
    /// The grid on `result` with each column in its format from `formats` and its key from `keys`, the cursor on the
    /// first row; `keys` is empty when the table the result was read from is unknown.
    pub(in crate::tui) fn new(result: &ResultSet, formats: Vec<Format>, keys: Vec<ColumnKey>, clock: &Clock) -> Self {
        let mut grid = Self {
            formats: vec![],
            widths: vec![],
            colourings: vec![],
            keys,
            shown: ShownRows::default(),
            row: 0,
            column: 0,
            top: 0,
            left: 0,
        };
        grid.set_formats(formats, result, clock);
        grid
    }

    /// The grid on `result`, every column raw.
    #[cfg(test)]
    pub(in crate::tui) fn raw(result: &ResultSet) -> Self {
        Self::new(result, vec![Format::Raw; result.columns().len()], vec![], &clock_at_epoch())
    }

    /// Whether the column at `column` is part of the primary key of the table the result was read from.
    pub(in crate::tui) fn is_primary(&self, column: usize) -> bool {
        self.keys.get(column).is_some_and(|key| key.is_primary)
    }

    /// Where the column at `column` points through a foreign key of the table the result was read from.
    pub(in crate::tui) fn points_to(&self, column: usize) -> Option<&Target> {
        self.keys.get(column)?.points_to.as_ref()
    }

    /// Each column's key in the table the result was read from, by index; empty when that table is unknown.
    pub(in crate::tui) fn keys(&self) -> &[ColumnKey] {
        &self.keys
    }

    /// The header of the column at `column` of `result`: its name escaped, then `→` when it points to another table,
    /// and `▲` or `▼` when the rows are sorted by it.
    pub(in crate::tui) fn header(&self, result: &ResultSet, column: usize) -> String {
        let name = render::printable(result.columns()[column].name.as_str());
        let points_to = if self.points_to(column).is_some() { POINTS_TO_MARK } else { "" };
        let sorted = match self.shown.sort() {
            Some(Sort { column: sorted, order: Order::Ascending }) if sorted == column => " ▲",
            Some(Sort { column: sorted, order: Order::Descending }) if sorted == column => " ▼",
            _ => "",
        };
        format!("{name}{points_to}{sorted}")
    }

    /// The rows shown and their order.
    pub(in crate::tui) const fn shown(&self) -> &ShownRows {
        &self.shown
    }

    /// Whether the cursor's row shows: a filter can hide every row.
    pub(super) fn shows_cursor_row(&self, result: &ResultSet) -> bool {
        self.row < result.rows().len() && self.shown.place_of(self.row).is_some()
    }

    /// Shows only the rows holding `filter` in one of their cells as the grid reads them; the cursor goes to the first.
    pub(super) fn filter_rows(&mut self, filter: String, result: &ResultSet, clock: &Clock) {
        self.shown = self.shown.filtered(filter, result, searched_text(&self.formats, clock));
        self.go_to_first_row(result);
    }

    /// Sorts the rows shown by the cursor's column: ascending, then descending, then in the result's order; the cursor
    /// goes to the first row.
    pub(super) fn step_sort(&mut self, result: &ResultSet, clock: &Clock) {
        let sort = Sort::after(self.shown.sort(), self.column);
        self.arrange(self.shown.filter().to_owned(), sort, result, clock);
        self.go_to_first_row(result);
        self.widths = self.column_widths(result, &self.formats, clock);
    }

    /// Filters and sorts the rows of `result` again, once a page added rows or a format changed how they read; the
    /// cursor stays on its row while it shows.
    pub(super) fn take_rows(&mut self, result: &ResultSet, clock: &Clock) {
        if self.shown.is_whole() {
            return;
        }
        self.arrange(self.shown.filter().to_owned(), self.shown.sort(), result, clock);
        if self.shown.place_of(self.row).is_none() {
            self.go_to_first_row(result);
        }
    }

    fn arrange(&mut self, filter: String, sort: Option<Sort>, result: &ResultSet, clock: &Clock) {
        self.shown = ShownRows::new(filter, sort, result, searched_text(&self.formats, clock));
    }

    fn go_to_first_row(&mut self, result: &ResultSet) {
        self.row = self.shown.row_at(0, result.rows().len()).unwrap_or(0);
        self.top = 0;
    }

    /// The cursor's place among the rows shown; the first when a filter hides its row.
    fn cursor_place(&self) -> usize {
        self.shown.place_of(self.row).unwrap_or(0)
    }

    /// Puts the cursor's column in the next format its kind allows; when its kind has no other, leaves it and returns
    /// its name.
    pub(super) fn step_format(&mut self, result: &ResultSet, clock: &Clock) -> Option<ColumnName> {
        let column = result.columns().get(self.column)?;
        if format::allowed(&column.kind).len() == 1 {
            return Some(column.name.clone());
        }
        let mut formats = self.formats.clone();
        formats[self.column] = formats[self.column].next(&column.kind);
        self.set_formats(formats, result, clock);
        None
    }

    /// Puts every column raw, or each in its readable format when all are raw already.
    pub(super) fn switch_every_format(&mut self, result: &ResultSet, clock: &Clock) {
        let mode = if self.formats.iter().all(|&format| format == Format::Raw) { Mode::Readable } else { Mode::Raw };
        self.set_formats(format::opening(result.columns(), &[], mode), result, clock);
    }

    fn set_formats(&mut self, formats: Vec<Format>, result: &ResultSet, clock: &Clock) {
        self.widths = self.column_widths(result, &formats, clock);
        self.colourings = colourings(result, &formats, SAMPLE_ROWS);
        self.formats = formats;
        self.take_rows(result, clock);
    }

    /// Moves the cursor by `step` among the rows shown of `result`, keeping its cell in the `room` the grid has.
    pub(super) fn step(&mut self, step: Move, result: &ResultSet, room: Room) {
        let loaded = result.rows().len();
        match step {
            Move::Left => self.column = self.column.saturating_sub(1),
            Move::Right => self.column = (self.column + 1).min(self.widths.len().saturating_sub(1)),
            vertical => self.row = self.shown.row_at(self.place_after(vertical, loaded, room), loaded).unwrap_or(self.row),
        }
        self.keep_in_view(room);
    }

    /// The place among the rows shown that the cursor goes to on `step` up or down.
    fn place_after(&self, step: Move, loaded: usize, room: Room) -> usize {
        let place = self.cursor_place();
        let last = self.shown.count(loaded).saturating_sub(1);
        let page = room.rows.max(1);
        match step {
            Move::Down => (place + 1).min(last),
            Move::Up => place.saturating_sub(1),
            Move::PageDown => (place + page).min(last),
            Move::PageUp => place.saturating_sub(page),
            Move::First => 0,
            Move::Last => last,
            Move::Left | Move::Right => place,
        }
    }

    /// Scrolls the least so the cursor's cell is in view: its row among the rows, its column whole when it fits.
    pub(super) fn keep_in_view(&mut self, room: Room) {
        self.keep_row_in_view(room.rows);
        self.keep_column_in_view(room.columns);
    }

    fn keep_row_in_view(&mut self, visible: usize) {
        let visible = visible.max(1);
        let place = self.cursor_place();
        if place < self.top {
            self.top = place;
        }
        if place >= self.top + visible {
            self.top = place + 1 - visible;
        }
    }

    fn keep_column_in_view(&mut self, columns: usize) {
        self.left = self.left.min(self.column);
        while self.left < self.column && self.drawn_through_cursor() > columns {
            self.left += 1;
        }
    }

    /// The terminal columns from the first column in view through the cursor's, each after its gap.
    fn drawn_through_cursor(&self) -> usize {
        self.widths[self.left..=self.column].iter().map(|width| GAP.len() + width).sum()
    }

    fn column_widths(&self, result: &ResultSet, formats: &[Format], clock: &Clock) -> Vec<usize> {
        let header = |column| render::width(&self.header(result, column));
        formats.iter().enumerate().map(|(column, &format)| column_width(result, column, header(column), format, clock)).collect()
    }
}

/// The text `/` searches in a cell of the column at `column`: what the grid shows, in its column's format of `formats`.
fn searched_text<'a>(formats: &'a [Format], clock: &'a Clock) -> impl Fn(usize, &Value) -> String + 'a {
    |column, value| format::cell(value, formats[column], clock, FILTER_CHARS)
}

fn column_width(result: &ResultSet, column: usize, header: usize, format: Format, clock: &Clock) -> usize {
    let cells = result.rows().iter().take(SAMPLE_ROWS).filter_map(|row| row.get(column));
    let widest_cell = cells.map(|value| render::width(&format::cell(value, format, clock, MAX_COLUMN_WIDTH))).max().unwrap_or(0);
    header.max(widest_cell).max(format::least_width(format)).min(MAX_COLUMN_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::{ColumnKind, Statement, Value};
    use crate::fixtures::column;

    fn result(rows: usize) -> ResultSet {
        let int = |name: &str| column(name, ColumnKind::Int);
        ResultSet::whole(
            vec![int("id"), int("total"), int("customer")],
            (0..rows).map(|n| vec![Value::Int(n as i64), Value::Int(12), Value::Text("nina".into())]).collect(),
        )
    }

    fn stepped(rows: usize, steps: &[Move], visible: usize) -> Grid {
        stepped_in(rows, steps, Room { rows: visible, columns: 80 })
    }

    fn stepped_in(rows: usize, steps: &[Move], room: Room) -> Grid {
        let result = result(rows);
        let mut grid = Grid::raw(&result);
        for &step in steps {
            grid.step(step, &result, room);
        }
        grid
    }

    #[test]
    fn a_column_is_as_wide_as_its_header_or_its_widest_cell() {
        let result = result(1).with_cell(0, 2, Value::Text("Zoë Nakamura".into()));
        assert_eq!(Grid::raw(&result).widths, [2, 5, 12]);
    }

    fn stepped_format(result: &ResultSet, steps: usize) -> Grid {
        let mut grid = Grid::raw(result);
        (0..steps).for_each(|_| _ = grid.step_format(result, &clock_at_epoch()));
        grid
    }

    #[test]
    fn a_format_step_moves_the_cursors_column_to_its_next_format_and_measures_it_again() {
        let result = result(1).with_cell(0, 0, Value::Int(1_048_576));
        let grid = stepped_format(&result, 2);
        assert_eq!((grid.formats[0], grid.widths[0]), (Format::Bytes, "1.0 MiB".len()));
    }

    #[test]
    fn a_format_step_on_a_column_with_one_format_leaves_it_raw_and_names_it() {
        let result = crate::fixtures::orders();
        let mut grid = Grid::raw(&result);
        grid.column = 2;
        assert_eq!((grid.step_format(&result, &clock_at_epoch()), grid.formats[2]), (Some(ColumnName::new("customer")), Format::Raw));
    }

    #[test]
    fn switching_every_format_puts_raw_columns_readable_then_raw_again() {
        let result = result(1);
        let clock = clock_at_epoch();
        let mut grid = Grid::raw(&result);
        grid.switch_every_format(&result, &clock);
        assert_eq!(grid.formats, [Format::Grouped; 3]);
        grid.switch_every_format(&result, &clock);
        assert_eq!(grid.formats, [Format::Raw; 3]);
    }

    #[test]
    fn switching_every_format_puts_a_grid_with_one_readable_column_raw() {
        let result = result(1);
        let mut grid = stepped_format(&result, 1);
        grid.switch_every_format(&result, &clock_at_epoch());
        assert_eq!(grid.formats, [Format::Raw; 3]);
    }

    #[test]
    fn a_relative_column_leaves_room_for_its_text_to_grow_as_the_clock_runs() {
        let clock = clock_at_epoch();
        let result = ResultSet::whole(
            vec![column("at", ColumnKind::TimestampTz)],
            vec![vec![Value::TimestampTz(clock.now - chrono::Duration::minutes(1))]],
        );
        assert_eq!(Grid::new(&result, vec![Format::Relative], vec![], &clock).widths, ["59 min ago".len()]);
    }

    #[test]
    fn a_long_cell_is_capped() {
        let result = result(1).with_cell(0, 2, Value::Text("n".repeat(300).into()));
        assert_eq!(Grid::raw(&result).widths[2], MAX_COLUMN_WIDTH);
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
    fn right_and_left_move_the_cursor_one_column_and_stop_at_the_ends() {
        assert_eq!(stepped(1, &[Move::Right, Move::Right, Move::Right], 5).column, 2);
        assert_eq!(stepped(1, &[Move::Right, Move::Left, Move::Left], 5).column, 0);
    }

    #[test]
    fn the_view_stays_while_the_cursor_moves_among_the_columns_in_it() {
        assert_eq!(stepped(1, &[Move::Right, Move::Right], 5).left, 0);
    }

    #[test]
    fn the_view_scrolls_the_least_once_the_cursor_leaves_it() {
        let narrow = Room { rows: 5, columns: 18 };
        let grid = stepped_in(1, &[Move::Right, Move::Right], narrow);
        assert_eq!((grid.column, grid.left), (2, 1));
        assert_eq!(stepped_in(1, &[Move::Right, Move::Right, Move::Left, Move::Left], narrow).left, 0);
    }

    #[test]
    fn a_column_wider_than_the_grid_is_first_in_view() {
        assert_eq!(stepped_in(1, &[Move::Right, Move::Right], Room { rows: 5, columns: 3 }).left, 2);
    }

    #[test]
    fn an_empty_result_keeps_the_cursor_on_the_first_row() {
        assert_eq!(stepped(0, &[Move::Down, Move::Last, Move::PageDown], 5).row, 0);
    }

    fn keyed() -> Grid {
        let result = ResultSet::whole(vec![column("id", ColumnKind::Int), column("customer_id", ColumnKind::Int)], vec![]);
        let keys = crate::keys::column_keys(&crate::fixtures::shop_catalog(), &Statement::new("SELECT * FROM orders"), result.columns());
        Grid::new(&result, vec![Format::Raw; 2], keys, &clock_at_epoch())
    }

    #[test]
    fn a_column_pointing_to_another_table_is_marked_and_as_wide_as_its_mark() {
        assert_eq!(keyed().widths, ["id".len(), "customer_id →".chars().count()]);
    }

    #[test]
    fn a_column_of_the_primary_key_says_so() {
        assert_eq!([keyed().is_primary(0), keyed().is_primary(1)], [true, false]);
    }

    #[test]
    fn a_grid_without_keys_marks_no_column() {
        assert_eq!(Grid::raw(&result(1)).points_to(0), None);
    }
}
