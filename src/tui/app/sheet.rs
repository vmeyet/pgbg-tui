//! One connection's sheet: its editor, the statement it ran and what came of it, and where its session stands.
use super::editor::Editor;
use super::grid::{Clock, Move, Reading, Room};
use super::{Action, Grid, Incoming, RunId, SessionId, TunnelState};
use crate::chart::{self, ChartError, Series};
use crate::connection::{Connection, ConnectionName, SchemaReading};
use crate::db::guard;
use crate::db::model::{Catalog, Extent, Page, ResultSet, Statement};
use crate::format::{self, Format};
use crate::query::{Header, Query};
use std::sync::Arc;
use std::time::Duration;

pub(in crate::tui) const LOST_SESSION: &str = "session lost, ⏎ opens a new one";
const CLOSED_WHILE_RUNNING: &str = "cancelled: its session was closed";
const REOPENED_WHILE_RUNNING: &str = "cancelled: its session was reopened";
/// The grid asks for the next page once its cursor is this close to the last row it has, so the page comes before the
/// cursor gets there.
const NEAR_END_ROWS: usize = 100;

/// The statement last sent, and the byte of the editor it starts at until the text changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    start: Option<usize>,
    statement: Statement,
}

/// Where the session stands, as the status line says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum SessionState {
    Opening,
    Open,
    Failed(String),
    /// Closed from the picker; the sheet stays, and picking it again opens a new session.
    Closed,
}

/// The last statement run and what came of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::tui) enum Run {
    #[default]
    Idle,
    Running {
        run: RunId,
    },
    /// `Esc` asked the server to cancel the running statement; its answer has not come yet.
    Cancelling {
        run: RunId,
    },
    Done {
        run: RunId,
        result: ResultSet,
        took: Duration,
        /// Boxed, so the other states of a run stay small.
        grid: Box<Grid>,
        view: View,
        /// The next page was asked for and has not come yet.
        is_page_asked: bool,
    },
    Failed(String),
}

/// Where the sheet's schema stands: read once first needed, read again on demand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) enum SchemaState {
    #[default]
    NotRead,
    Reading,
    /// Shared with whatever builds on it, and never changed: a new read replaces it whole.
    Read(Arc<Catalog>),
    /// The read failed, in its words; the session stays as it was.
    Failed(String),
}

/// How the result pane shows a result; `v` switches, and a new result starts on the grid, or on the chart its header asks
/// for when that chart fits the result.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum View {
    Grid,
    /// The chart of the header's `-- chart:`, or the one `chart::infer` picks when `v` is pressed, or why `v` has none.
    Chart(Result<Series, ChartError>),
}

/// One connection in the TUI: its editor text, its last run and its session's state, kept while another one is shown.
#[derive(Debug)]
pub(in crate::tui) struct Sheet {
    /// The connection as the config said when its session last opened.
    pub(in crate::tui) connection: Connection,
    pub(in crate::tui) editor: Editor,
    pub(in crate::tui) session: SessionState,
    /// The session whose words the sheet takes; another one's are dropped.
    pub(in crate::tui) session_id: SessionId,
    /// The connection's tunnel, once it has one and its task said where it stands.
    pub(in crate::tui) tunnel: Option<TunnelState>,
    pub(in crate::tui) run: Run,
    sent: Option<Sent>,
    last_run: Option<RunId>,
    /// The editor's byte the server's last error points at, until the text changes.
    pub(in crate::tui) error_mark: Option<usize>,
    /// The saved query the editor stands for, once one was opened in it.
    pub(in crate::tui) opened: Option<Query>,
    /// Why the last query asked for could not be opened, until one opens.
    pub(in crate::tui) query_error: Option<String>,
    /// The header of the editor's text: its `-- chart:` draws each result of the sheet, its `-- format:` reads it.
    pub(in crate::tui) header: Header,
    /// Whether the editor's text changed since it was opened or saved.
    pub(in crate::tui) is_dirty: bool,
    /// Why the last page asked for failed, until the next run.
    pub(in crate::tui) page_failure: Option<String>,
    pub(in crate::tui) schema: SchemaState,
}

