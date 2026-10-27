//! The sidebar: the saved queries, `▾` and `▸` folders, the queries under them, `●` on the one in the editor; or the
//! schema of the sheet on screen, its schemas and their tables.
use super::app::{App, Focus, Overlay, SchemaRow, SchemaState, SidebarTab};
use crate::connection::SchemaReading;
use crate::query::TreeRow;
use crate::render::printable;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use vtui::pane;
use vtui::text::truncate;

/// The tab shown in `area`, its title naming both, the shown one in brackets.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let focused = app.focus == Focus::Queries;
    let title = match app.sidebar {
        SidebarTab::Queries => "[Queries] Schema",
        SidebarTab::Schema => "Queries [Schema]",
    };
    let block = pane::framed(&app.theme, title, focused);
    let inside = block.inner(area);
    f.render_widget(block, area);
    match app.sidebar {
        SidebarTab::Queries => draw_queries(f, app, inside, focused),
        SidebarTab::Schema => draw_schema(f, app, inside, focused),
    }
}

/// The tree of queries, scrolled so the cursor's row shows; the cursor is drawn while the sidebar has the keys.
fn draw_queries(f: &mut Frame<'_>, app: &App, inside: Rect, focused: bool) {
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

/// The schema's tables under the filter line, scrolled so the cursor's row shows, or where the read stands.
fn draw_schema(f: &mut Frame<'_>, app: &App, inside: Rect, focused: bool) {
    let Some(catalog) = app.sheet().catalog() else {
        f.render_widget(Paragraph::new(schema_state(app)).wrap(Wrap { trim: false }), inside);
        return;
    };
    let is_filtering = app.overlay == Some(Overlay::SchemaFilter);
    let has_filter_line = is_filtering || !app.schema_tab.filter.text().is_empty();
    let [filter_area, rows_area] = Layout::vertical([Constraint::Length(u16::from(has_filter_line)), Constraint::Fill(1)]).areas(inside);
    if has_filter_line {
        let cursor = if is_filtering { "▌" } else { "" };
        let filter = Span::raw(format!("/{}", printable(app.schema_tab.filter.text())));
        f.render_widget(Paragraph::new(Line::from(vec![filter, Span::styled(cursor, Style::default().fg(app.theme.accent))])), filter_area);
    }
    let rows = app.schema_tab.rows(catalog);
    if rows.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("no table holds it", Style::default().fg(app.theme.faded))), rows_area);
        return;
    }
    let height = usize::from(rows_area.height);
    let selected = app.schema_tab.selected.min(rows.len() - 1);
    let scroll = selected.saturating_sub(height.saturating_sub(1));
    let width = usize::from(rows_area.width);
    let lines: Vec<Line<'_>> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(at, row)| schema_line(app, *row, focused && at == selected, width))
        .collect();
    f.render_widget(Paragraph::new(lines), rows_area);
}

/// What the tab says before a schema is read: off, not read yet, being read, or why the read failed.
fn schema_state(app: &App) -> Line<'static> {
    let sheet = app.sheet();
    let faded = Style::default().fg(app.theme.faded);
    if sheet.connection.schema() == SchemaReading::Off {
        return Line::styled("schema = \"off\" on this connection", faded);
    }
    match &sheet.schema {
        SchemaState::Failed(why) => Line::styled(format!("✗ {}", printable(why)), Style::default().fg(app.theme.danger)),
        SchemaState::Reading => Line::styled("reading the schema…", faded),
        SchemaState::NotRead | SchemaState::Read(_) => Line::styled("r reads the schema", faded),
    }
}

fn schema_line(app: &App, row: SchemaRow<'_>, is_cursor: bool, width: usize) -> Line<'static> {
    let theme = &app.theme;
    let bar = Span::styled(if is_cursor { "▎" } else { " " }, Style::default().fg(theme.accent));
    let (indent, mark, name, style) = match row {
        SchemaRow::Schema { name, is_open } => {
            ("", if is_open { "▾ " } else { "▸ " }, printable(name).into_owned(), Style::default().fg(theme.muted))
        }
        SchemaRow::Table { table, .. } => ("  ", "  ", printable(&table.name).into_owned(), Style::default()),
    };
    let room = width.saturating_sub(1 + indent.len() + 2);
    Line::from(vec![
        bar,
        Span::raw(indent),
        Span::styled(mark, Style::default().fg(theme.accent)),
        Span::styled(truncate(&name, room), style),
    ])
}
