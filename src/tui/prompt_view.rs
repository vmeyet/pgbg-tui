//! The prompt on the footer line: the path of a new query as it is typed, whether to open a query over unsaved text, or
//! whether to send a write.
use super::app::{App, Prompt};
use crate::{act, render};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// The prompt over the footer in `area`, followed by its keys.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, prompt: &Prompt, area: Rect) {
    let theme = app.theme();
    let mut spans = match prompt {
        Prompt::SaveAs { path, error } => {
            let (before, under, after) = path.split();
            let mut spans = vec![
                Span::styled(" save as › ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
                Span::raw(format!("{before}{under}{after}")),
                Span::styled("▌ ", Style::default().fg(theme.accent)),
            ];
            spans.extend(error.iter().map(|error| Span::styled(format!("{error} "), Style::default().fg(theme.danger))));
            spans
        }
        Prompt::OpenOverUnsaved(query) => {
            vec![Span::styled(format!(" the editor's text is not saved: open {} over it? ", query.path), Style::default().fg(theme.warn))]
        }
        Prompt::SendWrite(waiting) => {
            vec![Span::styled(
                format!(" {}? ", render::printable(&waiting.write.to_string())),
                Style::default().fg(theme.danger).add_modifier(Modifier::BOLD),
            )]
        }
    };
    let hints =
        act::footer(prompt.pane()).map(|hint| Span::styled(format!(" {} {} ", hint.keys, hint.does), Style::default().fg(theme.muted)));
    spans.extend(hints);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{app_on, connection, shop};
    use super::super::app::queries::tests::query;
    use super::super::app::{Answer, QueryAnswer};
    use super::super::ui;
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn the_question_escapes_a_control_character_and_a_reversing_mark_in_a_name() {
        let mut app = app_on(&connection("production", "allowed"));
        let text = "DELETE FROM \"orders\x1b[2K\u{202e}sredro\"".into();
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("orders/purge.sql"), text }));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("send_write_escaped", terminal.backend());
    }
}
