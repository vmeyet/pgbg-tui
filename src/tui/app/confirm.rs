//! Before a write is sent, the footer line asks for it with the count of the rows it would touch: `y` sends it, any
//! other key drops it.
use super::sheet::ToRun;
use super::{Action, App, LookId, Overlay, Prompt};
use crate::db::guard::Write;
use crate::db::model::ResultSet;
use crate::db::preview::Touched;
use crate::failure::Failure;
use std::fmt;

/// A statement the guard read as a write, waiting for a `y`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct WriteToSend {
    pub(in crate::tui) write: Write,
    pub(in crate::tui) rows: RowCount,
    to_run: ToRun,
}

/// The rows a write waiting for a `y` would touch, as far as its count read them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum RowCount {
    /// A write no read can count, or a read that tried to write.
    NotCounted,
    Counting(LookId),
    Counted(Touched),
    Failed(String),
}

/// Proof that the user answered `y` to a write: only this module builds one, and the backend runs no write without it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Confirmed(());

impl Confirmed {
    #[cfg(test)]
    pub(in crate::tui) const FOR_TESTS: Self = Self(());
}

impl App {
    /// Runs the statement under the cursor, asking first when it is a write.
    /// Does nothing while another statement runs.
    pub(super) fn run_editor(&mut self) -> Vec<Action> {
        let sheet = self.sheet_mut();
        let Some(to_run) = sheet.editor_run().filter(|_| !sheet.is_running()) else { return vec![] };
        let Some(write) = sheet.write_under_cursor() else { return sheet.run(to_run, None) };
        let (rows, counting) = self.count_rows(&to_run);
        self.overlay = Some(Overlay::Prompt(Prompt::SendWrite(Box::new(WriteToSend { write, rows, to_run }))));
        counting
    }

    /// The count of the rows `to_run` would touch as a write, and the lookup that reads it; none for a write no read
    /// counts.
    fn count_rows(&mut self, to_run: &ToRun) -> (RowCount, Vec<Action>) {
        let Some(lookup) = to_run.count() else { return (RowCount::NotCounted, vec![]) };
        let look = self.new_look();
        (RowCount::Counting(look), vec![Action::LookUp { connection: self.sheet().name().clone(), look, lookup }])
    }

    /// Whether the write waiting for a `y` waits on the count `look` too.
    pub(super) fn is_counting(&self, look: LookId) -> bool {
        matches!(&self.overlay, Some(Overlay::Prompt(Prompt::SendWrite(waiting))) if waiting.rows == RowCount::Counting(look))
    }

    /// Takes the count of the rows the write waiting for a `y` would touch.
    pub(super) fn rows_counted(&mut self, read: &Result<ResultSet, Failure>) {
        let Some(Overlay::Prompt(Prompt::SendWrite(waiting))) = &mut self.overlay else { return };
        waiting.rows = match read {
            Ok(read) => Touched::read(read).map_or_else(|| RowCount::Failed("it read no number".to_owned()), RowCount::Counted),
            Err(failure) => RowCount::Failed(failure.to_string()),
        };
    }

    /// Asks for the statement of the sheet on screen that tried to write, once no overlay takes the keys; a sheet off
    /// screen keeps its own until it shows.
    pub(super) fn ask_for_hidden_write(&mut self) {
        if self.overlay.is_some() {
            return;
        }
        let Some(to_run) = self.sheet_mut().take_tried_to_write() else { return };
        self.overlay =
            Some(Overlay::Prompt(Prompt::SendWrite(Box::new(WriteToSend { write: Write::Hidden, rows: RowCount::NotCounted, to_run }))));
    }

    pub(super) fn send_write(&mut self, waiting: WriteToSend) -> Vec<Action> {
        self.sheet_mut().run(waiting.to_run, Some(Confirmed(())))
    }
}

