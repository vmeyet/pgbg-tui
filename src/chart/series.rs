use super::axis::XAxis;
use super::{ChartKind, ChartSpec, default_x, default_y, is_numeric};
use crate::db::model::{Column, ResultSet, Value};
use std::fmt;

/// The longest x label kept: a 10 MB text cell costs no more than a short one.
const MAX_LABEL_CHARS: usize = 32;

/// The points a chart draws: one x position and label per point and, for each y column, one value per point.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Series {
    pub(crate) kind: ChartKind,
    /// The x column's name, the axis title.
    pub(crate) x: String,
    pub(crate) axis: XAxis,
    /// Each point's place on the axis, ascending.
    pub(crate) positions: Vec<f64>,
    pub(crate) labels: Vec<String>,
    pub(crate) y: Vec<YColumn>,
    /// Rows were left out: the result was cut, a row had no place in time, or there were more points than the kind draws.
    pub(crate) is_cut: bool,
}

/// A row and where it sits on the x axis.
#[derive(Clone, Copy)]
struct Placed {
    row: usize,
    position: f64,
}

/// One y column's values, one per point; `None` where there is nothing to draw: NULL, NaN or infinity.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct YColumn {
    pub(crate) name: String,
    pub(crate) values: Vec<Option<f64>>,
}

/// Why a result cannot be drawn as the spec asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChartError {
    UnknownColumn(String),
    NotNumeric(String),
    /// No x was given and the result has no text, date or time column.
    NoX,
    /// No y was given and the result has no numeric column.
    NoY,
    /// No row has a value to draw.
    Empty,
}

impl fmt::Display for ChartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownColumn(name) => write!(f, "the result has no column `{name}`"),
            Self::NotNumeric(name) => write!(f, "column `{name}` does not hold numbers"),
            Self::NoX => f.write_str("no text, date or time column to put on x"),
            Self::NoY => f.write_str("no numeric column to put on y"),
            Self::Empty => f.write_str("no row has a value to draw"),
        }
    }
}

impl std::error::Error for ChartError {}

impl Series {
    /// The points of `result` for `spec`, x and y filled as `chart::infer` would when the spec leaves them out.
    /// A line over dates or times is placed in time and sorted; anything else keeps the result's order.
    pub(crate) fn from_result(result: &ResultSet, spec: &ChartSpec) -> Result<Self, ChartError> {
        let x = x_column(result, spec)?;
        let y = y_columns(result, spec)?;
        let axis = XAxis::of(spec.kind, &result.columns[x].kind);
        let placed = place(result, x, axis);
        let kept: Vec<Placed> = spec.kind.keep(placed.len()).into_iter().map(|index| placed[index]).collect();
        let rows: Vec<usize> = kept.iter().map(|point| point.row).collect();
        let series = Self {
            kind: spec.kind,
            x: result.columns[x].name.clone(),
            axis,
            positions: kept.iter().map(|point| point.position).collect(),
            labels: rows.iter().map(|&row| result.rows[row][x].prefix(MAX_LABEL_CHARS)).collect(),
            y: y.iter().map(|&column| y_values(result, column, &rows)).collect(),
            is_cut: result.is_cut || kept.len() < result.rows.len(),
        };
        if series.has_no_value() {
            return Err(ChartError::Empty);
        }
        Ok(series)
    }

    fn has_no_value(&self) -> bool {
        self.y.iter().all(|column| column.values.iter().all(Option::is_none))
    }
}

fn place(result: &ResultSet, x: usize, axis: XAxis) -> Vec<Placed> {
    let mut placed: Vec<Placed> = result
        .rows
        .iter()
        .enumerate()
        .filter_map(|(row, values)| Some(Placed { row, position: axis.position(row, &values[x])? }))
        .collect();
    placed.sort_by(|left, right| left.position.total_cmp(&right.position));
    placed
}

fn x_column(result: &ResultSet, spec: &ChartSpec) -> Result<usize, ChartError> {
    match &spec.x {
        Some(name) => position(&result.columns, name),
        None => default_x(&result.columns).ok_or(ChartError::NoX),
    }
}

fn y_columns(result: &ResultSet, spec: &ChartSpec) -> Result<Vec<usize>, ChartError> {
    let names = if spec.y.is_empty() { default_y(&result.columns) } else { spec.y.clone() };
    if names.is_empty() {
        return Err(ChartError::NoY);
    }
    names.iter().map(|name| numeric_position(&result.columns, name)).collect()
}

