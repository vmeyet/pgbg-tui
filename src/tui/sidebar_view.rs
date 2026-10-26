//! The sidebar of saved queries: `▾` and `▸` folders, the queries under them, `●` on the one in the editor.
use super::app::{App, Focus};
use crate::query::TreeRow;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use vtui::pane;
use vtui::text::truncate;

/// The tree in `area`, scrolled so the cursor's row shows; the cursor is drawn while the sidebar has the keys.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let focused = app.focus == Focus::Queries;
    let block = pane::framed(&app.theme, "Queries", focused);
    let inside = block.inner(area);
    f.render_widget(block, area);
    let height = usize::from(inside.height);
    let scroll = app.queries.selected.saturating_sub(height.saturating_sub(1));
    let is_cursor = |at: usize| focused && at == app.queries.selected;
    let rows = app.queries.rows();
    let lines: Vec<Line<'_>> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(at, row)| line(app, row, is_cursor(at), usize::from(inside.width)))
        .collect();
    f.render_widget(Paragraph::new(lines), inside);
}

fn line(app: &App, row: &TreeRow, is_cursor: bool, width: usize) -> Line<'static> {
    let theme = &app.theme;
    let bar = Span::styled(if is_cursor { "▎" } else { " " }, Style::default().fg(theme.accent));
    let (depth, mark, name, style) = match row {
        TreeRow::Folder { name, depth, is_open, .. } => {
            (depth, if *is_open { "▾ " } else { "▸ " }, format!("{name}/"), Style::default().fg(theme.muted))
        }
        TreeRow::Query { name, depth, query } => {
            let is_open = app.sheet().opened.as_ref() == Some(query);
            let style = if is_open { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
            (depth, if is_open { "● " } else { "  " }, name.clone(), style)
        }
    };
    let indent = "  ".repeat(*depth);
    let room = width.saturating_sub(1 + indent.len() + 2);
    Line::from(vec![
        bar,
        Span::raw(indent),
        Span::styled(mark, Style::default().fg(theme.accent)),
        Span::styled(truncate(&name, room), style),
    ])
}
