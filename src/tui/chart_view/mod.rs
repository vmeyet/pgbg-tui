//! The result pane as a chart: a `Series` drawn as bars or a line, one colour per y column, or why there is none.
use crate::chart::{ChartError, ChartKind, Series};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use vtui::theme::Theme;

mod bar;
mod line;

const UNITS: [(f64, &str); 4] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "k")];

/// The chart of a result, or why it has none, in `area`.
pub(super) fn draw(f: &mut Frame<'_>, chart: &Result<Series, ChartError>, theme: &Theme, area: Rect) {
    match chart {
        Ok(series) => draw_series(f, series, theme, area),
        Err(error) => f.render_widget(no_chart(error, theme), area),
    }
}

fn no_chart<'a>(error: &ChartError, theme: &Theme) -> Paragraph<'a> {
    let why = Line::styled(format!("no chart: {error}"), Style::default().fg(theme.warn));
    let back = Line::styled("v shows the grid again", Style::default().fg(theme.faded));
    Paragraph::new(vec![why, back]).wrap(Wrap { trim: false })
}

fn draw_series(f: &mut Frame<'_>, series: &Series, theme: &Theme, area: Rect) {
    let legend_rows = u16::from(series.y.len() > 1);
    let [legend_area, plot] = Layout::vertical([Constraint::Length(legend_rows), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(legend(series, theme)), legend_area);
    match series.kind {
        ChartKind::Bar => bar::draw(f, series, theme, plot),
        ChartKind::Line => line::draw(f, series, theme, plot),
    }
}

/// `■ orders  ■ refunds`, each square in its column's colour.
fn legend(series: &Series, theme: &Theme) -> Line<'static> {
    let entries = series.y.iter().enumerate().flat_map(|(column, y)| {
        [
            Span::styled("■ ", Style::default().fg(colour(theme, column))),
            Span::styled(format!("{}  ", y.name), Style::default().fg(theme.muted)),
        ]
    });
    Line::from(entries.collect::<Vec<_>>())
}

/// Each y column takes the next of vtui's user colours, round again after the last.
fn colour(theme: &Theme, column: usize) -> Color {
    theme.users[column % theme.users.len()]
}

/// A number in a few characters: `12.5`, `1.2k`, `3.4M`.
fn short_number(value: f64) -> String {
    let unit = UNITS.iter().find(|(size, _)| value.abs() >= *size);
    let (scaled, suffix) = unit.map_or((value, ""), |&(size, suffix)| (value / size, suffix));
    let decimals = if scaled.abs() < 10.0 { 2 } else { 1 };
    let text = format!("{scaled:.decimals$}");
    format!("{}{suffix}", text.trim_end_matches('0').trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::ChartSpec;
    use crate::db::model::{Column, ColumnKind, ResultSet, Value};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    pub(super) fn render(chart: &Result<Series, ChartError>, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, chart, &Theme::default(), f.area())).unwrap();
        terminal
    }

    pub(super) fn series(result: &ResultSet, spec: &str) -> Result<Series, ChartError> {
        Series::from_result(result, &spec.parse::<ChartSpec>().unwrap())
    }

    pub(super) fn sales() -> ResultSet {
        let column = |name: &str, kind| Column { name: name.to_owned(), kind, type_name: String::new() };
        let row = |customer: &str, orders: i64, refunds: i64| vec![Value::Text(customer.into()), Value::Int(orders), Value::Int(refunds)];
        ResultSet {
            columns: vec![column("customer", ColumnKind::Text), column("orders", ColumnKind::Int), column("refunds", ColumnKind::Int)],
            rows: vec![row("nina", 1200, 30), row("Zoë Nakamura", 450, 0), row("noel", 3, 1)],
            is_cut: false,
        }
    }

    #[test]
    fn numbers_are_written_short() {
        let written = [0.0, 3.0, 0.25, 12.5, 999.0, 1200.0, 45_000.0, 3_400_000.0, 2e9, -1500.0].map(short_number);
        assert_eq!(written, ["0", "3", "0.25", "12.5", "999", "1.2k", "45k", "3.4M", "2B", "-1.5k"]);
    }

    #[test]
    fn several_y_columns_get_a_legend() {
        insta::assert_snapshot!("legend", render(&series(&sales(), "bar"), 40, 8).backend());
    }

    #[test]
    fn a_result_that_cannot_be_charted_says_why() {
        insta::assert_snapshot!("no_chart", render(&Err(ChartError::NoX), 50, 3).backend());
    }
}
