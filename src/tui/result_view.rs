//! The result pane: its row count and time in the title, and the grid, the chart or the statement's outcome inside.
use super::app::{App, Focus, Room, Run, ShownRows, View};
use super::chart_view;
use super::grid_view::{self, GridView};
use crate::db::model::{Extent, ResultSet};
use crate::render;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Wrap};
use vtui::pane;

/// The pane in `area` for the last statement run; returns the room its grid had.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) -> Room {
    let focused = app.focus() == Focus::Result;
    let run = &app.sheet().run;
    let block = pane::framed(app.theme(), &title(run, app.sheet().shown.as_deref()), focused);
    let inside = block.inner(area);
    f.render_widget(block, area);
    let body = match run {
        Run::Done { result, grid, view: View::Grid, .. } => {
            return grid_view::draw(f, GridView { result, grid, theme: app.theme(), focused, clock: app.clock() }, inside);
        }
        Run::Done { view: View::Chart(chart), .. } => {
            chart_view::draw(f, chart, app.theme(), inside);
            return Room::default();
        }
        Run::Idle => Text::styled("⏎ runs the statement under the cursor", Style::default().fg(app.theme().faded)),
        Run::Running { .. } => Text::styled("running… esc cancels it", Style::default().fg(app.theme().muted)),
        Run::Cancelling { .. } => Text::styled("cancelling…", Style::default().fg(app.theme().muted)),
        Run::Failed(err) => printable_lines(err).style(Style::default().fg(app.theme().danger)),
    };
    f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inside);
    Room::default()
}

/// Each line of `text` escaped on its own, so its line breaks still start new lines.
fn printable_lines(text: &str) -> Text<'static> {
    text.lines().map(|line| Line::from(render::printable(line).into_owned())).collect()
}

/// What the result is, `orders · 3 columns · 1 index` for a table's columns, else its rows and time.
fn title(run: &Run, shown: Option<&str>) -> String {
    match (run, shown) {
        (Run::Done { .. }, Some(shown)) => format!("Result ─ {}", render::printable(shown)),
        (Run::Done { result, grid, took, view, .. }, None) => {
            format!("Result ─ {} · {} ms ─ {}", row_count(result, grid.shown()), took.as_millis(), view_switch(view))
        }
        _ => "Result".to_owned(),
    }
}

/// `first 1 500 rows · more…` while more pages wait in the portal, or else the same count as everywhere else; `12 of`
/// those rows while a filter shows only some.
fn row_count(result: &ResultSet, shown: &ShownRows) -> String {
    let count = if shown.filter().is_empty() { render::row_count(result) } else { filtered_count(result, shown) };
    if result.extent() == Extent::More { format!("{count} · more…") } else { count }
}

/// `12 of 3 400 rows`, or `12 of the first 500 rows` when the result does not hold every row.
fn filtered_count(result: &ResultSet, shown: &ShownRows) -> String {
    let loaded = result.rows().len();
    let (kept, of) = (render::grouped(shown.count(loaded)), render::grouped(loaded));
    match result.extent() {
        Extent::All => format!("{kept} of {of} rows"),
        Extent::Cut | Extent::More => format!("{kept} of the first {of} rows"),
    }
}

/// `[grid] chart` or `grid [chart]`: the view shown is in brackets, the other is the one `v` switches to.
const fn view_switch(view: &View) -> &'static str {
    match view {
        View::Grid => "[grid] chart",
        View::Chart(_) => "grid [chart]",
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{on_the_result, press};
    use super::super::ui;
    use super::*;
    use crate::chart::ChartError;
    use crate::db::model::ColumnKind;
    use crate::fixtures::{column, orders, page};
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn a_result_waiting_on_more_pages_says_so() {
        let mut result = ResultSet::new(vec![], page(500, Extent::More));
        result.push(page(1000, Extent::More));
        assert_eq!(row_count(&result, &ShownRows::default()), "first 1 500 rows · more…");
    }

    #[test]
    fn a_result_paged_to_its_end_says_it_has_all_its_rows() {
        let mut result = ResultSet::new(vec![], page(500, Extent::More));
        result.push(page(1232, Extent::All));
        assert_eq!(row_count(&result, &ShownRows::default()), "all 1 732 rows");
    }

    #[test]
    fn a_result_of_one_page_counts_its_rows() {
        assert_eq!(row_count(&ResultSet::new(vec![], page(12, Extent::All)), &ShownRows::default()), "12 rows");
    }

    #[test]
    fn a_result_whose_portal_closed_early_says_its_rows_are_the_first_ones() {
        let mut result = ResultSet::new(vec![], page(500, Extent::More));
        result.push(page(0, Extent::Cut));
        assert_eq!(row_count(&result, &ShownRows::default()), "first 500 rows");
    }

    /// The screen after `keys` on `result`, pressed once a first frame gave the grid its room.
    fn pressed(result: ResultSet, keys: &str) -> String {
        let mut app = on_the_result(result);
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal.draw(|f| app.fit(ui::draw(f, &app))).unwrap();
        keys.chars().for_each(|c| press(&mut app, KeyCode::Char(c)));
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn a_filter_shows_the_rows_it_keeps_and_counts_them_in_the_title() {
        insta::assert_snapshot!("filtered", pressed(orders(), "/na"));
    }

    #[test]
    fn a_filter_on_a_cut_result_counts_among_the_first_rows() {
        let mut result = ResultSet::new(vec![column("n", ColumnKind::Int)], page(500, Extent::More));
        result.push(page(0, Extent::Cut));
        assert!(pressed(result, "/49").contains("Result ─ 15 of the first 500 rows"));
    }

    #[test]
    fn a_sorted_column_says_which_way_in_its_header() {
        insta::assert_snapshot!("sorted_down", pressed(orders(), "lss"));
    }

    #[test]
    fn the_title_brackets_the_view_shown() {
        assert_eq!([view_switch(&View::Grid), view_switch(&View::Chart(Err(ChartError::NoX)))], ["[grid] chart", "grid [chart]"]);
    }
}
