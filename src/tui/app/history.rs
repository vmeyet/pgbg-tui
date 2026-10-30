//! `ctrl-r`: the statements run on the connection on screen, narrowed as fzf does; picking one puts it in the editor.
use super::search::{Search, Searching};
use super::{Action, App, Overlay};
use crate::act::Pane;
use crate::connection::ConnectionName;
use crate::history::Entry;
use crossterm::event::KeyEvent;

/// The history of one connection, each statement once, and the search over it; empty until the backend has read it.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Recall {
    pub(in crate::tui) connection: ConnectionName,
    pub(in crate::tui) all: Vec<Entry>,
    pub(in crate::tui) search: Search<Entry>,
}

impl App {
    /// Opens the search over the history of the connection on screen, and asks the backend to read it.
    pub(super) fn recall(&mut self) -> Vec<Action> {
        let connection = self.sheet().name().clone();
        self.overlay = Some(Overlay::Recall(Recall { connection: connection.clone(), all: vec![], search: Search::new(&[]) }));
        vec![Action::ListHistory(connection)]
    }

    /// Takes the history read for `connection`, while its search is still open.
    pub(super) fn history_read(&mut self, connection: &ConnectionName, entries: Vec<Entry>) {
        let Some(Overlay::Recall(recall)) = &mut self.overlay else { return };
        if recall.connection != *connection {
            return;
        }
        recall.all = entries;
        recall.search.rank(&recall.all);
    }

    /// The search after `key`: a picked statement goes in the editor, below the text already there.
    pub(super) fn recall_key(&mut self, recall: Recall, key: KeyEvent) -> Vec<Action> {
        let Recall { connection, all, search } = recall;
        match search.after(key, Pane::Finder, &all) {
            Searching::Typed(search) | Searching::Unhandled(search, _) => {
                self.overlay = Some(Overlay::Recall(Recall { connection, all, search }));
            }
            Searching::Closed => {}
            Searching::Picked(entry) => self.put_in_editor(entry.statement.as_str()),
            Searching::Quit => return self.quit(),
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{app, shop, typed};
    use super::super::{Answer, Focus};
    use super::*;
    use crate::db::model::Statement;
    use crate::history::Outcome;
    use chrono::DateTime;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn entry(statement: &str) -> Entry {
        Entry {
            statement: Statement::new(statement),
            connection: shop(),
            started_at: DateTime::parse_from_rfc3339("2026-10-26T09:30:00Z").unwrap().to_utc(),
            took_ms: 38,
            outcome: Outcome::Rows { count: 4, is_cut: false },
        }
    }

    fn ctrl_r(app: &mut App) -> Vec<Action> {
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
    }

    fn recalled(app: App, typed: &str) -> App {
        let mut app = app;
        ctrl_r(&mut app);
        app.apply(Answer::History {
            connection: shop(),
            entries: vec![entry("SELECT total FROM orders"), entry("SELECT * FROM customers")],
        });
        for c in typed.chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app
    }

    #[test]
    fn ctrl_r_reads_the_history_of_the_connection_on_screen() {
        assert_eq!(ctrl_r(&mut app()), [Action::ListHistory(shop())]);
    }

    #[test]
    fn a_picked_statement_fills_an_empty_editor_and_takes_the_keys() {
        let app = recalled(app(), "cust");
        assert_eq!((app.sheet().editor.text(), app.focus), ("SELECT * FROM customers", Focus::Editor));
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn a_picked_statement_goes_under_the_text_already_there() {
        assert_eq!(recalled(typed("SELECT 1;"), "").sheet().editor.text(), "SELECT 1;\n\nSELECT total FROM orders");
    }

    #[test]
    fn a_statement_left_open_is_ended_before_its_trailing_comment() {
        let app = recalled(typed("SELECT 1 -- one"), "");
        assert_eq!(app.sheet().editor.text(), "SELECT 1; -- one\n\nSELECT total FROM orders");
    }

    #[test]
    fn enter_on_a_picked_statement_runs_it() {
        let mut app = recalled(typed("SELECT 1;"), "orders");
        let ran = app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(&*ran, [Action::Run { statement, .. }] if statement.as_str() == "SELECT total FROM orders"), "{ran:?}");
    }

    #[test]
    fn a_history_read_for_another_connection_is_left_out() {
        let mut app = app();
        ctrl_r(&mut app);
        app.apply(Answer::History { connection: "prod".parse().unwrap(), entries: vec![entry("SELECT 1")] });
        let Some(Overlay::Recall(recall)) = &app.overlay else { panic!("no search") };
        assert_eq!(recall.all, []);
    }

    #[test]
    fn esc_closes_the_search_and_leaves_the_editor_as_it_was() {
        let mut app = typed("SELECT 1");
        ctrl_r(&mut app);
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!((app.overlay.is_none(), app.sheet().editor.text()), (true, "SELECT 1"));
    }
}