impl Sheet {
    pub(super) fn new(connection: &Connection, session_id: SessionId) -> Self {
        Self {
            connection: connection.clone(),
            editor: Editor::default(),
            session: SessionState::Opening,
            session_id,
            tunnel: None,
            run: Run::Idle,
            sent: None,
            last_run: None,
            error_mark: None,
            opened: None,
            query_error: None,
            header: Header::default(),
            is_dirty: false,
            page_failure: None,
            schema: SchemaState::NotRead,
        }
    }

    /// Shows the result as a chart, or as the grid again.
    pub(super) fn switch_view(&mut self) {
        if let Run::Done { result, view, .. } = &mut self.run {
            *view = match view {
                View::Grid => View::Chart(chart::infer_series(result)),
                View::Chart(_) => View::Grid,
            };
        }
    }

    pub(in crate::tui) fn name(&self) -> &ConnectionName {
        self.connection.name()
    }

    /// Takes `connection` as it is now and waits for its new session `session_id`; the editor and the last run stay,
    /// but a statement running on the old session fails with it.
    pub(super) fn reopen(&mut self, connection: Connection, session_id: SessionId) {
        self.connection = connection;
        self.session_id = session_id;
        self.session = SessionState::Opening;
        self.tunnel = None;
        self.stop_paging();
        self.fail_running(REOPENED_WHILE_RUNNING);
        self.schema = SchemaState::NotRead;
    }

    /// Takes `connection` as it is now and leaves the session closed: picking the sheet opens it as it is then.
    pub(super) fn take_closed(&mut self, connection: Connection) {
        self.connection = connection;
    }

    /// Forgets the session, which the backend closes; a statement running on it fails, the editor stays.
    pub(super) fn close(&mut self) {
        self.session = SessionState::Closed;
        self.tunnel = None;
        self.stop_paging();
        self.fail_running(CLOSED_WHILE_RUNNING);
        if self.schema == SchemaState::Reading {
            self.schema = SchemaState::NotRead;
        }
    }

    /// Asks for the schema the first time something needs it; nothing once asked.
    #[cfg_attr(not(test), expect(dead_code, reason = "the schema sidebar asks for it, the next step"))]
    pub(super) fn want_schema(&mut self) -> Vec<Action> {
        if self.schema != SchemaState::NotRead {
            return vec![];
        }
        self.read_schema_again()
    }

    /// Asks for the schema again, unless a read is on its way, the session is closed, or the connection says
    /// `schema = "off"`; a failed session opens again for it.
    pub(super) fn read_schema_again(&mut self) -> Vec<Action> {
        let is_off = self.connection.schema() == SchemaReading::Off;
        let is_closed = self.session == SessionState::Closed;
        if is_off || is_closed || self.schema == SchemaState::Reading {
            return vec![];
        }
        self.schema = SchemaState::Reading;
        if matches!(self.session, SessionState::Failed(_)) {
            self.session = SessionState::Opening;
        }
        vec![Action::ReadSchema { connection: self.name().clone(), session: self.session_id }]
    }

    /// Fails a read waiting on a session that did not open or was lost, so it can be asked again.
    fn fail_schema_read(&mut self, why: &str) {
        if self.schema == SchemaState::Reading {
            self.schema = SchemaState::Failed(why.to_owned());
        }
    }

    /// Fails the running statement in `why`: the session it ran on is gone, and its answer with it.
    fn fail_running(&mut self, why: &str) {
        if self.is_running() {
            self.run = Run::Failed(why.to_owned());
        }
    }

    /// Cuts the result at the rows it has, since the portal its pages came from closes with the session.
    fn stop_paging(&mut self) {
        if let Run::Done { result, is_page_asked, .. } = &mut self.run {
            result.cut();
            *is_page_asked = false;
        }
    }

    /// Whether the result pane shows a chart, and so not the grid the cursor keys move.
    pub(super) fn is_chart_shown(&self) -> bool {
        matches!(self.run, Run::Done { view: View::Chart(_), .. })
    }

