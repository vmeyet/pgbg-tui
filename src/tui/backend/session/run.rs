//! One statement on the server: its cancels, the token an abort uses to reach it, and its answer.
use super::link::Queued;
use crate::connection::WritePolicy;
use crate::db::guard::Admitted;
use crate::db::model::CancelToken;
use crate::db::{Interrupted, Pages, Session, Started};
use crate::failure::Failure;
use crate::history::Outcome;
use crate::query::params;
use crate::tui::app::{Incoming, RunId};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::watch;
use tokio::time::Instant;

const UNCONFIRMED_WRITE: &str = "a write runs only once confirmed";

/// Holds the cancel token of what a session task runs on the server, a statement or a page, for its link's abort.
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

/// The cancels sent to a session task, each for a `RunId`, and the latest run a cancel was received for.
/// Ids only grow and a sheet sends one run at a time, so only the latest cancel can apply to a run that has not
/// started yet.
pub(in crate::tui::backend) struct Cancels {
    receiver: UnboundedReceiver<RunId>,
    latest: Option<RunId>,
}

impl Cancels {
    pub(super) fn new(receiver: UnboundedReceiver<RunId>) -> Self {
        Self { receiver, latest: None }
    }

    /// The next cancel, remembered when it is the latest; none once the session is stopping.
    pub(in crate::tui::backend) async fn recv(&mut self) -> Option<RunId> {
        let of = self.receiver.recv().await;
        self.latest = self.latest.max(of);
        of
    }

    /// Whether the latest cancel received is for `run`.
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

impl Cancels {
    /// Ends once the session is stopping, keeping the latest cancel heard meanwhile for the runs queued behind.
    pub(super) async fn until_stopping(&mut self) {
        while self.recv().await.is_some() {}
    }

    /// Ends once `run` is cancelled or the session is stopping, keeping the latest cancel heard meanwhile.
    pub(super) async fn until_cancelled(&mut self, run: RunId) {
        while !self.is_latest(run) && self.recv().await.is_some() {}
    }
}

/// A run's answer for the app, its outcome for the history, and its portal while more pages wait in it.
pub(super) struct Answered<'s> {
    pub(super) incoming: Incoming,
    pub(super) outcome: Outcome,
    pub(super) rest: Option<Pages<'s>>,
    /// A read failed on its link with no row shown yet: it may go once more should its session turn out lost.
    pub(super) may_go_again: bool,
}

impl Answered<'_> {
    /// The answer to `run`, stopped with its session before the server answered.
    fn stopped(run: RunId, interrupted: &Interrupted<()>) -> Self {
        warn_if_still_running(&interrupted.cancel);
        Self::failed(run, Failure::StoppedWithItsSession)
    }

    fn failed(run: RunId, failure: Failure) -> Self {
        Self { outcome: Outcome::from(&failure), incoming: Incoming::Failed { run, failure }, rest: None, may_go_again: false }
    }
}

/// Runs `queued`, asking the server to cancel it when its cancel comes; a cancel for another run does nothing.
/// A stop ends it as `CancelToken::unless_stopped` describes; `on_server` holds its token until it is answered or
/// dropped.
pub(super) async fn run_until_answered<'s>(
    session: &'s mut Session,
    writes: WritePolicy,
    queued: &Queued,
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

/// Asks the server to cancel what the session runs; a failed request is logged, and the statement keeps running.
async fn cancel(token: &CancelToken) {
    if let Err(err) = token.cancel().await {
        tracing::warn!("the cancel request failed: {err:#}");
    }
}

async fn answer_to<'s>(session: &'s mut Session, writes: WritePolicy, queued: &Queued) -> Answered<'s> {
    let run = queued.run;
    let admitted = match admit(queued, writes) {
        Ok(admitted) => admitted,
        Err(refusal) => return Answered::failed(run, Failure::Refused(refusal.to_string())),
    };
    let started = Instant::now();
    match session.start(&admitted).await {
        Ok(Started { result, rest }) => {
            let outcome = Outcome::rows(&result);
            Answered { outcome, incoming: Incoming::Result { run, result, took: started.elapsed() }, rest, may_go_again: false }
        }
        Err(err) => {
            let failure = Failure::from(&err);
            Answered { may_go_again: may_go_again(&admitted, &failure), ..Answered::failed(run, failure) }
        }
    }
}

