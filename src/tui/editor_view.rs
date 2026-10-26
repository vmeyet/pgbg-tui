//! The SQL editor pane: the text, the cursor kept in view, and whether the connection takes writes.
use super::app::App;
use crate::connection::WritePolicy;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use vtui::pane;

/// The editor in `area`, scrolled so the cursor's line shows, with the terminal's cursor on it.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let block = pane::framed(&app.theme, "Editor", true).title(badge(app));
    let inside = block.inner(area);
    f.render_widget(block, area);
    let (before, _, _) = app.editor.split();
    let row = before.matches('\n').count() as u16;
    let column = Line::from(before.rsplit('\n').next().unwrap_or_default()).width() as u16;
    let scroll = row.saturating_sub(inside.height.saturating_sub(1));
    f.render_widget(Paragraph::new(app.editor.text()).scroll((scroll, 0)), inside);
    let x = (inside.x + column).min(inside.right().saturating_sub(1));
    f.set_cursor_position(Position { x, y: inside.y + row - scroll });
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
