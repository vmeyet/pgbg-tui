//! The pure state machine: keys in, actions out, incoming answers applied. No clock, no I/O.
use crate::act::{self, Act, Pane};
use crate::connection::Connection;
use crossterm::event::{KeyCode, KeyEvent};
use vtui::help::Help;
use vtui::palette::Palette;
use vtui::theme::Theme;

pub(super) use grid::Grid;
use grid::Move;
pub(super) use picker::Picking;
pub(super) use protocol::{Action, Answer, Incoming, QueryAnswer, RunId, TunnelState};
pub(super) use queries::{Finder, Queries};
pub(super) use sheet::{Run, SessionState, Sheet, View};

mod editor;
mod grid;
pub(super) mod picker;
mod protocol;
pub(super) mod queries;
mod sheet;

/// The pane that has the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Focus {
    #[default]
    Editor,
    Result,
    Queries,
}

impl Focus {
    /// The order `tab` goes through the panes.
    const ORDER: [Self; 3] = [Self::Editor, Self::Result, Self::Queries];

    /// The pane of the action table whose keys work here.
    pub(super) fn pane(self) -> Pane {
        match self {
            Self::Editor => Pane::Editor,
            Self::Result => Pane::Result,
            Self::Queries => Pane::Queries,
        }
    }
}

/// What the frame just drawn had room for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Fit {
    /// The grid rows: a page, for `PgDn`.
    pub(super) grid_rows: usize,
    /// Whether the sidebar of queries was drawn, which a narrow terminal or no query at all leaves out.
    pub(super) has_sidebar: bool,
}

/// What lies over the panes and takes every key while it is open.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Overlay {
    /// The `?` list of keys.
    Help(Help),
    /// The `ctrl-p` finder of saved queries.
    Finder(Finder),
    /// The `ctrl-o` connection picker.
    Picker(Palette<Picking>),
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

/// The whole TUI state; `handle_key`, `apply` and `fit` are the only ways it changes.
#[derive(Debug)]
pub(super) struct App {
    pub(super) theme: Theme,
    /// Every connection of the config, by name, for the picker.
    pub(super) connections: Vec<Connection>,
    /// A sheet for each connection opened since pgbg started, in the order they were opened.
    sheets: Vec<Sheet>,
    /// The index in `sheets` of the one on screen.
    shown: usize,
    pub(super) focus: Focus,
    pub(super) overlay: Option<Overlay>,
    pub(super) queries: Queries,
    fit: Fit,
    pub(super) should_quit: bool,
}

impl App {
    /// The app on `first`, its session still opening, with `connections` to pick from.
    pub(super) fn new(connections: Vec<Connection>, first: &Connection, theme: Theme) -> Self {
        Self {
            theme,
            connections,
            sheets: vec![Sheet::new(first)],
            shown: 0,
            focus: Focus::Editor,
            overlay: None,
            queries: Queries::default(),
            fit: Fit::default(),
            should_quit: false,
        }
    }

