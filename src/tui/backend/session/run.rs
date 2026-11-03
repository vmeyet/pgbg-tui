//! One statement on the server: its cancels, the token an abort uses to reach it, and its answer.
use super::link::{Policy, Queued};
use crate::connection::WritePolicy;
use crate::db::guard::{Admitted, Repeat};
use crate::db::model::CancelToken;
use crate::db::{Interrupted, Pages, Session, Started};
use crate::failure::{Failure, LostWith};
use crate::history::Outcome;
use crate::query::params;
use crate::tui::app::{Incoming, RunId};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::watch;
use tokio::time::Instant;

const UNCONFIRMED_WRITE: &str = "a write runs only once confirmed";
pub(super) const MAY_HAVE_OUTSIDE_EFFECT: &str =
    "the link was lost and it calls a function pgbg cannot tell runs safely twice, so it did not go again: ⏎ runs it again";
pub(super) const STATE_LOST: &str = "the link was lost and a new session has none of the settings this one may have changed, so it did not go again: set them again if you need them, then ⏎ runs it";

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

/// A run's answer for the app, its outcome for the history, its portal while more pages wait in it, and what a second
/// sending of it risks.
pub(super) struct Answered<'s> {
    pub(super) incoming: Incoming,
    pub(super) outcome: Outcome,
    pub(super) rest: Option<Pages<'s>>,
    pub(super) repeat: Repeat,
}

impl Answered<'_> {
    /// The answer to `run`, stopped with its session before the server answered.
    fn stopped(run: RunId, interrupted: &Interrupted<()>) -> Self {
        warn_if_still_running(&interrupted.cancel);
        Self::failed(run, Failure::StoppedWithItsSession)
    }

    fn failed(run: RunId, failure: Failure) -> Self {
        Self { outcome: Outcome::from(&failure), incoming: Incoming::Failed { run, failure }, rest: None, repeat: Repeat::Never }
    }
}

/// Which sending of a run this is: a read that lost only its link goes once more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sending {
    First,
    Last,
}

/// What follows a failed sending: it goes again, or the failure to show stays.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Resend {
    Goes,
    Stays(Failure),
}

/// The one decision on sending a failed statement again by itself: only when its link failed, the session it failed on
/// held nothing a new one lacks, running it twice changes nothing, and this was not its last sending.
/// `Failure::Link` also holds failures that are not the link's, as a value that does not decode: on those the session
/// still stands, so `lost_with` is none.
pub(super) fn resend(failure: Failure, repeat: Repeat, lost_with: Option<LostWith>, sending: Sending) -> Resend {
    let Failure::Link(words) = &failure else { return Resend::Stays(failure) };
    match (repeat, lost_with) {
        (Repeat::Free, Some(LostWith::Nothing)) if sending == Sending::First => Resend::Goes,
        (Repeat::Free, Some(LostWith::Nothing)) | (Repeat::Never, _) | (_, None | Some(LostWith::UserTransaction)) => {
            Resend::Stays(failure)
        }
        (_, Some(LostWith::SessionState)) => Resend::Stays(Failure::Link(format!("{words}: {STATE_LOST}"))),
        (Repeat::OnAKey, Some(LostWith::Nothing)) => Resend::Stays(Failure::Link(format!("{words}: {MAY_HAVE_OUTSIDE_EFFECT}"))),
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
            Answered { outcome, incoming: Incoming::Result { run, result, took: started.elapsed() }, rest, repeat: Repeat::Never }
        }
        Err(err) => Answered { repeat: admitted.repeat(), ..Answered::failed(run, Failure::from(&err)) },
    }
}