/// Only a read that failed on its link may go again: a write may have landed, and a server's answer stands.
fn may_go_again(admitted: &Admitted, failure: &Failure) -> bool {
    admitted.is_read() && matches!(failure, Failure::Link(_))
}

/// `queued` with its parameters filled in and checked against the connection's `writes` policy; a write the user did
/// not confirm is refused here too, and one confirmed runs as a write even when the guard reads it as a read.
fn admit(queued: &Queued, writes: WritePolicy) -> anyhow::Result<Admitted> {
    let admitted = params::admit(&queued.statement, &queued.input.given, &queued.input.specs, writes)?;
    match queued.confirmed {
        Some(_) => Ok(admitted.confirmed()),
        None if admitted.write().is_some() => anyhow::bail!(UNCONFIRMED_WRITE),
        None => Ok(admitted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::Statement;
    use crate::query::params::ParamInput;
    use crate::tui::app::Confirmed;
    use tokio::sync::mpsc;

    fn queued(sql: &str, confirmed: Option<Confirmed>) -> Queued {
        Queued { run: RunId::FIRST, statement: Statement::new(sql), input: ParamInput::default(), confirmed }
    }

    #[test]
    fn an_unconfirmed_write_is_refused() {
        let refusal = admit(&queued("DELETE FROM orders", None), WritePolicy::Allowed).unwrap_err();
        assert_eq!(refusal.to_string(), UNCONFIRMED_WRITE);
    }

    #[test]
    fn a_confirmed_write_is_admitted() {
        let admitted = admit(&queued("DELETE FROM orders", Some(Confirmed::FOR_TESTS)), WritePolicy::Allowed).unwrap();
        assert!(admitted.write().is_some());
    }

    #[test]
    fn an_unconfirmed_read_is_admitted() {
        assert!(admit(&queued("SELECT * FROM orders", None), WritePolicy::Allowed).is_ok());
    }

    fn lost_link() -> Failure {
        Failure::Link("connection closed".into())
    }

    fn may_go_again_after(sql: &str, confirmed: Option<Confirmed>, writes: WritePolicy, failure: &Failure) -> bool {
        may_go_again(&admit(&queued(sql, confirmed), writes).unwrap(), failure)
    }

    #[test]
    fn a_read_on_a_read_only_connection_may_go_again_after_a_lost_link() {
        assert!(may_go_again_after("SELECT * FROM orders", None, WritePolicy::ReadOnly, &lost_link()));
    }

    #[test]
    fn a_read_on_a_writable_connection_may_go_again_after_a_lost_link() {
        assert!(may_go_again_after("SELECT * FROM orders", None, WritePolicy::Allowed, &lost_link()));
    }

    #[test]
    fn a_confirmed_write_never_goes_again() {
        assert!(!may_go_again_after("DELETE FROM orders", Some(Confirmed::FOR_TESTS), WritePolicy::Allowed, &lost_link()));
    }

    #[test]
    fn a_read_confirmed_as_a_write_never_goes_again() {
        assert!(!may_go_again_after("SELECT add_order()", Some(Confirmed::FOR_TESTS), WritePolicy::Allowed, &lost_link()));
    }

    #[test]
    fn a_read_the_server_failed_does_not_go_again() {
        let failure = Failure::Server { message: "FATAL: terminating connection".into(), position: None };
        assert!(!may_go_again_after("SELECT * FROM orders", None, WritePolicy::ReadOnly, &failure));
    }

    fn cancels() -> (mpsc::UnboundedSender<RunId>, Cancels) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (sender, Cancels::new(receiver))
    }

    #[tokio::test]
    async fn waiting_for_a_cancel_ends_at_the_cancel_of_its_run() {
        let (sender, mut cancels) = cancels();
        sender.send(RunId::FIRST).unwrap();
        cancels.until_cancelled(RunId::FIRST).await;
        assert!(cancels.is_latest(RunId::FIRST));
    }

    #[tokio::test]
    async fn waiting_for_a_cancel_ends_once_the_session_is_stopping() {
        let (sender, mut cancels) = cancels();
        drop(sender);
        cancels.until_cancelled(RunId::FIRST).await;
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_for_a_cancel_goes_on_past_the_cancel_of_another_run() {
        let (sender, mut cancels) = cancels();
        sender.send(RunId::FIRST).unwrap();
        let waited = tokio::time::timeout(std::time::Duration::from_secs(1), cancels.until_cancelled(RunId::FIRST.next())).await;
        assert!(waited.is_err());
    }

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
