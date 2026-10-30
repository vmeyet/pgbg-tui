//! The TUI's pure state: it takes keys and answers, and returns the actions to run.
//! It reads no clock and does no I/O.
use super::theme::Theme;
use crate::act::Pane;
use crate::config::AiSettings;
use crate::connection::{Connection, ConnectionName};
use crate::failure::Failure;
use crate::format::Mode;
use ask::Asks;
use ratatui::layout::Rect;
use vtui::click::Clicks;
use vtui::field::Field;
use vtui::help::Help;

pub(super) use ask::{AskStep, Suggestion};
pub(super) use colouring::Colouring;
pub(super) use column_stats::ColumnStats;
pub(super) use completion::Completions;
pub(super) use confirm::{Confirmed, WriteToSend};
pub(super) use detail::{Count, Counted, Detail, DetailFit, DetailScroll, DrawnDetail, DrawnRow, RowOf, ShownRow};
pub(super) use diff::Change;
pub(super) use form::{Form, PasswordFrom, Slot};
pub(super) use grid::{Clock, GAP, Grid, Reading, Room};
pub(super) use history::Recall;
pub(super) use panes::Panes;
pub(super) use picker::{Note, Picker};
pub(super) use protocol::{
    Action, Answer, AskFailure, AskId, ConfigAnswer, ConfigChangeId, Incoming, KeychainPassword, LookId, NoIdleGhost, QueryAnswer, RunId,
    SaveConnection, Saving, SessionId, TunnelState,
};
pub(super) use queries::{Finder, Queries};
pub(super) use save::{Opening, Prompt};
pub(super) use schema_tab::{SchemaRow, SchemaTab, SidebarTab};
pub(super) use search::Search;
pub(super) use sheet::{Run, SchemaState, SessionState, Sheet, View};
pub(super) use shown_rows::ShownRows;
pub(super) use table_finder::TableFinder;
pub(super) use versions::{Listed, VersionList};

pub(super) mod ask;
mod colouring;
mod column_stats;
mod completion;
mod condition;
mod confirm;
mod copy;
mod detail;
mod diff;
mod editor;
#[cfg(test)]
pub(super) mod fixtures;
pub(super) mod form;
mod ghost;
mod grid;
pub(super) mod history;
pub(super) mod idle_ghost;
mod keys;
mod mouse;
mod panes;
mod param_bar;
pub(super) mod picker;
mod protocol;
pub(super) mod queries;
mod save;
mod schema_tab;
mod search;
mod sheet;
mod shown_rows;
mod table_finder;
mod undo;
mod usage;
pub(super) mod versions;

/// The pane that has the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Focus {
    #[default]
    Editor,
    /// The parameter bar above the editor, drawn while the statement under the cursor has a `{{name}}`.
    Params,
    Result,
    Queries,
}

impl Focus {
    /// The order `tab` goes through the panes.
    const ORDER: [Self; 4] = [Self::Editor, Self::Params, Self::Result, Self::Queries];

    /// The pane of the action table whose keys work here.
    pub(super) fn pane(self) -> Pane {
        match self {
            Self::Editor => Pane::Editor,
            Self::Params => Pane::Params,
            Self::Result => Pane::Result,
            Self::Queries => Pane::Queries,
        }
    }
}

/// What the frame just drawn had room for.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Fit {
    /// The rows and columns the grid had, which also set the size of a `PgDn` page.
    pub(super) grid: Room,
    /// Where each pane was drawn; a terminal narrower than 80 columns leaves the sidebar out.
    pub(super) panes: Panes,
    /// Where the editor drew its text, inside its frame, so a click finds the character it lands on.
    pub(super) editor_text: Rect,
    /// Where the row's detail started and how far it scrolls.
    pub(super) detail: DetailFit,
}

/// What lies over the panes and takes every key while it is open.
#[derive(Debug, PartialEq)]
pub(super) enum Overlay {
    /// The `?` list of keys.
    Help(Help),
    /// The `ctrl-p` finder of saved queries.
    Finder(Finder),
    /// The `ctrl-o` connection picker.
    Picker(Picker),
    /// The form that adds or edits a connection, from the picker.
    Form(Box<Form>),
    /// The `ctrl-r` search over the connection's history.
    Recall(Recall),
    /// The `ctrl-v` list of the versions of the query on screen.
    Versions(VersionList),
    /// A question on the footer line: the path of a new query, or whether to open one over unsaved text.
    Prompt(Prompt),
    /// The row under the grid's cursor, one column per line.
    Detail(Detail),
    /// The figures of the grid's column over the rows loaded.
    ColumnStats(Box<ColumnStats>),
    /// A question on the footer line: which format the whole result is copied as.
    CopyAs,
    /// The schema tab's filter line, typed into from `/` until `⏎` or `esc`.
    SchemaFilter,
    /// The grid's filter on the footer line, typed into from `/`: each key filters the rows again.
    RowFilter(Field),
    /// The `ctrl-space` list of names that complete the word before the cursor.
    Completion(Completions),
    /// The `ctrl-t` finder of the schema's tables.
    TableFinder(Box<TableFinder>),
    /// The `ctrl-g` instruction on the footer line, then the ask waiting on the `[ai] command`, then its answer.
    Ask(AskStep),
}

