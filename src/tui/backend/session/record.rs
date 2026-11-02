//! The record of each run in the history of its connection, written in the background.
use super::link::Queued;
use crate::connection::ConnectionName;
use crate::db::model::Statement;
use crate::history::{Entry, Outcome, Store};
use chrono::{DateTime, Utc};
use tokio::time::Instant;

/// A run's history entry before its answer comes: what was sent, and when.
pub(super) struct RunStart {
    statement: Statement,
    started_at: DateTime<Utc>,
    clock: Instant,
}

impl RunStart {
    pub(super) fn now(queued: &Queued) -> Self {
        Self { statement: queued.statement.clone(), started_at: Utc::now(), clock: Instant::now() }
    }
}

/// Where the session task records its runs: the history of its connection, when pgbg keeps one.
pub(super) struct Recorder {
    pub(super) store: Option<Store>,
    pub(super) connection: ConnectionName,
}

impl Recorder {
    /// Records the run that `start` began, ended now; the next statement never waits on the disk.
    pub(super) fn record(&self, start: RunStart, outcome: Outcome) {
        let Some(store) = self.store.as_ref() else { return };
        let entry = Entry::of_run(start.statement, self.connection.clone(), start.started_at, start.clock.elapsed(), outcome);
        drop(store.record(entry));
    }
}