    pub(super) fn cancel(&mut self) -> Vec<Action> {
        let Run::Running { run } = self.run else { return vec![] };
        self.run = Run::Cancelling { run };
        vec![self.cancel_of(run)]
    }

    pub(super) fn cancel_of_running(&self) -> Option<Action> {
        self.running_run().map(|run| self.cancel_of(run))
    }

    fn cancel_of(&self, run: RunId) -> Action {
        Action::Cancel { connection: self.name().clone(), run }
    }

    fn running_run(&self) -> Option<RunId> {
        match self.run {
            Run::Running { run } | Run::Cancelling { run } => Some(run),
            _ => None,
        }
    }

    pub(in crate::tui) fn is_running(&self) -> bool {
        self.running_run().is_some()
    }

    fn is_answer_to_running(&self, run: RunId) -> bool {
        self.running_run() == Some(run)
    }

    /// Runs the statement under the cursor, the editor's other statements left alone.
    pub(super) fn run_editor(&mut self) -> Vec<Action> {
        if self.is_running() {
            return vec![];
        }
        let (before, _, _) = self.editor.split();
        let Some(range) = guard::statement_at(self.editor.text(), before.len()) else { return vec![] };
        let statement = Statement::new(&self.editor.text()[range.clone()]);
        let run = self.last_run.map_or(RunId::FIRST, RunId::next);
        self.run = Run::Running { run };
        self.last_run = Some(run);
        self.error_mark = None;
        self.page_failure = None;
        self.sent = Some(Sent { start: Some(range.start), statement: statement.clone() });
        vec![Action::Run { connection: self.name().clone(), run, statement }]
    }

    /// The editor's byte at character `position` of the statement sent; the end of it when the server
    /// points just past its last character.
    fn editor_byte(&self, position: usize) -> Option<usize> {
        let sent = self.sent.as_ref()?;
        let text = sent.statement.as_str();
        let offset = text.char_indices().map(|(at, _)| at).chain(std::iter::once(text.len())).nth(position.checked_sub(1)?)?;
        Some(sent.start? + offset)
    }

    /// Takes what the session said; a result opens in the formats its header and `reading` give it.
    pub(super) fn apply(&mut self, incoming: Incoming, reading: &Reading) {
        let is_closed = self.session == SessionState::Closed;
        match incoming {
            Incoming::Opened | Incoming::NotOpened(_) | Incoming::Closed(_) | Incoming::Tunnel(_) | Incoming::Schema(_) if is_closed => {}
            Incoming::Opened => self.session = SessionState::Open,
            Incoming::NotOpened(err) => {
                self.fail_schema_read(&err);
                self.session = SessionState::Failed(err);
            }
            Incoming::Closed(why) => {
                self.fail_schema_read(&why);
                self.session = SessionState::Failed(format!("{LOST_SESSION}: {why}"));
            }
            Incoming::Tunnel(state) => self.tunnel = Some(state),
            Incoming::Schema(read) => self.schema = read.map_or_else(SchemaState::Failed, SchemaState::Read),
            Incoming::Result { run, .. } | Incoming::Refused { run, .. } | Incoming::Failed { run, .. }
                if !self.is_answer_to_running(run) => {}
            Incoming::Result { run, result, took } => {
                let header_chart = self.header.chart.as_ref().and_then(|spec| Series::from_result(&result, spec).ok());
                let view = header_chart.map_or(View::Grid, |series| View::Chart(Ok(series)));
                let formats = format::opening(result.columns(), &self.header.formats, reading.mode);
                let grid = Box::new(Grid::new(&result, formats, &reading.clock));
                self.run = Run::Done { run, grid, result, took, view, is_page_asked: false };
            }
            Incoming::Page { run, .. } | Incoming::PageFailed { run, .. } if !self.is_waiting_on_page(run) => {}
            Incoming::Page { page, .. } => self.push_page(page),
            Incoming::PageFailed { error, .. } => {
                self.push_page(Page { rows: vec![], extent: Extent::Cut });
                self.page_failure = Some(format!("the next page failed: {error}"));
            }
            Incoming::Refused { why, .. } => self.run = Run::Failed(format!("not sent: {why}")),
            Incoming::Failed { error, position, .. } => {
                self.error_mark = position.and_then(|position| self.editor_byte(position));
                self.run = Run::Failed(error);
            }
        }
    }

