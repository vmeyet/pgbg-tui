//! The history ghost: the rest of the past statement that best continues what is typed, shown as any ghost and taken with
//! the same `→`.
//! A name of the schema wins over it; it wins over a keyword, which the statement holds anyway, and over the idle ask,
//! which then costs nothing.
use super::ghost::Ghost;
use super::idle_ghost::rest_after;
use super::{Action, Sheet};
use crate::complete::rest_of_keyword;
use crate::db::lex::{self, Span};
use crate::history::HistoryIndex;

impl Sheet {
    /// What asks the backend for this sheet's history, read once as the sheet opens.
    pub(super) fn read_history_index(&self) -> Action {
        Action::ReadHistoryIndex(self.name().clone())
    }

    /// Takes the history read, keeping the runs that ended before it came.
    pub(super) fn take_history_index(&mut self, read: HistoryIndex) {
        self.history_index.add_runs_of(read);
    }

    /// The ghost of the past statement that continues the statement the cursor ends; none inside a comment, where a taken
    /// rest would stay a comment.
    pub(super) fn history_ghost(&self) -> Option<Ghost> {
        let (before, _, _) = self.editor.split();
        if self.editor.lexed().span_at(before.len()) == Span::Comment {
            return None;
        }
        let typed = self.question()?.typed;
        let statement = self.history_index.best_after(typed)?;
        let rest = rest_after(typed, statement.as_str())?;
        Ghost::at_cursor(before.len(), &keyword_cased_as_typed(typed, rest))
    }
}

/// `rest` with the end of the keyword `typed` stops inside in the case typed; what follows stays as the past statement wrote it.
fn keyword_cased_as_typed(typed: &str, rest: String) -> String {
    let word_end = rest.find(|c| !lex::is_word_char(c)).unwrap_or(rest.len());
    let (end_of_word, after) = rest.split_at(word_end);
    match word_in_progress(typed).and_then(|word| rest_of_keyword(word, end_of_word)) {
        Some(cased) => format!("{cased}{after}"),
        None => rest,
    }
}

