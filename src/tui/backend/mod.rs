//! What the runner calls: the session of the open connection, served by a task that owns it.
use super::app::{Incoming, RunId, TunnelState};
use crate::connection::{Connection, WritePolicy};
use crate::db::model::{CancelToken, Route, ServerError, Statement};
use crate::db::{Session, guard};
use crate::tunnel::{Died, Tunnel};
use std::pin::pin;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// How long quitting waits for the session task to finish a cancel, so the terminal comes back soon.
const STOP_WAIT: Duration = Duration::from_secs(2);
const CANCELLED_BEFORE_SENT: &str = "cancelled before it was sent";
const TUNNEL_NOT_STARTED: &str = "its tunnel did not start";
const TUNNEL_DIED: &str = "its tunnel died";

/// The way to the session task: runs and cancels in, answers out on the loop's channel.
#[derive(Debug)]
pub(super) struct Backend {
    runs: UnboundedSender<Queued>,
    cancels: UnboundedSender<RunId>,
    task: JoinHandle<()>,
}

/// A statement waiting for the session, and its run.
#[derive(Debug)]
struct Queued {
    run: RunId,
    statement: Statement,
}

impl Backend {
    /// Starts the task that opens a session on `connection` and runs each statement in turn.
    pub(super) fn start(connection: Connection, answers: UnboundedSender<Incoming>) -> Self {
        let (runs, queue) = mpsc::unbounded_channel();
        let (cancels, cancelled) = mpsc::unbounded_channel();
        let task = tokio::spawn(SessionTask { connection, tunnel: None, session: None, cancelled, answers }.serve(queue));
        Self { runs, cancels, task }
    }

    /// Queues `statement` behind the one running; its answer comes back as `Incoming`.
    pub(super) fn run(&self, run: RunId, statement: Statement) {
        if self.runs.send(Queued { run, statement }).is_err() {
            tracing::warn!("the session task is gone: the statement is dropped");
        }
    }

    /// Cancels `run` if the session is on it, or keeps it from the server if it has not started; a later run is never touched.
    pub(super) fn cancel(&self, run: RunId) {
        if self.cancels.send(run).is_err() {
            tracing::warn!("the session task is gone: the cancel is dropped");
        }
    }

    /// Lets the session task finish what it was asked, a cancel included, waiting `STOP_WAIT` at most.
    pub(super) async fn stop(self) {
        let Self { runs, cancels, task } = self;
        drop((runs, cancels));
        match tokio::time::timeout(STOP_WAIT, task).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!("the session task failed: {err}"),
            Err(_) => tracing::warn!("the session task did not stop within {STOP_WAIT:?}"),
        }
    }
}

/// What woke the session task.
enum Woken {
    Queued(Option<Queued>),
    Closed(String),
    TunnelEnded(Died),
}

/// The task that owns the session and its tunnel: it opens them, runs each statement in turn, and says when they are lost.
struct SessionTask {
    connection: Connection,
    tunnel: Option<Tunnel>,
    session: Option<Session>,
    cancelled: UnboundedReceiver<RunId>,
    answers: UnboundedSender<Incoming>,
}

impl SessionTask {
    /// Opens the session at once, and again before a statement when the last try failed or the session was lost.
    async fn serve(mut self, mut queue: UnboundedReceiver<Queued>) {
        self.open().await;
        loop {
            match self.woken(&mut queue).await {
                Woken::Queued(Some(queued)) => self.run(queued).await,
                Woken::Queued(None) => break,
                Woken::Closed(why) => self.lose(why),
                Woken::TunnelEnded(died) => self.lose_tunnel(&died),
            }
        }
    }

    async fn woken(&mut self, queue: &mut UnboundedReceiver<Queued>) -> Woken {
        let Self { session, tunnel, .. } = self;
        tokio::select! {
            queued = queue.recv() => Woken::Queued(queued),
            why = closed(session.as_ref()) => Woken::Closed(why),
            died = ended(tunnel.as_mut()) => Woken::TunnelEnded(died),
        }
    }