    /// Moves the grid's cursor by `step` in the `room` it has, and asks for the next page once it nears the last row.
    pub(super) fn step_grid(&mut self, step: Move, room: Room) -> Vec<Action> {
        let Run::Done { run, result, grid, is_page_asked, .. } = &mut self.run else { return vec![] };
        grid.step(step, result, room);
        let is_near_end = result.rows().len().saturating_sub(grid.row) <= NEAR_END_ROWS;
        let has_more = result.extent() == Extent::More;
        if !has_more || !is_near_end || *is_page_asked {
            return vec![];
        }
        *is_page_asked = true;
        vec![Action::NextPage { connection: self.connection.name().clone(), session: self.session_id, run: *run }]
    }

    /// Puts the column under the grid's cursor in its next format.
    pub(super) fn step_format(&mut self, clock: &Clock) {
        if let Run::Done { result, grid, .. } = &mut self.run {
            grid.step_format(result, clock);
        }
    }

    /// Puts every column of the grid raw, or readable when all are raw.
    pub(super) fn switch_every_format(&mut self, clock: &Clock) {
        if let Run::Done { result, grid, .. } = &mut self.run {
            grid.switch_every_format(result, clock);
        }
    }

    /// Whether the grid shows a column as the time from now, which reads differently as the clock moves.
    pub(in crate::tui) fn shows_relative_time(&self) -> bool {
        matches!(&self.run, Run::Done { grid, view: View::Grid, .. } if grid.formats.contains(&Format::Relative))
    }

    /// Whether the result of `run` waits on its next page.
    fn is_waiting_on_page(&self, of: RunId) -> bool {
        matches!(&self.run, Run::Done { run, result, .. } if *run == of && result.extent() == Extent::More)
    }

    /// Adds `page` to the result, the grid's widths kept from the first page.
    fn push_page(&mut self, page: Page) {
        if let Run::Done { result, is_page_asked, .. } = &mut self.run {
            result.push(page);
            *is_page_asked = false;
        }
    }

