//! A result's portal, kept open across jobs: the session task serves its pages here until the portal closes.
use super::session::{Cancels, Job, Mailbox, Next, OnServer, SessionAnswers, Woken, ended, warn_if_still_running};
use crate::db::model::{CancelToken, Extent, Page};
use crate::db::{Fetched, Pages};
use crate::failure::{Failure, Lost};
use crate::tui::app::{Incoming, RunId};
use crate::tunnel::{Died, Tunnel};
use std::time::Duration;
use tokio::time::{Instant, sleep_until};

/// How long a portal waits for the grid to ask for its next page before it is closed.
/// Its transaction holds a snapshot, which stops the server from cleaning up the rows that snapshot can still see.
pub(super) const PAGES_IDLE_LIMIT: Duration = Duration::from_mins(5);

/// What serving the pages of `run` borrows from the session task while the portal stays open.
pub(super) struct Paging<'a, 's> {
    pub(super) pages: Pages<'s>,
    pub(super) run: RunId,
    pub(super) token: CancelToken,
    pub(super) mailbox: &'a mut Mailbox,
    pub(super) tunnel: Option<&'a mut Tunnel>,
    pub(super) idle_limit: Duration,
}

/// What woke the task while the portal was open.
enum Held {
    Queued(Option<Job>),
    Cancelled(Option<RunId>),
    Closed(Lost),
    TunnelEnded(Died),
    Idle,
}

/// How fetching a page ended.
enum Fetch<'s> {
    More(Page, Pages<'s>),
    Last(Page),
    /// The run's cancel stopped the fetch, so the portal is gone.
    Cancelled,
    /// The server failed the fetch, so the portal is gone.
    Failed(Failure),
    /// The session is stopping: the fetch was dropped, and the portal goes with the session.
    Closing,
}

/// Where the pages of one run go.
struct Outbox<'a> {
    answers: &'a SessionAnswers,
    run: RunId,
}

impl Outbox<'_> {
    fn send(&self, page: Page) {
        self.say(Incoming::Page { run: self.run, page });
    }

    /// Reports that the server failed the next page, so the result is cut.
    fn send_failure(&self, failure: Failure) {
        self.say(Incoming::PageFailed { run: self.run, failure });
    }

    fn say(&self, incoming: Incoming) {
        self.answers.say(incoming);
    }

    /// Reports that the portal closed before its last page, so the result is cut.
    fn send_cut(&self) {
        self.send(Page { rows: vec![], extent: Extent::Cut });
    }
}

/// The idle limit, kept a tenth below the server's, so pgbg closes the portal before the server ends the session.
fn idle_limit_before_the_server(idle_limit: Duration, on_server: Duration) -> Duration {
    idle_limit.min(on_server.saturating_sub(on_server / 10))
}

