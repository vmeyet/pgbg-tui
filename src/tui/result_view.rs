//! The result pane: its row count and time in the title, the grid or the chart inside, or what the statement came to.
use super::app::{App, Focus, Room, Run, View};
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
    let focused = app.focus == Focus::Result;
    let run = &app.sheet().run;
    let block = pane::framed(&app.theme, &title(run), focused);
    let inside = block.inner(area);
    f.render_widget(block, area);
    let body = match run {
        Run::Done { result, grid, view: View::Grid, .. } => {
            return grid_view::draw(f, GridView { result, grid, theme: &app.theme, focused }, inside);
        }
        Run::Done { view: View::Chart(chart), .. } => {
            chart_view::draw(f, chart, &app.theme, inside);
            return Room::default();
        }
        Run::Idle => Text::styled("⏎ runs the statement under the cursor", Style::default().fg(app.theme.faded)),
        Run::Running { .. } => Text::styled("running… esc cancels it", Style::default().fg(app.theme.muted)),
        Run::Cancelling { .. } => Text::styled("cancelling…", Style::default().fg(app.theme.muted)),
        Run::Failed(err) => printable_lines(err).style(Style::default().fg(app.theme.danger)),
    };
    f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inside);
    Room::default()
}

/// Each line of `text` escaped on its own, so its line breaks still break lines.
fn printable_lines(text: &str) -> Text<'static> {
    text.lines().map(|line| Line::from(render::printable(line).into_owned())).collect()
}

fn title(run: &Run) -> String {
    match run {
        Run::Done { result, took, view, .. } => {
            format!("Result ─ {} · {} ms ─ {}", row_count(result), took.as_millis(), view_switch(view))
        }
        _ => "Result".to_owned(),
    }
}

/// `first 1 500 rows · more…` while more pages wait in the portal, else the count every other place says.
fn row_count(result: &ResultSet) -> String {
    let count = render::row_count(result);
    if result.extent() == Extent::More { format!("{count} · more…") } else { count }
}

/// `[grid] chart` or `grid [chart]`: the view shown in brackets, the other one `v` switches to.
const fn view_switch(view: &View) -> &'static str {
    match view {
        View::Grid => "[grid] chart",
        View::Chart(_) => "grid [chart]",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::ChartError;
    use crate::fixtures::page;

    #[test]
    fn a_result_waiting_on_more_pages_says_so() {
        let mut result = ResultSet::new(vec![], page(500, Extent::More));
        result.push(page(1000, Extent::More));
        assert_eq!(row_count(&result), "first 1 500 rows · more…");
    }

    #[test]
    fn a_result_paged_to_its_end_says_it_has_all_its_rows() {
        let mut result = ResultSet::new(vec![], page(500, Extent::More));
        result.push(page(1232, Extent::All));
        assert_eq!(row_count(&result), "all 1 732 rows");
    }

    #[test]
    fn a_result_of_one_page_counts_its_rows() {
        assert_eq!(row_count(&ResultSet::new(vec![], page(12, Extent::All))), "12 rows");
    }

    #[test]
    fn a_result_whose_portal_closed_early_says_its_rows_are_the_first_ones() {
        let mut result = ResultSet::new(vec![], page(500, Extent::More));
        result.push(page(0, Extent::Cut));
        assert_eq!(row_count(&result), "first 500 rows");
    }

    #[test]
    fn the_title_brackets_the_view_shown() {
        assert_eq!([view_switch(&View::Grid), view_switch(&View::Chart(Err(ChartError::NoX)))], ["[grid] chart", "grid [chart]"]);
    }
}
