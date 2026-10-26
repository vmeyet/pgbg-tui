//! The result pane: its row count and time in the title, and as many rows as fit.
use super::app::{App, Run};
use crate::db::model::ResultSet;
use crate::render;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::{Paragraph, Wrap};
use vtui::pane;

/// The pane in `area` for the last statement run.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let block = pane::framed(&app.theme, &title(&app.run), false);
    let inside = block.inner(area);
    f.render_widget(block, area);
    let body = match &app.run {
        Run::Idle => Text::styled("⏎ runs the statement in the editor", Style::default().fg(app.theme.faded)),
        Run::Running => Text::styled("running…", Style::default().fg(app.theme.muted)),
        Run::Done { result, .. } => Text::raw(visible_rows(result, usize::from(inside.height))),
        Run::Failed(err) => Text::styled(err.as_str(), Style::default().fg(app.theme.danger)),
    };
    f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inside);
}

fn title(run: &Run) -> String {
    match run {
        Run::Done { result, took } => format!("Result ─ {} · {} ms", render::row_count(result.rows.len()), took.as_millis()),
        _ => "Result".to_owned(),
    }
}

/// The header and the rows that fit under it in `height` lines; the others are never formatted.
fn visible_rows(result: &ResultSet, height: usize) -> String {
    let header = result.columns.iter().map(|column| column.name.clone()).collect();
    let rows = result.rows.iter().take(height.saturating_sub(1)).map(|row| row.iter().map(ToString::to_string).collect());
    render::table(&std::iter::once(header).chain(rows).collect::<Vec<_>>())
}