    /// The sheet on screen.
    pub(super) fn sheet(&self) -> &Sheet {
        &self.sheets[self.shown]
    }

    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheets[self.shown]
    }

    /// What the runner should do as the TUI starts.
    pub(super) fn start() -> Vec<Action> {
        vec![Action::ListQueries]
    }

    /// Takes one key; returns what the runner should do about it.
    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if let Some(overlay) = self.overlay.take() {
            return self.overlay_key(overlay, key);
        }
        if let Some(act) = act::act_for(key, self.focus.pane()) {
            return self.act(act);
        }
        if self.focus == Focus::Editor {
            self.sheet_mut().edit(key);
        }
        vec![]
    }

    /// Takes text the terminal pasted: the editor or the finder gets it whole, so its line ends run nothing.
    pub(super) fn paste(&mut self, text: &str) -> Vec<Action> {
        if let Some(Overlay::Finder(finder)) = self.overlay.as_mut() {
            finder.paste(text, &self.queries.all);
            return vec![];
        }
        let is_editor_on_top = self.overlay.is_none() && self.focus == Focus::Editor;
        if is_editor_on_top {
            self.sheet_mut().paste(text);
        }
        vec![]
    }

    /// Whether the editor on screen waits for `recolour`.
    pub(super) fn has_stale_colours(&self) -> bool {
        self.sheet().editor.has_stale_colours()
    }

    /// Rebuilds the colours of the editor on screen, once its text stopped changing for a moment.
    pub(super) fn recolour(&mut self) {
        self.sheet_mut().editor.recolour();
    }

    /// Hands `key` to `overlay`, which stays open unless the key closes it.
    fn overlay_key(&mut self, overlay: Overlay, key: KeyEvent) -> Vec<Action> {
        match overlay {
            Overlay::Help(help) => {
                self.overlay = help_after(help, key, self.focus).map(Overlay::Help);
                vec![]
            }
            Overlay::Finder(finder) => {
                let (finder, actions) = self.finder_after(finder, key);
                self.overlay = finder.map(Overlay::Finder);
                actions
            }
            Overlay::Picker(picker) => self.picker_key(picker, key),
        }
    }

    fn act(&mut self, act: Act) -> Vec<Action> {
        match act {
            Act::Run => return self.sheet_mut().run_editor(),
            Act::Cancel => return self.sheet_mut().cancel(),
            Act::NewLine => self.sheet_mut().edit(KeyEvent::from(KeyCode::Enter)),
            Act::SwitchPane => self.focus = self.next_pane(),
            Act::Help => self.overlay = Some(Overlay::Help(Help::default())),
            Act::Quit => return self.quit(),
            Act::Down if self.focus == Focus::Queries => self.step_sidebar(1),
            Act::Up if self.focus == Focus::Queries => self.step_sidebar(-1),
            Act::Down => self.grid_step(Move::Down),
            Act::Up => self.grid_step(Move::Up),
            Act::PageDown => self.grid_step(Move::PageDown),
            Act::PageUp => self.grid_step(Move::PageUp),
            Act::Top => self.grid_step(Move::First),
            Act::Bottom => self.grid_step(Move::Last),
            Act::Left => self.grid_step(Move::Left),
            Act::Right => self.grid_step(Move::Right),
            Act::Find => return self.find(),
            Act::Connections => self.open_picker(),
            Act::Chart => self.sheet_mut().switch_view(),
            Act::Open => return self.open_in_sidebar(),
            Act::Close => {}
        }
        vec![]
    }

    /// The pane `tab` goes to: the next in `Focus::ORDER` that the frame drew.
    fn next_pane(&self) -> Focus {
        let at = Focus::ORDER.iter().position(|&focus| focus == self.focus).unwrap_or_default();
        let after = Focus::ORDER.iter().cycle().skip(at + 1).take(Focus::ORDER.len());
        after.copied().find(|&focus| self.is_drawn(focus)).unwrap_or_default()
    }

    fn is_drawn(&self, focus: Focus) -> bool {
        focus != Focus::Queries || self.fit.has_sidebar
    }

    /// Tells the app what the frame just drawn had room for: the grid's cursor stays among its rows, and the keys
    /// leave a sidebar no longer drawn.
    pub(super) fn fit(&mut self, fit: Fit) {
        self.fit = fit;
        if let Run::Done { grid, view: View::Grid, .. } = &mut self.sheet_mut().run {
            grid.keep_in_view(fit.grid_rows);
        }
        if !self.is_drawn(self.focus) {
            self.focus = Focus::Editor;
        }
    }

    fn grid_step(&mut self, step: Move) {
        let rows = self.fit.grid_rows;
        if let Run::Done { grid, result, .. } = &mut self.sheet_mut().run {
            grid.step(step, result, rows);
        }
    }

    /// Quits, cancelling every running statement first so no server runs one to its end.
    pub(super) fn quit(&mut self) -> Vec<Action> {
        self.should_quit = true;
        self.sheets.iter().filter_map(Sheet::cancel_of_running).collect()
    }

    /// Applies one answer: a session's to the sheet of its connection, the saved queries' to the sheet on screen.
    pub(super) fn apply(&mut self, answer: Answer) {
        match answer {
            Answer::Session { connection, incoming } => {
                if let Some(sheet) = self.sheets.iter_mut().find(|sheet| sheet.name == connection) {
                    sheet.apply(incoming);
                }
            }
            Answer::Queries(QueryAnswer::All(all)) => self.listed(all),
            Answer::Queries(QueryAnswer::Opened { query, text }) => self.opened(query, &text),
            Answer::Queries(QueryAnswer::NotOpened { query, error }) => {
                self.queries.error = Some(format!("could not open {}: {error}", query.path));
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::sheet::LOST_SESSION;
    use super::*;
    use crate::chart;
    use crate::connection::{ConnectionName, Entry};
    use crate::db::model::{Column, ColumnKind, ResultSet, Statement, Value};
    use crossterm::event::KeyModifiers;
    use std::time::Duration;

    pub(crate) fn connection(tier: &str, writes: &str) -> Connection {
        let entry: Entry =
            toml::from_str(&format!("tier = \"{tier}\"\nurl = \"postgres://nina@localhost/shop\"\nwrites = \"{writes}\"\n")).unwrap();
        Connection::new("shop", entry).unwrap()
    }

    pub(crate) fn app_on(connection: &Connection) -> App {
        App::new(vec![connection.clone()], connection, Theme::default())
    }

    pub(crate) fn app() -> App {
        app_on(&connection("local", "read-only"))
    }

    pub(crate) fn shop() -> ConnectionName {
        "shop".parse().unwrap()
    }

    pub(crate) fn to_shop(incoming: Incoming) -> Answer {
        Answer::Session { connection: shop(), incoming }
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
        assert_eq!(typed("SELECT 1").sheet().editor.text(), "SELECT 1");
    }

    #[test]
    fn enter_runs_the_text_of_the_editor() {
        let mut app = typed("SELECT 1");
        assert_eq!(enter(&mut app), [Action::Run { connection: shop(), run: RunId::FIRST, statement: Statement::new("SELECT 1") }]);
        assert_eq!(app.sheet().run, Run::Running { run: RunId::FIRST });
    }

    #[test]
    fn enter_on_an_empty_editor_runs_nothing() {
        let mut app = typed("  ");
        assert_eq!(enter(&mut app), []);
        assert_eq!(app.sheet().run, Run::Idle);
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
        assert_eq!(enter(&mut app), [Action::Run { connection: shop(), run: RunId::FIRST, statement: Statement::new("SELECT 2") }]);
    }

    #[test]
    fn esc_cancels_a_running_statement_once() {
        let mut app = typed("SELECT pg_sleep(30)");
        enter(&mut app);
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Esc)), [Action::Cancel { connection: shop(), run: RunId::FIRST }]);
        assert_eq!(app.sheet().run, Run::Cancelling { run: RunId::FIRST });
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
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        assert_eq!(
            app.sheet().run,
            Run::Done { grid: Grid::new(&orders()), result: orders(), took: Duration::from_millis(38), view: View::Grid }
        );
    }

    fn with_result() -> App {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
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

    fn view(app: &App) -> &View {
        let Run::Done { view, .. } = &app.sheet().run else { panic!("no result") };
        view
    }

    #[test]
    fn v_on_the_result_shows_its_chart_then_the_grid_again() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(view(&app), &View::Chart(chart::infer_series(&orders())));
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(view(&app), &View::Grid);
    }

    #[test]
    fn v_in_the_editor_types_a_v() {
        let mut app = with_result();
        press(&mut app, KeyCode::Char('v'));
        assert_eq!((view(&app), app.sheet().editor.text()), (&View::Grid, "SELECT 1v"));
    }

    #[test]
    fn a_new_result_starts_on_the_grid() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('v'));
        press(&mut app, KeyCode::Tab);
        enter(&mut app);
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST.next(), result: orders(), took: Duration::from_millis(5) }));
        assert_eq!(view(&app), &View::Grid);
    }

    #[test]
    fn j_on_the_result_moves_its_cursor_and_types_nothing() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('j'));
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(grid.row, 1);
        assert_eq!(app.sheet().editor.text(), "SELECT 1");
    }

    #[test]
    fn a_refused_statement_says_it_was_not_sent() {
        let mut app = typed("DELETE FROM orders");
        enter(&mut app);
        app.apply(to_shop(Incoming::Refused { run: RunId::FIRST, why: "`DELETE` is not a read".into() }));
        assert_eq!(app.sheet().run, Run::Failed("not sent: `DELETE` is not a read".into()));
    }

    fn failed_at(app: &mut App, position: usize) {
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, error: "ERROR: syntax error".into(), position: Some(position) }));
    }

    #[test]
    fn the_colours_follow_the_text_once_rebuilt() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert!(app.has_stale_colours());
        app.recolour();
        assert_eq!(app.sheet().editor.colours(), [vec![crate::syntax::Painted { bytes: 0..6, token: crate::syntax::Token::Keyword }]]);
    }

    #[test]
    fn a_pasted_statement_with_line_ends_runs_nothing() {
        let mut app = app();
        assert_eq!(app.paste("DELETE FROM orders\nWHERE id = 7;"), []);
        assert_eq!((app.sheet().editor.text(), &app.sheet().run), ("DELETE FROM orders\nWHERE id = 7;", &Run::Idle));
    }

    #[test]
    fn a_paste_into_the_result_pane_types_nothing() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        app.paste("jjj");
        assert_eq!(app.sheet().editor.text(), "SELECT 1");
    }

    #[test]
    fn a_server_error_marks_its_position_in_the_editor() {
        let mut app = typed("SELECT 1;\nSELECT * FROM no_such_orders");
        enter(&mut app);
        failed_at(&mut app, 15);
        assert_eq!(app.sheet().error_mark.map(|at| &app.sheet().editor.text()[at..]), Some("no_such_orders"));
    }

    #[test]
    fn a_position_past_the_last_character_marks_the_end_of_the_statement() {
        let mut app = typed("SELECT ;");
        app.handle_key(KeyEvent::from(KeyCode::Left));
        enter(&mut app);
        failed_at(&mut app, 7);
        assert_eq!(app.sheet().error_mark, Some(6));
    }

    #[test]
    fn the_mark_counts_characters_not_bytes() {
        let mut app = typed("SELECT 'zoë', x");
        enter(&mut app);
        failed_at(&mut app, 15);
        assert_eq!(app.sheet().error_mark.map(|at| &app.sheet().editor.text()[at..]), Some("x"));
    }

    #[test]
    fn editing_the_text_drops_the_mark() {
        let mut app = typed("SELECT x");
        enter(&mut app);
        failed_at(&mut app, 8);
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(app.sheet().error_mark, Some(7));
        app.handle_key(KeyEvent::from(KeyCode::Char('y')));
        assert_eq!(app.sheet().error_mark, None);
    }

    #[test]
    fn a_session_that_cannot_open_leaves_the_statement_waiting_on_it_to_its_own_answer() {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        app.apply(to_shop(Incoming::NotOpened("connection refused".into())));
        assert_eq!(app.sheet().session, SessionState::Failed("connection refused".into()));
        assert_eq!(app.sheet().run, Run::Running { run: RunId::FIRST });
    }

    #[test]
    fn a_session_that_cannot_open_leaves_an_idle_editor_idle() {
        let mut app = app();
        app.apply(to_shop(Incoming::NotOpened("connection refused".into())));
        assert_eq!(app.sheet().run, Run::Idle);
    }

    #[test]
    fn alt_enter_breaks_the_line_and_runs_nothing() {
        let mut app = typed("SELECT *");
        assert_eq!(app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)), []);
        assert_eq!(app.sheet().editor.text(), "SELECT *\n");
    }

    #[test]
    fn a_question_mark_in_the_editor_is_typed() {
        let app = typed("SELECT '?'");
        assert_eq!((app.sheet().editor.text(), &app.overlay), ("SELECT '?'", &None));
    }

    #[test]
    fn a_question_mark_on_the_result_opens_the_help_and_any_other_key_closes_it() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.overlay, Some(Overlay::Help(Help::default())));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, None);
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(grid.row, 0);
    }

    #[test]
    fn a_second_question_mark_lists_every_key() {
        let mut app = typed("SELECT 1");
        press(&mut app, KeyCode::F(1));
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.overlay, Some(Overlay::Help(Help { scroll: 0, every_key: true })));
    }

    #[test]
    fn each_run_gets_a_new_id() {
        let mut app = with_result();
        let Action::Run { run, .. } = enter(&mut app).remove(0) else { panic!("no run") };
        assert_eq!(run, RunId::FIRST.next());
    }

    #[test]
    fn an_answer_from_the_session_of_another_connection_is_dropped() {
        let mut app = typed("SELECT 1");
        enter(&mut app);
        let other =
            Answer::Session { connection: "prod".parse().unwrap(), incoming: Incoming::Refused { run: RunId::FIRST, why: "no".into() } };
        app.apply(other);
        assert_eq!(app.sheet().run, Run::Running { run: RunId::FIRST });
    }

    #[test]
    fn a_late_answer_to_an_earlier_run_is_dropped() {
        let mut app = with_result();
        enter(&mut app);
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, error: "ERROR: canceling statement".into(), position: None }));
        assert_eq!(app.sheet().run, Run::Running { run: RunId::FIRST.next() });
    }

    #[test]
    fn ctrl_c_while_a_statement_runs_cancels_it_and_quits() {
        let mut app = typed("SELECT pg_sleep(30)");
        enter(&mut app);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            [Action::Cancel { connection: shop(), run: RunId::FIRST }]
        );
        assert!(app.should_quit);
    }

    #[test]
    fn a_lost_session_says_so_in_the_status_line() {
        let mut app = app();
        app.apply(to_shop(Incoming::Opened));
        app.apply(to_shop(Incoming::Closed("terminating connection".into())));
        assert_eq!(app.sheet().session, SessionState::Failed(format!("{LOST_SESSION}: terminating connection")));
    }

    #[test]
    fn an_edit_while_the_statement_runs_keeps_its_error_unmarked() {
        let mut app = typed("SELECT x");
        enter(&mut app);
        app.handle_key(KeyEvent::from(KeyCode::Home));
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        failed_at(&mut app, 8);
        assert_eq!(app.sheet().error_mark, None);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
        assert_eq!(app.sheet().editor.text(), "SELECT 1");
    }
}
