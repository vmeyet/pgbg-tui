//! One statement on the server: its cancels, the token an abort reaches it by, and its answer.
use super::link::Queued;
use crate::connection::WritePolicy;
use crate::db::model::{CancelToken, ServerError};
use crate::db::{Interrupted, Pages, Session, Started};
use crate::history::Outcome;
use crate::query::params;
use crate::tui::app::{Incoming, RunId};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::watch;
use tokio::time::Instant;

pub(super) const CANCELLED_BEFORE_SENT: &str = "cancelled before it was sent";
pub(super) const STOPPED_WITH_ITS_SESSION: &str = "cancelled: its session stopped";

/// Holds the cancel token of what a session task has on the server, a statement or a page, for its link's abort.
#[derive(Debug)]
pub(in crate::tui::backend) struct OnServer(watch::Sender<Option<CancelToken>>);

impl OnServer {
    pub(super) fn new(sender: watch::Sender<Option<CancelToken>>) -> Self {
        Self(sender)
    }

    pub(in crate::tui::backend) fn starts(&self, token: &CancelToken) {
        self.0.send_replace(Some(token.clone()));
    }

    pub(in crate::tui::backend) fn ended(&self) {
        self.0.send_replace(None);
    }
}

/// The cancels a session task is sent, each for a `RunId`, and the latest run one was heard for: ids only grow, and a
/// sheet sends one run at a time, so it is the only one a run still to come can be cancelled by.
pub(in crate::tui::backend) struct Cancels {
    receiver: UnboundedReceiver<RunId>,
    latest: Option<RunId>,
}

impl Cancels {
    pub(super) fn new(receiver: UnboundedReceiver<RunId>) -> Self {
        Self { receiver, latest: None }
    }

    /// The next cancel, kept when it is the latest; none once the session is stopping.
    pub(in crate::tui::backend) async fn recv(&mut self) -> Option<RunId> {
        let of = self.receiver.recv().await;
        self.latest = self.latest.max(of);
        of
    }

    /// Whether the latest cancel heard is for `run`.
    pub(in crate::tui::backend) fn is_latest(&self, run: RunId) -> bool {
        self.latest == Some(run)
    }

    /// Whether `run` was cancelled before it started, by its own cancel or by its session stopping.
    pub(super) fn was_cancelled(&mut self, run: RunId) -> bool {
        loop {
            match self.receiver.try_recv() {
                Ok(of) => self.latest = self.latest.max(Some(of)),
                Err(TryRecvError::Empty) => return self.is_latest(run),
                Err(TryRecvError::Disconnected) => return true,
            }
        }
    }

    /// Asks the server to cancel `run` at each of its cancels, and ends once the session is stopping.
    pub(in crate::tui::backend) async fn stopping(&mut self, run: RunId, token: &CancelToken) {
        while let Some(of) = self.recv().await {
            if of == run {
                cancel(token).await;
            }
        }
    }
}

/// A run's answer for the app, what came of it for the history, and its portal while more pages wait in it.
pub(super) struct Answered<'s> {
    pub(super) incoming: Incoming,
    pub(super) outcome: Outcome,
    pub(super) rest: Option<Pages<'s>>,
}

impl Answered<'_> {
    /// The answer to `run`, stopped with its session before the server answered.
    fn stopped(run: RunId, interrupted: &Interrupted<()>) -> Self {
        warn_if_still_running(&interrupted.cancel);
        let incoming = Incoming::Failed { run, error: STOPPED_WITH_ITS_SESSION.to_owned(), position: None };
        Self { incoming, outcome: Outcome::LinkError, rest: None }
    }
}

/// Runs `queued`, asking the server to cancel it when its cancel comes; a cancel for another run does nothing. A stop
/// stops it as `CancelToken::unless_stopped` says. `on_server` holds its token until it is answered or dropped.
pub(super) async fn run_until_answered<'s>(
    session: &'s mut Session,
    writes: WritePolicy,
    queued: Queued,
    cancelled: &mut Cancels,
    on_server: &OnServer,
) -> Answered<'s> {
    let token = session.cancel_token();
    let run = queued.run;
    on_server.starts(&token);
    let answered = token.unless_stopped(answer_to(session, writes, queued), cancelled.stopping(run, &token)).await;
    on_server.ended();
    answered.unwrap_or_else(|interrupted| Answered::stopped(run, &interrupted))
}

/// Warns when the server did not take the cancel a stop sent it.
pub(in crate::tui::backend) fn warn_if_still_running(cancel: &anyhow::Result<()>) {
    if let Err(err) = cancel {
        tracing::warn!("the statement may still run on the server: {err:#}");
    }
}

/// Asks the server to cancel what the session runs; a request that failed is logged, and the statement runs on.
async fn cancel(token: &CancelToken) {
    if let Err(err) = token.cancel().await {
        tracing::warn!("the cancel request failed: {err:#}");
    }
}

async fn answer_to(session: &mut Session, writes: WritePolicy, queued: Queued) -> Answered<'_> {
    let Queued { run, statement, input } = queued;
    let admitted = match params::admit(&statement, &input.given, &input.specs, writes) {
        Ok(admitted) => admitted,
        Err(refusal) => {
            return Answered { incoming: Incoming::Refused { run, why: refusal.to_string() }, outcome: Outcome::Refused, rest: None };
        }
    };
    let started = Instant::now();
    let (result, rest) = match session.start(&admitted).await {
        Ok(Started { result, rest }) => (Ok(result), rest),
        Err(err) => (Err(err), None),
    };
    let outcome = Outcome::of(&result);
    let incoming = match result {
        Ok(result) => Incoming::Result { run, result, took: started.elapsed() },
        Err(err) => {
            let position = err.downcast_ref::<ServerError>().and_then(|server| server.position);
            Incoming::Failed { run, position, error: format!("{err:#}") }
        }
    };
    Answered { incoming, outcome, rest }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn a_run_whose_cancel_was_heard_is_the_latest_cancelled() {
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut cancels = Cancels::new(receiver);
        sender.send(RunId::FIRST).unwrap();
        cancels.recv().await;
        assert!(cancels.is_latest(RunId::FIRST));
    }

    #[tokio::test]
    async fn a_cancel_for_an_earlier_run_leaves_the_later_one_the_latest() {
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut cancels = Cancels::new(receiver);
        sender.send(RunId::FIRST.next()).unwrap();
        sender.send(RunId::FIRST).unwrap();
        cancels.recv().await;
        cancels.recv().await;
        assert!(cancels.is_latest(RunId::FIRST.next()));
    }
}
