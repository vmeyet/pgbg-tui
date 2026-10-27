//! A result, a row or a cell as text to paste elsewhere: CSV, JSON or Markdown, every value raw and whole.
use crate::db::model::{Column, ResultSet, Value};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use std::collections::BTreeSet;

/// The text a whole result is copied as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Csv,
    Json,
    Markdown,
}

impl Format {
    /// Its name, as the status line says what was copied.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Json => "JSON",
            Self::Markdown => "Markdown",
        }
    }
}

/// The value as it prints, whole: `NULL` for a null, NUMERIC with every digit.
pub(crate) fn cell(value: &Value) -> String {
    value.to_string()
}

/// The row as one CSV line, no header.
pub(crate) fn row(values: &[Value]) -> String {
    csv_line(values.iter().map(csv_field))
}

/// The whole result in `format`.
pub(crate) fn result(result: &ResultSet, format: Format) -> String {
    match format {
        Format::Csv => csv(result),
        Format::Json => json(result),
        Format::Markdown => markdown(result),
    }
}

/// A header line of the column names, then one line per row; a NULL is an empty field, as `COPY ... CSV` writes it.
/// A value starting with `=`, `+`, `-` or `@` is copied as it is: a spreadsheet may read it as a formula.
fn csv(result: &ResultSet) -> String {
    let header = csv_line(result.columns().iter().map(|column| csv_quoted(column.name.as_str())));
    let rows = result.rows().iter().map(|values| row(values));
    std::iter::once(header).chain(rows).collect()
}

fn csv_line(fields: impl Iterator<Item = String>) -> String {
    fields.collect::<Vec<_>>().join(",") + "\n"
}

fn csv_field(value: &Value) -> String {
    if *value == Value::Null { String::new() } else { csv_quoted(&value.to_string()) }
}

/// `text` in double quotes, its own doubled, when it holds a comma, a quote or a line end.
fn csv_quoted(text: &str) -> String {
    let needs_quotes = text.contains([',', '"', '\n', '\r']);
    if needs_quotes { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_owned() }
}

/// An array of one object per row, the columns in their order, each value as `pgbg run --json` gives it; a name two
/// columns share takes a suffix the second time (`count`, `count_2`), so no value is lost.
fn json(result: &ResultSet) -> String {
    // Writing to a `String` fails only on a map key that is not a string, and every key here is a column name.
    serde_json::to_string_pretty(&Rows(result)).unwrap_or_default() + "\n"
}

struct Rows<'a>(&'a ResultSet);

impl Serialize for Rows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let keys = unique_names(self.0.columns());
        let mut rows = serializer.serialize_seq(Some(self.0.rows().len()))?;
        for values in self.0.rows() {
            rows.serialize_element(&RowObject { keys: &keys, values })?;
        }
        rows.end()
    }
}

/// Each column's name, its first use kept and a later one suffixed `_2`, `_3` and on, past any name a column already
/// has: `count, count, count_2` gives `count, count_3, count_2`.
fn unique_names(columns: &[Column]) -> Vec<String> {
    let real: BTreeSet<&str> = columns.iter().map(|column| column.name.as_str()).collect();
    let mut taken = BTreeSet::new();
    let mut names = vec![];
    for column in columns {
        let is_free = |name: &String| !taken.contains(name) && (name == column.name.as_str() || !real.contains(name.as_str()));
        let name = (1..=2 * columns.len()).map(|nth| nth_name(column.name.as_str(), nth)).find(is_free).unwrap_or_default();
        taken.insert(name.clone());
        names.push(name);
    }
    names
}

fn nth_name(name: &str, nth: usize) -> String {
    if nth == 1 { name.to_owned() } else { format!("{name}_{nth}") }
}

struct RowObject<'a> {
    keys: &'a [String],
    values: &'a [Value],
}

impl Serialize for RowObject<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut object = serializer.serialize_map(Some(self.keys.len()))?;
        for (key, value) in self.keys.iter().zip(self.values) {
            object.serialize_entry(key, value)?;
        }
        object.end()
    }
}

