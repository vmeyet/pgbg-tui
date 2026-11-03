//! `^b` and `^f` in the editor, or `⌥e` and `⌥a`: the `EXPLAIN` of the statement under the cursor, sent as a run.
use super::{Action, App};
use crate::db::explain::Explain;

const NOT_A_READ: &str = "only a SELECT, WITH, VALUES or TABLE that changes nothing can be explained here";

impl App {
    /// Runs `explain` of the statement under the cursor, or says on the status line that it cannot be explained.
    pub(super) fn explain(&mut self, explain: Explain) -> Vec<Action> {
        self.sheet_mut().explain(explain).unwrap_or_else(|| {
            self.note = Some(NOT_A_READ.to_owned());
            vec![]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{app_on, connection, enter, first_run, press, shop, to_shop, typed};
    use super::super::{Incoming, RunId};
    use super::*;
    use crate::failure::Failure;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn option(app: &mut App, c: char) -> Vec<Action> {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
    }

    #[test]
    fn option_e_explains_the_statement_under_the_cursor_without_running_it() {
        let mut app = typed("SELECT 1;\nSELECT 2");
        assert_eq!(option(&mut app, 'e'), [first_run(&shop(), "EXPLAIN (FORMAT JSON) SELECT 2")]);
    }

    #[test]
    fn option_a_asks_for_a_read_only_analyze_with_the_buffers() {
        let mut app = typed("SELECT 2");
        let [Action::Analyze { statement, .. }] = &option(&mut app, 'a')[..] else { panic!("no analyze") };
        assert_eq!(statement.as_str(), "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT 2");
    }

    #[test]
    fn option_a_binds_the_parameters_as_a_run_does() {
        let mut app = typed("SELECT {{id}}");
        let [Action::Analyze { input, .. }] = &option(&mut app, 'a')[..] else { panic!("no analyze") };
        let Action::Run { input: as_run, .. } = enter(&mut typed("SELECT {{id}}")).remove(0) else { panic!("no run") };
        assert_eq!(*input, as_run);
    }

    #[test]
    fn either_key_on_a_write_sends_nothing_and_says_why_where_writes_are_allowed() {
        for key in ['e', 'a'] {
            let mut app = app_on(&connection("production", "allowed"));
            "DELETE FROM orders".chars().for_each(|c| press(&mut app, KeyCode::Char(c)));
            assert_eq!(option(&mut app, key), []);
            assert_eq!((app.note(), app.overlay.is_none()), (Some(NOT_A_READ), true));
        }
    }

    #[test]
    fn option_e_on_an_empty_editor_says_nothing() {
        let mut app = typed("  ");
        assert_eq!((option(&mut app, 'e'), app.note()), (vec![], None));
    }

    #[test]
    fn option_e_while_a_statement_runs_sends_nothing() {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        assert_eq!(option(&mut app, 'e'), []);
    }

    #[test]
    fn a_server_error_under_the_explain_marks_the_statement_in_the_editor() {
        let mut app = typed("SELECT 1;\nSELECT nope");
        option(&mut app, 'e');
        let position = "EXPLAIN (FORMAT JSON) SELECT n".len();
        let failure = Failure::Server { message: "ERROR: no such column".into(), position: Some(position), code: None };
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, failure }));
        assert_eq!(app.sheet().error_mark.clone().map(|at| &app.sheet().editor.text()[at]), Some("nope"));
    }

    #[test]
    fn a_server_error_in_the_explain_itself_marks_nothing() {
        let mut app = typed("SELECT 1");
        option(&mut app, 'e');
        let failure = Failure::Server { message: "ERROR: unknown option".into(), position: Some(3), code: None };
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, failure }));
        assert_eq!(app.sheet().error_mark, None);
    }
}
