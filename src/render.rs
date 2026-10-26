//! Plain-terminal output for the scriptable commands: columns aligned by width.
use crate::db::model::ResultSet;
use ratatui::text::Span;
use std::borrow::Cow;

const GAP: &str = "  ";

/// Rows as aligned lines; every column is padded to its widest cell, the last one is not.
pub(crate) fn table(rows: &[Vec<String>]) -> String {
    let rows: Vec<Vec<Cow<'_, str>>> = rows.iter().map(|row| row.iter().map(|cell| printable(cell)).collect()).collect();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> =
        (0..columns).map(|c| rows.iter().filter_map(|row| row.get(c)).map(|cell| width(cell)).max().unwrap_or(0)).collect();
    rows.iter().map(|row| line(row, &widths)).collect()
}

/// `1 row`, `12 rows`, or `first 10000 rows` when the result is cut.
pub(crate) fn row_count(result: &ResultSet) -> String {
    let count = match result.rows.len() {
        1 => "1 row".to_owned(),
        count => format!("{count} rows"),
    };
    if result.is_cut { format!("first {count}") } else { count }
}

/// `text` with each control character escaped, a newline as `\n` and ESC as `\u{1b}`, so a value
/// from the database can never move the cursor or send the terminal a command.
pub(crate) fn printable(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.chars().map(escape_control).collect())
}

fn escape_control(c: char) -> String {
    if c.is_control() { c.escape_default().collect() } else { c.into() }
}

fn line(row: &[Cow<'_, str>], widths: &[usize]) -> String {
    let padded: Vec<String> = row.iter().zip(widths).map(|(cell, &width)| padded(cell, width)).collect();
    padded.join(GAP).trim_end().to_owned() + "\n"
}

fn padded(cell: &str, columns: usize) -> String {
    format!("{cell}{}", " ".repeat(columns.saturating_sub(width(cell))))
}

/// The terminal columns `cell` takes: two for a wide character such as `日`.
fn width(cell: &str) -> usize {
    Span::raw(cell).width()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|&cell| cell.to_owned()).collect()
    }

    #[test]
    fn columns_align_on_their_widest_cell() {
        let rows = [row(&["local", "local", "postgres://nina@localhost/shop"]), row(&["staging", "staging", ""])];
        assert_eq!(table(&rows), "local    local    postgres://nina@localhost/shop\nstaging  staging\n");
    }

    fn numbers(count: usize, is_cut: bool) -> ResultSet {
        ResultSet { columns: vec![], rows: vec![vec![]; count], is_cut }
    }

    #[test]
    fn a_whole_result_counts_its_rows() {
        assert_eq!(row_count(&numbers(1, false)), "1 row");
        assert_eq!(row_count(&numbers(12, false)), "12 rows");
    }

    #[test]
    fn a_cut_result_says_its_rows_are_the_first_ones() {
        assert_eq!(row_count(&numbers(10, true)), "first 10 rows");
    }

    #[test]
    fn wide_characters_align_by_the_columns_they_take() {
        let rows = [row(&["日本", "nina"]), row(&["ab", "noel"])];
        assert_eq!(table(&rows), "日本  nina\nab    noel\n");
    }

    #[test]
    fn control_characters_print_escaped() {
        assert_eq!(printable("\u{1b}]0;title\u{7} nina\nnoel"), "\\u{1b}]0;title\\u{7} nina\\nnoel");
    }

    #[test]
    fn control_characters_in_a_cell_print_escaped() {
        let rows = [row(&["\u{1b}]0;title\u{7}", "nina\nnoel\t."])];
        assert_eq!(table(&rows), "\\u{1b}]0;title\\u{7}  nina\\nnoel\\t.\n");
    }

    #[test]
    fn printable_text_is_kept_as_is() {
        assert_eq!(printable("Zoë ✓ 12.50"), "Zoë ✓ 12.50");
    }

    #[test]
    fn no_rows_print_nothing() {
        assert_eq!(table(&[]), "");
    }
}
