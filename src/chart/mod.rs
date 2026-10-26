//! What to chart from a result: pure, drawing lives in `tui/chart_view/`.
pub(crate) use spec::ChartSpec;

use crate::db::model::{Column, ColumnKind, ResultSet};

mod spec;

/// A result with at most this many rows is drawn as bars, whatever its x.
const FEW_POINTS: usize = 12;

/// How a chart draws its points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChartKind {
    Bar,
    Line,
}

impl ChartKind {
    const ALL: [Self; 2] = [Self::Bar, Self::Line];

    /// The word a `-- chart:` header uses for it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Line => "line",
        }
    }
}

/// The chart a result asks for, or `None` when it has nothing to put on x or y.
///
/// x is the first text, date, time or timestamp column; y is every numeric column.
/// The kind is bar when x is text or there are at most 12 rows, line otherwise: a line reads a trend over time, bars compare a few values.
pub(crate) fn infer(result: &ResultSet) -> Option<ChartSpec> {
    let x = &result.columns[default_x(&result.columns)?];
    let y = default_y(&result.columns);
    if y.is_empty() {
        return None;
    }
    Some(ChartSpec { kind: suited_kind(x, result.rows.len()), x: Some(x.name.clone()), y })
}

fn default_x(columns: &[Column]) -> Option<usize> {
    columns.iter().position(|column| is_label(&column.kind))
}

fn default_y(columns: &[Column]) -> Vec<String> {
    columns.iter().filter(|column| is_numeric(&column.kind)).map(|column| column.name.clone()).collect()
}

fn suited_kind(x: &Column, rows: usize) -> ChartKind {
    let is_text = x.kind == ColumnKind::Text;
    if is_text || rows <= FEW_POINTS { ChartKind::Bar } else { ChartKind::Line }
}

const fn is_label(kind: &ColumnKind) -> bool {
    matches!(kind, ColumnKind::Text | ColumnKind::Date | ColumnKind::Time | ColumnKind::Timestamp | ColumnKind::TimestampTz)
}

const fn is_numeric(kind: &ColumnKind) -> bool {
    matches!(kind, ColumnKind::Int | ColumnKind::Float | ColumnKind::Numeric)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::Value;

    fn column(name: &str, kind: ColumnKind) -> Column {
        Column { name: name.to_owned(), kind, type_name: String::new() }
    }

    fn result(columns: Vec<Column>, rows: Vec<Vec<Value>>) -> ResultSet {
        ResultSet { columns, rows, is_cut: false }
    }

    fn rows(count: usize, row: impl Fn(usize) -> Vec<Value>) -> Vec<Vec<Value>> {
        (0..count).map(row).collect()
    }

    fn day(index: usize) -> Value {
        Value::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Days::new(index as u64))
    }

    #[test]
    fn a_text_column_goes_on_x_and_the_numeric_ones_on_y() {
        let orders = result(
            vec![column("id", ColumnKind::Int), column("customer", ColumnKind::Text), column("total", ColumnKind::Numeric)],
            vec![vec![Value::Int(7), Value::Text("nina".into()), Value::Numeric("12.50".into())]],
        );
        let spec = infer(&orders).unwrap();
        assert_eq!(spec, ChartSpec { kind: ChartKind::Bar, x: Some("customer".into()), y: vec!["id".into(), "total".into()] });
    }

    #[test]
    fn the_first_label_column_is_x() {
        let orders =
            result(vec![column("day", ColumnKind::Date), column("customer", ColumnKind::Text), column("total", ColumnKind::Float)], vec![]);
        assert_eq!(infer(&orders).unwrap().x.as_deref(), Some("day"));
    }

    #[test]
    fn a_result_without_a_label_column_has_no_chart() {
        let totals = result(vec![column("id", ColumnKind::Int), column("total", ColumnKind::Numeric)], vec![]);
        assert_eq!(infer(&totals), None);
    }

    #[test]
    fn a_result_without_a_numeric_column_has_no_chart() {
        let customers = result(vec![column("customer", ColumnKind::Text), column("is_paid", ColumnKind::Bool)], vec![]);
        assert_eq!(infer(&customers), None);
    }

    #[test]
    fn many_days_draw_a_line() {
        let orders =
            result(vec![column("day", ColumnKind::Date), column("orders", ColumnKind::Int)], rows(13, |i| vec![day(i), Value::Int(3)]));
        assert_eq!(infer(&orders).unwrap().kind, ChartKind::Line);
    }

    #[test]
    fn a_few_days_draw_bars() {
        let orders =
            result(vec![column("day", ColumnKind::Date), column("orders", ColumnKind::Int)], rows(12, |i| vec![day(i), Value::Int(3)]));
        assert_eq!(infer(&orders).unwrap().kind, ChartKind::Bar);
    }

    #[test]
    fn many_text_labels_still_draw_bars() {
        let customers = result(
            vec![column("customer", ColumnKind::Text), column("orders", ColumnKind::Int)],
            rows(40, |i| vec![Value::Text(format!("c{i}")), Value::Int(1)]),
        );
        assert_eq!(infer(&customers).unwrap().kind, ChartKind::Bar);
    }

    #[test]
    fn timestamps_and_times_can_be_x() {
        for kind in [ColumnKind::Time, ColumnKind::Timestamp, ColumnKind::TimestampTz] {
            let orders = result(vec![column("at", kind), column("orders", ColumnKind::Int)], vec![]);
            assert_eq!(infer(&orders).unwrap().x.as_deref(), Some("at"));
        }
    }
}