/// The whole TUI state; `handle_key`, `apply` and `fit` are the only ways it changes.
#[derive(Debug)]
pub(super) struct App {
    theme: Theme,
    /// Every connection of the config, by name, for the picker.
    connections: Vec<Connection>,
    /// A sheet for each connection opened since pgbg started, in the order they were opened.
    sheets: Vec<Sheet>,
    /// The index in `sheets` of the one on screen.
    shown: usize,
    focus: Focus,
    overlay: Option<Overlay>,
    queries: Queries,
    sidebar: SidebarTab,
    schema_tab: SchemaTab,
    fit: Fit,
    /// The row's detail as a frame last built it.
    drawn_detail: Option<DrawnDetail>,
    should_quit: bool,
    /// The id the next change to the config takes.
    next_config_change: ConfigChangeId,
    /// The id the next lookup takes.
    next_look: LookId,
    /// The id the next session opened takes.
    next_session: SessionId,
    /// Why a change to the config failed when no form waits on it; the status line says it until the next key.
    config_failure: Option<String>,
    /// What pgbg warned of as it started, as a file left where it no longer reads it; the status line says it until the
    /// next key.
    warning: Option<String>,
    /// What the last key did when it shows nowhere else, as a copy's outcome; the status line says it until the next key.
    note: Option<String>,
    /// The format mode new results open in, and the clock the loop last read.
    reading: Reading,
    asks: Asks,
    /// The last press of the mouse, so the next one can make a double or triple click.
    clicks: Clicks,
}

impl App {
    /// The app showing the sheet of `first`, whose session is still opening, with `connections` in the picker.
    pub(super) fn new(connections: Vec<Connection>, first: &Connection, theme: Theme) -> Self {
        Self {
            theme,
            connections,
            sheets: vec![Sheet::new(first, SessionId::FIRST)],
            shown: 0,
            focus: Focus::Editor,
            overlay: None,
            queries: Queries::default(),
            sidebar: SidebarTab::Queries,
            schema_tab: SchemaTab::default(),
            fit: Fit::default(),
            drawn_detail: None,
            should_quit: false,
            next_config_change: ConfigChangeId::FIRST,
            next_session: SessionId::FIRST.next(),
            next_look: LookId::FIRST,
            config_failure: None,
            warning: None,
            note: None,
            reading: Reading { mode: Mode::Raw, clock: grid::clock_at_epoch() },
            asks: Asks::new(None),
            clicks: Clicks::default(),
        }
    }

    /// The app with each new result opening in `mode`.
    pub(super) fn with_format_mode(self, mode: Mode) -> Self {
        Self { reading: Reading { mode, ..self.reading }, ..self }
    }

    /// The app saying `warning` on the status line until the first key.
    pub(super) fn with_warning(self, warning: Option<String>) -> Self {
        Self { warning, ..self }
    }

    /// The app asking the `[ai] command` of `settings`; none leaves asking off.
    pub(super) fn with_ai(self, settings: Option<AiSettings>) -> Self {
        Self { asks: Asks::new(settings), ..self }
    }

    /// Takes the clock the loop read for the next frame.
    pub(super) fn set_clock(&mut self, clock: Clock) {
        self.reading.clock = clock;
    }

    pub(super) fn theme(&self) -> &Theme {
        &self.theme
    }

    pub(super) fn connections(&self) -> &[Connection] {
        &self.connections
    }

    pub(super) fn focus(&self) -> Focus {
        self.focus
    }

    pub(super) fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    pub(super) fn queries(&self) -> &Queries {
        &self.queries
    }

    pub(super) fn sidebar(&self) -> SidebarTab {
        self.sidebar
    }

    pub(super) fn schema_tab(&self) -> &SchemaTab {
        &self.schema_tab
    }

    pub(super) fn drawn_detail(&self) -> Option<&DrawnDetail> {
        self.drawn_detail.as_ref()
    }

    pub(super) fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub(super) fn config_failure(&self) -> Option<&str> {
        self.config_failure.as_deref()
    }

    pub(super) fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// What pgbg warned of as it started, until the first key.
    pub(super) fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub(super) fn clock(&self) -> &Clock {
        &self.reading.clock
    }

