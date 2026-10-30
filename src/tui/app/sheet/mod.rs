//! One connection's sheet: its editor, the last statement run and its outcome, and the state of its session.
use super::editor::Editor;
use super::ghost::Ghost;
use super::grid::{Clock, Move, Reading, Room};
use super::param_bar::{Direction, ParamBar};
use super::{Action, Confirmed, Grid, Incoming, RunId, SessionId, TunnelState};
use crate::chart::{self, ChartError, Series};
use crate::complete::Index;
use crate::connection::{Connection, ConnectionName, WritePolicy};
use crate::db::guard::{self, Write};
use crate::db::model::{ColumnName, Extent, Page, ResultSet, Statement};
use crate::db::preview;
use crate::failure::Failure;
use crate::format::{self, Format};
use crate::keys::{self, Keys, Lookup};
use crate::query::params::{self, Filled, Numbering, ParamInput, ParamName};
use crate::query::{Header, Query};
use crossterm::event::KeyEvent;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

pub(in crate::tui) use schema::SchemaState;

mod schema;

pub(in crate::tui) const LOST_SESSION: &str = "session lost, ⏎ opens a new one";
const CLOSED_WHILE_RUNNING: &str = "cancelled: its session was closed";
const REOPENED_WHILE_RUNNING: &str = "cancelled: its session was reopened";
/// The grid asks for the next page once its cursor is this close to its last row, so the page arrives before the cursor
/// gets there.
const NEAR_END_ROWS: usize = 100;

/// The statement last sent, and the editor byte where it starts, until the text changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    start: Option<usize>,
    statement: Statement,
    /// The `$n` the server got for each `{{name}}`, which the server's error position counts in.
    numbering: Numbering,
    input: ParamInput,
}

/// A statement to run, the editor byte where it starts when it was cut from the editor, and the values of its parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct ToRun {
    statement: Statement,
    start: Option<usize>,
    input: ParamInput,
}

impl ToRun {
    /// The read that counts the rows this statement would touch as a write, its parameters bound as the write's would be;
    /// none when no read can count them.
    pub(in crate::tui::app) fn count(&self) -> Option<Lookup> {
        let read = preview::count_of(&self.statement)?;
        let Filled { statement, params } = params::fill_cut(&read, &self.input.given, &self.input.specs).ok()?;
        Some(Lookup { statement, params, own_timeout: Some(preview::COUNT_TIMEOUT) })
    }
}

/// The state of the session, as the status line shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum SessionState {
    Opening,
    Open,
    Failed(String),
    /// Closed from the picker; the sheet stays, and picking it again opens a new session.
    Closed,
}

/// The last statement run and its outcome.
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

/// How the result pane shows a result; `v` switches.
/// A new result starts on the grid, or on the chart its header asks for when that chart fits the result.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum View {
    Grid,
    /// The chart of the header's `-- chart:`, or the one `chart::infer` picks when `v` is pressed, or why there is none.
    Chart(Result<Series, ChartError>),
}

/// One connection in the TUI: its editor text, its last run and its session's state, kept while another one is shown.
#[derive(Debug)]
pub(in crate::tui) struct Sheet {
    /// The connection as the config had it when its session last opened.
    pub(in crate::tui) connection: Connection,
    pub(in crate::tui) editor: Editor,
    pub(in crate::tui) session: SessionState,
    /// The session whose answers the sheet takes; answers from any other session are dropped.
    pub(in crate::tui) session_id: SessionId,
    /// The state of the connection's tunnel, once it has one and its task has reported.
    pub(in crate::tui) tunnel: Option<TunnelState>,
    pub(in crate::tui) run: Run,
    sent: Option<Sent>,
    last_run: Option<RunId>,
    /// The editor byte the server's last error points at, until the text changes.
    pub(in crate::tui) error_mark: Option<usize>,
    /// The saved query the editor stands for, once one has been opened in it.
    pub(in crate::tui) opened: Option<Query>,
    /// Why the last query asked for could not be opened, until a query opens.
    pub(in crate::tui) query_error: Option<String>,
    /// The header of the editor's text: its `-- chart:` sets how each result of the sheet is drawn, its `-- format:`
    /// how its cells are shown.
    pub(in crate::tui) header: Header,
    /// Whether the editor's text changed since it was opened or saved.
    pub(in crate::tui) is_dirty: bool,
    /// Why the last page asked for failed, until the next run.
    pub(in crate::tui) page_failure: Option<String>,
    /// The result pane's title when it shows a table's columns instead of a run's result.
    pub(in crate::tui) shown: Option<String>,
    pub(in crate::tui) schema: SchemaState,
    /// The completion index of the schema read, once the backend has built it.
    pub(in crate::tui) index: Option<Arc<Index>>,
    pub(in crate::tui) ghost: Option<Ghost>,
    pub(in crate::tui) bar: ParamBar,
    /// The statement that tried to write while the sheet was off screen or under an overlay, asked about once it shows.
    tried_to_write: Option<ToRun>,
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
            shown: None,
            schema: SchemaState::NotRead,
            index: None,
            ghost: None,
            bar: ParamBar::default(),
            tried_to_write: None,
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

