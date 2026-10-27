//! A line in braille dots, x and y labelled at their ends and between, a gap where a value is missing.
use super::{colour, short_number};
use crate::chart::{Series, YColumn};
use crate::render::printable;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::Line;
use ratatui::widgets::{Axis, Chart, Dataset, GraphType};
use vtui::theme::Theme;

const Y_LABELS: u16 = 3;
/// The fewest x labels: the two ends.
const MIN_X_LABELS: u16 = 2;
const MAX_X_LABELS: u16 = 6;
/// The columns between two x labels.
const X_LABEL_GAP: u16 = 3;

pub(super) fn draw(f: &mut Frame<'_>, series: &Series, theme: &Theme, area: Rect) {
    let segments: Vec<Vec<Vec<(f64, f64)>>> = series.y.iter().map(|y| segments(&series.positions, y)).collect();
    let datasets = segments.iter().enumerate().flat_map(|(column, runs)| {
        let style = Style::default().fg(colour(theme, column));
        runs.iter().map(move |run| Dataset::default().marker(Marker::Braille).graph_type(GraphType::Line).style(style).data(run))
    });
    let x = bounds(series.positions.iter().copied());
    let y = bounds(series.y.iter().flat_map(|column| column.values.iter().flatten().copied()));
    let axis_style = Style::default().fg(theme.faded);
    let chart = Chart::new(datasets.collect())
        .x_axis(Axis::default().bounds(x).labels(x_labels(series, x, area.width)).style(axis_style))
        .y_axis(Axis::default().bounds(y).labels(spread(y, Y_LABELS).map(short_number)).style(axis_style));
    f.render_widget(chart, area);
}

/// The runs of points with a value: a missing one ends a run, so the line breaks there.
fn segments(positions: &[f64], y: &YColumn) -> Vec<Vec<(f64, f64)>> {
    let points = positions.iter().zip(&y.values).map(|(&x, value)| value.map(|value| (x, value)));
    let runs = points.collect::<Vec<_>>();
    runs.split(Option::is_none).map(|run| run.iter().flatten().copied().collect::<Vec<_>>()).filter(|run| !run.is_empty()).collect()
}

/// The lowest and highest value, widened by one each way when they are the same.
fn bounds(values: impl Iterator<Item = f64>) -> [f64; 2] {
    let [low, high] = values.fold([f64::INFINITY, f64::NEG_INFINITY], |[low, high], value| [low.min(value), high.max(value)]);
    let is_flat = high <= low;
    if is_flat { [low - 1.0, low + 1.0] } else { [low, high] }
}

/// `count` values from one bound to the other, evenly apart.
fn spread([low, high]: [f64; 2], count: u16) -> impl Iterator<Item = f64> {
    let step = (high - low) / f64::from(count - 1);
    (0..count).map(move |at| low + step * f64::from(at))
}

/// As many x labels as fit side by side, written as the x column shows its values.
fn x_labels(series: &Series, x: [f64; 2], width: u16) -> Vec<Line<'static>> {
    let widest = series.x_label_at(x[0]).chars().count().max(series.x_label_at(x[1]).chars().count()) as u16;
    let count = (width / (widest + X_LABEL_GAP)).clamp(MIN_X_LABELS, MAX_X_LABELS);
    spread(x, count).map(|position| Line::from(printable(&series.x_label_at(position)).into_owned())).collect()
}

#[cfg(test)]
mod tests {
    use super::super::tests::{render, series};
    use super::*;
    use crate::db::model::{ColumnKind, ResultSet, Value};
    use crate::fixtures::column;

    fn daily(orders: &[Option<i64>]) -> ResultSet {
        let day = |at: usize| Value::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Days::new(at as u64 * 2));
        let rows = orders.iter().enumerate().map(|(at, orders)| vec![day(at), orders.map_or(Value::Null, Value::Int)]).collect();
        ResultSet::whole(vec![column("day", ColumnKind::Date), column("orders", ColumnKind::Int)], rows)
    }

    #[test]
    fn a_missing_value_breaks_the_line() {
        let y = YColumn { name: "orders".into(), values: vec![Some(1.0), None, Some(3.0), Some(4.0)] };
        assert_eq!(segments(&[0.0, 1.0, 2.0, 3.0], &y), [vec![(0.0, 1.0)], vec![(2.0, 3.0), (3.0, 4.0)]]);
    }

    #[test]
    fn a_flat_line_gets_room_above_and_below() {
        assert_eq!(bounds([5.0, 5.0].into_iter()), [4.0, 6.0]);
    }

    #[test]
    fn a_line_over_days_is_labelled_with_dates_and_short_numbers() {
        let orders = [3, 1200, 450, 2600, 900, 4100].map(Some);
        insta::assert_snapshot!("line", render(&series(&daily(&orders), "line"), 60, 10).backend());
    }

    #[test]
    fn a_narrow_line_keeps_only_its_end_labels() {
        let orders = [3, 1200, 450, 2600].map(Some);
        let drawn = series(&daily(&orders), "line").unwrap();
        let ends = [drawn.positions[0], drawn.positions[3]];
        assert_eq!(x_labels(&drawn, ends, 20).len(), 2);
    }
}
