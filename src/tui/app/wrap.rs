//! How the editor's text falls into rows of a pane, for display only: the text itself never holds a break.
//! A row ends after its last whitespace, which stays on it and takes its cell, so every byte has a cell; a word wider
//! than the pane is cut at the last grapheme that fits.
//! The rows come from the line's text alone, and the cursor is drawn at its byte: typing can move a word and the cursor
//! in it down, but the ghost or a mark's space never moves what comes before the cursor.
//! A line that fills its last row exactly gets one more, empty, for the cursor after its last character.
use crate::render;
use ratatui::layout::Size;
use std::borrow::Cow;
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

/// The rest of a name or a statement, drawn faded at byte `at` of the text, where the cursor is: its first line after
/// the cursor, the others under the cursor's line.
#[derive(Clone, Copy)]
pub(in crate::tui) struct Ghost<'a> {
    pub(in crate::tui) at: usize,
    pub(in crate::tui) rest: &'a str,
}

impl<'a> Ghost<'a> {
    /// The ghost of its first line only, drawn after the cursor, and the lines after it.
    pub(in crate::tui) fn first_line(self) -> (Self, Vec<&'a str>) {
        let mut lines = self.rest.split('\n');
        let first = lines.next().unwrap_or_default();
        (Self { rest: first, ..self }, lines.collect())
    }

    /// The ghost with `at` counted from the line starting at byte `line_start`; `None` when it is on another line.
    pub(in crate::tui) fn within_line(self, line_start: usize, line_len: usize) -> Option<Self> {
        let at = self.at.checked_sub(line_start).filter(|&at| at <= line_len)?;
        Some(Self { at, ..self })
    }

    /// The text as drawn, its control characters escaped.
    pub(in crate::tui) fn shown(self) -> Cow<'a, str> {
        render::printable(self.rest)
    }
}

/// The cells `grapheme` takes when drawn; ratatui draws nothing for a control character.
fn cells(grapheme: &str) -> usize {
    if grapheme.contains(char::is_control) { 0 } else { render::width(grapheme) }
}

/// The cells `row` takes when drawn.
fn cells_of(row: &str) -> usize {
    row.graphemes(true).map(cells).sum()
}

/// One grapheme to lay out: its bytes, its cells and whether a row can end after it.
struct Piece {
    bytes: Range<usize>,
    width: usize,
    is_space: bool,
}

/// Where a row can end after a whitespace: the byte after it, and the cells of the row up to there.
struct AfterSpace {
    end: usize,
    cells: usize,
}

/// `pieces`, which make a line of `len` bytes, cut into rows `columns` wide: after the last whitespace of a row that ends
/// past byte `keep_to`, else where the pane ends.
fn flowed(pieces: impl Iterator<Item = Piece>, len: usize, columns: usize, keep_to: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let (mut start, mut used) = (0, 0);
    let mut after_space = None;
    for Piece { bytes, width, is_space } in pieces {
        if used + width > columns
            && let Some(AfterSpace { end, cells }) = after_space.take()
        {
            rows.push(start..end);
            (start, used) = (end, used - cells);
        }
        if used + width > columns && used > 0 {
            rows.push(start..bytes.start);
            (start, used) = (bytes.start, 0);
        }
        used += width;
        if is_space && used > 0 && bytes.end > keep_to {
            after_space = Some(AfterSpace { end: bytes.end, cells: used });
        }
    }
    rows.push(start..len);
    rows
}

fn ascii_pieces(line: &str) -> impl Iterator<Item = Piece> {
    line.bytes().enumerate().map(|(at, byte)| Piece { bytes: at..at + 1, width: 1, is_space: byte == b' ' })
}

fn grapheme_pieces(line: &str) -> impl Iterator<Item = Piece> {
    line.grapheme_indices(true).map(|(at, grapheme)| Piece {
        bytes: at..at + grapheme.len(),
        width: cells(grapheme),
        is_space: grapheme.chars().all(char::is_whitespace),
    })
}

/// `line` cut into rows `columns` wide, none ending at or before byte `keep_to` but where the pane ends; a pane not drawn
/// yet, 0 wide, cuts nothing.
/// No grapheme takes more cells than bytes, and a line of printable ASCII takes one cell a byte, so most lines need no
/// walk through their graphemes.
fn flow(line: &str, columns: usize, keep_to: usize) -> Vec<Range<usize>> {
    if columns == 0 || line.len() <= columns {
        return std::iter::once(0..line.len()).collect();
    }
    if line.bytes().all(|byte| matches!(byte, b' '..=b'~')) {
        return flowed(ascii_pieces(line), line.len(), columns, keep_to);
    }
    flowed(grapheme_pieces(line), line.len(), columns, keep_to)
}