    /// Takes `connection` as it is now and waits for its new session `session_id`.
    /// The editor and the last run stay, but a statement running on the old session fails with it.
    pub(super) fn reopen(&mut self, connection: Connection, session_id: SessionId) {
        self.connection = connection;
        self.session_id = session_id;
        self.session = SessionState::Opening;
        self.tunnel = None;
        self.stop_paging();
        self.fail_running(REOPENED_WHILE_RUNNING);
        self.forget_schema();
    }

    /// Takes `connection` as it is now and leaves the session closed: picking the sheet later opens it with the config
    /// of that moment, and reads its schema again.
    pub(super) fn take_closed(&mut self, connection: Connection) {
        self.connection = connection;
        self.forget_schema();
    }

    /// Forgets the session, which the backend closes; a statement running on it fails, and the editor stays.
    pub(super) fn close(&mut self) {
        self.session = SessionState::Closed;
        self.tunnel = None;
        self.stop_paging();
        self.fail_running(CLOSED_WHILE_RUNNING);
        if self.schema == SchemaState::Reading {
            self.schema = SchemaState::NotRead;
        }
    }

    /// Fails the running statement with `why`: the session it ran on is gone, and its answer with it.
    fn fail_running(&mut self, why: &str) {
        if self.is_running() {
            self.run = Run::Failed(why.to_owned());
        }
    }

    /// Stops the result at the rows it has, since the portal its pages came from closes with the session.
    fn stop_paging(&mut self) {
        if let Run::Done { result, is_page_asked, .. } = &mut self.run {
            result.cut();
            *is_page_asked = false;
        }
    }

    /// Whether the result pane shows a chart instead of the grid that the cursor keys move.
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

    /// The byte range of the statement under the editor's cursor.
    pub(super) fn range_under_cursor(&self) -> Option<Range<usize>> {
        let (before, _, _) = self.editor.split();
        guard::statement_at(self.editor.text(), self.editor.lexed(), before.len())
    }

    /// The statement under the editor's cursor, and its byte range in the editor's text.
    fn statement_under_cursor(&self) -> Option<(Range<usize>, Statement)> {
        let range = self.range_under_cursor()?;
        Some((range.clone(), Statement::new(&self.editor.text()[range])))
    }

    /// The `{{name}}` parameters of the statement under the cursor, each once; none when it has none or holds a `{{`
    /// that cannot be read.
    pub(in crate::tui) fn param_names(&self) -> Vec<ParamName> {
        let names = self.range_under_cursor().map(|range| params::names(self.editor.lexed().tokens_in(range)));
        names.and_then(Result::ok).unwrap_or_default()
    }

    /// The parameter bar's field with the keys, among the names of the statement under the cursor.
    pub(in crate::tui) fn param_at(&self) -> Option<ParamName> {
        let names = self.param_names();
        let last = names.len().checked_sub(1)?;
        names.into_iter().nth(self.bar.at.min(last))
    }

    /// Moves the keys to the bar's next or previous field; false past either end.
    pub(super) fn step_bar(&mut self, direction: Direction) -> bool {
        let count = self.param_names().len();
        self.bar.at = self.bar.at.min(count.saturating_sub(1));
        self.bar.step(count, direction)
    }

