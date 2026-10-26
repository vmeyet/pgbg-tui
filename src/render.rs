//! Plain-terminal output for the scriptable commands: columns aligned by width.
const GAP: &str = "  ";

/// Rows as aligned lines; every column is padded to its widest cell, the last one is not.
pub(crate) fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> =
        (0..columns).map(|c| rows.iter().filter_map(|row| row.get(c)).map(|cell| width(cell)).max().unwrap_or(0)).collect();
    rows.iter().map(|row| line(row, &widths)).collect()
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
    fn no_rows_print_nothing() {
        assert_eq!(table(&[]), "");
    }
}
