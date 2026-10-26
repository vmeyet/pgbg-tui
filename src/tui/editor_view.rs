//! The SQL editor pane: the text, the cursor kept in view, the server's error position, and whether the connection takes writes.
use super::app::{App, Focus};
use crate::connection::WritePolicy;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use vtui::pane;

/// The editor in `area`, scrolled so the cursor's line shows, with the terminal's cursor on it when it has the keys.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let focused = app.focus == Focus::Editor;
    let block = pane::framed(&app.theme, "Editor", focused).title(badge(app));
    let inside = block.inner(area);
    f.render_widget(block, area);
    let (before, _, _) = app.editor.split();
    let row = before.matches('\n').count() as u16;
    let column = Line::from(before.rsplit('\n').next().unwrap_or_default()).width() as u16;
    let scroll = row.saturating_sub(inside.height.saturating_sub(1));
    let mark = Style::default().fg(app.theme.danger).add_modifier(Modifier::REVERSED);
    f.render_widget(Paragraph::new(marked(app.editor.text(), app.error_mark, mark)).scroll((scroll, 0)), inside);
    if !focused {
        return;
    }
    let x = (inside.x + column).min(inside.right().saturating_sub(1));
    f.set_cursor_position(Position { x, y: inside.y + row - scroll });
}

/// `text` with the character at byte `mark` in `style`; a mark on a line's end draws a space there.
fn marked(text: &str, mark: Option<usize>, style: Style) -> Text<'_> {
    let Some((before, rest)) = mark.and_then(|at| text.split_at_checked(at)) else { return Text::raw(text) };
    let marked_len = rest.chars().next().filter(|&c| c != '\n').map_or(0, char::len_utf8);
    let (under, after) = rest.split_at(marked_len);
    let mut lines: Vec<Line<'_>> = before.split('\n').map(Line::raw).collect();
    let mut after_lines = after.split('\n');
    let mut joined = lines.pop().unwrap_or_default().spans;
    joined.push(Span::styled(if under.is_empty() { " " } else { under }, style));
    joined.extend(Line::raw(after_lines.next().unwrap_or_default()).spans);
    lines.push(Line::from(joined));
    lines.extend(after_lines.map(Line::raw));
    Text::from(lines)
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::{Backend as _, TestBackend};

    fn marked_text(text: &str, mark: usize) -> Vec<String> {
        let style = Style::default().add_modifier(Modifier::REVERSED);
        let shown = |span: &Span<'_>| if span.style == style { format!("[{}]", span.content) } else { span.content.to_string() };
        marked(text, Some(mark), style).lines.iter().map(|line| line.spans.iter().map(shown).collect()).collect()
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
