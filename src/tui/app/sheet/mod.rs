//! One connection's sheet: its editor, the last statement run and its outcome, and the state of its session.
use super::ask::Suggestion;
use super::editor::{Editor, below};
use super::ghost::Ghost;
use super::grid::{Clock, Move, Reading, Room};
use super::param_bar::{Direction, ParamBar};
use super::plan_tree::PlanTree;
use super::shown_columns::KeptColumns;
use super::{Action, Confirmed, Grid, Incoming, RunId, SessionId, TunnelState};
use crate::ai::{Fault, TableUse};
use crate::chart::{self, ChartColumns, ChartError, ChartNext, ChartSpec, Series};
use crate::complete::Index;
use crate::connection::{Connection, ConnectionName, WritePolicy};
use crate::db::explain::Explain;
use crate::db::guard::{self, Write};
use crate::db::model::{Column, ColumnName, ErrorCode, Extent, Page, ResultSet, Statement};
use crate::db::preview;
use crate::failure::Failure;
use crate::format::{self, Format};
use crate::history::HistoryIndex;
use crate::keys::{self, Keys, Lookup};
use crate::query::params::{self, Filled, Numbering, ParamInput, ParamName};
use crate::query::{Header, Query, StatementChart};
use chrono::{DateTime, Utc};
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
    /// The bytes of the `EXPLAIN` pgbg put before the editor's text, which the server's error position counts too.
    lead: usize,
    statement: Statement,
    /// The `$n` the server got for each `{{name}}`, which the server's error position counts in.
    numbering: Numbering,
    input: ParamInput,
    /// The `-- chart:` leading it, which draws its result rather than the file's.
    chart: StatementChart,
}

/// A statement to run, the editor byte where it starts when it was cut from the editor, and the values of its parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct ToRun {
    statement: Statement,
    start: Option<usize>,
    /// The bytes of the `EXPLAIN` pgbg put before the editor's text.
    lead: usize,
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
    /// A run or a lookup opens a new session after the last one was lost.
    Reconnecting,
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
        /// Boxed, as `grid` is.
        result: Box<ResultSet>,
        took: Duration,
        /// Boxed, so the other states of a run stay small.
        grid: Box<Grid>,
        view: View,
        /// The chart last left for the grid, so `v` comes back to the kind and orientation it had.
        left_chart: Option<ChartColumns>,
        /// The next page was asked for and has not come yet.
        is_page_asked: bool,
    },
    Failed(String),
}

/// How the result pane shows a result; `v` switches.
/// A new result starts on the plan it holds, else on the chart its statement or the file's header asks for when that chart
/// fits it, else on the grid.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum View {
    Grid,
    /// The chart of the `-- chart:` asked for when it fits, or the one `chart::infer` picks, or why there is none.
    Chart(Result<Series, ChartError>),
    /// The plan an `EXPLAIN (FORMAT JSON)` returned; boxed, so a run's other views stay small.
    Plan(Box<PlanTree>),
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
    /// The editor bytes of the token the server's last error points at, until the text changes or the next run starts;
    /// empty at the end of a line, where there is nothing to mark but its end.
    pub(in crate::tui) error_mark: Option<Range<usize>>,
    /// The SQLSTATE of the server's last error, until the text changes or the next run starts.
    pub(super) error_code: Option<ErrorCode>,
    /// The saved query the editor stands for, once one has been opened in it.
    pub(in crate::tui) opened: Option<Query>,
    /// Why the last query asked for could not be opened, until a query opens.
    pub(in crate::tui) query_error: Option<String>,
    /// The header of the editor's text: its `-- format:` sets how cells are shown, its `-- chart:` how each result of the
    /// sheet is drawn, but for a statement leading with its own.
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
    /// How much the history uses each table of the schema read, which orders what an ask shows of it.
    pub(in crate::tui) table_use: TableUse,
    /// The connection's successful statements, read once and added to at each result, which the ghost offers from.
    pub(super) history_index: HistoryIndex,
    pub(in crate::tui) ghost: Option<Ghost>,
    /// An idle answer that rewrites what is typed, until `ctrl-g` shows it or the next key drops it.
    pub(super) rewrite: Option<Suggestion>,
    pub(in crate::tui) bar: ParamBar,
    /// The statement that tried to write while the sheet was off screen or under an overlay, asked about once it shows.
    tried_to_write: Option<ToRun>,
    /// The last table browsed, replaced by the next one while the editor still holds exactly what browsing wrote.
    browsed: Option<Browsed>,
    /// What was chosen for the columns of the last statement's result, kept while the next one runs.
    kept_columns: Option<KeptColumns>,
}

