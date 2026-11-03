//! How the editor's text falls into rows of a pane, for display only: the text itself never holds a break.
//! A line is cut at the last grapheme that fits, never at a word: a word break would depend on what follows, so the
//! ghost or the next key could move the cursor.
//! A line that fills its last row exactly gets one more, empty, for the cursor after its last character.
use crate::render;
use ratatui::layout::Size;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation as _;

/// One row of the text as drawn: the line, and the row within that line, both from 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::tui) struct Row {
    pub(in crate::tui) line: usize,
    pub(in crate::tui) row: usize,
}

/// Where a byte of the text is drawn: its row, and the cells before it on that row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spot {
    row: Row,
    column: usize,
}

/// The cell of a pane something is drawn in, from its corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct InView {
    pub(in crate::tui) row: usize,
    pub(in crate::tui) column: usize,
}

/// The cells `grapheme` takes when drawn; ratatui draws nothing for a control character.
fn cells(grapheme: &str) -> usize {
    if grapheme.contains(char::is_control) { 0 } else { render::width(grapheme) }
}

/// The cells `row` takes when drawn.
fn cells_of(row: &str) -> usize {
    row.graphemes(true).map(cells).sum()
}

/// `line` cut where a pane `columns` wide ends; a pane not drawn yet, 0 wide, cuts nothing.
/// No grapheme takes more cells than bytes, and a line of printable ASCII takes one cell a byte, so most lines need no
/// walk through their graphemes.
pub(in crate::tui) fn cuts_of(line: &str, columns: usize) -> Vec<Range<usize>> {
    if columns == 0 || line.len() <= columns {
        return std::iter::once(0..line.len()).collect();
    }
    if line.bytes().all(|byte| matches!(byte, b' '..=b'~')) {
        return (0..line.len()).step_by(columns).map(|start| start..line.len().min(start + columns)).collect();
    }
    let mut rows = Vec::new();
    let (mut start, mut used) = (0, 0);
    for (at, grapheme) in line.grapheme_indices(true) {
        let width = cells(grapheme);
        if used + width > columns && at > start {
            rows.push(start..at);
            (start, used) = (at, 0);
        }
        used += width;
    }
    rows.push(start..line.len());
    rows
}

/// The bytes of a line of the text on each of its rows, with the empty row after a full last one.
pub(in crate::tui) fn rows_of(line: &str, columns: usize) -> Vec<Range<usize>> {
    let mut rows = cuts_of(line, columns);
    if let Some(last) = rows.last()
        && columns > 0
        && cells_of(&line[last.clone()]) >= columns
    {
        rows.push(line.len()..line.len());
    }
    rows
}

/// Where byte `at` of `text` is drawn; at the end of a full row, that is the start of the next.
fn spot_of(text: &str, at: usize, columns: usize) -> Spot {
    let before = &text[..at];
    let line = before.matches('\n').count();
    let start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let end = text[at..].find('\n').map_or(text.len(), |newline| at + newline);
    let rows = rows_of(&text[start..end], columns);
    let row = rows.iter().rposition(|row| row.start <= at - start).unwrap_or(0);
    Spot { row: Row { line, row }, column: cells_of(&text[start + rows[row].start..at]) }
}

/// The highest first row of a pane of `size` that still shows `target`, the row of byte `at` of `text`.
fn first_row_showing(text: &str, at: usize, target: Row, size: Size) -> Row {
    let above = usize::from(size.height).saturating_sub(1);
    if above <= target.row {
        return Row { row: target.row - above, ..target };
    }
    let mut left = above - target.row;
    for (index, earlier) in text[..at].rsplit('\n').skip(1).enumerate() {
        let count = rows_of(earlier, usize::from(size.width)).len();
        if left <= count {
            return Row { line: target.line - 1 - index, row: count - left };
        }
        left -= count;
    }
    Row::default()
}

/// `first` moved only as far as a pane of `size` needs to show byte `at` of `text`, and to a row its line still has.
pub(super) fn first_row_for(text: &str, first: Row, at: usize, size: Size) -> Row {
    let columns = usize::from(size.width);
    let target = spot_of(text, at, columns).row;
    let Row { line, row } = first.clamp(first_row_showing(text, at, target, size), target);
    if row == 0 || line == target.line {
        return Row { line, row };
    }
    let last_row = text.split('\n').nth(line).map_or(0, |content| rows_of(content, columns).len() - 1);
    Row { line, row: row.min(last_row) }
}

/// Where byte `at` of `text` is drawn in a pane of `size` whose first row is `first`; `None` when it is out of view.
pub(super) fn in_view(text: &str, first: Row, at: usize, size: Size) -> Option<InView> {
    let columns = usize::from(size.width);
    let spot = spot_of(text, at, columns);
    if spot.row < first {
        return None;
    }
    let lines_above = text[..at].rsplit('\n').skip(1).take(spot.row.line - first.line);
    let rows_above: usize = lines_above.map(|line| rows_of(line, columns).len()).sum();
    let row = rows_above + spot.row.row - first.row;
    (row < usize::from(size.height)).then_some(InView { row, column: spot.column })
}

