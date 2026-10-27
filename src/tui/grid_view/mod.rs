//! The results grid: the header, then only the rows and columns in view, numbers right-aligned, `▶` on the cursor's row
//! and its cell lit.
use super::app::{GAP, Grid, Room};
use crate::db::model::{Column, ColumnKind, ResultSet, Value};
use crate::render;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use vtui::text::truncate;
use vtui::theme::Theme;

const CURSOR: &str = "▶";
const NO_CURSOR: &str = " ";

/// What the grid is drawn from.
#[derive(Clone, Copy, Debug)]
pub(super) struct GridView<'a> {
    pub(super) result: &'a ResultSet,
    pub(super) grid: &'a Grid,
    pub(super) theme: &'a Theme,
    pub(super) focused: bool,
}

/// A column in view: its index in the result and the terminal columns it is drawn in.
#[derive(Clone, Copy, Debug)]
struct Shown {
    index: usize,
    width: usize,
}

/// The columns from `first` on that fit in `room`, each after its gap; the last one is cut to what is left.
fn in_view(widths: &[usize], first: usize, room: usize) -> Vec<Shown> {
    let mut left = room;
    let mut shown = vec![];
    for (index, &width) in widths.iter().enumerate().skip(first) {
        if left <= GAP.len() {
            break;
        }
        left -= GAP.len();
        let drawn = width.min(left);
        shown.push(Shown { index, width: drawn });
        left -= drawn;
    }
    shown
}

/// The grid in `area`; returns the room it had under its header and beside the cursor's mark.
pub(super) fn draw(f: &mut Frame<'_>, view: GridView<'_>, area: Rect) -> Room {
    let room = usize::from(area.width).saturating_sub(CURSOR.chars().count());
    let shown = in_view(&view.grid.widths, view.grid.left, room);
    let body_rows = usize::from(area.height).saturating_sub(1);
    let rows = view.result.rows().iter().enumerate().skip(view.grid.top).take(body_rows);
    let lines: Vec<Line<'_>> =
        std::iter::once(header(view, &shown)).chain(rows.map(|(index, row)| row_line(view, &shown, index, row))).collect();
    f.render_widget(Paragraph::new(lines), area);
    Room { rows: body_rows, columns: room }
}

fn header<'a>(view: GridView<'a>, shown: &[Shown]) -> Line<'a> {
    let style = Style::default().add_modifier(Modifier::BOLD);
    let cursor_style = style.fg(cursor_colour(view));
    let cells = shown.iter().map(|column| {
        let name = &view.result.columns()[column.index];
        let style = if column.index == view.grid.column { cursor_style } else { style };
        (aligned(&render::printable(&name.name), name, column.width), style)
    });
    line(Span::raw(NO_CURSOR), cells.collect())
}

fn row_line<'a>(view: GridView<'a>, shown: &[Shown], index: usize, row: &[Value]) -> Line<'a> {
    let is_current = index == view.grid.row;
    let cells = shown.iter().map(|column| {
        let is_cursor = is_current && column.index == view.grid.column;
        let style = if is_cursor { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
        (aligned(&cell(&row[column.index], column.width), &view.result.columns()[column.index], column.width), style)
    });
    let marker = Span::styled(if is_current { CURSOR } else { NO_CURSOR }, Style::default().fg(cursor_colour(view)));
    line(marker, cells.collect())
}

fn cursor_colour(view: GridView<'_>) -> ratatui::style::Color {
    if view.focused { view.theme.accent } else { view.theme.faded }
}

fn line(marker: Span<'static>, cells: Vec<(String, Style)>) -> Line<'static> {
    let spans = cells.into_iter().flat_map(|(text, style)| [Span::raw(GAP), Span::styled(text, style)]);
    Line::from(std::iter::once(marker).chain(spans).collect::<Vec<_>>())
}

/// Enough of `value` to fill `width` and show that it goes on.
fn cell(value: &Value, width: usize) -> String {
    render::cell_text(value, width + 1)
}

/// `text` cut to `width` and padded to it: numbers on the right, so their digits line up, the rest on the left.
fn aligned(text: &str, column: &Column, width: usize) -> String {
    let cut = truncate(text, width);
    let padding = " ".repeat(width.saturating_sub(Span::raw(cut.as_str()).width()));
    match column.kind {
        ColumnKind::Int | ColumnKind::Float | ColumnKind::Numeric => padding + &cut,
        _ => cut + &padding,
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::Grid;
    use super::super::app::tests::orders;
    use super::super::styled;
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(result: &ResultSet, grid: &Grid, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let view = GridView { result, grid, theme: &Theme::default(), focused: true };
        terminal.draw(|f| _ = draw(f, view, f.area())).unwrap();
        terminal
    }

    #[test]
    fn numbers_align_right_and_the_cursor_row_is_marked() {
        let result = orders();
        let mut grid = Grid::new(&result);
        grid.row = 1;
        insta::assert_snapshot!("aligned", render(&result, &grid, 40, 6).backend());
    }

    #[test]
    fn only_the_rows_in_view_are_drawn() {
        let result = orders();
        let mut grid = Grid::new(&result);
        (grid.row, grid.top) = (3, 2);
        insta::assert_snapshot!("scrolled_down", render(&result, &grid, 40, 3).backend());
    }

    #[test]
    fn a_scrolled_grid_starts_at_its_first_column_in_view() {
        let result = orders();
        let mut grid = Grid::new(&result);
        (grid.column, grid.left) = (1, 1);
        insta::assert_snapshot!("scrolled_right", render(&result, &grid, 40, 3).backend());
    }

    #[test]
    fn the_cursors_cell_is_lit_and_its_header_coloured() {
        let result = orders();
        let mut grid = Grid::new(&result);
        (grid.row, grid.column) = (1, 1);
        insta::assert_snapshot!("cursor_styled", styled::runs(render(&result, &grid, 30, 3).backend().buffer()));
    }

    #[test]
    fn a_narrow_grid_cuts_its_last_column() {
        let result = orders();
        insta::assert_snapshot!("narrow", render(&result, &Grid::new(&result), 22, 5).backend());
    }

    #[test]
    fn a_control_character_in_a_cell_is_drawn_escaped() {
        let result = orders().with_cell(0, 2, Value::Text("nina\u{1b}]0;owned\u{7}".into()));
        insta::assert_snapshot!("escaped", render(&result, &Grid::new(&result), 50, 2).backend());
    }
}