    /// Forgets where the statement sent stands in the editor, once the text it was cut from changed.
    pub(super) fn forget_sent_place(&mut self) {
        if let Some(sent) = &mut self.sent {
            sent.start = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::App;
    use super::super::tests::{app, app_on, on_the_result, shop, to_shop};
    use super::*;
    use crate::db::model::{ColumnKind, Value};
    use crate::fixtures::{self, column, orders, page, shop_schema};
    use crate::format::Mode;
    use crossterm::event::{KeyCode, KeyEvent};
    use std::time::Duration;

    fn first_page() -> App {
        on_the_result(ResultSet::new(vec![column("n", ColumnKind::Int)], page(500, Extent::More)))
    }

    fn press(app: &mut App, c: char) -> Vec<Action> {
        app.handle_key(KeyEvent::from(KeyCode::Char(c)))
    }

    fn rows(app: &App) -> usize {
        let Run::Done { result, .. } = &app.sheet().run else { panic!("no result") };
        result.rows().len()
    }

    fn next_page() -> Vec<Action> {
        vec![Action::NextPage { connection: shop(), session: SessionId::FIRST, run: RunId::FIRST }]
    }

    #[test]
    fn the_grid_near_its_last_row_asks_for_the_next_page() {
        assert_eq!(press(&mut first_page(), 'G'), next_page());
    }

    #[test]
    fn the_grid_far_from_its_last_row_asks_for_nothing() {
        assert_eq!(press(&mut first_page(), 'j'), []);
    }

    #[test]
    fn the_next_page_is_asked_once_until_it_comes() {
        let mut app = first_page();
        press(&mut app, 'G');
        assert_eq!(press(&mut app, 'k'), []);
    }

    #[test]
    fn a_page_adds_its_rows_and_the_grid_asks_again_near_the_new_end() {
        let mut app = first_page();
        press(&mut app, 'G');
        app.apply(to_shop(Incoming::Page { run: RunId::FIRST, page: page(500, Extent::More) }));
        assert_eq!((rows(&app), press(&mut app, 'G')), (1000, next_page()));
    }

    #[test]
    fn a_page_of_another_run_is_dropped() {
        let mut app = first_page();
        app.apply(to_shop(Incoming::Page { run: RunId::FIRST.next(), page: page(500, Extent::More) }));
        assert_eq!(rows(&app), 500);
    }

    #[test]
    fn a_page_keeps_the_column_widths_of_the_first() {
        let mut app = first_page();
        let wide = vec![vec![Value::Text("a value wider than any number".into())]];
        app.apply(to_shop(Incoming::Page { run: RunId::FIRST, page: Page { rows: wide, extent: Extent::All } }));
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(grid.widths, [3]);
    }

    #[test]
    fn a_failed_page_leaves_the_result_cut_and_says_why() {
        let mut app = first_page();
        app.apply(to_shop(Incoming::PageFailed { run: RunId::FIRST, error: "ERROR: division by zero".into() }));
        let Run::Done { result, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(result.extent(), Extent::Cut);
        assert_eq!(app.sheet().page_failure.as_deref(), Some("the next page failed: ERROR: division by zero"));
    }

    fn extent(app: &App) -> Extent {
        let Run::Done { result, .. } = &app.sheet().run else { panic!("no result") };
        result.extent()
    }

    #[test]
    fn closing_a_sheet_waiting_on_pages_cuts_its_result_and_asks_for_no_more() {
        let mut app = first_page();
        press(&mut app, 'G');
        app.sheet_mut().close();
        assert_eq!((extent(&app), press(&mut app, 'G')), (Extent::Cut, vec![]));
    }

    #[test]
    fn opening_a_sheet_again_cuts_the_result_its_old_session_paged() {
        let mut app = first_page();
        let connection = app.sheet().connection.clone();
        app.sheet_mut().reopen(connection, SessionId::FIRST.next());
        assert_eq!(extent(&app), Extent::Cut);
    }

    #[test]
    fn a_late_page_on_a_closed_sheet_is_dropped() {
        let mut app = first_page();
        press(&mut app, 'G');
        app.sheet_mut().close();
        app.apply(to_shop(Incoming::Page { run: RunId::FIRST, page: page(500, Extent::More) }));
        assert_eq!(rows(&app), 500);
    }

    #[test]
    fn a_late_page_failure_on_a_closed_sheet_says_nothing() {
        let mut app = first_page();
        press(&mut app, 'G');
        app.sheet_mut().close();
        app.apply(to_shop(Incoming::PageFailed { run: RunId::FIRST, error: "ERROR: canceling statement".into() }));
        assert_eq!(app.sheet().page_failure, None);
    }

    fn read_schema() -> Vec<Action> {
        vec![Action::ReadSchema { connection: shop(), session: SessionId::FIRST }]
    }

    fn catalog() -> Arc<Catalog> {
        Arc::new(Catalog { schema: shop_schema(), search_path: vec!["public".to_owned()], is_cut: false })
    }

    #[test]
    fn the_schema_is_asked_for_the_first_time_it_is_needed() {
        assert_eq!(app().sheet_mut().want_schema(), read_schema());
    }

    #[test]
    fn the_schema_is_asked_for_once() {
        let mut app = app();
        app.sheet_mut().want_schema();
        assert_eq!(app.sheet_mut().want_schema(), []);
    }

    #[test]
    fn a_connection_whose_schema_is_off_never_asks_for_it() {
        let off = fixtures::connection("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nschema = \"off\"\n");
        assert_eq!(app_on(&off).sheet_mut().want_schema(), []);
    }

    #[test]
    fn the_schema_read_is_kept_on_the_sheet() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Schema(Ok(catalog()))));
        assert_eq!(app.sheet().schema, SchemaState::Read(catalog()));
    }

