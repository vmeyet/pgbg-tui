//! What to chart and the points to draw, from a result: pure, drawing lives in `tui/chart_view/`.
pub(crate) use series::{ChartError, Series, YColumn};
pub(crate) use spec::ChartSpec;

use crate::db::model::{Column, ColumnKind, ResultSet, Value};

mod axis;
mod bar;
mod line;
mod series;
mod spec;

/// A result with at most this many rows is drawn as bars, whatever its x.
const FEW_POINTS: usize = 12;

/// How a chart draws its points; each kind keeps its own cap in its own file.
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

    /// The indexes of the points drawn out of `points`, in order: all of them when they fit the kind's cap.
    fn keep(self, points: usize) -> Vec<usize> {
        match self {
            Self::Bar => bar::keep(points),
            Self::Line => line::keep(points),
        }
    }
}

/// A chart's kind and its columns, by index in one result, so two columns of the same name stay apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChartColumns {
    pub(crate) kind: ChartKind,
    pub(crate) x: usize,
    pub(crate) y: Vec<usize>,
}

/// The chart a result asks for, or `None` when it has nothing to put on x or y.
///
/// x is the first text, date, time or timestamp column, else the first numeric column when it holds integers in ascending order
/// (`year`, `generate_series`); y is every other numeric column.
/// The kind is bar when x is text or there are at most 12 rows, line otherwise: a line reads a trend, bars compare a few values.
pub(crate) fn infer(result: &ResultSet) -> Option<ChartColumns> {
    let x = default_x(result)?;
    let y = default_y(&result.columns, x);
    if y.is_empty() {
        return None;
    }
    Some(ChartColumns { kind: suited_kind(&result.columns[x], result.rows.len()), x, y })
}

/// The series of the chart `infer` picks for `result`, or why there is none.
pub(crate) fn infer_series(result: &ResultSet) -> Result<Series, ChartError> {
    let columns = infer(result).ok_or_else(|| missing_axis(result))?;
    Series::from_columns(result, &columns)
}

fn missing_axis(result: &ResultSet) -> ChartError {
    if default_x(result).is_none() { ChartError::NoX } else { ChartError::NoY }
}

fn default_x(result: &ResultSet) -> Option<usize> {
    result.columns.iter().position(|column| is_label(&column.kind)).or_else(|| ascending_integers(result))
}

/// The first numeric column, when it is an integer one whose every value is greater than the one before.
fn ascending_integers(result: &ResultSet) -> Option<usize> {
    let first = result.columns.iter().position(|column| is_numeric(&column.kind))?;
    let is_integer = result.columns[first].kind == ColumnKind::Int;
    let integers: Option<Vec<i64>> = result.rows.iter().map(|row| integer(&row[first])).collect();
    let is_ascending = integers.is_some_and(|integers| integers.windows(2).all(|pair| pair[0] < pair[1]));
    (is_integer && is_ascending).then_some(first)
}

const fn integer(value: &Value) -> Option<i64> {
    if let Value::Int(integer) = value { Some(*integer) } else { None }
}

fn default_y(columns: &[Column], x: usize) -> Vec<usize> {
    (0..columns.len()).filter(|&column| column != x && is_numeric(&columns[column].kind)).collect()
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

    pub(super) fn column(name: &str, kind: ColumnKind) -> Column {
        Column { name: name.to_owned(), kind, type_name: String::new() }
    }

    pub(super) fn result(columns: Vec<Column>, rows: Vec<Vec<Value>>) -> ResultSet {
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
        assert_eq!(infer(&orders), Some(ChartColumns { kind: ChartKind::Bar, x: 1, y: vec![0, 2] }));
    }

    #[test]
    fn the_first_label_column_is_x() {
        let orders =
            result(vec![column("day", ColumnKind::Date), column("customer", ColumnKind::Text), column("total", ColumnKind::Float)], vec![]);
        assert_eq!(infer(&orders).unwrap().x, 0);
    }

    #[test]
    fn a_result_without_a_label_or_ascending_integer_column_has_no_chart() {
        let row = |id: i64| vec![Value::Int(id), Value::Numeric("1.50".into())];
        let totals = result(vec![column("id", ColumnKind::Int), column("total", ColumnKind::Numeric)], vec![row(7), row(3)]);
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
    fn an_inferred_series_says_which_axis_is_missing() {
        let numbers = result(vec![column("total", ColumnKind::Float)], vec![vec![Value::Float(1.5)]]);
        let names = result(vec![column("customer", ColumnKind::Text)], vec![vec![Value::Text("nina".into())]]);
        assert_eq!(infer_series(&numbers), Err(ChartError::NoX));
        assert_eq!(infer_series(&names), Err(ChartError::NoY));
    }

    #[test]
    fn an_inferred_series_draws_what_infer_picks() {
        let orders = result(
            vec![column("customer", ColumnKind::Text), column("total", ColumnKind::Int)],
            vec![vec![Value::Text("nina".into()), Value::Int(3)]],
        );
        assert_eq!(infer_series(&orders).unwrap().labels, ["nina"]);
    }

    #[test]
    fn ascending_integers_go_on_x_when_there_is_no_label_column() {
        let yearly = result(
            vec![column("year", ColumnKind::Int), column("orders", ColumnKind::Int)],
            rows(3, |i| vec![Value::Int(2024 + i as i64), Value::Int(9)]),
        );
        assert_eq!(infer(&yearly), Some(ChartColumns { kind: ChartKind::Bar, x: 0, y: vec![1] }));
    }

    #[test]
    fn integers_with_a_null_are_not_an_x() {
        let yearly = result(
            vec![column("year", ColumnKind::Int), column("orders", ColumnKind::Int)],
            vec![vec![Value::Null, Value::Int(1)], vec![Value::Int(2026), Value::Int(2)]],
        );
        assert_eq!(infer(&yearly), None);
    }

    #[test]
    fn two_columns_of_the_same_name_both_go_on_y() {
        let counts =
            result(vec![column("customer", ColumnKind::Text), column("count", ColumnKind::Int), column("count", ColumnKind::Int)], vec![]);
        assert_eq!(infer(&counts).unwrap().y, [1, 2]);
    }

    #[test]
    fn timestamps_and_times_can_be_x() {
        for kind in [ColumnKind::Time, ColumnKind::Timestamp, ColumnKind::TimestampTz] {
            let orders = result(vec![column("at", kind), column("orders", ColumnKind::Int)], vec![]);
            assert_eq!(infer(&orders).unwrap().x, 0);
        }
    }
}