/// The word the end of `typed` is part of; none after a blank, or in a literal or a quoted name.
fn word_in_progress(typed: &str) -> Option<&str> {
    lex::tokens(typed).last().filter(|last| last.range.end == typed.len())?.word(typed)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{enter, rest, to_shop, typed};
    use super::super::idle_ghost::tests::{idle_on, prompt_text};
    use super::super::picker::tests::{ctrl, on_local, press};
    use super::super::{Answer, App, Clock, Incoming, RunId};
    use super::*;
    use crate::failure::Failure;
    use crate::fixtures::{orders, run_at};
    use crate::history::History;
    use chrono::{DateTime, Local};
    use crossterm::event::{KeyCode, KeyEvent};
    use std::time::Duration;

    const ORDERS: &str = "SELECT * FROM orders";

    fn index_of(statements: &[&str]) -> HistoryIndex {
        HistoryIndex::of(&History { entries: statements.iter().map(|sql| run_at(sql, 1)).collect() })
    }

    fn read(app: &mut App, statements: &[&str]) {
        let connection = app.sheet().name().clone();
        app.apply(Answer::HistoryIndex { connection, index: index_of(statements) });
    }

    fn typed_over(statements: &[&str], text: &str) -> App {
        let mut app = typed("");
        read(&mut app, statements);
        press(&mut app, text);
        app
    }

    #[test]
    fn typing_shows_the_rest_of_a_past_statement() {
        assert_eq!(rest(&typed_over(&[ORDERS], "SELECT * ")), Some("FROM orders"));
    }

    #[test]
    fn a_word_no_name_completes_shows_the_rest_of_a_past_statement() {
        assert_eq!(rest(&typed_over(&["SELECT zzz_total FROM orders"], "SELECT zzz")), Some("_total FROM orders"));
    }

    #[test]
    fn a_past_statement_beats_a_keyword() {
        assert_eq!(rest(&typed_over(&[ORDERS], "SELECT * FR")), Some("OM orders"));
    }

    #[test]
    fn a_keyword_shows_when_no_past_statement_matches() {
        assert_eq!(rest(&typed_over(&["SELECT id FROM customers"], "SELECT * FR")), Some("OM"));
    }

    #[test]
    fn a_name_of_the_schema_beats_a_past_statement() {
        let mut app = idle_on("");
        read(&mut app, &["SELECT * FROM customers WHERE total > 10"]);
        press(&mut app, "SELECT * FROM cu");
        assert_eq!(rest(&app), Some("stomers"));
    }

    #[test]
    fn taking_a_past_statement_over_a_keyword_ends_the_statement() {
        let mut app = typed_over(&[ORDERS], "SELECT * FR");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.split(), (ORDERS, "", ""));
    }

    #[test]
    fn right_takes_the_past_statement() {
        let mut app = typed_over(&[ORDERS], "SELECT * ");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.split(), (ORDERS, "", ""));
    }

    #[test]
    fn tab_takes_the_past_statement() {
        let mut app = typed_over(&[ORDERS], "SELECT * ");
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.sheet().editor.split(), (ORDERS, "", ""));
    }

    #[test]
    fn deleting_a_character_shows_the_rest_again() {
        let mut app = typed_over(&[ORDERS], "SELECT * x");
        app.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(rest(&app), Some("FROM orders"));
    }

    #[test]
    fn two_typed_characters_leave_the_keyword() {
        assert_eq!(rest(&typed_over(&[ORDERS], "se")), Some("lect"));
    }

    #[test]
    fn three_typed_characters_show_the_rest() {
        assert_eq!(rest(&typed_over(&[ORDERS], "sel")), Some("ect * FROM orders"));
    }

    #[test]
    fn taking_the_rest_of_a_keyword_keeps_the_case_typed_and_the_statement_as_written() {
        let mut app = typed_over(&[ORDERS], "sel");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.split(), ("select * FROM orders", "", ""));
    }

    #[test]
    fn upper_case_typed_ends_a_keyword_a_past_statement_wrote_in_lower_case() {
        assert_eq!(rest(&typed_over(&["select * from orders"], "SEL")), Some("ECT * from orders"));
    }

    #[test]
    fn a_keyword_further_on_ends_in_the_case_typed() {
        assert_eq!(rest(&typed_over(&[ORDERS], "SELECT * fr")), Some("om orders"));
    }

    #[test]
    fn the_rest_of_a_name_stays_as_the_past_statement_wrote_it() {
        assert_eq!(rest(&typed_over(&["SELECT TOTAL FROM orders"], "select tot")), Some("AL FROM orders"));
    }

    #[test]
    fn a_longer_word_that_starts_like_a_keyword_stays_as_written() {
        assert_eq!(rest(&typed_over(&["SELECT * FROM\u{20ac} orders"], "SELECT * fr")), Some("OM\u{20ac} orders"));
    }

    #[test]
    fn the_rest_of_a_literal_that_starts_like_a_keyword_stays_as_written() {
        let past = "SELECT * FROM orders WHERE note = 'selECT'";
        assert_eq!(rest(&typed_over(&[past], "SELECT * FROM orders WHERE note = 'sel")), Some("ECT'"));
    }

    #[test]
    fn a_cursor_inside_the_statement_shows_nothing() {
        let mut app = typed_over(&[ORDERS], "SELECT FROM orders");
        (0..12).for_each(|_| drop(app.handle_key(KeyEvent::from(KeyCode::Left))));
        app.handle_key(KeyEvent::from(KeyCode::Char('*')));
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn a_comment_typed_above_is_not_matched() {
        let mut app = typed("");
        read(&mut app, &[ORDERS]);
        app.paste("-- the big ones\n");
        press(&mut app, "SELECT * ");
        assert_eq!(rest(&app), Some("FROM orders"));
    }

    #[test]
    fn a_comment_above_the_past_statement_stays_out_of_the_ghost() {
        assert_eq!(rest(&typed_over(&["-- the big ones\nSELECT * FROM orders"], "SELECT * ")), Some("FROM orders"));
    }

    #[test]
    fn a_cursor_inside_a_comment_shows_nothing() {
        assert_eq!(rest(&typed_over(&[ORDERS], "SELECT * -- no ")), None);
    }

    #[test]
    fn every_line_of_a_past_statement_shows() {
        assert_eq!(rest(&typed_over(&["SELECT *\nFROM orders\nWHERE total > 10"], "SELECT * ")), Some("FROM orders\nWHERE total > 10"));
    }

    #[test]
    fn what_follows_a_closed_statement_shows_nothing() {
        assert_eq!(rest(&typed_over(&["SELECT * FROM orders WHERE total > 10"], "SELECT * FROM orders; ")), None);
    }

    #[test]
    fn the_ghost_of_a_past_statement_holds_no_control_character_nor_reversing_mark() {
        let app = typed_over(&["SELECT * FROM orders WHERE note = '\u{1b}[31m\u{202e}nina'"], "SELECT * ");
        assert_eq!(rest(&app), Some("FROM orders WHERE note = '[31mnina'"));
    }

    #[test]
    fn the_history_of_another_connection_never_shows() {
        let mut app = on_local();
        read(&mut app, &[ORDERS]);
        ctrl(&mut app, 'o');
        press(&mut app, "staging");
        enter(&mut app);
        press(&mut app, "SELECT * ");
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn the_first_sheet_reads_its_history_as_the_tui_starts() {
        let app = typed("");
        assert!(app.start().contains(&Action::ReadHistoryIndex(app.sheet().name().clone())));
    }

    #[test]
    fn a_new_sheet_reads_its_history_once() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        press(&mut app, "staging");
        let opened = enter(&mut app);
        let reads = opened.iter().filter(|action| matches!(action, Action::ReadHistoryIndex(_))).count();
        assert_eq!(reads, 1, "{opened:?}");
    }

    fn run_to_a_result(app: &mut App) {
        enter(app);
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        ctrl(app, 'u');
    }

    fn after_a_result_of(sql: &str) -> App {
        let mut app = typed(sql);
        run_to_a_result(&mut app);
        app
    }

    #[test]
    fn a_new_run_shows_without_reading_the_history_again() {
        let mut app = after_a_result_of(ORDERS);
        press(&mut app, "SELECT * ");
        assert_eq!(rest(&app), Some("FROM orders"));
    }

    #[test]
    fn a_failed_run_never_shows() {
        let mut app = typed(ORDERS);
        enter(&mut app);
        let failure = Failure::Server { message: "ERROR: no such table".into(), position: None };
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, failure }));
        ctrl(&mut app, 'u');
        press(&mut app, "SELECT * ");
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn a_run_holding_a_password_never_shows() {
        let mut app = after_a_result_of("SELECT * FROM dblink('postgresql://nina:hunter2@db.example.com/shop', 'SELECT 1')");
        press(&mut app, "SELECT * ");
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn a_write_just_run_never_shows() {
        let mut app = after_a_result_of("DELETE FROM orders");
        press(&mut app, "DELETE ");
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn a_run_before_the_history_comes_is_kept() {
        let mut app = after_a_result_of(ORDERS);
        read(&mut app, &["SELECT id FROM customers"]);
        press(&mut app, "SELECT * ");
        assert_eq!(rest(&app), Some("FROM orders"));
    }

    #[test]
    fn a_new_run_is_the_most_recent_by_the_clock() {
        let mut app = typed("");
        read(&mut app, &["SELECT id FROM customers"]);
        app.set_clock(Clock { now: DateTime::UNIX_EPOCH + chrono::Duration::hours(1), zone: Local });
        press(&mut app, ORDERS);
        run_to_a_result(&mut app);
        press(&mut app, "SELECT ");
        assert_eq!(rest(&app), Some("* FROM orders"));
    }

    #[test]
    fn a_history_match_asks_the_idle_ghost_nothing() {
        let mut app = idle_on("");
        read(&mut app, &[ORDERS]);
        press(&mut app, "SELECT * ");
        assert_eq!((app.wants_idle_ghost(), app.ask_idle_ghost()), (false, vec![]));
    }

    #[test]
    fn with_no_history_match_the_idle_ghost_still_asks() {
        let mut app = idle_on("");
        read(&mut app, &["SELECT id FROM customers"]);
        press(&mut app, "SELECT * ");
        assert!(app.wants_idle_ghost());
    }

    #[test]
    fn a_dropped_ghost_stays_out_of_the_prompt() {
        let mut app = idle_on("");
        read(&mut app, &["SELECT * FROM orders WHERE note = 'nina'"]);
        press(&mut app, "SELECT * ");
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(!prompt_text(&mut app).contains("nina"));
    }
}
