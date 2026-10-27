//! The row's detail over the screen: each column's name and type, its whole value under it, wrapped, JSON indented
//! and coloured; every piece of text drawn through `render::printable`.
use super::app::{App, Overlay, Run};
use super::theme;
use crate::act::{self, Pane};
use crate::db::model::{Column, Value};
use crate::render;
use crate::syntax::{self, Painted};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use vtui::pane;
use vtui::theme::Theme;

/// No value is drawn longer, so a 10 MB document costs no more than a page of it.
const VALUE_MAX_CHARS: usize = 64 * 1024;
const INDENT: &str = "  ";
const SHARE_OF_SCREEN_PCT: u16 = 80;

/// The detail when it is open; returns how far it scrolls before its last line is at the bottom.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) -> usize {
    let Some(Overlay::Detail(detail)) = &app.overlay else { return 0 };
    let Run::Done { result, grid, .. } = &app.sheet().run else { return 0 };
    let Some(row) = result.rows.get(grid.row) else { return 0 };
    let area = centred(f.area());
    let block = pane::framed(&app.theme, &format!("Row {} of {}", grid.row + 1, result.rows.len()), true);
    let [body, footer] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(block.inner(area));
    let lines: Vec<Line<'static>> =
        result.columns.iter().zip(row).flat_map(|(column, value)| column_lines(&app.theme, column, value, body.width)).collect();
    let last_scroll = lines.len().saturating_sub(usize::from(body.height));
    let shown: Vec<Line<'static>> = lines.into_iter().skip(detail.scroll.min(last_scroll)).take(usize::from(body.height)).collect();
    f.render_widget(Clear, area);
    f.render_widget(block.style(Style::default().bg(app.theme.surface)), area);
    f.render_widget(Paragraph::new(shown), body);
    f.render_widget(Paragraph::new(Line::styled(hints(), Style::default().fg(app.theme.faded))), footer);
    last_scroll
}

fn centred(area: Rect) -> Rect {
    let width = area.width * SHARE_OF_SCREEN_PCT / 100;
    let height = area.height * SHARE_OF_SCREEN_PCT / 100;
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

fn hints() -> String {
    act::footer(Pane::Detail).map(|hint| format!("{} {}", hint.keys, hint.does)).collect::<Vec<_>>().join(" · ")
}

/// The column's name and type, its value under it, then a blank line.
fn column_lines(theme: &Theme, column: &Column, value: &Value, width: u16) -> Vec<Line<'static>> {
    let name = Span::styled(render::printable(&column.name).into_owned(), Style::default().add_modifier(Modifier::BOLD));
    let type_name = Span::styled(format!("  {}", render::printable(&column.type_name)), Style::default().fg(theme.muted));
    let header = if column.type_name.is_empty() { Line::from(name) } else { Line::from(vec![name, type_name]) };
    let room = usize::from(width).saturating_sub(INDENT.len());
    let value_lines = value_spans(theme, value).into_iter().flat_map(|spans| wrapped(&spans, room));
    let indented = value_lines.map(|spans| Line::from(std::iter::once(Span::raw(INDENT)).chain(spans).collect::<Vec<_>>()));
    std::iter::once(header).chain(indented).chain([Line::default()]).collect()
}

/// Each line of the value as coloured spans, then a note when it was cut.
fn value_spans(theme: &Theme, value: &Value) -> Vec<Vec<Span<'static>>> {
    if *value == Value::Null {
        return vec![vec![Span::styled("NULL", Style::default().fg(theme.faded))]];
    }
    let text = value.pretty_prefix(VALUE_MAX_CHARS + 1);
    let is_cut = text.chars().nth(VALUE_MAX_CHARS).is_some();
    let kept: String = text.chars().take(VALUE_MAX_CHARS).collect();
    let is_json = matches!(value, Value::Json(_));
    let lines = kept.split('\n').map(|line| painted_spans(theme, line, &line_colours(line, is_json)));
    let note = is_cut.then(|| vec![Span::styled(format!("… the first {VALUE_MAX_CHARS} characters"), Style::default().fg(theme.muted))]);
    lines.chain(note).collect()
}

fn line_colours(line: &str, is_json: bool) -> Vec<Painted> {
    if is_json { syntax::highlight_json_line(line) } else { vec![] }
}