    /// Applies `key` to the bar's field with the keys.
    pub(super) fn edit_bar(&mut self, key: KeyEvent) {
        if let Some(name) = self.param_at() {
            self.bar.edit(&name, key);
        }
    }

    /// Pastes `text` in the bar's field with the keys.
    pub(super) fn paste_in_bar(&mut self, text: &str) {
        if let Some(name) = self.param_at() {
            self.bar.paste(&name, text);
        }
    }

    /// The statement under the cursor with the values of the parameter bar; the editor's other statements are left out.
    pub(super) fn editor_run(&self) -> Option<ToRun> {
        let (range, statement) = self.statement_under_cursor()?;
        let names = params::names(self.editor.lexed().tokens_in(range.clone())).unwrap_or_default();
        let input = ParamInput { given: self.bar.given(&names), specs: self.header.params.clone() };
        Some(ToRun { statement, start: Some(range.start), input })
    }

    /// What the statement under the cursor writes, on a connection that allows writes; a read-only connection refuses
    /// a write without asking.
    pub(super) fn write_under_cursor(&self) -> Option<Write> {
        if self.connection.writes() == WritePolicy::ReadOnly {
            return None;
        }
        let range = self.range_under_cursor()?;
        guard::write_in(self.editor.text(), self.editor.lexed().tokens_in(range))
    }

    /// Runs `to_run`; the backend refuses a write unless `confirmed` shows the user answered `y`.
    pub(super) fn run(&mut self, to_run: ToRun, confirmed: Option<Confirmed>) -> Vec<Action> {
        self.run_statement(to_run.statement, to_run.start, to_run.input, confirmed)
    }

    /// Runs `statement` with `input` for its parameters; `start` is its first editor byte when it was cut from the
    /// editor.
    /// Does nothing while another statement runs.
    fn run_statement(
        &mut self,
        statement: Statement,
        start: Option<usize>,
        input: ParamInput,
        confirmed: Option<Confirmed>,
    ) -> Vec<Action> {
        if self.is_running() {
            return vec![];
        }
        let run = self.next_run();
        self.tried_to_write = None;
        self.run = Run::Running { run };
        self.sent = Some(Sent { start, numbering: params::numbering(&statement), statement: statement.clone(), input: input.clone() });
        vec![Action::Run { connection: self.name().clone(), run, statement, input, confirmed }]
    }

    /// Shows `result` in the result pane as if a run returned it, titled `title`.
    /// Does nothing while a statement runs, since it would drop that statement's answer.
    pub(super) fn show(&mut self, title: String, result: ResultSet, reading: &Reading) {
        if self.is_running() {
            return;
        }
        let run = self.next_run();
        self.shown = Some(title);
        let grid = Box::new(Grid::new(&result, format::opening(result.columns(), &[], reading.mode), Keys::default(), &reading.clock));
        self.run = Run::Done { run, grid, result, took: Duration::ZERO, view: View::Grid, is_page_asked: false };
    }

    /// A new run's id; the marks of the last run are dropped.
    fn next_run(&mut self) -> RunId {
        let run = self.last_run.map_or(RunId::FIRST, RunId::next);
        self.last_run = Some(run);
        self.error_mark = None;
        self.page_failure = None;
        self.shown = None;
        run
    }

    /// The editor byte at character `position` of the statement the server got, counted from 1; the statement's end
    /// when the server points just past its last character.
    fn editor_byte(&self, position: usize) -> Option<usize> {
        let sent = self.sent.as_ref()?;
        let offset = sent.numbering.template_byte(sent.statement.as_str(), position.checked_sub(1)?)?;
        Some(sent.start? + offset)
    }

