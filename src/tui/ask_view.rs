//! `ctrl-g` as drawn: the instruction, the wait and the answer's keys on the footer line.
use super::app::{App, AskStep};
use crate::act;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// The ask over the footer in `area`, followed by its keys.
pub(super) fn footer(f: &mut Frame<'_>, app: &App, step: &AskStep, area: Rect) {
    let theme = app.theme();
    let mut spans = match step {
        AskStep::Instruction(instruction) => vec![
            Span::styled(" ask › ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
            Span::raw(instruction.text().to_owned()),
            Span::styled("▌ ", Style::default().fg(theme.accent)),
        ],
        AskStep::Waiting(_) => vec![Span::styled(" asking the [ai] command… ", Style::default().fg(theme.warn))],
        AskStep::Answered(_) => {
            vec![Span::styled(" the [ai] command answered: ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD))]
        }
    };
    let hints =
        act::footer(step.pane()).map(|hint| Span::styled(format!(" {} {} ", hint.keys, hint.does), Style::default().fg(theme.muted)));
    spans.extend(hints);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::super::app::ask::tests::{asking_on, waiting};
    use super::super::app::{Answer, AskFailure, AskId};
    use super::super::ui;
    use crate::ai::CommandError;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &super::App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, app)).unwrap();
        terminal
    }

    #[test]
    fn ctrl_g_asks_for_the_instruction_on_the_footer_line() {
        let mut app = asking_on("SELECT * FROM orders");
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        "only the ids".chars().for_each(|c| _ = app.handle_key(KeyEvent::from(KeyCode::Char(c))));
        insta::assert_snapshot!("ask_instruction", render(&app, 60, 6).backend());
    }

    #[test]
    fn the_footer_says_while_the_command_runs() {
        insta::assert_snapshot!("ask_waiting", render(&waiting("SELECT * FROM orders", "only the ids"), 60, 6).backend());
    }

    #[test]
    fn a_failed_ask_says_why_in_the_status_line() {
        let mut app = waiting("SELECT * FROM orders", "only the ids");
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Err(AskFailure::Command(CommandError::TooLong)) });
        insta::assert_snapshot!("ask_failed", render(&app, 100, 6).backend());
    }
}
