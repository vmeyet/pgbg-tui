//! The pure state machine: keys in, actions out, incoming answers applied. No clock, no I/O.
use crate::act::{self, Act, Pane};
use crate::connection::{Connection, ConnectionName, Tier, WritePolicy};
use crate::db::guard;
use crate::db::model::{LocalPort, ResultSet, Statement};
use crate::syntax::Spans;
use crossterm::event::{KeyCode, KeyEvent};
use std::time::Duration;
use vtui::field::Field;
use vtui::help::Help;
use vtui::theme::Theme;

pub(super) use grid::Grid;

const LOST_SESSION: &str = "session lost, ⏎ opens a new one";
use grid::Move;

mod grid;
mod keys;

/// Which run of a statement an answer or a cancel is about, so a late one never lands on the next run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RunId(u64);

impl RunId {
    pub(super) const FIRST: Self = Self(1);

    pub(super) fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// What the app asks the runner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// Run this statement on the session.
    Run { run: RunId, statement: Statement },
    /// Cancel this run if the session is still on it.
    Cancel(RunId),
}

/// What the backend answers.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Incoming {
    Opened,
    /// The session could not open, in the error's words.
    NotOpened(String),
    /// The open session was lost, and why; the next run opens a new one.
    Closed(String),
    Tunnel(TunnelState),
    Result {
        run: RunId,
        result: ResultSet,
        took: Duration,
    },
    /// The guard kept the statement from the server, and why.
    Refused {
        run: RunId,
        why: String,
    },
    /// The server, or the link to it, failed the statement; the server may say where in it.
    Failed {
        run: RunId,
        error: String,
        /// A character count from 1 into the statement sent, as PostgreSQL counts.
        position: Option<usize>,
    },
}

/// The statement last sent, its run, and the byte of the editor it starts at until the text changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    run: RunId,
    start: Option<usize>,
    statement: Statement,
}

/// Where the session stands, as the status line says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Link {
    Opening,
    Open,
    Failed(String),
}

/// Where the connection's tunnel stands, as the status line says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TunnelState {
    Starting,
    Up(LocalPort),
    /// It did not start, or it ended, and how.
    Died(String),
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

impl Focus {
    /// The pane of the action table whose keys work here.
    pub(super) fn pane(self) -> Pane {
        match self {
            Self::Editor => Pane::Editor,
            Self::Result => Pane::Result,
        }
    }
}

/// The help after `key`: `?` lists every key, `j`/`k` scroll, any other key closes it.
fn help_after(help: Help, key: KeyEvent, focus: Focus) -> Option<Help> {
    let last_row = vtui::help::last_row(&act::sections(focus.pane(), help.every_key));
    match key.code {
        KeyCode::Char('?') if !help.every_key => Some(Help { scroll: 0, every_key: true }),
        KeyCode::Char('j') | KeyCode::Down => Some(Help { scroll: (help.scroll + 1).min(last_row), ..help }),
        KeyCode::Char('k') | KeyCode::Up => Some(Help { scroll: help.scroll.saturating_sub(1), ..help }),
        _ => None,
    }
}