/// `line` cut into rows `columns` wide, with no empty row after a full last one: for a text no cursor sits in.
pub(in crate::tui) fn cuts_of(line: &str, columns: usize) -> Vec<Range<usize>> {
    flow(line, columns, 0)
}

/// The bytes of a line of the text on each of its rows, with the empty row after a full last one.
fn rows_of(line: &str, columns: usize) -> Vec<Range<usize>> {
    let mut rows = cuts_of(line, columns);
    if let Some(last) = rows.last()
        && columns > 0
        && cells_of(&line[last.clone()]) >= columns
    {
        rows.push(line.len()..line.len());
    }
    rows
}

/// The row of `rows` that draws byte `at`: at the end of a row that goes on, that is the start of the next.
fn row_holding(rows: &[Range<usize>], at: usize) -> usize {
    rows.iter().rposition(|row| row.start <= at).unwrap_or(0)
}

/// The rows of `line` as drawn with no ghost, `painted_len` bytes long: a mark's space goes on the last row, which always
/// has room for it.
pub(in crate::tui) fn plain_rows(line: &str, painted_len: usize, columns: usize) -> Vec<Range<usize>> {
    let mut rows = rows_of(line, columns);
    if let Some(last) = rows.last_mut() {
        last.end = painted_len;
    }
    rows
}

/// The rows of `painted`, which is `line` as drawn with a ghost at byte `ghost_at`.
/// Everything up to the ghost stays on the row `line` alone gives it, so the ghost starts at the cursor whatever it holds,
/// and what follows it flows from there.
pub(in crate::tui) fn ghosted_rows(line: &str, painted: &str, ghost_at: usize, columns: usize) -> Vec<Range<usize>> {
    let rows = rows_of(line, columns);
    let held = row_holding(&rows, ghost_at);
    let start = rows[held].start;
    let flowing = flow(&painted[start..], columns, ghost_at - start);
    rows[..held].iter().cloned().chain(flowing.into_iter().map(|row| start + row.start..start + row.end)).collect()
}

/// Whether the end of a `ghost` that shows, with `at` counted in its line, lies at or past row `rows` of the `cuts` of
/// that line.
pub(in crate::tui) fn ghost_ends_past(cuts: &[Range<usize>], ghost: Ghost<'_>, rows: usize) -> bool {
    row_holding(cuts, ghost.at + ghost.shown().len() - 1) >= rows
}