impl fmt::Display for RowCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotCounted => Ok(()),
            Self::Counting(_) => f.write_str("counting rows…"),
            Self::Counted(touched) => touched.fmt(f),
            Self::Failed(why) => write!(f, "rows not counted: {why}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{app_on, connection, first_run, shop, to_shop, with_focus};
    use super::super::picker::tests::{on_local, press as picker_press, switch_to};
    use super::super::{Answer, Focus, Incoming, LookId, Run, RunId, SessionId};
    use super::*;
    use crate::db::model::{ColumnKind, ResultSet, Statement, Value};
    use crate::db::preview::COUNT_TIMEOUT;
    use crate::failure::Failure;
    use crate::fixtures::column;
    use crate::query::params::Given;
    use crossterm::event::{KeyCode, KeyEvent};

    fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    fn typed_on(writes: &str, text: &str) -> App {
        let mut app = app_on(&connection("production", writes));
        text.chars().for_each(|c| drop(press(&mut app, KeyCode::Char(c))));
        app
    }

    fn asked(app: &App) -> Option<String> {
        match &app.overlay {
            Some(Overlay::Prompt(Prompt::SendWrite(waiting))) => Some(waiting.write.to_string()),
            _ => None,
        }
    }

    fn rows_said(app: &App) -> Option<String> {
        match &app.overlay {
            Some(Overlay::Prompt(Prompt::SendWrite(waiting))) => Some(waiting.rows.to_string()),
            _ => None,
        }
    }

    fn count_read(rows: Result<ResultSet, Failure>) -> Answer {
        to_shop(Incoming::LookedUp { look: LookId::FIRST, rows })
    }

    fn counted(rows: i64) -> ResultSet {
        ResultSet::whole(vec![column("count", ColumnKind::Int)], vec![vec![Value::Int(rows)]])
    }

    #[test]
    fn a_write_counts_the_rows_it_would_touch_while_it_waits_for_y() {
        let mut app = with_focus(typed_on("allowed", "DELETE FROM orders WHERE id = {{id}}"), Focus::Params);
        press(&mut app, KeyCode::Char('7'));
        let asked = press(&mut app, KeyCode::Enter);
        let [Action::LookUp { look: LookId::FIRST, lookup, .. }] = &*asked else { panic!("no count: {asked:?}") };
        assert_eq!(lookup.statement.as_str(), "SELECT count(*) FROM (SELECT FROM orders WHERE id = $1 LIMIT 10001) AS touched");
        assert_eq!((lookup.params.len(), lookup.own_timeout), (1, Some(COUNT_TIMEOUT)));
        assert_eq!(rows_said(&app).as_deref(), Some("counting rows…"));
    }

    #[test]
    fn the_question_says_how_many_rows_the_write_would_touch() {
        let mut app = typed_on("allowed", "DELETE FROM orders");
        press(&mut app, KeyCode::Enter);
        app.apply(count_read(Ok(counted(42))));
        assert_eq!(rows_said(&app).as_deref(), Some("42 rows"));
    }

    #[test]
    fn a_count_answered_on_a_session_the_sheet_left_still_reaches_the_question() {
        let mut app = typed_on("allowed", "DELETE FROM orders");
        press(&mut app, KeyCode::Enter);
        let incoming = Incoming::LookedUp { look: LookId::FIRST, rows: Ok(counted(42)) };
        app.apply(Answer::Session { connection: shop(), session: SessionId::FIRST.next(), incoming });
        assert_eq!(rows_said(&app).as_deref(), Some("42 rows"));
    }

    #[test]
    fn a_count_that_failed_says_why_and_still_lets_y_send_the_write() {
        let mut app = typed_on("allowed", "DELETE FROM orders");
        press(&mut app, KeyCode::Enter);
        app.apply(count_read(Err(Failure::Refused("too slow".to_owned()))));
        assert!(rows_said(&app).is_some_and(|said| said.starts_with("rows not counted: ")), "{:?}", rows_said(&app));
        assert!(matches!(&*press(&mut app, KeyCode::Char('y')), [Action::Run { confirmed: Some(_), .. }]));
    }

    #[test]
    fn a_write_the_count_cannot_read_asks_without_counting() {
        let mut app = typed_on("allowed", "TRUNCATE orders");
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!(rows_said(&app).as_deref(), Some(""));
    }

    #[test]
    fn a_count_read_once_the_question_closed_changes_nothing() {
        let mut app = typed_on("allowed", "DELETE FROM orders");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.apply(count_read(Ok(counted(42)))), []);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn a_write_waits_for_y_before_it_is_sent() {
        let mut app = typed_on("allowed", "DELETE FROM orders");
        let asked = press(&mut app, KeyCode::Enter);
        assert!(!asked.iter().any(|action| matches!(action, Action::Run { .. })), "{asked:?}");
        assert_eq!(app.sheet().run, Run::Idle);
        let sent = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(&*sent, [Action::Run { confirmed: Some(Confirmed(())), .. }]), "{sent:?}");
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn a_read_is_sent_unconfirmed() {
        let mut app = typed_on("allowed", "SELECT * FROM orders");
        let sent = press(&mut app, KeyCode::Enter);
        assert!(matches!(&*sent, [Action::Run { confirmed: None, .. }]), "{sent:?}");
    }

    #[test]
    fn the_question_names_the_verb_and_the_table_of_the_write() {
        let mut app = typed_on("allowed", "SELECT 1;\nDELETE FROM orders WHERE id = 7");
        press(&mut app, KeyCode::Enter);
        assert_eq!(asked(&app).as_deref(), Some("DELETE from orders"));
    }

    #[test]
    fn any_other_key_drops_the_write_and_sends_nothing() {
        for key in [KeyCode::Char('n'), KeyCode::Char('Y'), KeyCode::Esc, KeyCode::Enter] {
            let mut app = typed_on("allowed", "DELETE FROM orders");
            press(&mut app, KeyCode::Enter);
            assert_eq!(press(&mut app, key), [], "{key:?}");
            assert_eq!((&app.overlay, &app.sheet().run), (&None, &Run::Idle), "{key:?}");
        }
    }

    #[test]
    fn a_read_is_sent_without_asking() {
        let mut app = typed_on("allowed", "SELECT * FROM orders");
        assert_eq!(press(&mut app, KeyCode::Enter), [first_run(&shop(), "SELECT * FROM orders")]);
        assert_eq!(asked(&app), None);
    }

    #[test]
    fn a_write_on_a_read_only_connection_goes_unasked_to_the_guard_that_refuses_it() {
        let mut app = typed_on("read-only", "DELETE FROM orders");
        assert_eq!(press(&mut app, KeyCode::Enter), [first_run(&shop(), "DELETE FROM orders")]);
        assert_eq!(asked(&app), None);
    }

    /// The app on a connection that allows writes, once `sql` was sent unconfirmed and the server found a write in it.
    fn tried_to_write(sql: &str) -> App {
        let mut app = typed_on("allowed", sql);
        press(&mut app, KeyCode::Enter);
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, failure: Failure::TriedToWrite }));
        app
    }

    #[test]
    fn a_statement_that_tried_to_write_asks_before_it_runs_again() {
        assert_eq!(
            asked(&tried_to_write("SELECT archive_orders()")).as_deref(),
            Some("this statement tried to write, and may run twice: run it")
        );
    }

    #[test]
    fn y_to_a_statement_that_tried_to_write_sends_it_again_confirmed() {
        let mut app = tried_to_write("SELECT archive_orders()");
        let sent = press(&mut app, KeyCode::Char('y'));
        let is_again = |statement: &Statement| statement.as_str() == "SELECT archive_orders()";
        assert!(matches!(&*sent, [Action::Run { statement, confirmed: Some(_), .. }] if is_again(statement)), "{sent:?}");
    }

    #[test]
    fn no_to_a_statement_that_tried_to_write_sends_nothing() {
        let mut app = tried_to_write("SELECT archive_orders()");
        assert_eq!(press(&mut app, KeyCode::Char('n')), []);
        assert!(matches!(&app.sheet().run, Run::Failed(why) if why.contains("tried to write")), "{:?}", app.sheet().run);
    }

    #[test]
    fn a_statement_that_tried_to_write_keeps_an_overlay_already_open() {
        let mut app = typed_on("allowed", "SELECT archive_orders()");
        press(&mut app, KeyCode::Enter);
        app.overlay = Some(Overlay::CopyAs);
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, failure: Failure::TriedToWrite }));
        assert_eq!(app.overlay, Some(Overlay::CopyAs));
    }

    #[test]
    fn a_statement_that_tried_to_write_under_an_overlay_is_asked_once_the_overlay_closes() {
        let mut app = typed_on("allowed", "SELECT archive_orders()");
        press(&mut app, KeyCode::Enter);
        app.overlay = Some(Overlay::CopyAs);
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, failure: Failure::TriedToWrite }));
        press(&mut app, KeyCode::Esc);
        assert_eq!(asked(&app).as_deref(), Some("this statement tried to write, and may run twice: run it"));
    }

    #[test]
    fn a_statement_that_tried_to_write_off_screen_is_asked_once_its_sheet_shows() {
        let mut app = on_local();
        switch_to(&mut app, "staging");
        let session = app.sheet().session_id;
        picker_press(&mut app, "SELECT archive_orders()");
        press(&mut app, KeyCode::Enter);
        switch_to(&mut app, "local");
        let failed = Incoming::Failed { run: RunId::FIRST, failure: Failure::TriedToWrite };
        app.apply(Answer::Session { connection: "staging".parse().unwrap(), session, incoming: failed });
        assert_eq!(asked(&app), None);
        switch_to(&mut app, "staging");
        assert_eq!(asked(&app).as_deref(), Some("this statement tried to write, and may run twice: run it"));
    }

    #[test]
    fn enter_in_the_parameter_bar_asks_too() {
        let mut app = with_focus(typed_on("allowed", "DELETE FROM orders WHERE id = {{id}}"), Focus::Params);
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(asked(&app).as_deref(), Some("DELETE from orders"));
        let sent = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(&*sent, [Action::Run { input, .. }] if input.given != Given::default()), "{sent:?}");
    }
}
