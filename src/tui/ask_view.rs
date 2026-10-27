//! `ctrl-g` as drawn: the instruction and the wait on the footer line, and the answer over the result pane as a diff of
//! the statement it would replace, a write named first in the failure colour.
use super::app::{App, AskStep, Change, Overlay, Suggestion};
use crate::act;
use crate::render::printable;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use vtui::pane;

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

/// The answer over the result pane in `area`, while it is shown.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(Overlay::Ask(AskStep::Answered(suggestion))) = app.overlay() else { return };
    let block = pane::framed(app.theme(), "Answer", true);
    let inside = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines(app, suggestion)), inside);
}

/// The write the answer holds, then each line of the diff after its mark.
fn lines(app: &App, suggestion: &Suggestion) -> Vec<Line<'static>> {
    let theme = app.theme();
    let write = suggestion.write.as_ref().map(|write| {
        let text = format!("writes: {write}");
        Line::styled(printable(&text).into_owned(), Style::default().fg(theme.danger).add_modifier(Modifier::BOLD))
    });
    let diff = suggestion.lines.iter().map(|line| {
        let (mark, style) = match line.change {
            Change::Kept => (' ', Style::default()),
            Change::Removed => ('-', Style::default().fg(theme.danger)),
            Change::Added => ('+', Style::default().fg(theme.success)),
        };
        Line::styled(format!("{mark} {}", printable(&line.text)), style)
    });
    write.into_iter().chain(diff).collect()
}

#[cfg(test)]
mod tests {
    use super::super::app::ask::tests::{answered, asking_on, waiting};
    use super::super::app::{Answer, AskFailure, AskId};
    use super::super::{styled, ui};
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
    fn the_answer_shows_as_a_diff_of_the_statement_under_the_cursor() {
        let app = answered("SELECT *\nFROM orders\nLIMIT 10", "SELECT id\nFROM orders\nLIMIT 10");
        insta::assert_snapshot!("ask_diff", render(&app, 60, 14).backend());
    }

    #[test]
    fn an_answer_that_writes_says_so_in_the_failure_colour() {
        let app = answered("SELECT * FROM orders", "DELETE FROM orders WHERE id = 7");
        insta::assert_snapshot!("ask_write_styled", styled::runs(render(&app, 60, 14).backend().buffer()));
    }

    #[test]
    fn a_failed_ask_says_why_in_the_status_line() {
        let mut app = waiting("SELECT * FROM orders", "only the ids");
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Err(AskFailure::Command(CommandError::TooLong)) });
        insta::assert_snapshot!("ask_failed", render(&app, 100, 6).backend());
    }
}