/// `queued` with its parameters filled in and checked against the connection's `writes` policy, unless it runs read only
/// whatever that says; a write the user did not confirm is refused here too, and one confirmed runs as a write even when
/// the guard reads it as a read.
fn admit(queued: &Queued, writes: WritePolicy) -> anyhow::Result<Admitted> {
    let writes = match queued.policy {
        Policy::OfTheConnection => writes,
        Policy::ReadOnly => WritePolicy::ReadOnly,
    };
    let admitted = params::admit(&queued.statement, &queued.input.given, &queued.input.specs, writes)?;
    match queued.confirmed {
        Some(_) => Ok(admitted.confirmed()),
        None if admitted.write().is_some() => anyhow::bail!(UNCONFIRMED_WRITE),
        None => Ok(admitted),
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures::{empty_table, next_answer, on_a_side_session, on_a_side_session_writing, shop, started_on};
    use super::*;
    use crate::db::explain::Explain;
    use crate::db::guard::Refusal;
    use crate::db::model::{ParamValue, Statement, Value};
    use crate::fixtures;
    use crate::plan::Plan;
    use crate::query::params::{Given, ParamInput, ParamName};
    use crate::tui::app::{Action, Confirmed};
    use tokio::sync::mpsc;

    fn queued(sql: &str, confirmed: Option<Confirmed>) -> Queued {
        Queued {
            run: RunId::FIRST,
            statement: Statement::new(sql),
            input: ParamInput::default(),
            confirmed,
            policy: Policy::OfTheConnection,
        }
    }

    fn analyzed(sql: &str) -> Queued {
        Queued { policy: Policy::ReadOnly, ..queued(sql, None) }
    }

    #[test]
    fn an_explain_analyze_of_a_write_from_a_key_is_refused_where_writes_are_allowed() {
        let refused = admit(&analyzed("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) DELETE FROM orders"), WritePolicy::Allowed).unwrap_err();
        assert!(matches!(refused.downcast_ref(), Some(Refusal::Write(word)) if word == "DELETE"), "{refused}");
    }

    #[test]
    fn an_explain_analyze_of_a_read_from_a_key_never_counts_as_a_write_to_confirm() {
        let admitted = admit(&analyzed("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT 1"), WritePolicy::Allowed).unwrap();
        assert!(admitted.is_read_only());
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

    const WORDS: &str = "connection closed";

    fn lost_link() -> Failure {
        Failure::Link(WORDS.into())
    }

    fn repeat_of(sql: &str, confirmed: Option<Confirmed>, writes: WritePolicy) -> Repeat {
        admit(&queued(sql, confirmed), writes).unwrap().repeat()
    }

    fn first_resend(repeat: Repeat, lost_with: Option<LostWith>) -> Resend {
        resend(lost_link(), repeat, lost_with, Sending::First)
    }

    fn stays_saying(why: &str) -> Resend {
        Resend::Stays(Failure::Link(format!("{WORDS}: {why}")))
    }

    #[test]
    fn a_read_of_a_table_repeats_freely_on_either_kind_of_connection() {
        for writes in [WritePolicy::ReadOnly, WritePolicy::Allowed] {
            assert_eq!(repeat_of("SELECT * FROM orders", None, writes), Repeat::Free);
        }
    }

    #[test]
    fn a_read_calling_an_unknown_function_repeats_only_on_a_key() {
        assert_eq!(repeat_of("SELECT dblink_exec('shop', 'DELETE FROM orders')", None, WritePolicy::ReadOnly), Repeat::OnAKey);
    }

    #[test]
    fn a_confirmed_write_never_repeats() {
        assert_eq!(repeat_of("DELETE FROM orders", Some(Confirmed::FOR_TESTS), WritePolicy::Allowed), Repeat::Never);
    }

    #[test]
    fn a_read_confirmed_as_a_write_never_repeats() {
        assert_eq!(repeat_of("SELECT add_order()", Some(Confirmed::FOR_TESTS), WritePolicy::Allowed), Repeat::Never);
    }

    #[test]
    fn an_analyze_from_a_key_of_a_table_repeats_freely() {
        let admitted = admit(&analyzed("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT * FROM orders"), WritePolicy::Allowed).unwrap();
        assert_eq!(admitted.repeat(), Repeat::Free);
    }

    #[test]
    fn an_analyze_from_a_key_calling_an_unknown_function_repeats_only_on_a_key() {
        let sql = "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT dblink_exec('shop', 'DELETE FROM orders')";
        assert_eq!(admit(&analyzed(sql), WritePolicy::Allowed).unwrap().repeat(), Repeat::OnAKey);
    }

    #[test]
    fn a_free_read_that_lost_only_its_link_goes_again() {
        assert_eq!(first_resend(Repeat::Free, Some(LostWith::Nothing)), Resend::Goes);
    }

    #[test]
    fn a_free_read_on_its_last_sending_stays_in_the_links_words() {
        assert_eq!(resend(lost_link(), Repeat::Free, Some(LostWith::Nothing), Sending::Last), Resend::Stays(lost_link()));
    }

    #[test]
    fn a_read_calling_an_unknown_function_stays_and_says_a_key_runs_it() {
        assert_eq!(first_resend(Repeat::OnAKey, Some(LostWith::Nothing)), stays_saying(MAY_HAVE_OUTSIDE_EFFECT));
    }

    #[test]
    fn a_free_read_on_a_session_whose_state_may_have_changed_stays_and_says_so() {
        assert_eq!(first_resend(Repeat::Free, Some(LostWith::SessionState)), stays_saying(STATE_LOST));
    }

    #[test]
    fn a_read_in_the_users_transaction_stays_in_the_links_words() {
        assert_eq!(first_resend(Repeat::Free, Some(LostWith::UserTransaction)), Resend::Stays(lost_link()));
    }

    #[test]
    fn a_write_stays_in_the_links_words_whatever_went_with_the_link() {
        for lost_with in [LostWith::Nothing, LostWith::SessionState, LostWith::UserTransaction] {
            assert_eq!(first_resend(Repeat::Never, Some(lost_with)), Resend::Stays(lost_link()));
        }
    }

    #[test]
    fn a_link_failure_on_a_session_still_standing_stays_in_its_words() {
        assert_eq!(first_resend(Repeat::Free, None), Resend::Stays(lost_link()));
        assert_eq!(first_resend(Repeat::OnAKey, None), Resend::Stays(lost_link()));
    }

    #[test]
    fn a_failure_in_the_servers_words_stays_as_it_is() {
        let failure = Failure::Server { message: "FATAL: terminating connection".into(), position: None, code: None };
        assert_eq!(resend(failure.clone(), Repeat::Free, Some(LostWith::Nothing), Sending::First), Resend::Stays(failure));
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

    fn writing_connection() -> crate::connection::Connection {
        fixtures::test_database("local", "writes = \"allowed\"\n")
    }

    /// A fresh `table` and a function `add_to_<table>()` that inserts a row into it, called by the `SELECT` it gives.
    async fn call_of_a_function_that_writes(table: &str) -> String {
        empty_table(table).await;
        on_a_side_session_writing(&format!(
            "CREATE OR REPLACE FUNCTION add_to_{table}() RETURNS int LANGUAGE sql AS $$ INSERT INTO {table} VALUES (1) RETURNING n $$"
        ))
        .await;
        format!("SELECT add_to_{table}()")
    }

    fn analyze(sql: &str, input: ParamInput) -> Action {
        let statement = Explain::Analyze.of(&Statement::new(sql)).unwrap();
        Action::Analyze { connection: shop(), run: RunId::FIRST, statement, input }
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_analyze_from_a_key_binds_its_parameters_and_returns_a_plan_the_tree_reads() {
        let (mut backend, mut answers) = started_on(writing_connection());
        let given = Given::typed([(ParamName::for_column("top", &[]), ParamValue::new("3"))].into());
        backend.dispatch(analyze("SELECT n FROM generate_series(1, {{top}}::int) AS n", ParamInput { given, specs: vec![] }));
        let Incoming::Result { result, .. } = next_answer(&mut answers).await else { panic!("no result") };
        let plan = Plan::of(&result).unwrap();
        assert_eq!((plan.nodes[0].kind.as_str(), plan.nodes[0].actual.as_ref().map(|actual| actual.rows)), ("Function Scan", Some(3.0)));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_analyze_from_a_key_of_a_function_that_writes_fails_keeps_nothing_and_asks_nothing() {
        let call = call_of_a_function_that_writes("orders_analyzed").await;
        let (mut backend, mut answers) = started_on(writing_connection());
        backend.dispatch(analyze(&call, ParamInput::default()));
        let answer = next_answer(&mut answers).await;
        assert!(matches!(&answer, Incoming::Failed { failure: Failure::Server { .. }, .. }), "{answer:?}");
        assert_eq!(on_a_side_session("SELECT n FROM orders_analyzed").await, Vec::<Vec<Value>>::new());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_analyze_from_a_key_runs_under_the_timeout_of_the_connection() {
        let connection = fixtures::test_database("local", "statement_timeout = \"50ms\"\n");
        let (mut backend, mut answers) = started_on(connection);
        backend.dispatch(analyze("SELECT pg_sleep(30)", ParamInput::default()));
        let answer = next_answer(&mut answers).await;
        assert!(
            matches!(&answer, Incoming::Failed { failure: Failure::Server { message, .. }, .. } if message.contains("statement timeout")),
            "{answer:?}"
        );
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_explain_from_a_key_does_not_run_the_statement() {
        let call = call_of_a_function_that_writes("orders_explained").await;
        let (backend, mut answers) = started_on(writing_connection());
        let statement = Explain::Plan.of(&Statement::new(call)).unwrap();
        backend.run(&shop(), RunId::FIRST, statement, ParamInput::default(), None);
        let Incoming::Result { result, .. } = next_answer(&mut answers).await else { panic!("no result") };
        assert!(Plan::of(&result).is_some_and(|plan| plan.nodes[0].actual.is_none()));
        assert_eq!(on_a_side_session("SELECT n FROM orders_explained").await, Vec::<Vec<Value>>::new());
    }
}