/// The editor's text before a table was browsed, and the text browsing wrote.
#[derive(Clone, Debug, PartialEq)]
struct Browsed {
    before: String,
    written: String,
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
            error_code: None,
            opened: None,
            query_error: None,
            header: Header::default(),
            is_dirty: false,
            page_failure: None,
            shown: None,
            schema: SchemaState::NotRead,
            index: None,
            table_use: TableUse::default(),
            history_index: HistoryIndex::default(),
            ghost: None,
            rewrite: None,
            bar: ParamBar::default(),
            tried_to_write: None,
            browsed: None,
            kept_columns: None,
        }
    }

    /// Shows the result as its plan or a chart, or as the grid again.
    pub(super) fn switch_view(&mut self) {
        if let Run::Done { result, view, left_chart, .. } = &mut self.run {
            if let View::Chart(Ok(series)) = view {
                *left_chart = Some(series.columns.clone());
            }
            *view = match (&*view, &*left_chart) {
                (View::Grid, Some(columns)) => View::Chart(Series::from_columns(result, columns.clone())),
                (View::Grid, None) => plan_view_of(result)
                    .unwrap_or_else(|| View::Chart(chart::asked_or_inferred_series(chart_asked(self.sent.as_ref(), &self.header), result))),
                (View::Chart(_) | View::Plan(_), _) => View::Grid,
            };
        }
    }

    /// Why the `-- chart:` leading the statement sent cannot be read, which no header of the file says below its top.
    pub(in crate::tui) fn sent_chart_warning(&self) -> Option<&str> {
        self.sent.as_ref()?.chart.warning.as_deref()
    }

    /// Draws the chart on screen as its next kind, with the same columns.
    pub(super) fn step_chart_kind(&mut self) {
        if let Run::Done { result, view: View::Chart(Ok(series)), .. } = &mut self.run {
            series.step_kind(result);
        }
    }

    /// Stands the bars of the chart on screen up, or lays them on their side.
    pub(super) fn flip_chart(&mut self) {
        if let Run::Done { view: View::Chart(Ok(series)), .. } = &mut self.run {
            series.flip();
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
    pub(in crate::tui) fn is_chart_shown(&self) -> bool {
        self.chart_next().is_some()
    }

    /// Where the chart's keys lead, while the result pane shows a chart: nowhere when the chart could not be drawn.
    pub(in crate::tui) fn chart_next(&self) -> Option<ChartNext> {
        let Run::Done { view: View::Chart(chart), .. } = &self.run else { return None };
        Some(chart.as_ref().map_or_else(|_| ChartNext::default(), Series::next))
    }

    /// Whether the result pane shows a plan, whose keys then take over from the grid's.
    pub(super) fn is_plan_shown(&self) -> bool {
        matches!(self.run, Run::Done { view: View::Plan(_), .. })
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
        Some(ToRun { statement, start: Some(range.start), lead: 0, input })
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
    /// Does nothing while another statement runs.
    pub(super) fn run(&mut self, to_run: ToRun, confirmed: Option<Confirmed>) -> Vec<Action> {
        let Some(run) = self.mark_sent(&to_run) else { return vec![] };
        let ToRun { statement, input, .. } = to_run;
        vec![Action::Run { connection: self.name().clone(), run, statement, input, confirmed }]
    }

    /// Runs `explain` of the statement under the cursor, its parameters bound as a run binds them; none when the guard
    /// does not read that statement as a query that changes nothing.
    pub(super) fn explain(&mut self, explain: Explain) -> Option<Vec<Action>> {
        let Some(to_run) = self.editor_run() else { return Some(vec![]) };
        let to_run = ToRun { statement: explain.of(&to_run.statement)?, lead: explain.lead().len(), ..to_run };
        if explain == Explain::Plan {
            return Some(self.run(to_run, None));
        }
        let Some(run) = self.mark_sent(&to_run) else { return Some(vec![]) };
        let ToRun { statement, input, .. } = to_run;
        Some(vec![Action::Analyze { connection: self.name().clone(), run, statement, input }])
    }

    /// Writes `preview` at the end of the editor, in place of the table browsed last while the editor still holds what
    /// that wrote, and runs `preview` alone, whatever the text above it; nothing while a statement runs.
    pub(super) fn browse(&mut self, preview: Statement) -> Vec<Action> {
        if self.is_running() {
            return vec![];
        }
        let before = match self.browsed.take() {
            Some(browsed) if browsed.written == self.editor.text() => browsed.before,
            _ => self.editor.text().to_owned(),
        };
        let written = below(&before, preview.as_str());
        let start = written.len() - preview.as_str().len();
        self.editor.rewrite(&written);
        self.text_changed();
        self.browsed = Some(Browsed { before, written });
        self.run(ToRun { statement: preview, start: Some(start), lead: 0, input: ParamInput::default() }, None)
    }

    /// Marks `to_run` as the statement running and gives its new id; none while another statement runs.
    fn mark_sent(&mut self, to_run: &ToRun) -> Option<RunId> {
        if self.is_running() {
            return None;
        }
        let run = self.next_run();
        let ToRun { statement, start, lead, input } = to_run.clone();
        self.tried_to_write = None;
        self.run = Run::Running { run };
        self.sent = Some(Sent {
            start,
            lead,
            numbering: params::numbering(&statement),
            chart: StatementChart::of(statement.as_str()),
            statement,
            input,
        });
        Some(run)
    }

    /// The grid on a new `result`, under what was chosen for the last one when it has the same columns.
    fn grid_on(&self, result: &ResultSet, reading: &Reading) -> Grid {
        let formats = format::opening(result.columns(), &self.header.formats, reading.mode);
        let mut grid = Grid::new(result, formats, self.keys_of(result), &reading.clock);
        if let Some(kept) = &self.kept_columns {
            grid.take_kept_columns(kept, result);
        }
        grid
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
        self.run = Run::Done {
            run,
            grid,
            result: Box::new(result),
            took: Duration::ZERO,
            view: View::Grid,
            left_chart: None,
            is_page_asked: false,
        };
    }

    /// A new run's id; the marks of the last run are dropped, and what was chosen for the columns of a statement's result
    /// kept, unless a table's columns show in its place.
    fn next_run(&mut self) -> RunId {
        if let (Run::Done { result, grid, .. }, None) = (&self.run, &self.shown) {
            self.kept_columns = Some(grid.keep_columns(result));
        }
        let run = self.last_run.map_or(RunId::FIRST, RunId::next);
        self.last_run = Some(run);
        self.error_mark = None;
        self.editor.stop_revealing();
        self.error_code = None;
        self.page_failure = None;
        self.shown = None;
        run
    }

    /// Marks what the server's `position` points at and brings it into view; no position, or one that cannot be read back
    /// in the editor, marks nothing.
    fn mark_error(&mut self, position: Option<usize>) {
        self.error_mark = position.and_then(|position| self.editor_byte(position)).and_then(|at| self.failing_bytes(at));
        if let Some(mark) = &self.error_mark {
            self.editor.reveal(mark.start);
        }
    }

    /// The editor byte at character `position` of the statement the server got, counted from 1; the statement's end
    /// when the server points just past its last character.
    fn editor_byte(&self, position: usize) -> Option<usize> {
        let sent = self.sent.as_ref()?;
        let offset = sent.numbering.template_byte(sent.statement.as_str(), position.checked_sub(1)?)?;
        Some(sent.start? + offset.checked_sub(sent.lead)?)
    }

    /// The token at editor byte `at`, or else the one character there, as where a statement ends or a comment never closes.
    fn failing_bytes(&self, at: usize) -> Option<Range<usize>> {
        if let Some(token) = self.editor.lexed().token_at(at) {
            return Some(token.range.clone());
        }
        let character = self.editor.text().get(at..)?.chars().next().filter(|&c| c != '\n');
        Some(at..at + character.map_or(0, char::len_utf8))
    }

    /// What an ask for a fix may tell of the server's last error, while the editor still holds the statement it failed.
    pub(super) fn fault(&self) -> Option<Fault> {
        let start = self.sent.as_ref()?.start?;
        Some(Fault { code: self.error_code.clone()?, at: self.error_mark.as_ref().and_then(|mark| mark.start.checked_sub(start)) })
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
            | Incoming::Reconnecting
            | Incoming::Tunnel(_)
            | Incoming::Schema(_)
            | Incoming::Index { .. }
            | Incoming::TableUse(_)
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
            Incoming::Reconnecting => self.session = SessionState::Reconnecting,
            Incoming::Tunnel(state) => self.tunnel = Some(state),
            Incoming::Schema(read) => self.take_schema(read),
            Incoming::Index { catalog, index } => self.take_index(&catalog, index),
            Incoming::TableUse(table_use) => self.table_use = table_use,
            Incoming::Result { run, .. } | Incoming::Failed { run, .. } if !self.is_answer_to_running(run) => {}
            Incoming::Result { run, result, took } => {
                self.add_sent_to_history_index(reading.clock.now);
                let view = plan_view_of(&result).unwrap_or_else(|| self.asked_view(&result));
                let grid = Box::new(self.grid_on(&result, reading));
                self.run = Run::Done { run, grid, result: Box::new(result), took, view, left_chart: None, is_page_asked: false };
            }
            Incoming::Page { run, .. } | Incoming::PageFailed { run, .. } if !self.is_waiting_on_page(run) => {}
            Incoming::Page { page, .. } => self.push_page(page, &reading.clock),
            Incoming::LookedUp { .. } => {}
            Incoming::PageFailed { failure, .. } => {
                self.push_page(Page { rows: vec![], extent: Extent::Cut }, &reading.clock);
                self.page_failure = Some(format!("the next page failed: {failure}"));
            }
            Incoming::Failed { failure, .. } => {
                self.mark_error(failure.position());
                self.error_code = failure.code().cloned();
                self.run = Run::Failed(failure.to_string());
            }
        }
    }

    /// How `result` opens as asked: as the chart its statement or else the file names, when that chart fits, else as the grid.
    fn asked_view(&self, result: &ResultSet) -> View {
        Series::asked(chart_asked(self.sent.as_ref(), &self.header), result).map_or(View::Grid, |series| View::Chart(Ok(series)))
    }

    fn add_sent_to_history_index(&mut self, at: DateTime<Utc>) {
        if let Some(sent) = &self.sent {
            self.history_index.add(&sent.statement, at);
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
        let Sent { statement, start, lead, input, .. } = self.sent.clone()?;
        Some(ToRun { statement, start, lead, input })
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

    /// The grid on screen, when a result is.
    pub(super) fn grid_mut(&mut self) -> Option<&mut Grid> {
        match &mut self.run {
            Run::Done { grid, .. } => Some(grid),
            _ => None,
        }
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

    /// Puts the cursor at the end of the statement last sent, while the editor still holds it, so an ask about its result
    /// is about it, whichever statement the cursor moved to since.
    pub(super) fn go_to_sent(&mut self) {
        if let Some(Sent { start: Some(start), lead, statement, .. }) = &self.sent {
            self.editor.move_cursor_to(start + statement.as_str().len() - lead);
        }
    }

    /// The columns of the result on screen, while the editor still holds the statement that gave it; none for what the
    /// schema tab shows, which no statement read.
    pub(super) fn result_columns(&self) -> &[Column] {
        let is_in_editor = self.sent.as_ref().is_some_and(|sent| sent.start.is_some());
        match &self.run {
            Run::Done { result, .. } if is_in_editor && self.shown.is_none() => result.columns(),
            _ => &[],
        }
    }

    /// Forgets the position of the statement sent in the editor, once the text it was cut from has changed.
    pub(super) fn forget_sent_place(&mut self) {
        if let Some(sent) = &mut self.sent {
            sent.start = None;
        }
    }
}

/// The view of the plan `result` holds, when it holds one.
fn plan_view_of(result: &ResultSet) -> Option<View> {
    PlanTree::of(result).map(|tree| View::Plan(Box::new(tree)))
}

/// The `-- chart:` leading the statement sent, else the file's: a file of several statements draws each its own way.
fn chart_asked<'a>(sent: Option<&'a Sent>, file: &'a Header) -> Option<&'a ChartSpec> {
    sent.and_then(|sent| sent.chart.spec.as_ref()).or(file.chart.as_ref())
}

#[cfg(test)]
mod tests {
    use super::super::App;
    use super::super::fixtures::{app, on_the_result, shop, to_shop, view};
    use super::*;
    use crate::chart::ChartKind;
    use crate::db::model::{ColumnKind, Value};
    use crate::failure::Failure;
    use crate::fixtures::{ESTIMATED_PLAN, column, explain_result, orders, page, shop_column};
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

    /// The app with [`orders`] from the statement at the end of `text`, run from the editor.
    fn ran(text: &str) -> App {
        let mut app = app();
        app.sheet_mut().paste(text);
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::ZERO }));
        app
    }

    fn drawn_kind(app: &App) -> Option<ChartKind> {
        match view(app) {
            View::Chart(Ok(series)) => Some(series.columns.kind),
            _ => None,
        }
    }

    #[test]
    fn a_chart_line_above_a_later_statement_opens_its_result_as_that_chart() {
        assert_eq!(drawn_kind(&ran("SELECT 1;\n-- chart: line x=id y=total\nSELECT * FROM orders")), Some(ChartKind::Line));
    }

    #[test]
    fn a_statement_s_own_chart_line_wins_over_the_file_header() {
        let text = "-- chart: grouped x=id y=total\nSELECT 1;\n-- chart: stacked x=id y=total\nSELECT * FROM orders";
        assert_eq!(drawn_kind(&ran(text)), Some(ChartKind::Stacked));
    }

    #[test]
    fn a_statement_with_no_chart_line_of_its_own_takes_the_file_header() {
        assert_eq!(drawn_kind(&ran("-- chart: line x=id y=total\nSELECT 1;\nSELECT * FROM orders")), Some(ChartKind::Line));
    }

    #[test]
    fn a_result_holding_a_plan_opens_as_the_plan_under_a_chart_line() {
        let mut app = app();
        app.sheet_mut().paste("-- chart: line x=id y=total\nEXPLAIN (FORMAT JSON) SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: explain_result(ESTIMATED_PLAN), took: Duration::ZERO }));
        assert!(matches!(view(&app), View::Plan(_)));
    }

    #[test]
    fn a_chart_line_above_one_statement_leaves_the_others_on_the_grid() {
        let mut app = app();
        app.sheet_mut().paste("-- comment only\nSELECT 1;\n-- chart: line x=id y=total\nSELECT 2");
        app.sheet_mut().editor.move_cursor_to(0);
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::ZERO }));
        assert_eq!(drawn_kind(&app), None);
    }

    #[test]
    fn v_on_a_chart_opened_on_its_own_goes_to_the_grid_and_back() {
        let mut app = ran("SELECT 1;\n-- chart: line x=id y=total\nSELECT * FROM orders");
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Char('v')));
        assert_eq!(drawn_kind(&app), None);
        app.handle_key(KeyEvent::from(KeyCode::Char('v')));
        assert_eq!(drawn_kind(&app), Some(ChartKind::Line));
    }

    #[test]
    fn a_chart_line_that_cannot_be_read_warns_once_its_statement_runs() {
        let app = ran("SELECT 1;\n-- chart: pie\nSELECT * FROM orders");
        assert_eq!(drawn_kind(&app), None);
        assert!(app.sheet().sent_chart_warning().is_some_and(|warning| warning.contains("pie")));
    }

    #[test]
    fn a_server_error_marks_its_place_below_a_chart_line_sent_with_the_statement() {
        let mut app = app();
        app.sheet_mut().paste("SELECT 1;\n-- chart: line x=id y=total\nSELECT nope");
        let sent = app.handle_key(KeyEvent::from(KeyCode::Enter));
        let [Action::Run { statement, .. }] = &*sent else { panic!("{sent:?}") };
        let position = statement.as_str().find("nope").unwrap() + 1;
        app.apply(to_shop(Incoming::Failed {
            run: RunId::FIRST,
            failure: Failure::Server { message: "ERROR: column does not exist".into(), position: Some(position), code: None },
        }));
        assert_eq!(app.sheet().error_mark.clone().map(|mark| &app.sheet().editor.text()[mark]), Some("nope"));
    }
}
