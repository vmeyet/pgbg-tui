//! The SQL editor pane: the text in its syntax colours, the cursor kept in view, the server's error position, and whether the connection takes writes.
use super::app::{App, Focus};
use super::theme;
use crate::connection::WritePolicy;
use crate::syntax::{Spans, Token};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use std::ops::Range;
use vtui::pane;
use vtui::theme::Theme;

/// The editor in `area`, scrolled so the cursor's line shows, with the terminal's cursor on it when it has the keys.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let focused = app.focus == Focus::Editor;
    let block = pane::framed(&app.theme, &title(app), focused).title(badge(app));
    let inside = block.inner(area);
    f.render_widget(block, area);
    let (before, _, _) = app.editor.split();
    let row = before.matches('\n').count() as u16;
    let column = Line::from(before.rsplit('\n').next().unwrap_or_default()).width() as u16;
    let scroll = row.saturating_sub(inside.height.saturating_sub(1));
    let mark = Style::default().fg(app.theme.danger).add_modifier(Modifier::REVERSED);
    let text = coloured(app.editor.text(), &app.colours, app.error_mark, mark, &app.theme);
    f.render_widget(Paragraph::new(text).scroll((scroll, 0)), inside);
    if !focused {
        return;
    }
    let x = (inside.x + column).min(inside.right().saturating_sub(1));
    f.set_cursor_position(Position { x, y: inside.y + row - scroll });
}

/// `text` line by line, each token in its colour, and the character at byte `mark` in `mark_style` on top;
/// a mark on a line's end draws a space there.
fn coloured<'a>(text: &'a str, colours: &[Spans], mark: Option<usize>, mark_style: Style, theme: &Theme) -> Text<'a> {
    let mut line_start = 0;
    let mut lines = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let marked = mark.and_then(|at| at.checked_sub(line_start)).filter(|&at| at <= line.len()).map(|at| marked_range(line, at));
        let spans = colours.get(index).map_or(&[][..], Vec::as_slice);
        lines.push(coloured_line(line, spans, marked, mark_style, theme));
        line_start += line.len() + 1;
    }
    Text::from(lines)
}

/// The bytes of the character at `at`; empty at the line's end.
fn marked_range(line: &str, at: usize) -> Range<usize> {
    at..at + line[at..].chars().next().map_or(0, char::len_utf8)
}

/// The line cut wherever a token or the mark starts or stops.
fn coloured_line<'a>(
    line: &'a str,
    spans: &[(Range<usize>, Token)],
    mark: Option<Range<usize>>,
    mark_style: Style,
    theme: &Theme,
) -> Line<'a> {
    let mut edges: Vec<usize> = spans.iter().map(|(range, _)| range).chain(&mark).flat_map(|range| [range.start, range.end]).collect();
    edges.extend([0, line.len()]);
    edges.retain(|&edge| edge <= line.len() && line.is_char_boundary(edge));
    edges.sort_unstable();
    edges.dedup();
    let token_colour = |at: usize| spans.iter().find(|(range, _)| range.contains(&at)).map(|(_, token)| theme::token(theme, *token));
    let style_at = |at: usize| {
        let is_marked = mark.as_ref().is_some_and(|range| range.contains(&at));
        if is_marked { mark_style } else { token_colour(at).map_or_else(Style::default, |colour| Style::default().fg(colour)) }
    };
    let mut pieces: Vec<Span<'a>> = edges.windows(2).map(|piece| Span::styled(&line[piece[0]..piece[1]], style_at(piece[0]))).collect();
    if mark.is_some_and(|range| range.is_empty()) {
        pieces.push(Span::styled(" ", mark_style));
    }
    Line::from(pieces)
}

/// `Editor`, then the path of the saved query in it.
fn title(app: &App) -> String {
    app.queries.opened.as_ref().map_or_else(|| "Editor".to_owned(), |path| format!("Editor · {path}"))
}

fn badge(app: &App) -> Span<'static> {
    match app.writes {
        WritePolicy::ReadOnly => Span::styled(" READ ONLY ", Style::default().fg(app.theme.muted)),
        WritePolicy::Allowed => Span::styled(" WRITES ", Style::default().fg(app.theme.danger).add_modifier(Modifier::BOLD)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::tests::typed;
    use super::*;
    use crate::syntax;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::{Backend as _, TestBackend};

    fn marked_text(text: &str, mark: usize) -> Vec<String> {
        let style = Style::default().add_modifier(Modifier::REVERSED);
        let shown = |span: &Span<'_>| if span.style == style { format!("[{}]", span.content) } else { span.content.to_string() };
        let text = coloured(text, &[], Some(mark), style, &Theme::default());
        text.lines.iter().map(|line| line.spans.iter().map(shown).collect()).collect()
    }

    fn painted(text: &str, mark: Option<usize>) -> String {
        let mark_style = Style::default().fg(Theme::default().danger).add_modifier(Modifier::REVERSED);
        let text = coloured(text, &syntax::highlight(text), mark, mark_style, &Theme::default());
        let piece = |span: &Span<'_>| format!("{:?}/{:?} {:?}", span.style.fg, span.style.add_modifier, span.content);
        text.lines.iter().map(|line| line.spans.iter().map(piece).collect::<Vec<_>>().join("\n")).collect::<Vec<_>>().join("\n---\n")
    }

    #[test]
    fn the_marked_character_is_drawn_in_the_mark_style() {
        assert_eq!(marked_text("SELECT *\nFROM ordrs\nLIMIT 1", 14), ["SELECT *", "FROM [o]rdrs", "LIMIT 1"]);
    }

    #[test]
    fn a_mark_at_the_end_of_a_line_or_the_text_draws_a_space() {
        assert_eq!(marked_text("SELECT\nFROM", 6), ["SELECT[ ]", "FROM"]);
        assert_eq!(marked_text("SELECT", 6), ["SELECT[ ]"]);
    }

    #[test]
    fn the_error_mark_draws_over_a_coloured_token() {
        insta::assert_snapshot!("coloured_marked", painted("SELECT id, 'paid', 10 -- recent\nFROM ordrs", Some(37)));
    }

    #[test]
    fn the_cursor_follows_the_text_and_the_editor_scrolls_to_keep_it() {
        let mut app = typed("SELECT");
        for _ in 0..4 {
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        }
        app.handle_key(KeyEvent::from(KeyCode::Char('1')));
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        terminal.draw(|f| draw(f, &app, f.area())).unwrap();
        assert_eq!(terminal.backend_mut().get_cursor_position().unwrap(), Position { x: 3, y: 3 });
        insta::assert_snapshot!("editor_scrolled", terminal.backend());
    }
}
