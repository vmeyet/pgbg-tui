//! One connection's sheet: its editor, the statement it ran and what came of it, and where its session stands.
use super::editor::Editor;
use super::grid::{Move, Room};
use super::{Action, Grid, Incoming, RunId, SessionId, TunnelState};
use crate::chart::{self, ChartError, Series};
use crate::connection::{Connection, ConnectionName};
use crate::db::guard;
use crate::db::model::{Extent, Page, ResultSet, Statement};
use crate::query::{Header, Query};
use std::time::Duration;

pub(in crate::tui) const LOST_SESSION: &str = "session lost, ⏎ opens a new one";
const CLOSED_WHILE_RUNNING: &str = "cancelled: its session was closed";
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
    /// The header of the editor's text: its `-- chart:` draws each result of the sheet.
    pub(in crate::tui) header: Header,
    /// Whether the editor's text changed since it was opened or saved.
    pub(in crate::tui) is_dirty: bool,
    /// Why the last page asked for failed, until the next run.
    pub(in crate::tui) page_failure: Option<String>,
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

    /// Takes `connection` as it is now and waits for its new session `session_id`; the editor and the last run stay.
    pub(super) fn reopen(&mut self, connection: Connection, session_id: SessionId) {
        self.connection = connection;
        self.session_id = session_id;
        self.session = SessionState::Opening;
        self.tunnel = None;
    }

    /// Forgets the session, which the backend closes; a statement running on it fails, the editor stays.
    pub(super) fn close(&mut self) {
        self.session = SessionState::Closed;
        self.tunnel = None;
        if self.is_running() {
            self.run = Run::Failed(CLOSED_WHILE_RUNNING.to_owned());
        }
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

    pub(super) fn apply(&mut self, incoming: Incoming) {
        let is_closed = self.session == SessionState::Closed;
        match incoming {
            Incoming::Opened | Incoming::NotOpened(_) | Incoming::Closed(_) | Incoming::Tunnel(_) if is_closed => {}
            Incoming::Opened => self.session = SessionState::Open,
            Incoming::NotOpened(err) => self.session = SessionState::Failed(err),
            Incoming::Closed(why) => self.session = SessionState::Failed(format!("{LOST_SESSION}: {why}")),
            Incoming::Tunnel(state) => self.tunnel = Some(state),
            Incoming::Result { run, .. } | Incoming::Refused { run, .. } | Incoming::Failed { run, .. }
                if !self.is_answer_to_running(run) => {}
            Incoming::Result { run, result, took } => {
                let header_chart = self.header.chart.as_ref().and_then(|spec| Series::from_result(&result, spec).ok());
                let view = header_chart.map_or(View::Grid, |series| View::Chart(Ok(series)));
                self.run = Run::Done { run, grid: Box::new(Grid::new(&result)), result, took, view, is_page_asked: false };
            }
            Incoming::Page { run, page } => self.push_page(run, page),
            Incoming::PageFailed { run, error } => {
                self.push_page(run, Page { rows: vec![], extent: Extent::Cut });
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

    /// Adds `page` to the result of `of`, the grid's widths kept from the first page; a page of another run is dropped.
    fn push_page(&mut self, of: RunId, page: Page) {
        if let Run::Done { run, result, is_page_asked, .. } = &mut self.run
            && *run == of
            && result.extent() == Extent::More
        {
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
    use super::super::tests::{shop, to_shop, typed};
    use super::*;
    use crate::db::model::{ColumnKind, Value};
    use crate::fixtures::{column, orders};
    use crossterm::event::{KeyCode, KeyEvent};

    fn numbers(count: usize) -> Vec<Vec<Value>> {
        (0..count).map(|n| vec![Value::Int(n as i64)]).collect()
    }

    fn page(count: usize, extent: Extent) -> Page {
        Page { rows: numbers(count), extent }
    }

    fn with_result(result: ResultSet) -> App {
        let mut app = typed("SELECT n FROM orders");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result, took: Duration::ZERO }));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app
    }

    fn first_page() -> App {
        with_result(ResultSet::new(vec![column("n", ColumnKind::Int)], page(500, Extent::More)))
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

    #[test]
    fn a_whole_result_never_asks_for_a_page() {
        assert_eq!(press(&mut with_result(orders()), 'G'), []);
    }
}