    async fn open(&mut self) {
        let Some(route) = self.route().await else {
            self.session = None;
            self.send(Incoming::NotOpened(TUNNEL_NOT_STARTED.to_owned()));
            return;
        };
        self.session = match Session::open(&self.connection, route).await {
            Ok(session) => {
                self.send(Incoming::Opened);
                Some(session)
            }
            Err(err) => {
                self.send(Incoming::NotOpened(format!("{err:#}")));
                None
            }
        };
    }

    async fn run(&mut self, queued: Queued) {
        if self.session.is_none() {
            self.open().await;
        }
        let Some(session) = self.session.as_mut() else { return };
        let run = queued.run;
        let answer = if was_cancelled(&mut self.cancelled, run) {
            Incoming::Failed { run, error: CANCELLED_BEFORE_SENT.to_owned(), position: None }
        } else {
            run_until_answered(session, self.connection.writes(), queued, &mut self.cancelled).await
        };
        self.send(answer);
        if let Some(why) = self.session.as_ref().and_then(Session::lost) {
            self.lose(why);
        }
    }

    /// The way to the connection: direct, or through its tunnel, started again unless it still runs;
    /// none when the tunnel does not start, and its state says why.
    async fn route(&mut self) -> Option<Route> {
        let Some(spec) = self.connection.tunnel() else { return Some(Route::Direct) };
        if let Some(tunnel) = self.tunnel.as_mut()
            && tunnel.is_running()
        {
            return Some(tunnel.route());
        }
        self.send(Incoming::Tunnel(TunnelState::Starting));
        match Tunnel::start(spec).await {
            Ok(tunnel) => {
                self.send(Incoming::Tunnel(TunnelState::Up(tunnel.port())));
                Some(self.tunnel.insert(tunnel).route())
            }
            Err(err) => {
                self.tunnel = None;
                self.send(Incoming::Tunnel(TunnelState::Died(format!("{err:#}"))));
                None
            }
        }
    }

    /// Closes the session a dead tunnel carried; the next statement starts both again.
    fn lose_tunnel(&mut self, died: &Died) {
        self.tunnel = None;
        self.send(Incoming::Tunnel(TunnelState::Died(died.to_string())));
        if self.session.is_some() {
            self.lose(TUNNEL_DIED.to_owned());
        }
    }

    fn lose(&mut self, why: String) {
        self.session = None;
        self.send(Incoming::Closed(why));
    }

    fn send(&self, incoming: Incoming) {
        if self.answers.send(incoming).is_err() {
            tracing::warn!("the loop is gone: an answer is dropped");
        }
    }
}

/// Why `session` ended, once it has; never, when there is none.
async fn closed(session: Option<&Session>) -> String {
    match session {
        Some(session) => session.closed().await,
        None => std::future::pending().await,
    }
}

/// How `tunnel` ended, once it has; never, when there is none.
async fn ended(tunnel: Option<&mut Tunnel>) -> Died {
    match tunnel {
        Some(tunnel) => tunnel.ended().await,
        None => std::future::pending().await,
    }
}

/// Whether a cancel for `run` came before it started; a cancel for an earlier run is dropped.
fn was_cancelled(cancelled: &mut UnboundedReceiver<RunId>, run: RunId) -> bool {
    std::iter::from_fn(|| cancelled.try_recv().ok()).any(|cancelled| cancelled == run)
}

/// Runs `queued`, asking the server to cancel it when its cancel comes; a cancel for another run does nothing.
async fn run_until_answered(
    session: &mut Session,
    writes: WritePolicy,
    queued: Queued,
    cancelled: &mut UnboundedReceiver<RunId>,
) -> Incoming {
    let token = session.cancel_token();
    let run = queued.run;
    let mut answer = pin!(answer_to(session, writes, queued));
    loop {
        tokio::select! {
            answer = &mut answer => return answer,
            Some(cancelled) = cancelled.recv() => {
                if cancelled == run {
                    cancel(&token).await;
                }
            }
        }
    }
}

async fn cancel(token: &CancelToken) {
    if let Err(err) = token.cancel().await {
        tracing::warn!("the cancel request failed: {err:#}");
    }
}

