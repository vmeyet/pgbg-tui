//! `ctrl-g` as drawn: the instruction and the wait on the footer line, and the answer over the result pane as a diff of
//! the statement it would replace, each write named first in the failure colour.
use super::app::{App, AskStep, Change, Overlay, Suggestion};
use super::field_line;
use super::ui::{draw_with_hints, footer_label, muted_hints};
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
    let spans = match step {
        AskStep::Instruction(instruction) => {
            let note = Span::styled(blank_means(app, instruction.text()), Style::default().fg(theme.muted));
            let after = std::iter::once(note).chain(muted_hints(app, step.pane()));
            return field_line::draw(f, footer_label(app, " ask › "), instruction, after, area);
        }
        AskStep::WaitingForSchema { .. } => vec![Span::styled(" reading schema… ", Style::default().fg(theme.warn))],
        AskStep::Waiting { .. } => vec![Span::styled(asking(app), Style::default().fg(theme.warn))],
        AskStep::Answered(_) => vec![footer_label(app, " the [ai] command answered: ")],
    };
    draw_with_hints(f, app, spans, step.pane(), area);
}

/// What `⏎` does on an instruction still blank, when it does anything.
fn blank_means(app: &App, typed: &str) -> &'static str {
    if typed.is_empty() && app.offers_fix() { " blank asks for a fix " } else { "" }
}

/// ` asking claude… `: what the footer says while the `[ai] command` runs.
pub(super) fn asking(app: &App) -> String {
    format!(" asking {}… ", printable(app.ai_program().unwrap_or("the [ai] command")))
}

/// The answer over the result pane in `area`, while it is shown.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(Overlay::Ask(AskStep::Answered(suggestion))) = app.overlay() else { return };
    let block = pane::framed(app.theme(), "Answer", true);
    let inside = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines(app, suggestion, usize::from(inside.height))), inside);
}