/// The whole TUI state; `handle_key`, `apply` and `fit_grid` are the only ways it changes.
#[derive(Debug)]
pub(super) struct App {
    pub(super) theme: Theme,
    pub(super) name: ConnectionName,
    pub(super) tier: Tier,
    pub(super) writes: WritePolicy,
    pub(super) editor: Field,
    /// The editor's syntax colours, one entry per line, rebuilt only when its text changes.
    pub(super) colours: Vec<Spans>,
    pub(super) link: Link,
    /// The connection's tunnel, once it has one and its task said where it stands.
    pub(super) tunnel: Option<TunnelState>,
    pub(super) run: Run,
    pub(super) focus: Focus,
    /// The `?` overlay while it is open; it takes every key.
    pub(super) help: Option<Help>,
    /// The grid rows the last frame had room for: a page, for `PgDn`.
    grid_rows: usize,
    sent: Option<Sent>,
    /// The editor's byte the server's last error points at, until the text changes.
    pub(super) error_mark: Option<usize>,
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
            colours: Vec::new(),
            link: Link::Opening,
            tunnel: None,
            run: Run::Idle,
            focus: Focus::Editor,
            help: None,
            grid_rows: 0,
            sent: None,
            error_mark: None,
            should_quit: false,
        }
    }

    /// Takes one key; returns what the runner should do about it.
    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if let Some(help) = self.help {
            self.help = help_after(help, key, self.focus);
            return vec![];
        }
        if let Some(act) = act::act_for(key, self.focus.pane()) {
            return self.act(act);
        }
        if self.focus == Focus::Editor {
            self.edit(key);
        }
        vec![]
    }

    fn act(&mut self, act: Act) -> Vec<Action> {
        match act {
            Act::Run => return self.run_editor(),
            Act::Cancel => return self.cancel(),
            Act::NewLine => self.edit(KeyEvent::from(KeyCode::Enter)),
            Act::SwitchPane => self.focus = if self.focus == Focus::Editor { Focus::Result } else { Focus::Editor },
            Act::Help => self.help = Some(Help::default()),
            Act::Quit => return self.quit(),
            Act::Down => self.grid_step(Move::Down),
            Act::Up => self.grid_step(Move::Up),
            Act::PageDown => self.grid_step(Move::PageDown),
            Act::PageUp => self.grid_step(Move::PageUp),
            Act::Top => self.grid_step(Move::First),
            Act::Bottom => self.grid_step(Move::Last),
            Act::Left => self.grid_step(Move::Left),
            Act::Right => self.grid_step(Move::Right),
        }
        vec![]
    }

    /// Tells the app how many grid rows the frame just drawn had room for, and keeps the cursor among them.
    pub(super) fn fit_grid(&mut self, rows: usize) {
        self.grid_rows = rows;
        if let Run::Done { grid, .. } = &mut self.run {
            grid.keep_in_view(rows);
        }
    }

    fn grid_step(&mut self, step: Move) {
        if let Run::Done { grid, result, .. } = &mut self.run {
            grid.step(step, result, self.grid_rows);
        }
    }

    fn cancel(&mut self) -> Vec<Action> {
        let Some(run) = self.running_run().filter(|_| self.run == Run::Running) else { return vec![] };
        self.run = Run::Cancelling;
        vec![Action::Cancel(run)]
    }

    /// Quits, cancelling the running statement first so the server does not run it to its end.
    fn quit(&mut self) -> Vec<Action> {
        self.should_quit = true;
        self.running_run().map(Action::Cancel).into_iter().collect()
    }

    fn is_running(&self) -> bool {
        matches!(self.run, Run::Running | Run::Cancelling)
    }

    fn running_run(&self) -> Option<RunId> {
        self.sent.as_ref().filter(|_| self.is_running()).map(|sent| sent.run)
    }

    fn is_answer_to_running(&self, run: RunId) -> bool {
        self.running_run() == Some(run)
    }

    /// Runs the statement under the cursor, the editor's other statements left alone.
    fn run_editor(&mut self) -> Vec<Action> {
        if self.is_running() {
            return vec![];
        }
        let (before, _, _) = self.editor.split();
        let Some(range) = guard::statement_at(self.editor.text(), before.len()) else { return vec![] };
        let statement = Statement::new(&self.editor.text()[range.clone()]);
        let run = self.sent.as_ref().map_or(RunId::FIRST, |sent| sent.run.next());
        self.run = Run::Running;
        self.error_mark = None;
        self.sent = Some(Sent { run, start: Some(range.start), statement: statement.clone() });
        vec![Action::Run { run, statement }]
    }

    /// The editor's byte at character `position` of the statement sent; the end of it when the server
    /// points just past its last character.
    fn editor_byte(&self, position: usize) -> Option<usize> {
        let sent = self.sent.as_ref()?;
        let text = sent.statement.as_str();
        let offset = text.char_indices().map(|(at, _)| at).chain(std::iter::once(text.len())).nth(position.checked_sub(1)?)?;
        Some(sent.start? + offset)
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
            Incoming::Closed(why) => self.link = Link::Failed(format!("{LOST_SESSION}: {why}")),
            Incoming::Tunnel(state) => self.tunnel = Some(state),
            Incoming::Result { run, .. } | Incoming::Refused { run, .. } | Incoming::Failed { run, .. }
                if !self.is_answer_to_running(run) => {}
            Incoming::Result { result, took, .. } => self.run = Run::Done { grid: Grid::new(&result), result, took },
            Incoming::Refused { why, .. } => self.run = Run::Failed(format!("not sent: {why}")),
            Incoming::Failed { error, position, .. } => {
                self.error_mark = position.and_then(|position| self.editor_byte(position));
                self.run = Run::Failed(error);
            }
        }
    }

    /// Forgets where the statement sent stands in the editor, once the text it was cut from changed.
    fn forget_sent_place(&mut self) {
        if let Some(sent) = &mut self.sent {
            sent.start = None;
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::connection::Entry;
    use crate::db::model::{Column, ColumnKind, Value};
    use crossterm::event::KeyModifiers;

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
            is_cut: false,
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
        assert_eq!(enter(&mut app), [Action::Run { run: RunId::FIRST, statement: Statement::new("SELECT 1") }]);
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
        assert_eq!(enter(&mut app), [Action::Run { run: RunId::FIRST, statement: Statement::new("SELECT 2") }]);
    }

    #[test]
    fn esc_cancels_a_running_statement_once() {
        let mut app = typed("SELECT pg_sleep(30)");
        enter(&mut app);
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Esc)), [Action::Cancel(RunId::FIRST)]);
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
        app.apply(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) });
        assert_eq!(app.run, Run::Done { grid: Grid::new(&orders()), result: orders(), took: Duration::from_millis(38) });
    }

    fn with_result() -> App {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        app.apply(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) });
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
        app.apply(Incoming::Refused { run: RunId::FIRST, why: "`DELETE` is not a read".into() });
        assert_eq!(app.run, Run::Failed("not sent: `DELETE` is not a read".into()));
    }

    fn failed_at(app: &mut App, position: usize) {
        app.apply(Incoming::Failed { run: RunId::FIRST, error: "ERROR: syntax error".into(), position: Some(position) });
    }

    #[test]
    fn the_colours_follow_the_text_as_it_is_typed() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(app.colours, [vec![(0..6, crate::syntax::Token::Keyword)]]);
    }

    #[test]
    fn a_server_error_marks_its_position_in_the_editor() {
        let mut app = typed("SELECT 1;\nSELECT * FROM no_such_orders");
        enter(&mut app);
        failed_at(&mut app, 15);
        assert_eq!(app.error_mark.map(|at| &app.editor.text()[at..]), Some("no_such_orders"));
    }

    #[test]
    fn a_position_past_the_last_character_marks_the_end_of_the_statement() {
        let mut app = typed("SELECT ;");
        app.handle_key(KeyEvent::from(KeyCode::Left));
        enter(&mut app);
        failed_at(&mut app, 7);
        assert_eq!(app.error_mark, Some(6));
    }

    #[test]
    fn the_mark_counts_characters_not_bytes() {
        let mut app = typed("SELECT 'zoë', x");
        enter(&mut app);
        failed_at(&mut app, 15);
        assert_eq!(app.error_mark.map(|at| &app.editor.text()[at..]), Some("x"));
    }

    #[test]
    fn editing_the_text_drops_the_mark() {
        let mut app = typed("SELECT x");
        enter(&mut app);
        failed_at(&mut app, 8);
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(app.error_mark, Some(7));
        app.handle_key(KeyEvent::from(KeyCode::Char('y')));
        assert_eq!(app.error_mark, None);
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
    fn alt_enter_breaks_the_line_and_runs_nothing() {
        let mut app = typed("SELECT *");
        assert_eq!(app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)), []);
        assert_eq!(app.editor.text(), "SELECT *\n");
    }

    #[test]
    fn a_question_mark_in_the_editor_is_typed() {
        let app = typed("SELECT '?'");
        assert_eq!((app.editor.text(), app.help), ("SELECT '?'", None));
    }

    #[test]
    fn a_question_mark_on_the_result_opens_the_help_and_any_other_key_closes_it() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.help, Some(Help::default()));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.help, None);
        let Run::Done { grid, .. } = &app.run else { panic!("no result") };
        assert_eq!(grid.row, 0);
    }

    #[test]
    fn a_second_question_mark_lists_every_key() {
        let mut app = typed("SELECT 1");
        press(&mut app, KeyCode::F(1));
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.help, Some(Help { scroll: 0, every_key: true }));
    }

    #[test]
    fn each_run_gets_a_new_id() {
        let mut app = with_result();
        let Action::Run { run, .. } = enter(&mut app).remove(0) else { panic!("no run") };
        assert_eq!(run, RunId::FIRST.next());
    }

    #[test]
    fn a_late_answer_to_an_earlier_run_is_dropped() {
        let mut app = with_result();
        enter(&mut app);
        app.apply(Incoming::Failed { run: RunId::FIRST, error: "ERROR: canceling statement".into(), position: None });
        assert_eq!(app.run, Run::Running);
    }

    #[test]
    fn ctrl_c_while_a_statement_runs_cancels_it_and_quits() {
        let mut app = typed("SELECT pg_sleep(30)");
        enter(&mut app);
        assert_eq!(app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), [Action::Cancel(RunId::FIRST)]);
        assert!(app.should_quit);
    }

    #[test]
    fn a_lost_session_says_so_in_the_status_line() {
        let mut app = app();
        app.apply(Incoming::Opened);
        app.apply(Incoming::Closed("terminating connection".into()));
        assert_eq!(app.link, Link::Failed(format!("{LOST_SESSION}: terminating connection")));
    }

    #[test]
    fn an_edit_while_the_statement_runs_keeps_its_error_unmarked() {
        let mut app = typed("SELECT x");
        enter(&mut app);
        app.handle_key(KeyEvent::from(KeyCode::Home));
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        failed_at(&mut app, 8);
        assert_eq!(app.error_mark, None);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
        assert_eq!(app.editor.text(), "SELECT 1");
    }
}
