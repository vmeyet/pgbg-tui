//! The result pane: its row count and time in the title, the grid inside, or what the statement came to.
use super::app::{App, Focus, Run};
use super::grid_view::{self, GridView};
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
        Run::Done { result, grid, .. } => return grid_view::draw(f, GridView { result, grid, theme: &app.theme, focused }, inside),
        Run::Idle => Text::styled("⏎ runs the statement under the cursor", Style::default().fg(app.theme.faded)),
        Run::Running => Text::styled("running… esc cancels it", Style::default().fg(app.theme.muted)),
        Run::Cancelling => Text::styled("cancelling…", Style::default().fg(app.theme.muted)),
        Run::Failed(err) => Text::styled(err.as_str(), Style::default().fg(app.theme.danger)),
    };
    f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inside);
    0
}

fn title(run: &Run) -> String {
    match run {
        Run::Done { result, took, .. } => format!("Result ─ {} · {} ms", render::row_count(result), took.as_millis()),
        _ => "Result".to_owned(),
    }
}
