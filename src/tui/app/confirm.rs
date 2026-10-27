//! Before a write is sent, the footer line asks for it: `y` sends it, any other key drops it.
use super::sheet::ToRun;
use super::{Action, App, Overlay, Prompt};
use crate::connection::ConnectionName;
use crate::db::guard::Write;

/// A statement the guard read as a write, waiting for a `y`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct WriteToSend {
    pub(in crate::tui) write: Write,
    to_run: ToRun,
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
        self.overlay = Some(Overlay::Prompt(Prompt::SendWrite(Box::new(WriteToSend { write, to_run }))));
        vec![]
    }

    /// Asks for `to_run`, which tried to write on `connection`, once its sheet is on screen and no overlay takes the
    /// keys; otherwise its failed run says so, and running it again asks.
    pub(super) fn ask_for_hidden_write(&mut self, connection: &ConnectionName, to_run: ToRun) {
        if self.overlay.is_some() || self.sheet().name() != connection {
            return;
        }
        self.overlay = Some(Overlay::Prompt(Prompt::SendWrite(Box::new(WriteToSend { write: Write::Hidden, to_run }))));
    }

    pub(super) fn send_write(&mut self, waiting: WriteToSend) -> Vec<Action> {
        self.sheet_mut().run(waiting.to_run, Some(Confirmed(())))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app_on, connection, first_run, shop, to_shop, with_focus};
    use super::super::{Focus, Incoming, Run, RunId};
    use super::*;
    use crate::db::model::Statement;
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

    #[test]
    fn a_write_waits_for_y_before_it_is_sent() {
        let mut app = typed_on("allowed", "DELETE FROM orders");
        assert_eq!(press(&mut app, KeyCode::Enter), []);
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
        app.apply(to_shop(Incoming::TriedToWrite { run: RunId::FIRST }));
        app
    }

    #[test]
    fn a_statement_that_tried_to_write_asks_before_it_runs_again() {
        assert_eq!(asked(&tried_to_write("SELECT archive_orders()")).as_deref(), Some("this statement tried to write: run it"));
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
        app.apply(to_shop(Incoming::TriedToWrite { run: RunId::FIRST }));
        assert_eq!(app.overlay, Some(Overlay::CopyAs));
    }

    #[test]
    fn enter_in_the_parameter_bar_asks_too() {
        let mut app = with_focus(typed_on("allowed", "DELETE FROM orders WHERE id = {{id}}"), Focus::Params);
        press(&mut app, KeyCode::Char('7'));
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!(asked(&app).as_deref(), Some("DELETE from orders"));
        let sent = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(&*sent, [Action::Run { input, .. }] if input.given != Given::default()), "{sent:?}");
    }
}
