//! Bars, drawn horizontally so a long text label stays readable: one row per bar, a group of rows per point.
use super::{colour, short_number};
use crate::chart::Series;
use crate::render::printable;
use ratatui::Frame;
use ratatui::layout::{Direction, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Bar, BarChart, BarGroup};
use vtui::theme::Theme;

/// The length of the longest bar in the widget's own units, fine enough for any terminal width.
const FULL_BAR: u32 = 10_000;

pub(super) fn draw(f: &mut Frame<'_>, series: &Series, theme: &Theme, area: Rect) {
    let longest = series.y.iter().flat_map(|column| column.values.iter().flatten()).fold(0.0, |longest: f64, &value| longest.max(value));
    let groups: Vec<BarGroup<'_>> = (0..series.labels.len()).map(|point| group(series, point, longest, theme)).collect();
    let chart = BarChart::grouped(groups)
        .direction(Direction::Horizontal)
        .bar_width(1)
        .bar_gap(0)
        .group_gap(u16::from(series.y.len() > 1))
        .max(u64::from(FULL_BAR))
        .label_style(Style::default().fg(theme.muted));
    f.render_widget(chart, area);
}

/// The bars of one point, with its label on the first bar.
fn group<'a>(series: &'a Series, point: usize, longest: f64, theme: &Theme) -> BarGroup<'a> {
    let bars: Vec<Bar<'a>> = series.y.iter().enumerate().map(|(column, y)| bar(y.values[point], longest, theme, column)).collect();
    let labelled: Vec<Bar<'a>> = bars
        .into_iter()
        .enumerate()
        .map(|(at, bar)| if at == 0 { bar.label(printable(&series.labels[point]).into_owned()) } else { bar })
        .collect();
    BarGroup::new(labelled)
}

/// A bar whose length is `value` relative to `longest`; a negative value draws no bar, and a missing value draws
/// nothing at all.
fn bar<'a>(value: Option<f64>, longest: f64, theme: &Theme, column: usize) -> Bar<'a> {
    let length = value.map_or(0, |value| (value.max(0.0) / longest * f64::from(FULL_BAR)) as u64);
    let fill = colour(theme, column);
    Bar::default()
        .value(length)
        .text_value(value.map(short_number).unwrap_or_default())
        .style(Style::default().fg(fill))
        .value_style(Style::default().fg(theme.base).bg(fill))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{render, sales, series};
    use crate::chart::ChartError;

    #[test]
    fn bars_are_as_long_as_their_value_and_say_it() {
        insta::assert_snapshot!("bars", render(&series(&sales(), "bar y=orders"), 40, 4).backend());
    }

    #[test]
    fn a_gap_draws_no_bar() {
        let sales = sales().with_cell(1, 1, crate::db::model::Value::Null);
        let drawn: Result<_, ChartError> = series(&sales, "bar y=orders");
        insta::assert_snapshot!("bars_gap", render(&drawn, 40, 4).backend());
    }
}