    /// Takes an answer from the session; a result opens in the formats its header and `reading` give it.
    /// A statement the server found a write in waits in [`Sheet::take_tried_to_write`] to be asked about.
    pub(super) fn apply(&mut self, incoming: Incoming, reading: &Reading) {
        if let Incoming::Failed { run, failure: Failure::TriedToWrite } = incoming {
            self.tried_to_write = self.tried_to_write(run);
            return;
        }
        let is_closed = self.session == SessionState::Closed;
        match incoming {
            Incoming::Opened
            | Incoming::NotOpened(_)
            | Incoming::Closed(_)
            | Incoming::Tunnel(_)
            | Incoming::Schema(_)
            | Incoming::Index { .. }
                if is_closed => {}
            Incoming::Opened => self.session = SessionState::Open,
            Incoming::NotOpened(failure) => {
                let why = failure.to_string();
                self.fail_schema_read(&why);
                self.session = SessionState::Failed(why);
            }
            Incoming::Closed(lost) => {
                let why = lost.to_string();
                self.fail_schema_read(&why);
                self.session = SessionState::Failed(format!("{LOST_SESSION}: {why}"));
            }
            Incoming::Tunnel(state) => self.tunnel = Some(state),
            Incoming::Schema(read) => self.take_schema(read),
            Incoming::Index { catalog, index } => self.take_index(&catalog, index),
            Incoming::Result { run, .. } | Incoming::Failed { run, .. } if !self.is_answer_to_running(run) => {}
            Incoming::Result { run, result, took } => {
                let header_chart = self.header.chart.as_ref().and_then(|spec| Series::from_result(&result, spec).ok());
                let view = header_chart.map_or(View::Grid, |series| View::Chart(Ok(series)));
                let formats = format::opening(result.columns(), &self.header.formats, reading.mode);
                let grid = Box::new(Grid::new(&result, formats, self.keys_of(&result), &reading.clock));
                self.run = Run::Done { run, grid, result, took, view, is_page_asked: false };
            }
            Incoming::Page { run, .. } | Incoming::PageFailed { run, .. } if !self.is_waiting_on_page(run) => {}
            Incoming::Page { page, .. } => self.push_page(page, &reading.clock),
            Incoming::LookedUp { .. } => {}
            Incoming::PageFailed { failure, .. } => {
                self.push_page(Page { rows: vec![], extent: Extent::Cut }, &reading.clock);
                self.page_failure = Some(format!("the next page failed: {failure}"));
            }
            Incoming::Failed { failure, .. } => {
                self.error_mark = failure.position().and_then(|position| self.editor_byte(position));
                self.run = Run::Failed(failure.to_string());
            }
        }
    }

    /// The keys of the columns of `result` in the tables they were read from, once the schema is read.
    fn keys_of(&self, result: &ResultSet) -> Keys {
        self.catalog().map(|catalog| keys::keys(catalog, result.columns())).unwrap_or_default()
    }

    /// The statement that tried to write and waits to be asked about, once.
    pub(super) fn take_tried_to_write(&mut self) -> Option<ToRun> {
        self.tried_to_write.take()
    }

    /// Takes the answer that the run `run` tried to write and the server refused the write; gives the statement sent,
    /// when `run` is the one running.
    fn tried_to_write(&mut self, run: RunId) -> Option<ToRun> {
        if !self.is_answer_to_running(run) {
            return None;
        }
        self.run = Run::Failed(Failure::TriedToWrite.to_string());
        let Sent { statement, start, input, .. } = self.sent.clone()?;
        Some(ToRun { statement, start, input })
    }

    /// Moves the grid's cursor by `step` in the `room` it has, and asks for the next page once it nears the last row,
    /// unless a filter or a sort works on the rows loaded.
    pub(super) fn step_grid(&mut self, step: Move, room: Room) -> Vec<Action> {
        let Run::Done { run, result, grid, is_page_asked, .. } = &mut self.run else { return vec![] };
        grid.step(step, result, room);
        let is_near_end = result.rows().len().saturating_sub(grid.row) <= NEAR_END_ROWS;
        let has_more = result.extent() == Extent::More;
        if !has_more || !is_near_end || *is_page_asked || !grid.shown().is_whole() {
            return vec![];
        }
        *is_page_asked = true;
        vec![Action::NextPage { connection: self.connection.name().clone(), session: self.session_id, run: *run }]
    }

