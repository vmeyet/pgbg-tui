//! The results grid: the header, then only the rows and columns in view, with numbers right-aligned.
//! `▶` marks the cursor's row and its cell is highlighted; each cell shows in its column's format and colouring.
//! Cells are written straight into the frame's buffer, so a cell in several colours costs no more than one.
use super::app::{Clock, GAP, Grid, Room};
use super::ink::{Ink, ink};
use crate::db::model::{Column, ResultSet, Value};
use crate::format::{self, Format};
use crate::render;
use crate::syntax::highlight_json_line;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use vtui::text::truncate;
use vtui::theme::Theme;

const CURSOR: &str = "▶";
const NO_CURSOR: &str = " ";
/// The terminal columns the cursor's mark takes, before the first column.
const MARK_WIDTH: u16 = 1;

/// What the grid is drawn from.
#[derive(Clone, Copy, Debug)]
pub(super) struct GridView<'a> {
    pub(super) result: &'a ResultSet,
    pub(super) grid: &'a Grid,
    pub(super) theme: &'a Theme,
    pub(super) focused: bool,
    pub(super) clock: &'a Clock,
}

/// A column in view: its index in the result, the terminal column it starts at and the terminal columns it is drawn in.
#[derive(Clone, Copy, Debug)]
struct Shown {
    index: usize,
    x: u16,
    width: u16,
}

/// The columns from `first` onward that fit between terminal columns `left` and `end`, each after its gap; the last one
/// is cut to the space left.
fn in_view(widths: &[usize], first: usize, left: u16, end: u16) -> Vec<Shown> {
    let gap = GAP.len() as u16;
    let mut x = left;
    let mut shown = vec![];
    for (index, &width) in widths.iter().enumerate().skip(first) {
        if end - x <= gap {
            break;
        }
        x += gap;
        let drawn = u16::try_from(width).unwrap_or(u16::MAX).min(end - x);
        shown.push(Shown { index, x, width: drawn });
        x += drawn;
    }
    shown
}

/// The grid in `area`; returns the room it had below its header and next to the cursor's mark.
pub(super) fn draw(f: &mut Frame<'_>, view: GridView<'_>, area: Rect) -> Room {
    let room = area.width.saturating_sub(MARK_WIDTH);
    let shown = in_view(&view.grid.widths, view.grid.left, area.x + MARK_WIDTH, area.right().max(area.x + MARK_WIDTH));
    let body_rows = usize::from(area.height).saturating_sub(1);
    let buffer = f.buffer_mut();
    if area.height > 0 {
        draw_header(buffer, view, &shown, area.y);
    }
    let loaded = view.result.rows();
    let rows = (view.grid.top..view.grid.top + body_rows).map_while(|place| view.grid.shown().row_at(place, loaded.len()));
    for (y, index) in (area.y + 1..).zip(rows) {
        draw_row(buffer, view, &shown, Position { x: area.x, y }, index, &loaded[index]);
    }
    Room { rows: body_rows, columns: usize::from(room) }
}

