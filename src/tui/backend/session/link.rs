//! The handle to one session task: it sends jobs, cancels and schema asks, and sees what the task has on the server so
//! an abort can cancel it.
use super::open::Opener;
use super::record::Recorder;
use super::run::{Cancels, OnServer, warn_if_still_running};
use super::{Mailbox, SessionTask};
use crate::connection::{Connection, ConnectionName};
use crate::db::model::{CancelToken, Statement};
use crate::history::Store;
use crate::query::params::ParamInput;
use crate::signal::STOP_WAIT;
use crate::tui::app::{Answer, Confirmed, Incoming, RunId, SessionId};
use crate::tui::backend::schema::SchemaReader;
use crate::tui::backend::send;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

/// Where the answers of one session go, each tagged with its connection and its `SessionId`.
#[derive(Clone, Debug)]
pub(in crate::tui::backend) struct SessionAnswers {
    pub(in crate::tui::backend) connection: ConnectionName,
    pub(in crate::tui::backend) session: SessionId,
    pub(in crate::tui::backend) answers: UnboundedSender<Answer>,
}

impl SessionAnswers {
    pub(in crate::tui::backend) fn say(&self, incoming: Incoming) {
        send(&self.answers, Answer::Session { connection: self.connection.clone(), session: self.session, incoming });
    }
}

/// The handle to one session task, which serves the session `session`: jobs and cancels go in.
#[derive(Debug)]
pub(in crate::tui::backend) struct SessionLink {
    pub(in crate::tui::backend) session: SessionId,
    jobs: UnboundedSender<Job>,
    cancels: UnboundedSender<RunId>,
    /// Shared with the schema reader, which it wakes at each ask.
    schema_asks: Arc<Notify>,
    /// What the task runs on the server that no cancel has reached yet, shared with the task; an abort cancels it first.
    pub(in crate::tui::backend) running: watch::Receiver<Option<CancelToken>>,
    task: JoinHandle<()>,
}

/// What the session task is asked to do, one job at a time.
#[derive(Debug)]
pub(in crate::tui::backend) enum Job {
    Run(Queued),
    /// Fetch the next page of this run's result, while its portal is open.
    NextPage {
        run: RunId,
    },
    /// Open the session again, unless it is open.
    Open,
}

/// A statement waiting for the session, its run, and whether the user confirmed it, which a write requires.
#[derive(Debug)]
pub(in crate::tui::backend) struct Queued {
    pub(in crate::tui::backend) run: RunId,
    /// As typed, with its `{{name}}` parameters: the history stores this form, never the values.
    pub(in crate::tui::backend) statement: Statement,
    pub(in crate::tui::backend) input: ParamInput,
    pub(in crate::tui::backend) confirmed: Option<Confirmed>,
}

impl SessionLink {
    /// Starts the task that opens the session `answers` names on `connection` and runs each statement in turn,
    /// recording each run in `history`; a result's portal waits `idle_limit` for its next page.
    pub(in crate::tui::backend) fn start(
        connection: Connection,
        answers: SessionAnswers,
        history: Option<Store>,
        idle_limit: Duration,
    ) -> Self {
        let (jobs, queue) = mpsc::unbounded_channel();
        let (cancels, cancelled) = mpsc::unbounded_channel();
        let schema_asks = Arc::new(Notify::new());
        let (on_server, running) = watch::channel(None);
        let (route, open_route) = watch::channel(None);
        let session = answers.session;
        let reader =
            SchemaReader { connection: connection.clone(), answers: answers.clone(), asks: Arc::clone(&schema_asks), route: open_route };
        let (cancelled, on_server) = (Cancels::new(cancelled), OnServer::new(on_server));
        let mailbox = Mailbox { queue, cancelled, on_server, answers };
        let recorder = Recorder { store: history, connection: connection.name().clone() };
        let task = SessionTask { opener: Opener::new(connection, route), mailbox, recorder, idle_limit };
        Self { session, jobs, cancels, schema_asks, running, task: tokio::spawn(task.serve(reader)) }
    }

    /// Queues `job` behind the running one.
    pub(in crate::tui::backend) fn send(&self, job: Job) {
        if self.jobs.send(job).is_err() {
            tracing::warn!("the session task is gone: a statement or an open is dropped");
        }
    }

    /// Reads the schema on a second session that takes the same route as this one, opening this one first if needed.
    pub(in crate::tui::backend) fn read_schema(&self) {
        self.send(Job::Open);
        self.schema_asks.notify_one();
    }

    /// Cancels `run` if the session is running it, or stops it before it reaches the server if it has not started; a
    /// later run is never touched.
    pub(in crate::tui::backend) fn cancel(&self, run: RunId) {
        if self.cancels.send(run).is_err() {
            tracing::warn!("the session task is gone: the cancel is dropped");
        }
    }

    /// Stops the session task, which cancels the running statement and drops it, waiting `STOP_WAIT` at most.
    /// After that, the server is asked to cancel any statement or page whose cancel did not reach it, and the task is
    /// aborted, which cuts the session's link.
    pub(in crate::tui::backend) async fn stop(self) {
        let Self { jobs, cancels, running, mut task, .. } = self;
        drop((jobs, cancels));
        match tokio::time::timeout(STOP_WAIT, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!("the session task failed: {err}"),
            Err(_) => {
                tracing::warn!("the session task did not stop within {STOP_WAIT:?}: it is aborted");
                let on_server = running.borrow().clone();
                if let Some(token) = on_server {
                    warn_if_still_running(&token.cancel_as_stopping().await);
                }
                task.abort();
            }
        }
    }
}