/// `line` cut at its painted pieces, each one escaped on its own and in its token's colour.
fn painted_spans(theme: &Theme, line: &str, painted: &[Painted]) -> Vec<Span<'static>> {
    let mut spans = vec![];
    let mut at = 0;
    for piece in painted {
        spans.push(Span::raw(render::printable(&line[at..piece.bytes.start]).into_owned()));
        let colour = Style::default().fg(theme::token(theme, piece.token));
        spans.push(Span::styled(render::printable(&line[piece.bytes.clone()]).into_owned(), colour));
        at = piece.bytes.end;
    }
    spans.push(Span::raw(render::printable(&line[at..]).into_owned()));
    spans.into_iter().filter(|span| !span.content.is_empty()).collect()
}

/// `spans` broken into lines of at most `columns` terminal columns, each piece keeping its style.
fn wrapped(spans: &[Span<'static>], columns: usize) -> Vec<Vec<Span<'static>>> {
    let columns = columns.max(1);
    let mut lines = vec![];
    let mut line: Vec<Span<'static>> = vec![];
    let mut used = 0;
    for (c, style) in spans.iter().flat_map(|span| span.content.chars().map(|c| (c, span.style))) {
        let taken = render::width(c.encode_utf8(&mut [0; 4]));
        if used + taken > columns && used > 0 {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        used += taken;
        match line.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(c),
            _ => line.push(Span::styled(c.to_string(), style)),
        }
    }
    lines.push(line);
    lines
}

#[cfg(test)]
mod tests {
    use super::super::app::tests::{orders, to_shop, typed};
    use super::super::app::{Fit, Incoming, RunId};
    use super::super::styled;
    use super::*;
    use crate::db::model::{ColumnKind, ResultSet};
    use crate::fixtures::column;
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::Duration;

    fn detail_of(result: ResultSet) -> App {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result, took: Duration::from_millis(38) }));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Char('j')));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app
    }

    fn render(app: &App, width: u16, height: u16) -> (Terminal<TestBackend>, usize) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut last_scroll = 0;
        terminal.draw(|f| last_scroll = draw(f, app)).unwrap();
        (terminal, last_scroll)
    }

    fn a_note() -> ResultSet {
        let note = "Leave at the door.\nRing twice: the bell is \u{202e}broken";
        let mut result = orders();
        result.columns.push(Column { type_name: "text".into(), ..column("note", ColumnKind::Text) });
        result.rows.iter_mut().for_each(|row| row.push(Value::Text(note.into())));
        result
    }

    #[test]
    fn each_column_shows_its_name_type_and_whole_value_wrapped() {
        insta::assert_snapshot!("detail", render(&detail_of(a_note()), 40, 20).0.backend());
    }

    #[test]
    fn a_value_from_the_database_cannot_reverse_the_detail() {
        let drawn = render(&detail_of(a_note()), 40, 20).0.backend().to_string();
        assert!(drawn.contains("{202e}broken") && !drawn.contains('\u{202e}'), "{drawn}");
    }

    #[test]
    fn json_is_indented_and_coloured() {
        let items = Value::Json(serde_json::json!({"sku": "TEA-01", "count": 2, "gift": null}));
        let result =
            ResultSet { columns: vec![column("items", ColumnKind::Json)], rows: vec![vec![items.clone()], vec![items]], is_cut: false };
        insta::assert_snapshot!("detail_json_styled", styled::runs(render(&detail_of(result), 40, 12).0.backend().buffer()));
    }

    #[test]
    fn a_detail_taller_than_the_popup_says_how_far_it_scrolls() {
        let (_, last_scroll) = render(&detail_of(a_note()), 40, 10);
        assert_eq!(last_scroll, 9);
    }

    #[test]
    fn a_scrolled_detail_starts_further_down() {
        let mut app = detail_of(a_note());
        app.fit(Fit { detail_last_scroll: 9, ..Fit::default() });
        (0..3).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Char('j'))));
        insta::assert_snapshot!("detail_scrolled", render(&app, 40, 10).0.backend());
    }

    #[test]
    fn a_long_line_wraps_at_the_columns_given_and_keeps_its_style() {
        let style = Style::default().fg(ratatui::style::Color::Red);
        let lines = wrapped(&[Span::raw("ab"), Span::styled("cdé", style)], 2);
        let texts: Vec<Vec<(&str, Style)>> =
            lines.iter().map(|line| line.iter().map(|span| (span.content.as_ref(), span.style)).collect()).collect();
        assert_eq!(texts, [vec![("ab", Style::default())], vec![("cd", style)], vec![("é", style)]]);
    }
}
