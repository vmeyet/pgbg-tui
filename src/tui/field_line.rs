//! A `Field` typed on one row of the screen, with the terminal's cursor where the next character goes.
use crate::render;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use vtui::field::Field;

/// What keeps the text from touching what follows it, the cursor at its end taking the first column.
const GAP: &str = "  ";

/// `label`, the text of `field` then `after` in `area`, scrolled sideways only as far as the cursor needs, so what follows
/// the field is pushed off first.
pub(super) fn draw(f: &mut Frame<'_>, label: Span<'static>, field: &Field, after: impl IntoIterator<Item = Span<'static>>, area: Rect) {
    let cursor = label.width() + before_cursor_width(field);
    let text = Span::raw(render::printable(field.text()).into_owned());
    let spans = [label, text, Span::raw(GAP)].into_iter().chain(after);
    draw_scrolled(f, Line::from_iter(spans), 0, cursor, area);
}

/// `line` in `area`, from about column `scroll` or further when the cursor needs it, with the terminal's cursor on its
/// column `cursor`.
pub(super) fn draw_scrolled(f: &mut Frame<'_>, line: Line<'static>, scroll: usize, cursor: usize, area: Rect) {
    let shows_cursor = (cursor + 1).saturating_sub(usize::from(area.width));
    let scroll = first_whole_column(&line, scroll.max(shows_cursor));
    f.render_widget(Paragraph::new(line).scroll((0, to_cells(scroll))), area);
    let x = area.x.saturating_add(to_cells(cursor.saturating_sub(scroll))).min(area.right().saturating_sub(1));
    f.set_cursor_position(Position { x, y: area.y });
}

/// The columns the text before the cursor of `field` takes, as drawn.
pub(super) fn before_cursor_width(field: &Field) -> usize {
    let (before, _, _) = field.split();
    render::width(&render::printable(before))
}

/// The first column at or past `wanted` where a character of `line` starts: a `Paragraph` scrolled into the middle of a
/// wide character draws it whole, which would move all that follows one column away from where the cursor counts it.
fn first_whole_column(line: &Line<'_>, wanted: usize) -> usize {
    let mut column = 0;
    for grapheme in line.styled_graphemes(Style::default()) {
        if column >= wanted {
            break;
        }
        column += render::width(grapheme.symbol);
    }
    column
}

fn to_cells(columns: usize) -> u16 {
    u16::try_from(columns).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const WIDTH: u16 = 10;

    fn drawn(label: &'static str, field: &Field, after: &'static str) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, 1)).unwrap();
        terminal.draw(|f| draw(f, Span::raw(label), field, [Span::raw(after)], f.area())).unwrap();
        terminal
    }

    fn cursor_column(terminal: &Terminal<TestBackend>) -> u16 {
        terminal.backend().cursor_position().x
    }

    fn row(terminal: &Terminal<TestBackend>) -> String {
        terminal.backend().buffer().content().iter().map(ratatui::buffer::Cell::symbol).collect()
    }

    #[test]
    fn what_follows_a_short_field_shows_after_a_gap() {
        assert_eq!(row(&drawn("/", &Field::new("ab"), "keys")), "/ab  keys ");
    }

    #[test]
    fn the_cursor_sits_after_the_text_of_a_field() {
        assert_eq!(cursor_column(&drawn("/", &Field::new("ab"), "")), 3);
    }

    #[test]
    fn the_cursor_sits_in_the_middle_of_a_field_it_moved_into() {
        let mut field = Field::new("ab");
        field.left();
        assert_eq!(cursor_column(&drawn("/", &field, "")), 2);
    }

    #[test]
    fn a_wide_character_before_the_cursor_counts_for_two_columns() {
        let mut field = Field::new("日本");
        field.left();
        assert_eq!(cursor_column(&drawn("/", &field, "")), 3);
    }

    #[test]
    fn the_start_of_a_field_past_the_row_shows_from_its_label() {
        let mut field = Field::new("abcdefghijkl");
        field.start();
        let terminal = drawn("/", &field, "");
        assert_eq!((cursor_column(&terminal), row(&terminal)), (1, "/abcdefghi".to_owned()));
    }

    #[test]
    fn a_field_past_the_row_keeps_the_cursor_on_the_last_column() {
        let terminal = drawn("/", &Field::new("abcdefghijkl"), "keys");
        assert_eq!(cursor_column(&terminal), WIDTH - 1);
        assert_eq!(row(&terminal), "defghijkl ");
    }

    #[test]
    fn a_cursor_at_the_end_of_a_field_that_just_fits_needs_no_scroll() {
        let terminal = drawn("/", &Field::new("abcdefgh"), "");
        assert_eq!((cursor_column(&terminal), row(&terminal)), (WIDTH - 1, "/abcdefgh ".to_owned()));
    }

    #[test]
    fn a_wide_character_cut_by_the_scroll_goes_whole_so_the_cursor_stays_on_its_column() {
        let terminal = drawn("/", &Field::new("日本語の本日"), "");
        let cursor = cursor_column(&terminal);
        assert_eq!(terminal.backend().buffer()[(cursor - 1, 0)].symbol(), " ");
        assert_eq!(terminal.backend().buffer()[(cursor - 2, 0)].symbol(), "日");
    }

    #[test]
    fn the_cursor_before_a_wide_character_on_a_scrolled_row_sits_on_it() {
        let mut field = Field::new("日本語の本日");
        field.left();
        let terminal = drawn("/", &field, "");
        let cursor = cursor_column(&terminal);
        assert_eq!(terminal.backend().buffer()[(cursor, 0)].symbol(), "日");
    }

    #[test]
    fn the_width_before_the_cursor_counts_what_printable_draws() {
        assert_eq!(before_cursor_width(&Field::new("a\u{202e}")), 9);
    }
}