/// The byte drawn at `column` of row `row` of `line`: the grapheme whose cells hold it, else the end of a line's last
/// row, or the last grapheme of a row that goes on below, since its end is drawn on the next row.
fn byte_in_row(line: &str, rows: &[Range<usize>], row: usize, column: usize) -> usize {
    let bytes = rows[row].clone();
    let mut used = 0;
    let mut last = bytes.start;
    for (at, grapheme) in line[bytes.clone()].grapheme_indices(true) {
        used += cells(grapheme);
        if used > column {
            return bytes.start + at;
        }
        last = bytes.start + at;
    }
    if row + 1 < rows.len() { last } else { bytes.end }
}

/// The byte of `text` drawn `down` rows under `first`, at `column`; `None` below the last row.
pub(super) fn byte_under(text: &str, first: Row, down: usize, column: usize, columns: usize) -> Option<usize> {
    let mut left = first.row + down;
    let mut start: usize = text.split('\n').take(first.line).map(|line| line.len() + 1).sum();
    for line in text.get(start..)?.split('\n') {
        let rows = rows_of(line, columns);
        if left < rows.len() {
            return Some(start + byte_in_row(line, &rows, left, column));
        }
        left -= rows.len();
        start += line.len() + 1;
    }
    None
}

/// The byte of `text` drawn on the row above byte `at`, at its column; `at` itself on the first row.
pub(super) fn byte_above(text: &str, at: usize, columns: usize) -> usize {
    let Spot { row, column } = spot_of(text, at, columns);
    if row.row > 0 {
        return byte_under(text, Row { row: row.row - 1, ..row }, 0, column, columns).unwrap_or(at);
    }
    let Some(line_above) = text[..at].rsplit('\n').nth(1) else { return at };
    let last_row = Row { line: row.line - 1, row: rows_of(line_above, columns).len() - 1 };
    byte_under(text, last_row, 0, column, columns).unwrap_or(at)
}