/// Serves the pages of `paging.run` until its portal closes: after its last page, a cancel of the run, a new run, the
/// idle limit, the session closing, or the end of its link or tunnel.
/// A close before the last page sends an empty `Cut` page.
/// Returns what the session task does next.
pub(super) async fn serve_pages(paging: Paging<'_, '_>) -> Next {
    let Paging { mut pages, run, token, mailbox, mut tunnel, idle_limit } = paging;
    let Mailbox { queue, cancelled, on_server, answers } = mailbox;
    let outbox = Outbox { answers, run };
    let idle_limit = idle_limit_before_the_server(idle_limit, pages.idle_limit_on_server());
    let mut deadline = Instant::now() + idle_limit;
    loop {
        let held = tokio::select! {
            queued = queue.recv() => Held::Queued(queued),
            cancel_of = cancelled.recv() => Held::Cancelled(cancel_of),
            why = pages.closed() => Held::Closed(why),
            died = ended(tunnel.as_deref_mut()) => Held::TunnelEnded(died),
            () = sleep_until(deadline) => Held::Idle,
        };
        let is_queue_closed = queue.is_closed();
        match held {
            Held::Queued(Some(Job::NextPage { run: of })) if of == run && !is_queue_closed => {
                match next_page(pages, run, &token, cancelled, on_server).await {
                    Fetch::More(page, rest) => {
                        outbox.send(page);
                        pages = rest;
                        deadline = Instant::now() + idle_limit;
                    }
                    Fetch::Last(page) => {
                        outbox.send(page);
                        return Next::Wait;
                    }
                    Fetch::Cancelled => {
                        outbox.send_cut();
                        return Next::Wait;
                    }
                    Fetch::Failed(failure) => {
                        outbox.send_failure(failure);
                        return Next::Wait;
                    }
                    Fetch::Closing => {
                        outbox.send_cut();
                        return Next::Stop;
                    }
                }
            }
            Held::Queued(Some(Job::NextPage { .. } | Job::Open)) if !is_queue_closed => {}
            Held::Cancelled(Some(of)) if of != run => {}
            Held::Closed(why) => return gone(pages, &outbox, Woken::Closed(why)),
            Held::TunnelEnded(died) => return gone(pages, &outbox, Woken::TunnelEnded(died)),
            held => {
                close(pages).await;
                outbox.send_cut();
                return still_to_act_on(held, is_queue_closed);
            }
        }
    }
}

/// Drops the portal and cuts the result without a rollback: the link or the tunnel ended, and the transaction with it.
fn gone(pages: Pages<'_>, outbox: &Outbox<'_>, woken: Woken) -> Next {
    drop(pages);
    outbox.send_cut();
    Next::Act(woken)
}

/// What to do once the portal is closed: run a queued job, stop when the queue is closed, or else wait.
fn still_to_act_on(held: Held, is_queue_closed: bool) -> Next {
    match held {
        Held::Queued(Some(job)) if !is_queue_closed => Next::Act(Woken::Queued(Some(job))),
        Held::Queued(_) => Next::Stop,
        Held::Cancelled(_) | Held::Closed(_) | Held::TunnelEnded(_) | Held::Idle => Next::Wait,
    }
}

/// Fetches the next page, asking the server to cancel it when its run's cancel comes.
/// A stop ends it as `CancelToken::unless_stopped` describes; `on_server` holds `token` until the page is fetched or
/// dropped.
async fn next_page<'s>(pages: Pages<'s>, run: RunId, token: &CancelToken, cancelled: &mut Cancels, on_server: &OnServer) -> Fetch<'s> {
    on_server.starts(token);
    let fetched = token.unless_stopped(pages.next(), cancelled.stopping(run, token)).await;
    on_server.ended();
    let is_cancelled = cancelled.is_latest(run);
    match fetched {
        Err(interrupted) => {
            warn_if_still_running(&interrupted.cancel);
            Fetch::Closing
        }
        Ok(Ok(Fetched { page, rest: Some(rest) })) => Fetch::More(page, rest),
        Ok(Ok(Fetched { page, rest: None })) => Fetch::Last(page),
        Ok(Err(_)) if is_cancelled => Fetch::Cancelled,
        Ok(Err(err)) => Fetch::Failed(Failure::from(&err)),
    }
}

