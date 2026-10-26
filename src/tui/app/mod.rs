//! The pure state machine: keys in, actions out, incoming answers applied. No clock, no I/O.
use crate::connection::{Connection, ConnectionName, Tier, WritePolicy};
use crate::db::guard;
use crate::db::model::{ResultSet, Statement};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;
use vtui::field::Field;
use vtui::theme::Theme;

pub(super) use grid::Grid;
use grid::Move;
pub(super) use grid::cell_text;

mod grid;
mod keys;

/// What the app asks the runner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// Run this statement on the session.
    Run(Statement),
    /// Cancel the statement the session is running.
    Cancel,
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
    /// `Esc` asked the server to cancel the running statement; its answer has not come yet.
    Cancelling,
    Done {
        result: ResultSet,
        took: Duration,
        grid: Grid,
    },
    Failed(String),
}

/// The pane that has the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Focus {
    #[default]
    Editor,
    Result,
}

/// The whole TUI state; `handle_key`, `apply` and `fit_grid` are the only ways it changes.
#[derive(Debug)]
pub(super) struct App {
    pub(super) theme: Theme,
    pub(super) name: ConnectionName,
    pub(super) tier: Tier,
    pub(super) writes: WritePolicy,
    pub(super) editor: Field,
    pub(super) link: Link,
    pub(super) run: Run,
    pub(super) focus: Focus,
    /// The grid rows the last frame had room for: a page, for `PgDn`.
    grid_rows: usize,
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
            focus: Focus::Editor,
            grid_rows: 0,
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
            KeyCode::Esc if self.run == Run::Running => {
                self.run = Run::Cancelling;
                vec![Action::Cancel]
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Editor => Focus::Result,
                    Focus::Result => Focus::Editor,
                };
                vec![]
            }
            _ => match self.focus {
                Focus::Editor => self.editor_key(key),
                Focus::Result => {
                    self.grid_key(key);
                    vec![]
                }
            },
        }
    }

    /// Tells the app how many grid rows the frame just drawn had room for, and keeps the cursor among them.
    pub(super) fn fit_grid(&mut self, rows: usize) {
        self.grid_rows = rows;
        if let Run::Done { grid, .. } = &mut self.run {
            grid.keep_in_view(rows);
        }
    }

    fn editor_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if key.code == KeyCode::Enter && key.modifiers.is_empty() {
            return self.run_editor();
        }
        keys::edit(&mut self.editor, key);
        vec![]
    }

    fn grid_key(&mut self, key: KeyEvent) {
        if let (Run::Done { grid, .. }, Some(step)) = (&mut self.run, Move::from_key(key)) {
            grid.step(step, self.grid_rows);
        }
    }

    fn is_running(&self) -> bool {
        matches!(self.run, Run::Running | Run::Cancelling)
    }

    /// Runs the statement under the cursor, the editor's other statements left alone.
    fn run_editor(&mut self) -> Vec<Action> {
        if self.is_running() {
            return vec![];
        }
        let (before, _, _) = self.editor.split();
        let Some(range) = guard::statement_at(self.editor.text(), before.len()) else { return vec![] };
        self.run = Run::Running;
        vec![Action::Run(Statement::new(&self.editor.text()[range]))]
    }

    /// Applies one answer of the backend.
    pub(super) fn apply(&mut self, incoming: Incoming) {
        match incoming {
            Incoming::Opened => self.link = Link::Open,
            Incoming::NotOpened(err) => {
                if self.is_running() {
                    self.run = Run::Failed(err.clone());
                }
                self.link = Link::Failed(err);
            }
            Incoming::Result { result, took } => self.run = Run::Done { grid: Grid::new(&result), result, took },
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
        let row = |id: i64, total: &str, customer: Value| vec![Value::Int(id), Value::Numeric(total.into()), customer];
        ResultSet {
            columns: vec![column("id", ColumnKind::Int), column("total", ColumnKind::Numeric), column("customer", ColumnKind::Text)],
            rows: vec![
                row(7, "12.50", Value::Text("nina".into())),
                row(12, "1499.00", Value::Null),
                row(103, "3.20", Value::Text("Zoë Nakamura".into())),
                row(104, "0.99", Value::Text("noel".into())),
            ],
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
    fn enter_runs_only_the_statement_under_the_cursor() {
        let mut app = typed("SELECT 1;\nSELECT 2;\nSELECT 3");
        app.handle_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(enter(&mut app), [Action::Run(Statement::new("SELECT 2"))]);
    }

    #[test]
    fn esc_cancels_a_running_statement_once() {
        let mut app = typed("SELECT pg_sleep(30)");
        enter(&mut app);
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Esc)), [Action::Cancel]);
        assert_eq!(app.run, Run::Cancelling);
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Esc)), []);
    }

    #[test]
    fn esc_with_nothing_running_cancels_nothing() {
        assert_eq!(typed("SELECT 1").handle_key(KeyEvent::from(KeyCode::Esc)), []);
    }

    #[test]
    fn enter_while_a_cancel_is_on_its_way_sends_nothing() {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(enter(&mut app), []);
    }

    #[test]
    fn a_result_replaces_the_running_state() {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        app.apply(Incoming::Result { result: orders(), took: Duration::from_millis(38) });
        assert_eq!(app.run, Run::Done { grid: Grid::new(&orders()), result: orders(), took: Duration::from_millis(38) });
    }

    fn with_result() -> App {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        app.apply(Incoming::Result { result: orders(), took: Duration::from_millis(38) });
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
    }

    #[test]
    fn tab_moves_the_keys_to_the_result_and_back() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Result);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn j_on_the_result_moves_its_cursor_and_types_nothing() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('j'));
        let Run::Done { grid, .. } = &app.run else { panic!("no result") };
        assert_eq!(grid.row, 1);
        assert_eq!(app.editor.text(), "SELECT 1");
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