fn position(columns: &[Column], name: &str) -> Result<usize, ChartError> {
    columns.iter().position(|column| column.name == name).ok_or_else(|| ChartError::UnknownColumn(name.to_owned()))
}

fn numeric_position(columns: &[Column], name: &str) -> Result<usize, ChartError> {
    let index = position(columns, name)?;
    if !is_numeric(&columns[index].kind) {
        return Err(ChartError::NotNumeric(name.to_owned()));
    }
    Ok(index)
}

fn y_values(result: &ResultSet, column: usize, rows: &[usize]) -> YColumn {
    YColumn { name: result.columns[column].name.clone(), values: rows.iter().map(|&row| number(&result.rows[row][column])).collect() }
}

/// The value as a finite float; a NUMERIC past `f64`'s range, or `NaN`, has none.
#[expect(clippy::cast_precision_loss, reason = "a chart draws to a terminal cell, far coarser than 53 bits")]
fn number(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Int(int) => *int as f64,
        Value::Float(float) => *float,
        Value::Numeric(text) => text.parse().ok()?,
        _ => return None,
    };
    number.is_finite().then_some(number)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{column, result};
    use super::super::{bar, line};
    use super::*;
    use crate::db::model::ColumnKind;

    fn spec(text: &str) -> ChartSpec {
        text.parse().unwrap()
    }

    fn orders() -> ResultSet {
        result(
            vec![column("customer", ColumnKind::Text), column("orders", ColumnKind::Int), column("total", ColumnKind::Numeric)],
            vec![
                vec![Value::Text("nina".into()), Value::Int(3), Value::Numeric("12.50".into())],
                vec![Value::Null, Value::Int(1), Value::Numeric("1499.00".into())],
            ],
        )
    }

    fn totals(values: Vec<Value>, kind: ColumnKind) -> ResultSet {
        result(
            vec![column("customer", ColumnKind::Text), column("total", kind)],
            values.into_iter().map(|value| vec![Value::Text("nina".into()), value]).collect(),
        )
    }

    fn values(series: &Series) -> &[Option<f64>] {
        &series.y[0].values
    }

    #[test]
    fn a_full_spec_gives_one_label_and_one_value_per_row() {
        let series = Series::from_result(&orders(), &spec("bar x=customer y=total")).unwrap();
        assert_eq!(series.labels, ["nina", "NULL"]);
        assert_eq!(series.y, [YColumn { name: "total".into(), values: vec![Some(12.5), Some(1499.0)] }]);
        assert!(!series.is_cut);
    }

    #[test]
    fn a_spec_without_x_or_y_fills_them_from_the_result() {
        let series = Series::from_result(&orders(), &spec("line")).unwrap();
        assert_eq!(series.x, "customer");
        assert_eq!(series.y.iter().map(|column| column.name.as_str()).collect::<Vec<_>>(), ["orders", "total"]);
        assert_eq!(series.kind, ChartKind::Line);
    }

    #[test]
    fn a_given_x_can_be_any_column() {
        assert_eq!(Series::from_result(&orders(), &spec("bar x=orders y=total")).unwrap().labels, ["3", "1"]);
    }

    #[test]
    fn an_unknown_column_is_named() {
        assert_eq!(Series::from_result(&orders(), &spec("bar x=day")), Err(ChartError::UnknownColumn("day".into())));
        assert_eq!(Series::from_result(&orders(), &spec("bar y=refunds")), Err(ChartError::UnknownColumn("refunds".into())));
    }

    #[test]
    fn a_y_that_does_not_hold_numbers_is_refused() {
        assert_eq!(Series::from_result(&orders(), &spec("bar y=customer")), Err(ChartError::NotNumeric("customer".into())));
    }

    #[test]
    fn a_result_with_nothing_for_x_or_y_says_which() {
        let numbers = result(vec![column("total", ColumnKind::Int)], vec![vec![Value::Int(1)]]);
        let names = result(vec![column("customer", ColumnKind::Text)], vec![vec![Value::Text("nina".into())]]);
        assert_eq!(Series::from_result(&numbers, &spec("bar")), Err(ChartError::NoX));
        assert_eq!(Series::from_result(&names, &spec("bar")), Err(ChartError::NoY));
    }

    #[test]
    fn a_result_without_rows_is_empty() {
        assert_eq!(Series::from_result(&totals(vec![], ColumnKind::Int), &spec("bar")), Err(ChartError::Empty));
    }

    #[test]
    fn a_result_of_only_nulls_is_empty() {
        assert_eq!(Series::from_result(&totals(vec![Value::Null, Value::Null], ColumnKind::Int), &spec("bar")), Err(ChartError::Empty));
    }

    #[test]
    fn null_nan_infinity_and_undecoded_values_leave_a_gap() {
        let floats = vec![
            Value::Null,
            Value::Float(f64::NAN),
            Value::Float(f64::INFINITY),
            Value::Float(f64::NEG_INFINITY),
            Value::Unsupported,
            Value::Float(2.5),
        ];
        let series = Series::from_result(&totals(floats, ColumnKind::Float), &spec("bar")).unwrap();
        assert_eq!(values(&series), [None, None, None, None, None, Some(2.5)]);
    }

    #[test]
    fn numeric_text_is_read_as_a_number_and_its_specials_leave_a_gap() {
        let numerics = ["1499.00", "-0.5", "NaN", "Infinity", "-Infinity", "1e400"].map(|text| Value::Numeric(text.into()));
        let series = Series::from_result(&totals(numerics.into(), ColumnKind::Numeric), &spec("bar")).unwrap();
        assert_eq!(values(&series), [Some(1499.0), Some(-0.5), None, None, None, None]);
    }

    #[test]
    fn a_long_label_is_cut_to_its_first_characters() {
        let long = result(
            vec![column("customer", ColumnKind::Text), column("total", ColumnKind::Int)],
            vec![vec![Value::Text("n".repeat(500)), Value::Int(1)]],
        );
        assert_eq!(Series::from_result(&long, &spec("bar")).unwrap().labels[0].chars().count(), MAX_LABEL_CHARS);
    }

    #[test]
    fn bars_past_the_cap_keep_the_first_rows_and_say_they_are_cut() {
        let many = totals((0..100).map(Value::Int).collect(), ColumnKind::Int);
        let series = Series::from_result(&many, &spec("bar")).unwrap();
        assert_eq!(values(&series).len(), bar::MAX_POINTS);
        assert_eq!(values(&series)[bar::MAX_POINTS - 1], Some(63.0));
        assert!(series.is_cut);
    }

    #[test]
    fn a_line_past_the_cap_is_sampled_and_says_it_is_cut() {
        let many = totals((0..5000).map(Value::Int).collect(), ColumnKind::Int);
        let series = Series::from_result(&many, &spec("line")).unwrap();
        assert_eq!(values(&series).len(), line::MAX_POINTS);
        assert_eq!(values(&series).last(), Some(&Some(4999.0)));
        assert!(series.is_cut);
    }

    #[test]
    fn a_cut_result_gives_a_cut_series() {
        let cut = ResultSet { is_cut: true, ..totals(vec![Value::Int(1)], ColumnKind::Int) };
        assert!(Series::from_result(&cut, &spec("bar")).unwrap().is_cut);
    }

    fn daily(days: &[Value]) -> ResultSet {
        result(
            vec![column("day", ColumnKind::Date), column("orders", ColumnKind::Int)],
            days.iter().enumerate().map(|(index, day)| vec![day.clone(), Value::Int(index as i64)]).collect(),
        )
    }

    fn day(day: u32) -> Value {
        Value::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, day).unwrap())
    }

    #[test]
    fn a_line_over_dates_shows_a_gap_in_time() {
        let series = Series::from_result(&daily(&[day(1), day(2), day(5)]), &spec("line")).unwrap();
        let steps: Vec<f64> = series.positions.windows(2).map(|pair| (pair[1] - pair[0]) / 86_400.0).collect();
        assert_eq!(steps, [1.0, 3.0]);
    }

    #[test]
    fn a_line_over_dates_is_drawn_in_time_order() {
        let series = Series::from_result(&daily(&[day(3), day(1), day(2)]), &spec("line")).unwrap();
        assert_eq!(series.labels, ["2026-01-01", "2026-01-02", "2026-01-03"]);
        assert_eq!(values(&series), [Some(1.0), Some(2.0), Some(0.0)]);
    }

    #[test]
    fn a_row_with_no_place_in_time_is_left_out_of_a_line() {
        let series = Series::from_result(&daily(&[day(1), Value::Null, Value::Infinity, day(2)]), &spec("line")).unwrap();
        assert_eq!(series.labels, ["2026-01-01", "2026-01-02"]);
        assert!(series.is_cut);
    }

    #[test]
    fn bars_over_dates_keep_the_result_order() {
        let series = Series::from_result(&daily(&[day(3), Value::Null, day(1)]), &spec("bar")).unwrap();
        assert_eq!(series.labels, ["2026-01-03", "NULL", "2026-01-01"]);
        assert_eq!(series.positions, [0.0, 1.0, 2.0]);
    }
}