/// Ends the portal's transaction; if that fails, the session task later finds the session lost.
async fn close(pages: Pages<'_>) {
    if let Err(err) = pages.close().await {
        tracing::warn!("closing a result's portal failed: {err:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::super::Backend;
    use super::*;
    use crate::connection::ConnectionName;
    use crate::connection::WritePolicy;
    use crate::db::model::{Params, Route, Statement, Value};
    use crate::db::{Session, guard};
    use crate::fixtures;
    use crate::query::Roots;
    use crate::query::params::ParamInput;
    use crate::tui::app::Confirmed;
    use crate::tui::app::{Answer, SessionId};
    use std::path::PathBuf;
    use tokio::sync::mpsc::{self, UnboundedReceiver};

    const SLOW_AFTER_THE_FIRST_PAGE: &str = "SELECT n, pg_sleep(CASE WHEN n > 500 THEN 30 ELSE 0 END) FROM generate_series(1, 1000) AS n";

    #[test]
    fn a_server_stricter_than_the_idle_limit_has_the_portal_close_before_it() {
        assert_eq!(idle_limit_before_the_server(PAGES_IDLE_LIMIT, Duration::from_mins(1)), Duration::from_secs(54));
    }

    #[test]
    fn a_server_more_patient_than_the_idle_limit_leaves_it_as_it_is() {
        assert_eq!(idle_limit_before_the_server(PAGES_IDLE_LIMIT, Duration::from_mins(6)), PAGES_IDLE_LIMIT);
    }

    fn shop() -> ConnectionName {
        fixtures::test_database_as("shop").name().clone()
    }

    fn started(idle_limit: Duration) -> (Backend, UnboundedReceiver<Answer>) {
        let (sender, answers) = mpsc::unbounded_channel();
        let mut backend = Backend { idle_limit, ..Backend::new(Roots::default(), PathBuf::from("config.toml"), sender) };
        backend.open(fixtures::test_database_as("shop"), SessionId::FIRST);
        (backend, answers)
    }

    async fn next(answers: &mut UnboundedReceiver<Answer>) -> Incoming {
        loop {
            match answers.recv().await.unwrap() {
                Answer::Session { incoming: Incoming::Opened, .. } => {}
                Answer::Session { incoming, .. } => return incoming,
                other => panic!("not a session's answer: {other:?}"),
            }
        }
    }

    fn run(backend: &Backend, run: RunId, sql: &str) {
        backend.run(&shop(), run, Statement::new(sql), ParamInput::default(), Some(Confirmed::FOR_TESTS));
    }

    fn ask_next_page(backend: &Backend) {
        backend.next_page(&shop(), SessionId::FIRST, RunId::FIRST);
    }

    async fn on_a_side_session(sql: &str) -> Vec<Vec<Value>> {
        let mut session = Session::open(&fixtures::test_database_as("shop"), Route::Direct).await.unwrap();
        session.run(&guard::admit(Statement::new(sql), Params::default(), WritePolicy::ReadOnly).unwrap()).await.unwrap().rows().to_vec()
    }

    fn page_of(incoming: Incoming) -> Page {
        let Incoming::Page { page, .. } = incoming else { panic!("not a page: {incoming:?}") };
        page
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_big_result_comes_a_page_at_a_time_as_the_grid_asks() {
        let (backend, mut answers) = started(PAGES_IDLE_LIMIT);
        run(&backend, RunId::FIRST, "SELECT generate_series(1, 1200)");
        next(&mut answers).await;
        ask_next_page(&backend);
        assert_eq!(page_of(next(&mut answers).await).extent, Extent::More);
        ask_next_page(&backend);
        let last = page_of(next(&mut answers).await);
        assert_eq!((last.rows.len(), last.extent), (200, Extent::All));
    }

    /// Runs a result bigger than a page as the second run, after the first run read the session's backend pid.
    async fn holding_a_portal(backend: &Backend, answers: &mut UnboundedReceiver<Answer>) -> Value {
        run(backend, RunId::FIRST, "SELECT pg_backend_pid()");
        let Incoming::Result { result, .. } = next(answers).await else { panic!("no pid") };
        run(backend, RunId::FIRST.next(), "SELECT generate_series(1, 1200)");
        next(answers).await;
        result.rows()[0][0].clone()
    }

    async fn state_of(pid: &Value) -> Vec<Vec<Value>> {
        on_a_side_session(&format!("SELECT state FROM pg_stat_activity WHERE pid = {pid}")).await
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_portal_left_idle_past_its_limit_is_closed_and_the_result_cut() {
        let (backend, mut answers) = started(Duration::ZERO);
        let pid = holding_a_portal(&backend, &mut answers).await;
        assert_eq!(page_of(next(&mut answers).await), Page { rows: vec![], extent: Extent::Cut });
        assert_eq!(state_of(&pid).await, [[Value::Text("idle".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn closing_the_session_with_its_portal_open_cuts_the_result_and_ends_its_transaction() {
        let (mut backend, mut answers) = started(PAGES_IDLE_LIMIT);
        let pid = holding_a_portal(&backend, &mut answers).await;
        backend.close(&shop());
        assert_eq!(page_of(next(&mut answers).await), Page { rows: vec![], extent: Extent::Cut });
        let is_held = |state: &Vec<Vec<Value>>| state.iter().flatten().any(|state| *state == Value::Text("idle in transaction".into()));
        while is_held(&state_of(&pid).await) {}
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cancel_in_the_middle_of_a_page_closes_the_portal_and_the_result_is_cut() {
        let (backend, mut answers) = started(PAGES_IDLE_LIMIT);
        run(&backend, RunId::FIRST, SLOW_AFTER_THE_FIRST_PAGE);
        next(&mut answers).await;
        ask_next_page(&backend);
        let is_active = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLOW_AFTER_THE_FIRST_PAGE}' AND state = 'active'");
        while on_a_side_session(&is_active).await.is_empty() {}
        backend.cancel(&shop(), RunId::FIRST);
        assert_eq!(page_of(next(&mut answers).await), Page { rows: vec![], extent: Extent::Cut });
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cancel_of_the_next_run_while_a_page_is_fetched_keeps_that_run_from_the_server() {
        let (backend, mut answers) = started(PAGES_IDLE_LIMIT);
        run(&backend, RunId::FIRST, SLOW_AFTER_THE_FIRST_PAGE);
        next(&mut answers).await;
        ask_next_page(&backend);
        let is_active = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLOW_AFTER_THE_FIRST_PAGE}' AND state = 'active'");
        while on_a_side_session(&is_active).await.is_empty() {}
        run(&backend, RunId::FIRST.next(), "SELECT 7");
        backend.cancel(&shop(), RunId::FIRST.next());
        backend.cancel(&shop(), RunId::FIRST);
        next(&mut answers).await;
        let answer = next(&mut answers).await;
        assert!(matches!(&answer, Incoming::Failed { run, .. } if *run == RunId::FIRST.next()), "{answer:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_page_on_the_server_leaves_its_cancel_to_an_abort_until_it_ends() {
        let (backend, mut answers) = started(PAGES_IDLE_LIMIT);
        run(&backend, RunId::FIRST, SLOW_AFTER_THE_FIRST_PAGE);
        next(&mut answers).await;
        ask_next_page(&backend);
        let is_active = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLOW_AFTER_THE_FIRST_PAGE}' AND state = 'active'");
        while on_a_side_session(&is_active).await.is_empty() {}
        let running = backend.sessions[&shop()].running.clone();
        assert!(running.borrow().is_some());
        backend.cancel(&shop(), RunId::FIRST);
        next(&mut answers).await;
        assert!(running.borrow().is_none());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_page_the_server_fails_says_why() {
        let (backend, mut answers) = started(PAGES_IDLE_LIMIT);
        run(&backend, RunId::FIRST, "SELECT 1 / (n - 600) FROM generate_series(1, 1000) AS n");
        next(&mut answers).await;
        ask_next_page(&backend);
        let failed = next(&mut answers).await;
        assert!(matches!(&failed, Incoming::PageFailed { failure, .. } if failure.to_string().contains("division by zero")), "{failed:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_new_run_closes_the_open_portal_first() {
        let (backend, mut answers) = started(PAGES_IDLE_LIMIT);
        run(&backend, RunId::FIRST, "SELECT generate_series(1, 1200)");
        next(&mut answers).await;
        run(&backend, RunId::FIRST.next(), "SELECT 7");
        assert_eq!(page_of(next(&mut answers).await).extent, Extent::Cut);
        assert!(matches!(next(&mut answers).await, Incoming::Result { run, .. } if run == RunId::FIRST.next()));
    }
}
