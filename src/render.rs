//! Plain-terminal output for the scriptable commands: columns aligned by width.
use std::borrow::Cow;

const GAP: &str = "  ";

/// Rows as aligned lines; every column is padded to its widest cell, the last one is not.
pub(crate) fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> =
        (0..columns).map(|c| rows.iter().filter_map(|row| row.get(c)).map(|cell| width(cell)).max().unwrap_or(0)).collect();
    rows.iter().map(|row| line(row, &widths)).collect()
}

/// `1 row`, `12 rows`.
pub(crate) fn row_count(count: usize) -> String {
    match count {
        1 => "1 row".to_owned(),
        count => format!("{count} rows"),
    }
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

fn line(row: &[String], widths: &[usize]) -> String {
    let padded: Vec<String> = row.iter().zip(widths).map(|(cell, width)| format!("{cell:<width$}")).collect();
    padded.join(GAP).trim_end().to_owned() + "\n"
}

fn width(cell: &str) -> usize {
    cell.chars().count()
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

    #[test]
    fn control_characters_print_escaped() {
        assert_eq!(printable("\u{1b}]0;title\u{7} nina\nnoel"), "\\u{1b}]0;title\\u{7} nina\\nnoel");
    }

    #[test]
    fn no_rows_print_nothing() {
        assert_eq!(table(&[]), "");
    }
}
