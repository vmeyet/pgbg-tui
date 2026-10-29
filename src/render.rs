//! Text for the terminal: aligned tables for the scriptable commands, and values escaped and cut for every view.
use crate::db::model::{Extent, ResultSet, Value};
use crate::format;
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

/// `1 row`, `12 rows`, `first 10 000 rows` when the result does not hold every row, `all 1 732 rows` once paged to its end.
pub(crate) fn row_count(result: &ResultSet) -> String {
    let count = match result.rows().len() {
        1 => "1 row".to_owned(),
        count => format!("{} rows", grouped(count)),
    };
    match result.extent() {
        Extent::Cut | Extent::More => format!("first {count}"),
        Extent::All if result.is_paged() => format!("all {count}"),
        Extent::All => count,
    }
}

/// `count` grouped by thousands: `1 732`, `12 000`.
pub(crate) fn grouped(count: usize) -> String {
    format::grouped_digits(&count.to_string())
}

/// The first `chars` characters `value` prints, control characters escaped.
pub(crate) fn cell_text(value: &Value, chars: usize) -> String {
    printable(&value.prefix(chars)).into_owned()
}

/// `text` with each control character escaped, a newline as `\n` and ESC as `\u{1b}`, so a value
/// from the database can never move the cursor, send the terminal a command or reorder the text around it.
pub(crate) fn printable(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_unprintable) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.chars().map(escape_unprintable).collect())
}

/// A control character, or a mark that reverses the text around it.
pub(crate) fn is_unprintable(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn escape_unprintable(c: char) -> String {
    if is_unprintable(c) { c.escape_default().collect() } else { c.into() }
}

fn line(row: &[Cow<'_, str>], widths: &[usize]) -> String {
    let padded: Vec<String> = row.iter().zip(widths).map(|(cell, &width)| padded(cell, width)).collect();
    padded.join(GAP).trim_end().to_owned() + "\n"
}

/// `cell` followed by the spaces that make it `columns` wide.
pub(crate) fn padded(cell: &str, columns: usize) -> String {
    format!("{cell}{}", " ".repeat(columns.saturating_sub(width(cell))))
}

/// The terminal columns `cell` takes: two for a wide character such as `日`.
pub(crate) fn width(cell: &str) -> usize {
    Span::raw(cell).width()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::page;

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|&cell| cell.to_owned()).collect()
    }

    #[test]
    fn columns_align_on_their_widest_cell() {
        let rows = [row(&["local", "local", "postgres://nina@localhost/shop"]), row(&["staging", "staging", ""])];
        assert_eq!(table(&rows), "local    local    postgres://nina@localhost/shop\nstaging  staging\n");
    }

    fn numbers(count: usize, extent: Extent) -> ResultSet {
        ResultSet::new(vec![], page(count, extent))
    }

    #[test]
    fn a_whole_result_counts_its_rows() {
        assert_eq!(row_count(&numbers(1, Extent::All)), "1 row");
        assert_eq!(row_count(&numbers(12, Extent::All)), "12 rows");
    }

    #[test]
    fn a_cut_result_says_its_rows_are_the_first_ones() {
        assert_eq!(row_count(&numbers(10, Extent::Cut)), "first 10 rows");
    }

    #[test]
    fn a_result_waiting_on_more_pages_counts_its_first_rows_with_their_thousands_apart() {
        assert_eq!(row_count(&numbers(1500, Extent::More)), "first 1 500 rows");
    }

    #[test]
    fn a_result_paged_to_its_end_says_it_has_all_its_rows() {
        let mut result = numbers(500, Extent::More);
        result.push(page(1232, Extent::All));
        assert_eq!(row_count(&result), "all 1 732 rows");
    }

    #[test]
    fn a_count_shows_its_thousands_apart() {
        assert_eq!(
            [grouped(7), grouped(500), grouped(1732), grouped(120_000), grouped(1_000_000)],
            ["7", "500", "1 732", "120 000", "1 000 000"]
        );
    }

    #[test]
    fn wide_characters_align_by_the_columns_they_take() {
        let rows = [row(&["日本", "nina"]), row(&["ab", "noel"])];
        assert_eq!(table(&rows), "日本  nina\nab    noel\n");
    }

    #[test]
    fn a_cell_shows_its_first_characters_escaped() {
        assert_eq!(cell_text(&Value::Text("nina\nnoel".into()), 5), "nina\\n");
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
    fn bidi_controls_print_escaped_so_text_cannot_reorder_itself() {
        assert_eq!(printable("nina\u{202e}lmth.exe\u{2066}"), "nina\\u{202e}lmth.exe\\u{2066}");
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
