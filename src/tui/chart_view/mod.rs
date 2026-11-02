//! The result pane as a chart: a `Series` drawn as bars or a line, one colour per y column, or why there is none.
use crate::chart::{ChartError, ChartKind, Series};
use crate::render::printable;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use vtui::theme::Theme;

mod bar;
mod line;

const UNITS: [(f64, &str); 4] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "k")];
/// The order series take vtui's user colours in: neighbours, which touch in a stack, stay apart on every palette, where the
/// palette's own order puts the two blues of Nord or Tokyo Night side by side.
const SERIES_ORDER: [usize; 6] = [0, 5, 1, 4, 2, 3];
/// The legend takes at most one row in this many, so the plot keeps the pane.
const PANE_ROWS_PER_LEGEND_ROW: u16 = 3;

/// The chart of a result in `area`, or why it has none.
pub(super) fn draw(f: &mut Frame<'_>, chart: &Result<Series, ChartError>, theme: &Theme, area: Rect) {
    match chart {
        Ok(series) => draw_series(f, series, theme, area),
        Err(error) => f.render_widget(no_chart(error, theme), area),
    }
}

fn no_chart<'a>(error: &ChartError, theme: &Theme) -> Paragraph<'a> {
    let why = Line::styled(printable(&format!("no chart: {error}")).into_owned(), Style::default().fg(theme.warn));
    let back = Line::styled("v shows the grid again", Style::default().fg(theme.faded));
    Paragraph::new(vec![why, back]).wrap(Wrap { trim: false })
}

fn draw_series(f: &mut Frame<'_>, series: &Series, theme: &Theme, area: Rect) {
    let legend = if series.y.len() > 1 { legend(series, theme, area.width) } else { vec![] };
    let legend_rows = u16::try_from(legend.len()).unwrap_or(u16::MAX).min(area.height / PANE_ROWS_PER_LEGEND_ROW);
    let [legend_area, plot] = Layout::vertical([Constraint::Length(legend_rows), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(legend), legend_area);
    match series.kind {
        ChartKind::Bar => bar::draw(f, series, theme, plot),
        ChartKind::Line => line::draw(f, series, theme, plot),
    }
}

/// `■ orders  ■ refunds`, each name in its series' colour, going on to the next row when `width` is full.
fn legend(series: &Series, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = vec![];
    for (column, y) in series.y.iter().enumerate() {
        let entry = Span::styled(format!("■ {}  ", printable(y.name.as_str())), Style::default().fg(colour(theme, column)));
        match rows.last_mut() {
            Some(row) if row.width() + entry.width() <= usize::from(width) => row.spans.push(entry),
            _ => rows.push(Line::from(entry)),
        }
    }
    rows
}

/// The colour of the series in y column `column`: the seventh takes the colour of the first, and so on.
fn colour(theme: &Theme, column: usize) -> Color {
    theme.users[SERIES_ORDER[column % SERIES_ORDER.len()]]
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
    use crate::db::model::{ColumnKind, ResultSet, Value};
    use crate::fixtures::column;
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
        let row = |customer: &str, orders: i64, refunds: i64| vec![Value::Text(customer.into()), Value::Int(orders), Value::Int(refunds)];
        ResultSet::whole(
            vec![column("customer", ColumnKind::Text), column("orders", ColumnKind::Int), column("refunds", ColumnKind::Int)],
            vec![row("nina", 1200, 30), row("Zoë Nakamura", 450, 0), row("noel", 3, 1)],
        )
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

    /// A customer and `columns` numeric columns, `series_0` first.
    fn wide(columns: usize) -> ResultSet {
        let names = (0..columns).map(|at| column(&format!("series_{at}"), ColumnKind::Int));
        let values = (0..columns).map(|at| Value::Int(at as i64 + 1));
        ResultSet::whole(
            std::iter::once(column("customer", ColumnKind::Text)).chain(names).collect(),
            vec![std::iter::once(Value::Text("nina".into())).chain(values).collect()],
        )
    }

    #[test]
    fn a_legend_too_wide_for_one_row_goes_on_to_the_next() {
        insta::assert_snapshot!("legend_rows", render(&series(&wide(5), "bar"), 28, 12).backend());
    }

    #[test]
    fn the_legend_never_takes_the_pane_from_the_plot() {
        let drawn = render(&series(&wide(6), "bar"), 12, 2).backend().to_string();
        assert_eq!(drawn.matches('■').count(), 0, "{drawn}");
    }

    #[test]
    fn a_legend_name_is_drawn_in_the_colour_of_its_series() {
        let terminal = render(&series(&sales(), "bar"), 40, 8);
        let buffer = terminal.backend().buffer();
        let theme = Theme::default();
        assert_eq!([buffer[(2, 0)].fg, buffer[(12, 0)].fg], [colour(&theme, 0), colour(&theme, 1)]);
    }

    fn every_palette() -> impl Iterator<Item = Theme> {
        Theme::NAMES.into_iter().map(|name| Theme::named(name).unwrap())
    }

    fn distance(left: Color, right: Color) -> Option<f64> {
        let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (left, right) else { return None };
        let squares = [(r1, r2), (g1, g2), (b1, b2)].map(|(one, other)| f64::from(one.abs_diff(other)).powi(2));
        Some(squares.iter().sum::<f64>().sqrt())
    }

    #[test]
    fn six_series_get_six_colours_none_of_them_the_ground_on_every_palette() {
        for theme in every_palette() {
            let colours: Vec<Color> = (0..SERIES_ORDER.len()).map(|at| colour(&theme, at)).collect();
            let distinct: std::collections::HashSet<String> = colours.iter().map(|colour| format!("{colour:?}")).collect();
            assert_eq!(distinct.len(), colours.len(), "{}", theme.name);
            assert!(!colours.contains(&theme.base), "{}", theme.name);
        }
    }

    #[test]
    fn neighbouring_series_stay_far_apart_on_every_rgb_palette() {
        const FAR_APART: f64 = 60.0;
        for theme in every_palette() {
            for at in 0..SERIES_ORDER.len() {
                let apart = distance(colour(&theme, at), colour(&theme, at + 1));
                assert!(apart.is_none_or(|apart| apart >= FAR_APART), "{} {at}: {apart:?}", theme.name);
            }
        }
    }

    #[test]
    fn colours_start_over_past_the_palette() {
        let theme = Theme::default();
        assert_eq!(colour(&theme, SERIES_ORDER.len() + 2), colour(&theme, 2));
    }

    #[test]
    fn a_label_or_a_column_name_cannot_reverse_the_text_around_it() {
        let result = ResultSet::whole(
            vec![column("customer", ColumnKind::Text), column("total\u{202e}", ColumnKind::Int), column("refunds", ColumnKind::Int)],
            vec![vec![Value::Text("nina\u{2067}".into()), Value::Int(12), Value::Int(1)]],
        );
        let drawn = render(&series(&result, "bar"), 60, 6).backend().to_string();
        assert!(drawn.contains("\\u{2067}") && drawn.contains("\\u{202e}"), "{drawn}");
        assert!(!drawn.contains(['\u{202e}', '\u{2067}']), "{drawn}");
    }

    #[test]
    fn a_result_that_cannot_be_charted_says_why() {
        insta::assert_snapshot!("no_chart", render(&Err(ChartError::NoX), 50, 3).backend());
    }
}
