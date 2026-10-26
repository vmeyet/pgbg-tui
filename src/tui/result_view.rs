//! The result pane: its row count and time in the title, the grid or the chart inside, or what the statement came to.
use super::app::{App, Focus, Run, View};
use super::chart_view;
use super::grid_view::{self, GridView};
use super::theme;
use crate::render;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::{Paragraph, Wrap};
use vtui::pane;

/// The pane in `area` for the last statement run; returns the grid rows it had room for.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) -> usize {
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
            return 0;
        }
        Run::Idle => Text::styled("⏎ runs the statement under the cursor", Style::default().fg(app.theme.faded)),
        Run::Running { .. } => Text::styled("running… esc cancels it", Style::default().fg(app.theme.muted)),
        Run::Cancelling { .. } => Text::styled("cancelling…", Style::default().fg(app.theme.muted)),
        Run::Failed(err) => Text::styled(err.as_str(), Style::default().fg(theme::failure(&app.theme))),
    };
    f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inside);
    0
}

fn title(run: &Run) -> String {
    match run {
        Run::Done { result, took, view, .. } => {
            format!("Result ─ {} · {} ms ─ {}", render::row_count(result), took.as_millis(), view_switch(view))
        }
        _ => "Result".to_owned(),
    }
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

    #[test]
    fn the_title_brackets_the_view_shown() {
        assert_eq!([view_switch(&View::Grid), view_switch(&View::Chart(Err(ChartError::NoX)))], ["[grid] chart", "grid [chart]"]);
    }
}