    /// Puts the column under the grid's cursor in its next format; when its kind has no other, returns its name.
    pub(super) fn step_format(&mut self, clock: &Clock) -> Option<ColumnName> {
        let Run::Done { result, grid, .. } = &mut self.run else { return None };
        grid.step_format(result, clock)
    }

    /// Shows only the grid's rows holding `filter` in one of their cells.
    pub(super) fn filter_rows(&mut self, filter: String, clock: &Clock) {
        if let Run::Done { result, grid, .. } = &mut self.run {
            grid.filter_rows(filter, result, clock);
        }
    }

    /// Sorts the grid's rows by the column under its cursor, the next way.
    pub(super) fn step_sort(&mut self, clock: &Clock) {
        if let Run::Done { result, grid, .. } = &mut self.run {
            grid.step_sort(result, clock);
        }
    }

    /// The column under the grid's cursor, when pgbg does not decode its type.
    pub(super) fn undecoded_column(&self) -> Option<ColumnName> {
        let Run::Done { result, grid, .. } = &self.run else { return None };
        result.columns().get(grid.column).filter(|column| !column.kind.is_decoded()).map(|column| column.name.clone())
    }

    /// Whether the grid shows a row under its cursor, which a filter can hide.
    pub(super) fn shows_cursor_row(&self) -> bool {
        matches!(&self.run, Run::Done { result, grid, .. } if grid.shows_cursor_row(result))
    }

    /// Puts every column of the grid raw, or readable when all are raw.
    pub(super) fn switch_every_format(&mut self, clock: &Clock) {
        if let Run::Done { result, grid, .. } = &mut self.run {
            grid.switch_every_format(result, clock);
        }
    }

    /// Whether the grid shows a column as time from now, which changes as the clock moves.
    pub(in crate::tui) fn shows_relative_time(&self) -> bool {
        matches!(&self.run, Run::Done { grid, view: View::Grid, .. } if grid.formats.contains(&Format::Relative))
    }

    /// Whether the result of the run `of` waits on its next page.
    fn is_waiting_on_page(&self, of: RunId) -> bool {
        matches!(&self.run, Run::Done { run, result, .. } if *run == of && result.extent() == Extent::More)
    }

    /// Adds `page` to the result, keeping the grid's widths from the first page, through its filter and sort.
    fn push_page(&mut self, page: Page, clock: &Clock) {
        if let Run::Done { result, grid, is_page_asked, .. } = &mut self.run {
            result.push(page);
            grid.take_rows(result, clock);
            *is_page_asked = false;
        }
    }

    /// Forgets the position of the statement sent in the editor, once the text it was cut from has changed.
    pub(super) fn forget_sent_place(&mut self) {
        if let Some(sent) = &mut self.sent {
            sent.start = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::App;
    use super::super::fixtures::{app, on_the_result, shop, to_shop};
    use super::*;
    use crate::db::model::{ColumnKind, Value};
    use crate::failure::Failure;
    use crate::fixtures::{column, orders, page, shop_column};
    use crate::format::Mode;
    use crossterm::event::{KeyCode, KeyEvent};
    use std::sync::Arc;
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
        app.apply(to_shop(Incoming::PageFailed { run: RunId::FIRST, failure: Failure::Link("ERROR: division by zero".into()) }));
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
        app.apply(to_shop(Incoming::PageFailed { run: RunId::FIRST, failure: Failure::Link("ERROR: canceling statement".into()) }));
        assert_eq!(app.sheet().page_failure, None);
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

    #[test]
    fn a_result_knows_where_its_foreign_key_points_once_the_schema_is_read() {
        let mut app = app();
        app.apply(to_shop(Incoming::Schema(Ok(Arc::new(crate::fixtures::shop_catalog())))));
        "SELECT * FROM orders".chars().for_each(|c| _ = press(&mut app, c));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let columns = vec![shop_column("orders", "id", ColumnKind::Int), shop_column("orders", "customer_id", ColumnKind::Int)];
        app.apply(to_shop(Incoming::Result {
            run: RunId::FIRST,
            result: ResultSet::whole(columns, vec![]),
            took: Duration::from_millis(38),
        }));
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(grid.points_to(1).map(|target| target.table.as_str()), Some("customers"));
    }
}
