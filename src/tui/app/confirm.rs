//! A write asks on the footer line before it is sent: `y` sends it, any other key drops it.
use super::sheet::ToRun;
use super::{Action, App, Overlay, Prompt};
use crate::db::guard::Write;

/// A statement the guard read as a write, waiting for a `y`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct WriteToSend {
    pub(in crate::tui) write: Write,
    to_run: ToRun,
}

impl App {
    /// Runs the statement under the cursor; a write asks first. Nothing while another one runs.
    pub(super) fn run_editor(&mut self) -> Vec<Action> {
        let sheet = self.sheet_mut();
        let Some(to_run) = sheet.editor_run().filter(|_| !sheet.is_running()) else { return vec![] };
        let Some(write) = sheet.write_under_cursor() else { return sheet.run(to_run) };
        self.overlay = Some(Overlay::Prompt(Prompt::SendWrite(Box::new(WriteToSend { write, to_run }))));
        vec![]
    }

    pub(super) fn send_write(&mut self, waiting: WriteToSend) -> Vec<Action> {
        self.sheet_mut().run(waiting.to_run)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app_on, connection, first_run, shop, with_focus};
    use super::super::{Focus, Run};
    use super::*;
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
        assert_eq!(press(&mut app, KeyCode::Char('y')), [first_run(&shop(), "DELETE FROM orders")]);
        assert_eq!(app.overlay, None);
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