/// The byte of `text` drawn on the row below byte `at`, at its column; `at` itself on the last row.
pub(super) fn byte_below(text: &str, at: usize, columns: usize) -> usize {
    let Spot { row, column } = spot_of(text, at, columns);
    byte_under(text, row, 1, column, columns).unwrap_or(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANE: Size = Size { width: 4, height: 3 };

    fn cut(line: &str, columns: usize) -> Vec<&str> {
        rows_of(line, columns).into_iter().map(|row| &line[row]).collect()
    }

    fn cut_alone(line: &str, columns: usize) -> Vec<&str> {
        cuts_of(line, columns).into_iter().map(|row| &line[row]).collect()
    }

    fn row(line: usize, row: usize) -> Row {
        Row { line, row }
    }

    #[test]
    fn a_line_that_fits_is_one_row() {
        assert_eq!(cut("orders", 8), ["orders"]);
    }

    #[test]
    fn an_empty_line_is_one_empty_row() {
        assert_eq!(cut("", 8), [""]);
    }

    #[test]
    fn a_pane_not_drawn_yet_cuts_nothing() {
        assert_eq!(cut("orders", 0), ["orders"]);
    }

    #[test]
    fn a_line_wider_than_the_pane_is_cut_where_the_pane_ends() {
        assert_eq!(cut("customers", 4), ["cust", "omer", "s"]);
    }

    #[test]
    fn a_cut_adds_no_empty_row_after_a_full_last_one() {
        assert_eq!(cut_alone("orders", 6), ["orders"]);
        assert_eq!(cut_alone("abc日本", 4), ["abc", "日本"]);
    }

    #[test]
    fn a_line_with_a_control_character_is_not_cut_by_its_bytes() {
        assert_eq!(cut_alone("a\tbcdef", 4), ["a\tbcd", "ef"]);
    }

    #[test]
    fn a_line_that_fills_its_last_row_gets_an_empty_row_for_the_cursor() {
        assert_eq!(cut("orders", 6), ["orders", ""]);
    }

    #[test]
    fn a_wide_character_that_does_not_fit_starts_the_next_row() {
        assert_eq!(cut("abc日本", 4), ["abc", "日本", ""]);
    }

    #[test]
    fn a_combining_mark_stays_with_its_letter_on_a_full_row() {
        assert_eq!(cut("Zoe\u{308}x", 3), ["Zoe\u{308}", "x"]);
    }

    #[test]
    fn a_character_wider_than_the_pane_still_takes_a_row() {
        assert_eq!(cut("日a", 1), ["日", "a", ""]);
    }

    #[test]
    fn a_control_character_takes_no_cell_as_it_draws_none() {
        assert_eq!(cut("a\tbcd", 4), ["a\tbcd", ""]);
    }

    #[test]
    fn the_end_of_a_full_row_is_drawn_at_the_start_of_the_next() {
        assert_eq!(spot_of("customers", 4, 4), Spot { row: row(0, 1), column: 0 });
    }

    #[test]
    fn a_byte_counts_the_cells_of_wide_characters_before_it_on_its_row() {
        assert_eq!(spot_of("id\nabc日本x", 9, 4), Spot { row: row(1, 1), column: 2 });
    }

    #[test]
    fn a_spot_counts_decomposed_letters_as_they_draw() {
        assert_eq!(spot_of("Zoe\u{308}x", 5, 8), Spot { row: row(0, 0), column: 3 });
    }

    #[test]
    fn the_first_row_moves_up_to_a_byte_above_the_view() {
        assert_eq!(first_row_for("customers\nid", row(1, 0), 5, PANE), row(0, 1));
    }

    #[test]
    fn the_first_row_moves_down_only_as_far_as_a_byte_below_the_view_needs() {
        assert_eq!(first_row_for("id\ncustomers", row(0, 0), 12, PANE), row(1, 0));
    }

    #[test]
    fn the_first_row_stays_while_the_byte_shows() {
        assert_eq!(first_row_for("customers\nid", row(0, 1), 11, PANE), row(0, 1));
    }

    #[test]
    fn the_first_row_stops_at_the_top_of_the_text() {
        assert_eq!(first_row_for("id", row(0, 0), 2, PANE), row(0, 0));
    }

    #[test]
    fn a_first_row_a_wider_pane_no_longer_has_moves_to_the_lines_last() {
        let wider = Size { width: 40, height: 3 };
        assert_eq!(first_row_for("customers\nid", row(0, 2), 11, wider), row(0, 0));
    }

    #[test]
    fn a_byte_shows_under_the_rows_of_the_lines_above_it() {
        assert_eq!(in_view("customers\nid", row(0, 1), 11, PANE), Some(InView { row: 2, column: 1 }));
    }

    #[test]
    fn a_byte_above_or_below_the_view_is_not_in_it() {
        let text = "customers\nid";
        assert_eq!(in_view(text, row(0, 1), 0, PANE), None);
        assert_eq!(in_view(text, row(0, 0), 11, PANE), None);
    }

    #[test]
    fn a_cell_finds_its_byte_through_the_rows_above_it() {
        assert_eq!(byte_under("customers\nid", row(0, 1), 1, 0, 4), Some(8));
        assert_eq!(byte_under("customers\nid", row(0, 1), 2, 1, 4), Some(11));
    }

    #[test]
    fn a_cell_past_a_last_row_finds_the_end_of_the_line() {
        assert_eq!(byte_under("customers\nid", row(0, 0), 2, 3, 4), Some(9));
    }

    #[test]
    fn a_cell_below_the_last_row_finds_nothing() {
        assert_eq!(byte_under("id", row(0, 0), 5, 0, 4), None);
    }

    #[test]
    fn a_cell_in_the_gap_a_wide_character_left_stays_on_its_row() {
        assert_eq!(byte_under("abc日", row(0, 0), 0, 3, 4), Some(2));
    }

    #[test]
    fn either_cell_of_a_wide_character_finds_it() {
        assert_eq!(byte_under("日本", row(0, 0), 0, 3, 8), Some(3));
    }

    #[test]
    fn up_goes_to_the_row_above_within_a_line() {
        assert_eq!(byte_above("customers", 5, 4), 1);
    }

    #[test]
    fn up_goes_to_the_last_row_of_the_line_above() {
        assert_eq!(byte_above("customers\nid", 10, 4), 8);
    }

    #[test]
    fn up_on_the_first_row_stays() {
        assert_eq!(byte_above("customers", 2, 4), 2);
    }

    #[test]
    fn up_lands_on_the_cell_above_past_wide_characters() {
        assert_eq!(byte_above("日本語abc", 12, 4), 6);
    }

    #[test]
    fn down_goes_to_the_row_below_within_a_line() {
        assert_eq!(byte_below("customers", 1, 4), 5);
    }

    #[test]
    fn down_goes_to_the_first_row_of_the_line_below() {
        assert_eq!(byte_below("customers\nid", 8, 4), 10);
    }

    #[test]
    fn down_on_the_last_row_stays() {
        assert_eq!(byte_below("customers", 8, 4), 8);
    }

    #[test]
    fn down_keeps_to_the_end_of_a_shorter_last_row() {
        assert_eq!(byte_below("customers", 7, 4), 9);
    }

    #[test]
    fn down_reaches_the_empty_row_of_a_full_line() {
        assert_eq!(byte_below("orders", 2, 6), 6);
    }

    #[test]
    fn a_pane_not_drawn_yet_moves_by_lines() {
        assert_eq!(byte_above("id\ncustomers", 12, 0), 2);
    }
}