/// Each write the answer holds, then the lines of the diff after their mark, cut to `height` lines with the last one
/// saying how many more there are.
fn lines(app: &App, suggestion: &Suggestion, height: usize) -> Vec<Line<'static>> {
    let theme = app.theme();
    let writes = suggestion.writes.iter().map(|write| {
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
    let all: Vec<Line<'static>> = writes.chain(diff).collect();
    cut_to(all, height, Style::default().fg(theme.muted))
}

/// `lines` when they fit in `height`, else the first ones and a last line, in `style`, saying how many are not shown.
fn cut_to(mut lines: Vec<Line<'static>>, height: usize, style: Style) -> Vec<Line<'static>> {
    if lines.len() <= height {
        return lines;
    }
    let shown = height.saturating_sub(1);
    let hidden = lines.len() - shown;
    lines.truncate(shown);
    lines.push(Line::styled(format!("… {hidden} more lines"), style));
    lines
}

#[cfg(test)]
mod tests {
    use super::super::app::ask::tests::{answered, asking_on, ctrl_g, failed_on, press, type_in, waiting};
    use super::super::app::ask_schema::tests::{held_for_the_schema, schema_refused};
    use super::super::app::fixtures::to_shop;
    use super::super::app::idle_ghost::tests::{UNMASKABLE_COMMENT, ghosted, idle_on};
    use super::super::app::{Answer, AskFailure, AskId, Incoming};
    use super::super::{styled, ui};
    use crate::ai::{AnswerError, CommandError, ServerErrors};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;

    fn render(app: &super::App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, app)).unwrap();
        terminal
    }

    fn instruction(typed: &str) -> super::App {
        let mut app = asking_on("SELECT * FROM orders");
        ctrl_g(&mut app);
        type_in(&mut app, typed);
        app
    }

    /// What the cell under the terminal's cursor shows, on a screen 60 by 6.
    fn under_cursor(app: &super::App) -> String {
        let terminal = render(app, 60, 6);
        terminal.backend().buffer()[terminal.backend().cursor_position()].symbol().to_owned()
    }

    #[test]
    fn ctrl_g_asks_for_the_instruction_on_the_footer_line() {
        let mut app = asking_on("SELECT * FROM orders");
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        "only the ids".chars().for_each(|c| _ = app.handle_key(KeyEvent::from(KeyCode::Char(c))));
        insta::assert_snapshot!("ask_instruction", render(&app, 60, 6).backend());
    }

    #[test]
    fn the_cursor_shows_while_the_instruction_is_typed() {
        assert!(render(&instruction("only"), 60, 6).backend().cursor_visible());
    }

    #[test]
    fn the_cursor_follows_the_arrows_into_the_middle_of_the_instruction() {
        let mut app = instruction("only");
        press(&mut app, KeyCode::Left);
        assert_eq!(under_cursor(&app), "y");
    }

    #[test]
    fn a_letter_typed_in_the_middle_sits_just_before_the_cursor() {
        let mut app = instruction("only");
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('x'));
        let terminal = render(&app, 60, 6);
        let Position { x, y } = terminal.backend().cursor_position();
        assert_eq!(terminal.backend().buffer()[(x - 1, y)].symbol(), "x");
    }

    #[test]
    fn a_character_drawn_escaped_counts_for_the_columns_it_is_drawn_on() {
        let mut app = instruction("");
        app.paste("\nx");
        press(&mut app, KeyCode::Left);
        assert_eq!(under_cursor(&app), "x");
    }

    #[test]
    fn an_instruction_wider_than_the_footer_scrolls_to_keep_the_cursor_on_screen() {
        let terminal = render(&instruction(&"orders ".repeat(12)), 60, 6);
        assert_eq!(terminal.backend().cursor_position(), Position { x: 58, y: 4 });
        insta::assert_snapshot!("ask_scrolled", terminal.backend());
    }

    #[test]
    fn a_blank_instruction_says_it_asks_for_a_fix_and_keeps_its_keys() {
        let mut app = failed_on("SELECT nope", ServerErrors::Code, Some(8));
        ctrl_g(&mut app);
        insta::assert_snapshot!("ask_blank_means_fix", render(&app, 80, 6).backend());
    }

    #[test]
    fn the_footer_says_while_the_command_runs() {
        insta::assert_snapshot!("ask_waiting", render(&waiting("SELECT * FROM orders", "only the ids"), 60, 6).backend());
    }

    #[test]
    fn the_footer_says_while_an_ask_waits_for_the_schema() {
        insta::assert_snapshot!("ask_waiting_for_schema", render(&held_for_the_schema(), 60, 6).backend());
    }

    #[test]
    fn an_ask_with_no_schema_says_why_in_the_status_line() {
        let mut app = held_for_the_schema();
        app.apply(to_shop(Incoming::Schema(Err(schema_refused()))));
        insta::assert_snapshot!("ask_no_schema", render(&app, 140, 6).backend());
    }

    #[test]
    fn the_footer_names_the_program_while_the_idle_ghost_asks() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.ask_idle_ghost();
        insta::assert_snapshot!("idle_asking", render(&app, 60, 6).backend());
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
    fn an_answer_taller_than_its_pane_says_how_many_lines_are_not_shown() {
        let answer = (1..=20).map(|n| format!("SELECT {n}")).collect::<Vec<_>>().join("\n");
        insta::assert_snapshot!("ask_cut", render(&answered("SELECT 1", &answer), 60, 14).backend());
    }

    #[test]
    fn the_footer_says_a_rewrite_is_waiting() {
        let app = ghosted("SELECT * FROM orders WHERE ", "SELECT id FROM orders");
        insta::assert_snapshot!("rewrite_waiting", render(&app, 60, 6).backend());
    }

    #[test]
    fn ctrl_g_shows_the_waiting_rewrite_as_a_diff() {
        let mut app = ghosted("SELECT * FROM orders WHERE ", "SELECT id FROM orders");
        ctrl_g(&mut app);
        insta::assert_snapshot!("rewrite_diff", render(&app, 60, 14).backend());
    }

    #[test]
    fn a_comment_left_out_says_why_in_the_status_line() {
        let mut app = idle_on(UNMASKABLE_COMMENT);
        app.ask_idle_ghost();
        insta::assert_snapshot!("comment_left_out", render(&app, 120, 6).backend());
    }

    #[test]
    fn an_answer_without_a_statement_shows_its_reason_in_the_status_line() {
        let mut app = waiting("SELECT * FROM orders", "join the invoices");
        let answer = Err(AskFailure::Answer(AnswerError::Cannot("no key links orders and invoices".into())));
        app.apply(Answer::Asked { ask: AskId::FIRST, answer });
        insta::assert_snapshot!("ask_cannot", render(&app, 120, 6).backend());
    }

    #[test]
    fn a_failed_ask_says_why_in_the_status_line() {
        let mut app = waiting("SELECT * FROM orders", "only the ids");
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Err(AskFailure::Command(CommandError::TooLong)) });
        insta::assert_snapshot!("ask_failed", render(&app, 100, 6).backend());
    }
}