/// Each column's name, bold when it is part of the primary key, followed by `→` when it points to another table.
fn draw_header(buffer: &mut Buffer, view: GridView<'_>, shown: &[Shown], y: u16) {
    for column in shown {
        let is_bold = view.grid.is_primary(column.index);
        let style = if is_bold { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
        let style = if column.index == view.grid.column { style.fg(cursor_colour(view)) } else { style };
        let text = truncate(&view.grid.header(view.result, column.index), column.width.into());
        let at = Position { x: column.x, y };
        draw_aligned(buffer, at, column.width, &view.result.columns()[column.index], &text, Ink::Whole(style), view.theme);
    }
}

fn draw_row(buffer: &mut Buffer, view: GridView<'_>, shown: &[Shown], mark: Position, index: usize, row: &[Value]) {
    let is_current = index == view.grid.row;
    let marker = if is_current { CURSOR } else { NO_CURSOR };
    buffer.set_string(mark.x, mark.y, marker, Style::default().fg(cursor_colour(view)));
    for column in shown {
        let at = Position { x: column.x, y: mark.y };
        if is_current && column.index == view.grid.column {
            buffer.set_style(Rect { x: at.x, y: at.y, width: column.width, height: 1 }, Style::default().add_modifier(Modifier::REVERSED));
        }
        let value = &row[column.index];
        let text = cell(value, view.grid.formats[column.index], view.clock, column.width.into());
        let ink = ink(&text, value, view.grid.colourings[column.index], view.theme);
        draw_aligned(buffer, at, column.width, &view.result.columns()[column.index], &text, ink, view.theme);
    }
}

fn cursor_colour(view: GridView<'_>) -> Color {
    if view.focused { view.theme.accent } else { view.theme.faded }
}

/// Enough of `value` in `format` to fill `width`, cut there with `…` when it continues past it.
fn cell(value: &Value, format: Format, clock: &Clock, width: usize) -> String {
    let text = format::cell(value, format, clock, width + 1);
    if render::width(&text) <= width { text } else { truncate(&text, width) }
}

/// `text`, which fits `width`, in `ink` at `at`: numbers on the right, so their digits line up, and other values on the
/// left.
fn draw_aligned(buffer: &mut Buffer, at: Position, width: u16, column: &Column, text: &str, ink: Ink, theme: &Theme) {
    let padding = usize::from(width).saturating_sub(render::width(text));
    let x = if column.kind.is_number() { at.x + u16::try_from(padding).unwrap_or(0) } else { at.x };
    draw_inked(buffer, Position { x, y: at.y }, text, ink, theme);
}

fn draw_inked(buffer: &mut Buffer, at: Position, text: &str, ink: Ink, theme: &Theme) {
    match ink {
        Ink::Whole(style) => _ = draw_piece(buffer, at, text, style),
        Ink::Split { at: split, head, tail } => {
            let x = draw_piece(buffer, at, &text[..split], head);
            draw_piece(buffer, Position { x, y: at.y }, &text[split..], tail);
        }
        Ink::Json => draw_json(buffer, at, text, theme),
    }
}

/// `text` in one pass: each JSON token in its colour, and what lies between plain.
fn draw_json(buffer: &mut Buffer, at: Position, text: &str, theme: &Theme) {
    let mut x = at.x;
    let mut drawn = 0;
    for painted in highlight_json_line(text) {
        let colour = Style::default().fg(theme.syntax.colour(painted.token));
        x = draw_piece(buffer, Position { x, y: at.y }, &text[drawn..painted.bytes.start], Style::default());
        x = draw_piece(buffer, Position { x, y: at.y }, &text[painted.bytes.clone()], colour);
        drawn = painted.bytes.end;
    }
    draw_piece(buffer, Position { x, y: at.y }, &text[drawn..], Style::default());
}

/// `piece` at `at` in `style`, over the cell's own; returns the terminal column after it.
fn draw_piece(buffer: &mut Buffer, at: Position, piece: &str, style: Style) -> u16 {
    buffer.set_stringn(at.x, at.y, piece, usize::MAX, style).0
}

#[cfg(test)]
mod tests {
    use super::super::app::Grid;
    use super::super::styled;
    use super::*;
    use crate::db::model::{ColumnKind, JsonText};
    use crate::fixtures::{column, orders, shop_column};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(result: &ResultSet, grid: &Grid, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let clock = Clock { now: at("2026-07-14T12:00:00Z"), zone: chrono::Local };
        let view = GridView { result, grid, theme: &Theme::default(), focused: true, clock: &clock };
        terminal.draw(|f| _ = draw(f, view, f.area())).unwrap();
        terminal
    }

    /// The wall clock, read in a release build only, by CI's own step: `cargo test --release --lib -- --ignored budget::`.
    #[cfg(not(debug_assertions))]
    mod budget {
        use super::*;
        use crate::fixtures::{best_of, capped};
        use std::time::Duration;

        /// A 200 × 50 frame takes 0.3 ms on a laptop whatever the row in view, CI about four times that: ten times slower
        /// fails, as drawing every row instead of the window would.
        const FRAME_BUDGET: Duration = Duration::from_millis(4);

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn a_frame_on_the_last_of_ten_thousand_rows_stays_within_the_budget() {
            let result = capped();
            let mut grid = Grid::raw(&result);
            (grid.row, grid.top) = (9_999, 9_950);
            let mut terminal = Terminal::new(TestBackend::new(200, 50)).unwrap();
            let clock = Clock { now: at("2026-07-14T12:00:00Z"), zone: chrono::Local };
            let view = GridView { result: &result, grid: &grid, theme: &Theme::default(), focused: true, clock: &clock };
            let took = best_of(|| terminal.draw(|f| _ = draw(f, view, f.area())).unwrap().area);
            assert!(took < FRAME_BUDGET, "took {took:?}");
        }
    }

    #[test]
    fn numbers_align_right_and_the_cursor_row_is_marked() {
        let result = orders();
        let mut grid = Grid::raw(&result);
        grid.row = 1;
        insta::assert_snapshot!("aligned", render(&result, &grid, 40, 6).backend());
    }

    #[test]
    fn only_the_rows_in_view_are_drawn() {
        let result = orders();
        let mut grid = Grid::raw(&result);
        (grid.row, grid.top) = (3, 2);
        insta::assert_snapshot!("scrolled_down", render(&result, &grid, 40, 3).backend());
    }

    #[test]
    fn a_scrolled_grid_starts_at_its_first_column_in_view() {
        let result = orders();
        let mut grid = Grid::raw(&result);
        (grid.column, grid.left) = (1, 1);
        insta::assert_snapshot!("scrolled_right", render(&result, &grid, 40, 3).backend());
    }

    #[test]
    fn the_cursors_cell_is_lit_and_its_header_coloured() {
        let result = orders();
        let mut grid = Grid::raw(&result);
        (grid.row, grid.column) = (1, 1);
        insta::assert_snapshot!("cursor_styled", styled::runs(render(&result, &grid, 30, 3).backend().buffer()));
    }

    fn at(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(rfc3339).unwrap().to_utc()
    }

    #[test]
    fn each_cell_reads_in_its_columns_format() {
        let paid_at = |at: &str| vec![Value::Int(1_234_567), Value::TimestampTz(self::at(at))];
        let columns = vec![column("amount", ColumnKind::Int), column("paid_at", ColumnKind::TimestampTz)];
        let result = ResultSet::whole(columns, vec![paid_at("2026-07-14T09:00:00Z"), paid_at("2026-07-12T12:00:00Z")]);
        let clock = Clock { now: at("2026-07-14T12:00:00Z"), zone: chrono::Local };
        let grid = Grid::new(&result, vec![Format::Grouped, Format::Relative], vec![], &clock);
        insta::assert_snapshot!("formatted", render(&result, &grid, 30, 3).backend());
    }

    #[test]
    fn a_narrow_grid_cuts_its_last_column() {
        let result = orders();
        insta::assert_snapshot!("narrow", render(&result, &Grid::raw(&result), 22, 5).backend());
    }

    /// The styled runs of `rows` of one column of `kind`, each column raw, the cursor off them on a row beyond.
    fn coloured(kind: ColumnKind, rows: Vec<Value>, width: u16) -> String {
        let height = u16::try_from(rows.len()).unwrap() + 1;
        let result = ResultSet::whole(vec![column("c", kind)], rows.into_iter().map(|value| vec![value]).collect());
        let mut grid = Grid::raw(&result);
        grid.row = usize::MAX;
        styled::runs(render(&result, &grid, width, height).backend().buffer())
    }

    #[test]
    fn a_null_is_dimmed_and_slanted_beside_the_text_null() {
        insta::assert_snapshot!("null", coloured(ColumnKind::Text, vec![Value::Null, Value::Text("NULL".into())], 8));
    }

    #[test]
    fn each_repeated_text_takes_its_own_colour() {
        let statuses = ["paid", "open", "paid", "open"].map(|status| Value::Text(status.into()));
        insta::assert_snapshot!("repeated_texts", coloured(ColumnKind::Text, statuses.into(), 8));
    }

    #[test]
    fn true_is_in_the_success_colour_and_false_dimmed() {
        insta::assert_snapshot!("booleans", coloured(ColumnKind::Bool, vec![Value::Bool(true), Value::Bool(false)], 8));
    }

    #[test]
    fn a_boolean_reads_as_a_mark_when_checked() {
        let result = ResultSet::whole(vec![column("paid", ColumnKind::Bool)], vec![vec![Value::Bool(true)], vec![Value::Bool(false)]]);
        let clock = Clock { now: at("2026-07-14T12:00:00Z"), zone: chrono::Local };
        let grid = Grid::new(&result, vec![Format::Check], vec![], &clock);
        insta::assert_snapshot!("checked", render(&result, &grid, 8, 3).backend());
    }

    #[test]
    fn a_negative_number_is_in_the_failure_colour() {
        insta::assert_snapshot!("negative", coloured(ColumnKind::Int, vec![Value::Int(-12), Value::Int(7)], 8));
    }

    #[test]
    fn a_uuid_dims_all_but_its_first_eight_digits() {
        let id = uuid::Uuid::parse_str("1b4e28ba-2fa1-11d2-883f-0016d3cca427").unwrap();
        insta::assert_snapshot!("uuid", coloured(ColumnKind::Uuid, vec![Value::Uuid(id)], 40));
    }

    #[test]
    fn a_json_value_colours_its_keys_strings_and_numbers() {
        let json = JsonText::of(&serde_json::json!({"id": 7, "status": "paid"}));
        insta::assert_snapshot!("json", coloured(ColumnKind::Json, vec![Value::Json(json)], 30));
    }

    #[test]
    fn a_timestamp_dims_its_time_after_its_date() {
        insta::assert_snapshot!("timestamp", coloured(ColumnKind::TimestampTz, vec![Value::TimestampTz(at("2026-07-14T09:30:00Z"))], 30));
    }

    #[test]
    fn a_primary_key_header_is_bold_and_a_foreign_key_header_points_on() {
        let result = ResultSet::whole(
            vec![
                shop_column("orders", "id", ColumnKind::Int),
                shop_column("orders", "customer_id", ColumnKind::Int),
                shop_column("orders", "total", ColumnKind::Numeric),
            ],
            vec![vec![Value::Int(7), Value::Int(3), Value::Numeric("12.50".into())]],
        );
        let keys = crate::keys::column_keys(&crate::fixtures::shop_catalog(), result.columns());
        let clock = Clock { now: at("2026-07-14T12:00:00Z"), zone: chrono::Local };
        let mut grid = Grid::new(&result, vec![Format::Raw; 3], keys, &clock);
        grid.column = usize::MAX;
        insta::assert_snapshot!("keys", styled::runs(render(&result, &grid, 30, 2).backend().buffer()));
    }

    #[test]
    fn a_control_character_in_a_cell_is_drawn_escaped() {
        let result = orders().with_cell(0, 2, Value::Text("nina\u{1b}]0;owned\u{7}".into()));
        insta::assert_snapshot!("escaped", render(&result, &Grid::raw(&result), 50, 2).backend());
    }
}