/// A table with a header row; a `|` in a cell is escaped and a line end becomes `<br>`, so each row stays one line.
fn markdown(result: &ResultSet) -> String {
    let header = markdown_line(result.columns().iter().map(|column| markdown_cell(column.name.as_str())));
    let rule = markdown_line(result.columns().iter().map(|_| "---".to_owned()));
    let rows = result.rows().iter().map(|values| markdown_line(values.iter().map(|value| markdown_cell(&value.to_string()))));
    [header, rule].into_iter().chain(rows).collect()
}

fn markdown_line(cells: impl Iterator<Item = String>) -> String {
    format!("| {} |\n", cells.collect::<Vec<_>>().join(" | "))
}

fn markdown_cell(text: &str) -> String {
    text.replace('\\', "\\\\").replace('|', "\\|").replace("\r\n", "<br>").replace(['\n', '\r'], "<br>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::orders;

    #[test]
    fn csv_has_a_header_and_leaves_a_null_empty() {
        assert_eq!(result(&orders(), Format::Csv), "id,total,customer\n7,12.50,nina\n12,1499.00,\n103,3.20,Zoë Nakamura\n104,0.99,noel\n");
    }

    #[test]
    fn a_csv_field_with_a_comma_a_quote_or_a_line_end_is_quoted() {
        let values = [Value::Text("Nakamura, Zoë".into()), Value::Text("say \"hi\"".into()), Value::Text("door\nbell".into())];
        assert_eq!(row(&values), "\"Nakamura, Zoë\",\"say \"\"hi\"\"\",\"door\nbell\"\n");
    }

    #[test]
    fn numeric_keeps_every_digit() {
        let total = Value::Numeric("12345678901234567890.000000000001".into());
        assert_eq!(cell(&total), "12345678901234567890.000000000001");
        assert_eq!(row(std::slice::from_ref(&total)), "12345678901234567890.000000000001\n");
    }

    #[test]
    fn a_null_cell_copies_as_null() {
        assert_eq!(cell(&Value::Null), "NULL");
    }

    #[test]
    fn json_is_one_object_per_row_with_null_and_numeric_as_run_json_gives_them() {
        let copied: serde_json::Value = serde_json::from_str(&result(&orders(), Format::Json)).unwrap();
        assert_eq!(copied[1], serde_json::json!({"id": 12, "total": "1499.00", "customer": null}));
        assert_eq!(copied.as_array().map(Vec::len), Some(4));
    }

    #[test]
    fn json_keeps_the_columns_in_their_order() {
        assert!(result(&orders(), Format::Json).starts_with("[\n  {\n    \"id\": 7,\n    \"total\": \"12.50\","));
    }

    #[test]
    fn json_gives_a_name_two_columns_share_a_suffix_so_no_value_is_lost() {
        let int = |name: &str| crate::fixtures::column(name, crate::db::model::ColumnKind::Int);
        let counts =
            ResultSet::whole(vec![int("count"), int("count"), int("count_2")], vec![vec![Value::Int(1), Value::Int(2), Value::Int(3)]]);
        let copied: serde_json::Value = serde_json::from_str(&result(&counts, Format::Json)).unwrap();
        assert_eq!(copied[0], serde_json::json!({"count": 1, "count_3": 2, "count_2": 3}));
    }

    #[test]
    fn markdown_is_a_table_with_a_header() {
        let copied = result(&orders(), Format::Markdown);
        let lines: Vec<&str> = copied.lines().take(4).collect();
        assert_eq!(lines, ["| id | total | customer |", "| --- | --- | --- |", "| 7 | 12.50 | nina |", "| 12 | 1499.00 | NULL |"]);
    }

    #[test]
    fn a_pipe_or_a_line_end_in_a_markdown_cell_keeps_the_row_on_one_line() {
        assert_eq!(markdown_cell("a|b\nc\\d"), "a\\|b<br>c\\\\d");
    }

    #[test]
    fn a_lone_carriage_return_in_a_markdown_cell_is_a_line_break_too() {
        assert_eq!(markdown_cell("a\rb\r\nc"), "a<br>b<br>c");
    }
}