    #[test]
    fn a_schema_that_could_not_be_read_says_why() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Schema(Err("permission denied for table pg_class".to_owned()))));
        assert_eq!(app.sheet().schema, SchemaState::Failed("permission denied for table pg_class".to_owned()));
    }

    #[test]
    fn a_schema_read_can_be_read_again() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Schema(Ok(catalog()))));
        assert_eq!(app.sheet_mut().read_schema_again(), read_schema());
    }

    #[test]
    fn a_schema_being_read_is_not_asked_for_again() {
        let mut app = app();
        app.sheet_mut().want_schema();
        assert_eq!(app.sheet_mut().read_schema_again(), []);
    }

    #[test]
    fn a_sheet_opened_again_asks_its_new_session_for_the_schema() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Schema(Ok(catalog()))));
        let connection = app.sheet().connection.clone();
        app.sheet_mut().reopen(connection, SessionId::FIRST.next());
        assert_eq!(app.sheet_mut().want_schema(), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST.next() }]);
    }

    #[test]
    fn a_sheet_closed_while_its_schema_is_read_asks_again_once_it_opens() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.sheet_mut().close();
        let connection = app.sheet().connection.clone();
        app.sheet_mut().reopen(connection, SessionId::FIRST.next());
        assert_eq!(app.sheet_mut().want_schema(), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST.next() }]);
    }

    #[test]
    fn a_session_that_does_not_open_fails_the_schema_waiting_on_it() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::NotOpened("password authentication failed for user \"nina\"".to_owned())));
        assert_eq!(app.sheet().schema, SchemaState::Failed("password authentication failed for user \"nina\"".to_owned()));
    }

    #[test]
    fn a_session_lost_while_its_schema_is_read_fails_the_read_so_it_can_be_asked_again() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Closed("terminating connection".to_owned())));
        assert_eq!(app.sheet_mut().read_schema_again(), read_schema());
    }

    #[test]
    fn asking_for_the_schema_on_a_failed_session_opens_it_again() {
        let mut app = app();
        app.apply(to_shop(Incoming::NotOpened("connection refused".to_owned())));
        app.sheet_mut().want_schema();
        assert_eq!(app.sheet().session, SessionState::Opening);
    }

    #[test]
    fn a_closed_sheet_asks_for_no_schema() {
        let mut app = app();
        app.sheet_mut().close();
        assert_eq!(app.sheet_mut().want_schema(), []);
    }

    fn formats(app: &App) -> Vec<Format> {
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        grid.formats.clone()
    }

    #[test]
    fn f_puts_the_column_under_the_cursor_in_its_next_format() {
        let mut app = on_the_result(orders());
        press(&mut app, 'l');
        press(&mut app, 'f');
        assert_eq!(formats(&app), [Format::Raw, Format::Grouped, Format::Raw]);
    }

    #[test]
    fn shift_f_puts_every_column_readable_then_raw_again() {
        let mut app = on_the_result(orders());
        press(&mut app, 'F');
        assert_eq!(formats(&app), [Format::Grouped, Format::Grouped, Format::Raw]);
        press(&mut app, 'F');
        assert_eq!(formats(&app), [Format::Raw; 3]);
    }

    #[test]
    fn a_result_opens_readable_when_the_config_says_so() {
        let mut app = app().with_format_mode(Mode::Readable);
        "SELECT 1".chars().for_each(|c| _ = press(&mut app, c));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        assert_eq!(formats(&app), [Format::Grouped, Format::Grouped, Format::Raw]);
    }

    #[test]
    fn a_grid_showing_a_relative_time_says_so() {
        let result = ResultSet::whole(vec![column("paid_at", ColumnKind::TimestampTz)], vec![]);
        let mut app = on_the_result(result);
        press(&mut app, 'f');
        assert!(!app.sheet().shows_relative_time());
        press(&mut app, 'f');
        assert!(app.sheet().shows_relative_time());
    }

    #[test]
    fn a_whole_result_never_asks_for_a_page() {
        assert_eq!(press(&mut on_the_result(orders()), 'G'), []);
    }
}