/// Where byte `at` of `text` is drawn.
fn spot_of(text: &str, at: usize, columns: usize) -> Spot {
    let before = &text[..at];
    let line = before.matches('\n').count();
    let start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let end = text[at..].find('\n').map_or(text.len(), |newline| at + newline);
    let rows = rows_of(&text[start..end], columns);
    let row = row_holding(&rows, at - start);
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
/// row, or the last grapheme with cells of a row that goes on below: its end is drawn on the next row, and a `Place` cannot
/// name a grapheme with none.
fn byte_in_row(line: &str, rows: &[Range<usize>], row: usize, column: usize) -> usize {
    let bytes = rows[row].clone();
    let mut used = 0;
    let mut last = bytes.start;
    for (at, grapheme) in line[bytes.clone()].grapheme_indices(true) {
        let width = cells(grapheme);
        used += width;
        if used > column {
            return bytes.start + at;
        }
        if width > 0 {
            last = bytes.start + at;
        }
    }
    if row + 1 < rows.len() { last } else { bytes.end }
}

/// What a row counted down from the first row of a line finds there.
enum InLine {
    /// The byte of the line drawn at the cell asked for.
    Byte(usize),
    /// The row lies below the line, which takes this many rows.
    RowsTaken(usize),
}

/// What `line` draws at `column` of its row `row`, with no ghost.
fn under_in_line(line: &str, row: usize, column: usize, columns: usize) -> InLine {
    let rows = rows_of(line, columns);
    if row < rows.len() { InLine::Byte(byte_in_row(line, &rows, row, column)) } else { InLine::RowsTaken(rows.len()) }
}

/// What `line` draws at `column` of its row `row` under `ghost`, with `at` counted in the line; every cell of the ghost
/// finds the byte the ghost is drawn at.
fn under_in_ghosted_line(line: &str, ghost: Ghost<'_>, row: usize, column: usize, columns: usize) -> InLine {
    let (first, below) = ghost.first_line();
    let (at, shown) = (first.at, first.shown());
    let painted = format!("{}{shown}{}", &line[..at], &line[at..]);
    let rows = if shown.is_empty() { rows_of(line, columns) } else { ghosted_rows(line, &painted, at, columns) };
    if row < rows.len() {
        let drawn = byte_in_row(&painted, &rows, row, column);
        return InLine::Byte(if drawn < at { drawn } else { at.max(drawn.saturating_sub(shown.len())) });
    }
    let rows_below: usize = below.iter().map(|text| cuts_of(&render::printable(text), columns).len()).sum();
    let taken = rows.len() + rows_below;
    if row < taken { InLine::Byte(at) } else { InLine::RowsTaken(taken) }
}

/// The byte of `text` drawn `down` rows under `first`, at `column`, under `ghost` when one is drawn; `None` below the last
/// row.
pub(super) fn byte_drawn_under(
    text: &str,
    first: Row,
    down: usize,
    column: usize,
    columns: usize,
    ghost: Option<Ghost<'_>>,
) -> Option<usize> {
    let mut left = first.row + down;
    let mut start: usize = text.split('\n').take(first.line).map(|line| line.len() + 1).sum();
    for line in text.get(start..)?.split('\n') {
        let found = match ghost.and_then(|ghost| ghost.within_line(start, line.len())) {
            Some(ghost) => under_in_ghosted_line(line, ghost, left, column, columns),
            None => under_in_line(line, left, column, columns),
        };
        match found {
            InLine::Byte(at) => return Some(start + at),
            InLine::RowsTaken(rows) => left -= rows,
        }
        start += line.len() + 1;
    }
    None
}

/// The byte of `text` drawn `down` rows under `first`, at `column`, with no ghost; `None` below the last row.
fn byte_under(text: &str, first: Row, down: usize, column: usize, columns: usize) -> Option<usize> {
    byte_drawn_under(text, first, down, column, columns, None)
}

/// The cells before byte `at` of `text` on its row.
pub(super) fn column_of(text: &str, at: usize, columns: usize) -> usize {
    spot_of(text, at, columns).column
}

/// The byte of `text` drawn at `column` of the row above byte `at`; `None` on the first row.
pub(super) fn byte_above(text: &str, at: usize, column: usize, columns: usize) -> Option<usize> {
    let row = spot_of(text, at, columns).row;
    if row.row > 0 {
        return byte_under(text, Row { row: row.row - 1, ..row }, 0, column, columns);
    }
    let line_above = text[..at].rsplit('\n').nth(1)?;
    byte_under(text, Row { line: row.line - 1, row: rows_of(line_above, columns).len() - 1 }, 0, column, columns)
}

/// The byte of `text` drawn at `column` of the row below byte `at`; `None` on the last row.
pub(super) fn byte_below(text: &str, at: usize, column: usize, columns: usize) -> Option<usize> {
    byte_under(text, spot_of(text, at, columns).row, 1, column, columns)
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

    fn up(text: &str, at: usize, columns: usize) -> usize {
        byte_above(text, at, column_of(text, at, columns), columns).unwrap_or(at)
    }

    fn down(text: &str, at: usize, columns: usize) -> usize {
        byte_below(text, at, column_of(text, at, columns), columns).unwrap_or(at)
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
        assert_eq!(cut_alone("a\tbcdef", 4), ["a\t", "bcde", "f"]);
    }

    #[test]
    fn a_row_ends_after_its_last_whitespace_so_a_word_moves_whole() {
        assert_eq!(cut("FROM orders", 8), ["FROM ", "orders"]);
    }

    #[test]
    fn a_word_wider_than_the_pane_is_cut_where_the_pane_ends() {
        assert_eq!(cut("FROM customers_total", 8), ["FROM ", "customer", "s_total"]);
    }

    #[test]
    fn a_word_that_ends_at_the_pane_moves_down_so_its_whitespace_has_a_cell() {
        assert_eq!(cut("FROM id x", 7), ["FROM ", "id x"]);
    }

    #[test]
    fn a_whitespace_with_no_room_starts_the_next_row() {
        assert_eq!(cut("ABCDE FROM", 5), ["ABCDE", " FROM", ""]);
    }

    #[test]
    fn a_wide_word_moves_whole_past_the_whitespace() {
        assert_eq!(cut("日本 語語語", 6), ["日本 ", "語語語", ""]);
    }

    #[test]
    fn a_wide_character_after_a_moved_word_still_fits_or_starts_a_row() {
        assert_eq!(cut(" abc日", 4), [" ", "abc", "日"]);
    }

    #[test]
    fn no_row_holds_only_a_character_with_no_cell() {
        assert_eq!(cut("\t日a", 1), ["\t日", "a", ""]);
    }

    #[test]
    fn a_cell_past_a_row_that_ends_on_a_control_character_finds_its_last_character_with_cells() {
        assert_eq!(byte_under("a\tbcdef", row(0, 0), 0, 3, 4), Some(0));
    }

    #[test]
    fn nothing_is_above_the_first_row_or_below_the_last() {
        assert_eq!(byte_above("customers", 2, 2, 4), None);
        assert_eq!(byte_below("customers", 8, 0, 4), None);
    }

    #[test]
    fn a_column_past_a_shorter_row_finds_the_column_on_the_next_one() {
        assert_eq!(byte_above("abcdefgh\nid", 11, 6, 20), Some(6));
    }

    const GHOST: Option<Ghost<'static>> = Some(Ghost { at: 7, rest: "stomers\nLIMIT 1" });

    fn drawn_under(text: &str, down: usize, column: usize) -> Option<usize> {
        byte_drawn_under(text, row(0, 0), down, column, 10, GHOST)
    }

    #[test]
    fn a_cell_of_the_ghost_finds_the_byte_it_is_drawn_at() {
        assert_eq!(drawn_under("FROM cu\nid", 0, 8), Some(7));
        assert_eq!(drawn_under("FROM cu\nid", 1, 0), Some(7));
        assert_eq!(drawn_under("FROM cu\nid", 2, 3), Some(7));
    }

    #[test]
    fn a_cell_before_the_ghost_finds_its_byte() {
        assert_eq!(drawn_under("FROM cu\nid", 0, 3), Some(3));
    }

    #[test]
    fn a_cell_under_the_ghost_counts_its_rows() {
        assert_eq!(drawn_under("FROM cu\nid", 3, 1), Some(9));
    }

    #[test]
    fn a_cell_past_the_end_of_a_row_under_the_ghost_finds_the_end_of_the_line() {
        assert_eq!(drawn_under("FROM cu\nid", 3, 7), Some(10));
    }

    #[test]
    fn a_cell_below_the_ghost_and_the_text_finds_nothing() {
        assert_eq!(drawn_under("FROM cu\nid", 4, 0), None);
    }

    #[test]
    fn a_cell_after_a_ghost_in_a_line_finds_its_byte_in_the_text() {
        let ghost = Some(Ghost { at: 2, rest: "xyz" });
        assert_eq!(byte_drawn_under("id orders", row(0, 0), 0, 6, 20, ghost), Some(3));
    }

    #[test]
    fn a_ghost_on_a_line_above_the_first_row_changes_nothing() {
        let ghost = Some(Ghost { at: 1, rest: "xyz" });
        assert_eq!(byte_drawn_under("a\nid", row(1, 0), 0, 1, 20, ghost), Some(3));
    }

    #[test]
    fn a_ghost_ends_on_the_row_that_holds_its_last_byte() {
        let cuts = [0..10, 10..14];
        let ghost = Ghost { at: 7, rest: "stomers" };
        assert!(!ghost_ends_past(&cuts, ghost, 2));
        assert!(ghost_ends_past(&cuts, ghost, 1));
    }

    #[test]
    fn a_control_character_alone_is_no_place_to_end_a_row() {
        assert_eq!(cut("\tabcde", 4), ["\tabcd", "e"]);
    }

    #[test]
    fn the_rows_hold_every_byte_of_the_line_in_order() {
        for line in ["FROM orders WHERE id = 7", "  a   b  ", "日本 語語語 x", "Zoe\u{308} nina", "a\tb c", "customers", ""] {
            for columns in 1..12 {
                let rows = rows_of(line, columns);
                let joined: String = rows.iter().map(|row| &line[row.clone()]).collect();
                assert_eq!(joined, line, "{line:?} at {columns}");
                assert!(rows.windows(2).all(|pair| pair[0].end == pair[1].start), "{line:?} at {columns}");
            }
        }
    }

    #[test]
    fn a_printable_ascii_line_is_cut_as_its_graphemes_would_be() {
        for line in ["SELECT id FROM orders WHERE total > 10", "  a   b  ", "customers_total", "a b c d e f g"] {
            for columns in 1..12 {
                let by_bytes = flowed(ascii_pieces(line), line.len(), columns, 0);
                let by_graphemes = flowed(grapheme_pieces(line), line.len(), columns, 0);
                assert_eq!(by_bytes, by_graphemes, "{line:?} at {columns}");
            }
        }
    }

    fn painted(line: &str, painted: &str, ghost_at: Option<usize>, columns: usize) -> Vec<String> {
        let rows = match ghost_at {
            Some(at) => ghosted_rows(line, painted, at, columns),
            None => plain_rows(line, painted.len(), columns),
        };
        rows.into_iter().map(|row| painted[row].to_owned()).collect()
    }

    #[test]
    fn a_ghost_never_moves_the_word_it_completes_to_another_row() {
        assert_eq!(painted("FROM cu", "FROM customers", Some(7), 10), ["FROM custo", "mers"]);
    }

    #[test]
    fn a_whitespace_in_a_ghost_can_end_the_row_past_the_cursor() {
        assert_eq!(painted("FROM ", "FROM id AND nina", Some(5), 10), ["FROM id ", "AND nina"]);
    }

    #[test]
    fn a_ghost_flows_from_the_row_the_text_alone_gives_the_cursor() {
        assert_eq!(painted("FROM custom", "FROM cuxstom", Some(7), 8), ["FROM ", "cuxstom"]);
    }

    #[test]
    fn a_ghost_starts_on_the_empty_row_of_a_full_line() {
        assert_eq!(painted("orders", "orders id", Some(6), 6), ["orders", " id"]);
    }

    #[test]
    fn a_marks_space_goes_on_the_last_row_or_the_empty_one_after_it() {
        assert_eq!(painted("order", "order ", None, 6), ["order "]);
        assert_eq!(painted("orders", "orders ", None, 6), ["orders", " "]);
    }

    #[test]
    fn a_byte_on_the_whitespace_at_a_break_is_drawn_on_it() {
        assert_eq!(spot_of("FROM orders", 4, 8), Spot { row: row(0, 0), column: 4 });
    }

    #[test]
    fn the_byte_after_the_whitespace_at_a_break_is_drawn_at_the_start_of_the_next_row() {
        assert_eq!(spot_of("FROM orders", 5, 8), Spot { row: row(0, 1), column: 0 });
    }

    #[test]
    fn a_cell_past_the_end_of_a_shorter_row_finds_its_whitespace() {
        assert_eq!(byte_under("FROM orders", row(0, 0), 0, 7, 8), Some(4));
    }

    #[test]
    fn down_from_the_whitespace_at_a_break_goes_to_the_cell_below() {
        assert_eq!(down("FROM orders", 4, 8), 9);
    }

    #[test]
    fn up_past_the_end_of_a_shorter_row_stops_on_its_whitespace() {
        assert_eq!(up("FROM orders", 11, 8), 4);
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
        assert_eq!(up("customers", 5, 4), 1);
    }

    #[test]
    fn up_goes_to_the_last_row_of_the_line_above() {
        assert_eq!(up("customers\nid", 10, 4), 8);
    }

    #[test]
    fn up_on_the_first_row_stays() {
        assert_eq!(up("customers", 2, 4), 2);
    }

    #[test]
    fn up_lands_on_the_cell_above_past_wide_characters() {
        assert_eq!(up("日本語abc", 12, 4), 6);
    }

    #[test]
    fn down_goes_to_the_row_below_within_a_line() {
        assert_eq!(down("customers", 1, 4), 5);
    }

    #[test]
    fn down_goes_to_the_first_row_of_the_line_below() {
        assert_eq!(down("customers\nid", 8, 4), 10);
    }

    #[test]
    fn down_on_the_last_row_stays() {
        assert_eq!(down("customers", 8, 4), 8);
    }

    #[test]
    fn down_keeps_to_the_end_of_a_shorter_last_row() {
        assert_eq!(down("customers", 7, 4), 9);
    }

    #[test]
    fn down_reaches_the_empty_row_of_a_full_line() {
        assert_eq!(down("orders", 2, 6), 6);
    }

    #[test]
    fn a_pane_not_drawn_yet_moves_by_lines() {
        assert_eq!(up("id\ncustomers", 12, 0), 2);
    }
}