async fn answer_to(session: &mut Session, writes: WritePolicy, queued: Queued) -> Incoming {
    let Queued { run, statement } = queued;
    let admitted = match guard::admit(statement, writes) {
        Ok(admitted) => admitted,
        Err(refusal) => return Incoming::Refused { run, why: refusal.to_string() },
    };
    let started = Instant::now();
    match session.run(&admitted).await {
        Ok(result) => Incoming::Result { run, result, took: started.elapsed() },
        Err(err) => {
            let position = err.downcast_ref::<ServerError>().and_then(|server| server.position);
            Incoming::Failed { run, position, error: format!("{err:#}") }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Entry;
    use crate::db::model::Value;

    const SLEEP: &str = "SELECT pg_sleep(30)";

    fn connection() -> Connection {
        let url = std::env::var("PGBG_TEST_DATABASE_URL").unwrap();
        let entry: Entry = toml::from_str(&format!("tier = \"local\"\nurl = \"{url}\"\n")).unwrap();
        Connection::new("shop", entry).unwrap()
    }

    fn started() -> (Backend, UnboundedReceiver<Incoming>) {
        started_on(connection())
    }

    fn started_on(connection: Connection) -> (Backend, UnboundedReceiver<Incoming>) {
        let (sender, answers) = mpsc::unbounded_channel();
        (Backend::start(connection, sender), answers)
    }

    /// A connection to the test database through a forwarder whose command line carries `marker`, so a test can kill it.
    fn tunnelled(marker: &str) -> Connection {
        let database = url::Url::parse(&std::env::var("PGBG_TEST_DATABASE_URL").unwrap()).unwrap();
        let (host, port) = (database.host_str().unwrap().to_owned(), database.port().unwrap_or(5432));
        let command =
            format!("[\"python3\", \"-c\", {:?}, \"{{port}}\", \"{host}\", \"{port}\", \"{marker}\"]", crate::tunnel::tests::FORWARD);
        let entry: Entry =
            toml::from_str(&format!("tier = \"local\"\nurl = \"{database}\"\ntunnel = {{ command = {command} }}\n")).unwrap();
        Connection::new("shop", entry).unwrap()
    }

    async fn answers_until(answers: &mut UnboundedReceiver<Incoming>, last: impl Fn(&Incoming) -> bool) -> Vec<Incoming> {
        let mut seen = vec![];
        while seen.last().is_none_or(|answer| !last(answer)) {
            seen.push(answers.recv().await.unwrap());
        }
        seen
    }

    fn is_tunnel_death(answer: &Incoming) -> bool {
        matches!(answer, Incoming::Tunnel(TunnelState::Died(_)))
    }

    async fn kill_tunnel_marked(marker: &str) {
        tokio::process::Command::new("pkill").args(["-f", marker]).status().await.unwrap();
    }

    #[tokio::test]
    async fn a_tunnel_that_does_not_start_says_why_and_the_session_does_not_open() {
        let entry: Entry = toml::from_str(
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n\
             tunnel = { command = [\"sh\", \"-c\", \"echo 'bastion.example.com: no route' >&2; exit 255\", \"{port}\"] }\n",
        )
        .unwrap();
        let (_backend, mut answers) = started_on(Connection::new("shop", entry).unwrap());
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::NotOpened(_))).await;
        assert_eq!(seen[0], Incoming::Tunnel(TunnelState::Starting));
        assert!(matches!(&seen[1], Incoming::Tunnel(TunnelState::Died(how)) if how.contains("no route")), "{seen:?}");
        assert_eq!(seen[2], Incoming::NotOpened(TUNNEL_NOT_STARTED.to_owned()));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_tunnel_that_dies_closes_its_session_and_says_why() {
        let marker = "pgbg-test-tunnel-that-dies";
        let (_backend, mut answers) = started_on(tunnelled(marker));
        answers_until(&mut answers, |answer| *answer == Incoming::Opened).await;
        kill_tunnel_marked(marker).await;
        let mut seen = vec![];
        while !seen.iter().any(|answer| matches!(answer, Incoming::Closed(_))) || !seen.iter().any(is_tunnel_death) {
            seen.push(answers.recv().await.unwrap());
        }
        assert!(
            seen.iter().any(|answer| matches!(answer, Incoming::Tunnel(TunnelState::Died(how)) if how.contains("`python3` died"))),
            "{seen:?}"
        );
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_after_the_tunnel_died_starts_it_again() {
        let marker = "pgbg-test-tunnel-started-again";
        let (backend, mut answers) = started_on(tunnelled(marker));
        answers_until(&mut answers, |answer| *answer == Incoming::Opened).await;
        kill_tunnel_marked(marker).await;
        answers_until(&mut answers, is_tunnel_death).await;
        backend.run(RunId::FIRST, Statement::new("SELECT 1"));
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::Result { .. })).await;
        let seen: Vec<Incoming> = seen.into_iter().filter(|answer| !matches!(answer, Incoming::Closed(_))).collect();
        assert_eq!(seen[0], Incoming::Tunnel(TunnelState::Starting));
        assert!(matches!(seen[1], Incoming::Tunnel(TunnelState::Up(_))), "{seen:?}");
        assert_eq!(seen[2], Incoming::Opened);
    }

    async fn next_answer(answers: &mut UnboundedReceiver<Incoming>) -> Incoming {
        loop {
            match answers.recv().await.unwrap() {
                Incoming::Opened => {}
                answer => return answer,
            }
        }
    }

    async fn answer_to(backend: &Backend, answers: &mut UnboundedReceiver<Incoming>, run: RunId, sql: &str) -> Incoming {
        backend.run(run, Statement::new(sql));
        next_answer(answers).await
    }

    async fn on_a_side_session(sql: &str) -> Vec<Vec<Value>> {
        let mut session = Session::open(&connection(), Route::Direct).await.unwrap();
        session.run(&guard::admit(Statement::new(sql), WritePolicy::ReadOnly).unwrap()).await.unwrap().rows
    }

    async fn backend_pid(backend: &Backend, answers: &mut UnboundedReceiver<Incoming>) -> Value {
        let Incoming::Result { result, .. } = answer_to(backend, answers, RunId::FIRST, "SELECT pg_backend_pid()").await else {
            panic!("no pid")
        };
        result.rows[0][0].clone()
    }

    async fn until_sleeping() {
        let is_sleeping = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLEEP}' AND state = 'active'");
        while on_a_side_session(&is_sleeping).await.is_empty() {}
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_after_the_session_died_runs_on_a_new_one() {
        let (backend, mut answers) = started();
        let pid = backend_pid(&backend, &mut answers).await;
        on_a_side_session(&format!("SELECT pg_terminate_backend({pid}, 5000)")).await;
        next_answer(&mut answers).await;
        let after = answer_to(&backend, &mut answers, RunId::FIRST.next(), "SELECT 1").await;
        assert!(matches!(after, Incoming::Result { .. }), "{after:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_that_dies_while_idle_says_so_at_once() {
        let (backend, mut answers) = started();
        let pid = backend_pid(&backend, &mut answers).await;
        on_a_side_session(&format!("SELECT pg_terminate_backend({pid}, 5000)")).await;
        let answer = next_answer(&mut answers).await;
        assert!(matches!(&answer, Incoming::Closed(why) if why.contains("terminating connection")), "{answer:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cancel_that_comes_before_its_run_keeps_it_from_the_server() {
        let (backend, mut answers) = started();
        backend.run(RunId::FIRST, Statement::new(SLEEP));
        backend.cancel(RunId::FIRST);
        let answer = next_answer(&mut answers).await;
        assert_eq!(answer, Incoming::Failed { run: RunId::FIRST, error: CANCELLED_BEFORE_SENT.to_owned(), position: None });
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cancel_for_an_earlier_run_leaves_the_running_one_alone() {
        let (backend, mut answers) = started();
        backend_pid(&backend, &mut answers).await;
        backend.cancel(RunId::FIRST);
        let answer = answer_to(&backend, &mut answers, RunId::FIRST.next(), "SELECT pg_sleep(0.2)").await;
        assert!(matches!(answer, Incoming::Result { .. }), "{answer:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn stopping_after_a_cancel_waits_for_the_cancelled_run_to_end() {
        let (backend, mut answers) = started();
        backend.run(RunId::FIRST, Statement::new(SLEEP));
        until_sleeping().await;
        backend.cancel(RunId::FIRST);
        backend.stop().await;
        let answer = next_answer(&mut answers).await;
        assert!(matches!(&answer, Incoming::Failed { error, .. } if error.contains("canceling statement")), "{answer:?}");
    }
}
