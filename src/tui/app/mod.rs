//! The pure state machine: keys in, actions out, incoming answers applied. No clock, no I/O.
use crate::connection::{Connection, ConnectionName, Tier, WritePolicy};
use crate::db::model::{ResultSet, Statement};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;
use vtui::field::Field;
use vtui::theme::Theme;

mod keys;

/// What the app asks the runner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// Run this statement on the session.
    Run(Statement),
}

/// What the backend answers.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Incoming {
    Opened,
    /// The session could not open, in the error's words.
    NotOpened(String),
    Result {
        result: ResultSet,
        took: Duration,
    },
    /// The guard kept the statement from the server, and why.
    Refused(String),
    /// The server, or the link to it, failed the statement.
    Failed(String),
}

/// Where the session stands, as the status line says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Link {
    Opening,
    Open,
    Failed(String),
}

/// The last statement run and what came of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Run {
    #[default]
    Idle,
    Running,
    Done {
        result: ResultSet,
        took: Duration,
    },
    Failed(String),
}

/// The whole TUI state; `handle_key` and `apply` are the only ways it changes.
#[derive(Debug)]
pub(super) struct App {
    pub(super) theme: Theme,
    pub(super) name: ConnectionName,
    pub(super) tier: Tier,
    pub(super) writes: WritePolicy,
    pub(super) editor: Field,
    pub(super) link: Link,
    pub(super) run: Run,
    pub(super) should_quit: bool,
}

impl App {
    /// The app on `connection`, its session still opening.
    pub(super) fn new(connection: &Connection, theme: Theme) -> Self {
        Self {
            theme,
            name: connection.name().clone(),
            tier: connection.tier(),
            writes: connection.writes(),
            editor: Field::default(),
            link: Link::Opening,
            run: Run::Idle,
            should_quit: false,
        }
    }

    /// Takes one key; returns what the runner should do about it.
    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if control => {
                self.should_quit = true;
                vec![]
            }
            KeyCode::Enter if key.modifiers.is_empty() => self.run_editor(),
            _ => {
                keys::edit(&mut self.editor, key);
                vec![]
            }
        }
    }

    fn run_editor(&mut self) -> Vec<Action> {
        let is_empty = self.editor.text().trim().is_empty();
        if is_empty || self.run == Run::Running {
            return vec![];
        }
        self.run = Run::Running;
        vec![Action::Run(Statement::new(self.editor.text()))]
    }

    /// Applies one answer of the backend.
    pub(super) fn apply(&mut self, incoming: Incoming) {
        match incoming {
            Incoming::Opened => self.link = Link::Open,
            Incoming::NotOpened(err) => {
                if self.run == Run::Running {
                    self.run = Run::Failed(err.clone());
                }
                self.link = Link::Failed(err);
            }
            Incoming::Result { result, took } => self.run = Run::Done { result, took },
            Incoming::Refused(why) => self.run = Run::Failed(format!("not sent: {why}")),
            Incoming::Failed(err) => self.run = Run::Failed(err),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::connection::Entry;
    use crate::db::model::{Column, ColumnKind, Value};

    pub(crate) fn connection(tier: &str, writes: &str) -> Connection {
        let entry: Entry =
            toml::from_str(&format!("tier = \"{tier}\"\nurl = \"postgres://nina@localhost/shop\"\nwrites = \"{writes}\"\n")).unwrap();
        Connection::new("shop", entry).unwrap()
    }

    pub(crate) fn app() -> App {
        App::new(&connection("local", "read-only"), Theme::default())
    }

    pub(crate) fn typed(text: &str) -> App {
        let mut app = app();
        for c in text.chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app
    }

    pub(crate) fn orders() -> ResultSet {
        let column = |name: &str, kind| Column { name: name.to_owned(), kind, type_name: String::new() };
        ResultSet {
            columns: vec![column("id", ColumnKind::Int), column("customer", ColumnKind::Text)],
            rows: vec![vec![Value::Int(7), Value::Text("nina".into())], vec![Value::Int(12), Value::Null]],
        }
    }

    fn enter(app: &mut App) -> Vec<Action> {
        app.handle_key(KeyEvent::from(KeyCode::Enter))
    }

    #[test]
    fn typing_goes_to_the_editor() {
        assert_eq!(typed("SELECT 1").editor.text(), "SELECT 1");
    }

    #[test]
    fn enter_runs_the_text_of_the_editor() {
        let mut app = typed("SELECT 1");
        assert_eq!(enter(&mut app), [Action::Run(Statement::new("SELECT 1"))]);
        assert_eq!(app.run, Run::Running);
    }

    #[test]
    fn enter_on_an_empty_editor_runs_nothing() {
        let mut app = typed("  ");
        assert_eq!(enter(&mut app), []);
        assert_eq!(app.run, Run::Idle);
    }

    #[test]
    fn enter_while_a_statement_runs_sends_no_second_one() {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        assert_eq!(enter(&mut app), []);
    }

    #[test]
    fn a_result_replaces_the_running_state() {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        app.apply(Incoming::Result { result: orders(), took: Duration::from_millis(38) });
        assert_eq!(app.run, Run::Done { result: orders(), took: Duration::from_millis(38) });
    }

    #[test]
    fn a_refused_statement_says_it_was_not_sent() {
        let mut app = typed("DELETE FROM orders");
        enter(&mut app);
        app.apply(Incoming::Refused("`DELETE` is not a read".into()));
        assert_eq!(app.run, Run::Failed("not sent: `DELETE` is not a read".into()));
    }

    #[test]
    fn a_session_that_cannot_open_fails_the_statement_waiting_on_it() {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        app.apply(Incoming::NotOpened("connection refused".into()));
        assert_eq!(app.link, Link::Failed("connection refused".into()));
        assert_eq!(app.run, Run::Failed("connection refused".into()));
    }

    #[test]
    fn a_session_that_cannot_open_leaves_an_idle_editor_idle() {
        let mut app = app();
        app.apply(Incoming::NotOpened("connection refused".into()));
        assert_eq!(app.run, Run::Idle);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
        assert_eq!(app.editor.text(), "SELECT 1");
    }
}
