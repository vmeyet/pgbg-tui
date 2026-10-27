//! One connection's sheet: its editor, the statement it ran and what came of it, and where its session stands.
use super::editor::Editor;
use super::{Action, Grid, Incoming, RunId, TunnelState};
use crate::chart::{self, ChartError, Series};
use crate::connection::{Connection, ConnectionName};
use crate::db::guard;
use crate::db::model::{ResultSet, Statement};
use crate::query::{Header, Query};
use std::time::Duration;

pub(in crate::tui) const LOST_SESSION: &str = "session lost, ⏎ opens a new one";
const CLOSED_WHILE_RUNNING: &str = "cancelled: its session was closed";

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
        result: ResultSet,
        took: Duration,
        grid: Grid,
        view: View,
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
}

impl Sheet {
    pub(super) fn new(connection: &Connection) -> Self {
        Self {
            connection: connection.clone(),
            editor: Editor::default(),
            session: SessionState::Opening,
            tunnel: None,
            run: Run::Idle,
            sent: None,
            last_run: None,
            error_mark: None,
            opened: None,
            query_error: None,
            header: Header::default(),
            is_dirty: false,
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

    /// Takes `connection` as it is now and waits for its new session; the editor and the last run stay.
    pub(super) fn reopen(&mut self, connection: Connection) {
        self.connection = connection;
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
            Incoming::Result { result, took, .. } => {
                let header_chart = self.header.chart.as_ref().and_then(|spec| Series::from_result(&result, spec).ok());
                let view = header_chart.map_or(View::Grid, |series| View::Chart(Ok(series)));
                self.run = Run::Done { grid: Grid::new(&result), result, took, view };
            }
            Incoming::Refused { why, .. } => self.run = Run::Failed(format!("not sent: {why}")),
            Incoming::Failed { error, position, .. } => {
                self.error_mark = position.and_then(|position| self.editor_byte(position));
                self.run = Run::Failed(error);
            }
        }
    }

    /// Forgets where the statement sent stands in the editor, once the text it was cut from changed.
    pub(super) fn forget_sent_place(&mut self) {
        if let Some(sent) = &mut self.sent {
            sent.start = None;
        }
    }
}