    /// The format mode new results open in.
    pub(super) fn format_mode(&self) -> Mode {
        self.reading.mode
    }

    /// The sheet on screen.
    pub(super) fn sheet(&self) -> &Sheet {
        &self.sheets[self.shown]
    }

    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheets[self.shown]
    }

    /// What the runner should do as the TUI starts: open the first sheet's session, list its queries.
    pub(super) fn start(&self) -> Vec<Action> {
        let first = &self.sheets[0];
        vec![Action::Open { connection: first.connection.clone(), session: first.session_id }, self.list_queries()]
    }

    /// Whether the editor on screen changed since its colours were built, so it needs `recolour`.
    pub(super) fn has_stale_colours(&self) -> bool {
        self.sheet().editor.has_stale_colours()
    }

    /// Whether the loop waits for the editor to sit idle: to end the undo step of the characters typed, or to ask the
    /// idle ghost.
    pub(super) fn waits_for_idle(&self) -> bool {
        self.sheet().editor.is_typing() || self.wants_idle_ghost()
    }

    /// The editor sat idle: the characters typed so far make one undo step, and the idle ghost asks when it would.
    pub(super) fn sat_idle(&mut self) -> Vec<Action> {
        self.sheet_mut().editor.end_typing();
        self.ask_idle_ghost()
    }

    /// Rebuilds the colours of the editor on screen, once its text stopped changing for a moment.
    pub(super) fn recolour(&mut self) {
        self.sheet_mut().editor.recolour();
    }

    /// Tells the app what the frame just drawn had room for: the grid's cursor stays among its rows, and the keys
    /// leave a sidebar no longer drawn.
    pub(super) fn fit(&mut self, mut fit: Fit) {
        let built = fit.detail.drawn.take();
        let (room, editor_rows) = (fit.grid, fit.editor_text.height);
        self.fit = fit;
        self.sheet_mut().editor.keep_in_view(editor_rows);
        if let Run::Done { grid, view: View::Grid, .. } = &mut self.sheet_mut().run {
            grid.keep_in_view(room);
        }
        if !self.is_drawn(self.focus) {
            self.focus = Focus::Editor;
        }
        if let Some(Overlay::Detail(detail)) = &mut self.overlay {
            detail.scroll = DetailScroll::Lines(self.fit.detail.first_line);
        }
        self.keep_drawn_detail(built);
    }

    /// A new id for a session about to open, so answers from the session before it are dropped.
    fn new_session(&mut self) -> SessionId {
        let id = self.next_session;
        self.next_session = id.next();
        id
    }

    /// A new id for a change to the config, so its answer reaches only what asked for it.
    fn new_config_change(&mut self) -> ConfigChangeId {
        let id = self.next_config_change;
        self.next_config_change = id.next();
        id
    }

    /// Quits, after cancelling every running statement so no server keeps running it.
    pub(super) fn quit(&mut self) -> Vec<Action> {
        self.should_quit = true;
        self.sheets.iter().filter_map(Sheet::cancel_of_running).collect()
    }

    /// Applies one answer: a session's or a query read to the sheet of its connection, the list of queries to the app;
    /// returns what the runner should do about it.
    pub(super) fn apply(&mut self, answer: Answer) -> Vec<Action> {
        let actions = self.take_answer(answer);
        self.ask_for_hidden_write();
        self.fill_table_finder();
        self.and_the_schema_shown(actions)
    }

    /// Takes what the session `session` of `connection` said: the count of the write waiting for its `y` to that
    /// question, whatever the session; a lookup's rows to the detail its `LookId` names, as a failure once the sheet
    /// left the session, so a lookup always ends; the rest to that sheet while the session is still its own.
    fn take_session_answer(&mut self, connection: &ConnectionName, session: SessionId, incoming: Incoming) -> Vec<Action> {
        if let Incoming::LookedUp { look, rows } = &incoming
            && self.is_counting(*look)
        {
            self.rows_counted(rows);
            return vec![];
        }
        let is_its_session = self.sheet_of(connection).is_some_and(|sheet| sheet.session_id == session);
        if let Incoming::LookedUp { look, rows } = incoming {
            let rows = if is_its_session { rows } else { Err(Failure::StoppedWithItsSession) };
            return self.looked_up(look, rows);
        }
        let reading = self.reading;
        if let Some(sheet) = self.sheet_of_mut(connection).filter(|_| is_its_session) {
            sheet.apply(incoming, &reading);
        }
        vec![]
    }

    fn take_answer(&mut self, answer: Answer) -> Vec<Action> {
        match answer {
            Answer::Session { connection, session, incoming } => return self.take_session_answer(&connection, session, incoming),
            Answer::Queries(QueryAnswer::All { folder, queries }) => self.listed(&folder, queries),
            Answer::Queries(QueryAnswer::Opened { connection, query, text }) => self.opened(&connection, query, &text),
            Answer::Queries(QueryAnswer::NotOpened { connection, query, error }) => {
                if let Some(sheet) = self.sheet_of_mut(&connection) {
                    sheet.query_error = Some(format!("could not open {}: {error}", query.path));
                }
            }
            Answer::Queries(QueryAnswer::Saved { connection, query, text }) => {
                self.saved(&connection, query, &text);
                return vec![self.list_queries()];
            }
            Answer::Queries(QueryAnswer::NotSaved { connection, path, error }) => self.not_saved(&connection, &path, &error),
            Answer::Config { id, answer: ConfigAnswer::Changed { connections, warning } } => {
                return self.take_connections(id, connections, warning);
            }
            Answer::Config { id, answer: ConfigAnswer::Failed { why, password } } => self.config_failed(id, why, password),
            Answer::History { connection, entries } => self.history_read(&connection, entries),
            Answer::Versions { query, versions } => self.versions_read(&query, versions),
            Answer::Copied(note) => self.copied(note),
            Answer::Exported { text, done } => return vec![Action::Copy { text, done }],
            Answer::Pasted(Some(text)) => return self.paste(&text),
            Answer::Pasted(None) => self.note = Some("nothing pasted: the clipboard could not be read".to_owned()),
            Answer::Asked { ask, answer } => return self.asked(ask, answer),
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{app, enter, first_run, press, shop, to_shop, typed, view, with_result};
    use super::sheet::LOST_SESSION;
    use super::*;
    use crate::failure::{Failure, Lost};
    use crate::fixtures::orders;
    use crossterm::event::{KeyCode, KeyEvent};
    use std::time::Duration;

    #[test]
    fn enter_runs_the_text_of_the_editor() {
        let mut app = typed("SELECT 1");
        assert_eq!(enter(&mut app), [first_run(&shop(), "SELECT 1")]);
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
        assert_eq!(enter(&mut app), [first_run(&shop(), "SELECT 2")]);
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
            Run::Done {
                run: RunId::FIRST,
                grid: Box::new(Grid::raw(&orders())),
                result: orders(),
                took: Duration::from_millis(38),
                view: View::Grid,
                is_page_asked: false
            }
        );
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
    fn a_refused_statement_says_it_was_not_sent() {
        let mut app = typed("DELETE FROM orders");
        enter(&mut app);
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, failure: Failure::Refused("`DELETE` is not a read".into()) }));
        assert_eq!(app.sheet().run, Run::Failed("not sent: `DELETE` is not a read".into()));
    }

    fn failed_at(app: &mut App, position: usize) {
        app.apply(to_shop(Incoming::Failed {
            run: RunId::FIRST,
            failure: Failure::Server { message: "ERROR: syntax error".into(), position: Some(position) },
        }));
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
    fn the_mark_counts_in_the_text_the_server_got_and_lands_in_the_template() {
        let mut app = typed("SELECT {{a}}, nope");
        enter(&mut app);
        failed_at(&mut app, 12);
        assert_eq!(app.sheet().error_mark.map(|at| &app.sheet().editor.text()[at..]), Some("nope"));
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
        app.apply(to_shop(Incoming::NotOpened(Failure::Login("connection refused".into()))));
        assert_eq!(app.sheet().session, SessionState::Failed("connection refused".into()));
        assert_eq!(app.sheet().run, Run::Running { run: RunId::FIRST });
    }

    #[test]
    fn a_session_that_cannot_open_leaves_an_idle_editor_idle() {
        let mut app = app();
        app.apply(to_shop(Incoming::NotOpened(Failure::Login("connection refused".into()))));
        assert_eq!(app.sheet().run, Run::Idle);
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
        let other = Answer::Session {
            connection: "prod".parse().unwrap(),
            session: SessionId::FIRST,
            incoming: Incoming::Failed { run: RunId::FIRST, failure: Failure::Refused("no".into()) },
        };
        app.apply(other);
        assert_eq!(app.sheet().run, Run::Running { run: RunId::FIRST });
    }

    #[test]
    fn a_late_answer_to_an_earlier_run_is_dropped() {
        let mut app = with_result();
        enter(&mut app);
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, failure: Failure::Link("ERROR: canceling statement".into()) }));
        assert_eq!(app.sheet().run, Run::Running { run: RunId::FIRST.next() });
    }

    #[test]
    fn a_lost_session_says_so_in_the_status_line() {
        let mut app = app();
        app.apply(to_shop(Incoming::Opened));
        app.apply(to_shop(Incoming::Closed(Lost::LinkFailed("terminating connection".into()))));
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
}
