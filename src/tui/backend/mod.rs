//! What the runner calls: a session for each connection opened, each served by a task that owns it, and the saved queries.
use super::app::{Action, Answer, Incoming, RunId, TunnelState};
use crate::connection::{Connection, ConnectionName, WritePolicy};
use crate::db::model::{CancelToken, Route, ServerError, Statement};
use crate::db::{Pages, Session, Started, guard};
use crate::history::{Entry, History, Outcome, Store};
use crate::query::{Query, Roots};
use crate::tunnel::{Died, Tunnel};
use chrono::Utc;
use config::ConfigFile;
use futures_util::future;
use paging::{PAGES_IDLE_LIMIT, Paging, serve_pages};
use queries::QueryFiles;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::pin;
use std::time::Duration;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

mod clipboard;
mod config;
mod paging;
mod queries;

/// How long closing a session waits for its task to finish a cancel before it aborts the task, so the terminal comes back soon.
const STOP_WAIT: Duration = Duration::from_secs(2);
/// How long closing a session waits for the server to take a cancel request before it aborts the session task.
const CANCEL_WAIT: Duration = Duration::from_secs(1);
const CANCELLED_BEFORE_SENT: &str = "cancelled before it was sent";
const TUNNEL_NOT_STARTED: &str = "its tunnel did not start";
const TUNNEL_DIED: &str = "its tunnel died";

/// The way to each session task, one per connection opened, kept until pgbg quits; answers go out on the loop's channel.
#[derive(Debug)]
pub(super) struct Backend {
    sessions: BTreeMap<ConnectionName, SessionLink>,
    answers: UnboundedSender<Answer>,
    queries: QueryFiles,
    config: ConfigFile,
    /// Where each run is added and `ctrl-r` reads from; none keeps no history.
    history: Option<Store>,
    /// How long a result's portal waits for its next page.
    idle_limit: Duration,
}

/// The way to one session task: jobs and cancels in.
#[derive(Debug)]
struct SessionLink {
    jobs: UnboundedSender<Job>,
    cancels: UnboundedSender<RunId>,
    /// What the task has on the server and no cancel reached yet, shared with the task; an abort cancels it first.
    running: watch::Receiver<Option<CancelToken>>,
    task: JoinHandle<()>,
}

/// What the session task is asked to do, in turn.
#[derive(Debug)]
enum Job {
    Run(Queued),
    /// Fetch the next page of this run's result, while its portal is open.
    NextPage {
        run: RunId,
    },
    /// Open the session again, unless it is open.
    Open,
}

/// A statement waiting for the session, and its run.
#[derive(Debug)]
struct Queued {
    run: RunId,
    statement: Statement,
}

impl Backend {
    /// The backend with no session yet; saved queries are read under `roots`, the config is changed at `config`.
    pub(super) fn new(roots: Roots, config: PathBuf, answers: UnboundedSender<Answer>) -> Self {
        let queries = QueryFiles { roots, answers: answers.clone() };
        let config = ConfigFile { path: config, answers: answers.clone() };
        Self { sessions: BTreeMap::new(), answers, queries, config, history: None, idle_limit: PAGES_IDLE_LIMIT }
    }

    /// The backend adding each run to `history`.
    pub(super) fn with_history(self, history: Option<Store>) -> Self {
        Self { history, ..self }
    }

    /// The file of `query` for the user's editor; blocks on the file system, so the loop calls it with the terminal lent.
    pub(super) fn query_file(&self, query: &Query) -> anyhow::Result<PathBuf> {
        self.queries.file(query)
    }

    /// Hands `action` over without waiting on it; the loop never awaits the database.
    pub(super) fn dispatch(&mut self, action: Action) {
        match action {
            Action::Open(connection) => self.open(connection),
            Action::Run { connection, run, statement } => self.run(&connection, run, statement),
            Action::NextPage { connection, run } => self.next_page(&connection, run),
            Action::Cancel { connection, run } => self.cancel(&connection, run),
            Action::ListQueries => self.queries.list(),
            Action::OpenQuery { connection, query } => self.queries.open(connection, query),
            Action::SaveQuery { connection, query, text } => self.queries.save(connection, query, text),
            Action::SaveNewQuery { connection, path, text } => self.queries.save_new(connection, path, text),
            Action::EditOutside { query, .. } => {
                tracing::warn!("{} goes to the loop, which lends the terminal, not to the backend", query.path);
            }
            Action::Close(connection) => self.close(&connection),
            Action::Reopen(connection) => {
                self.close(connection.name());
                self.open(connection);
            }
            Action::RemoveConnection { id, connection } => self.config.remove(id, connection),
            Action::SaveConnection(save) => self.config.save(save),
            Action::ListHistory(connection) => self.list_history(connection),
            Action::Copy { text, done } => clipboard::copy(text, done, self.answers.clone()),
        }
    }

    /// Reads the history of `connection` on a blocking thread; one that cannot be read is logged and comes back empty.
    fn list_history(&self, connection: ConnectionName) {
        let (history, answers) = (self.history.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || {
            let read = history.map(|store| store.read(&connection)).transpose();
            let entries = read.unwrap_or_else(|err| {
                tracing::warn!("the history of `{connection}` was not read: {err}");
                None
            });
            send(&answers, Answer::History { connection, entries: entries.map(History::distinct).unwrap_or_default() });
        });
    }

    /// Stops the session task of `connection` in the background, which drops its session and kills its tunnel.
    fn close(&mut self, connection: &ConnectionName) {
        if let Some(session) = self.sessions.remove(connection) {
            tokio::spawn(session.stop());
        }
    }

    /// Starts the task that opens a session on `connection`, or asks the one that serves it to open it again unless it is open.
    fn open(&mut self, connection: Connection) {
        if let Some(session) = self.sessions.get(connection.name()) {
            session.send(Job::Open);
            return;
        }
        let name = connection.name().clone();
        let (cancels, cancelled) = mpsc::unbounded_channel();
        let (on_server, running) = watch::channel(None);
        let (answers, history, idle_limit) = (self.answers.clone(), self.history.clone(), self.idle_limit);
        let on_server = OnServer(on_server);
        let task = SessionTask { connection, tunnel: None, session: None, cancelled, on_server, answers, history, idle_limit };
        _ = self.sessions.insert(name, SessionLink::start(task, cancels, running));
    }

    /// Queues `statement` on the session of `connection`; its answer comes back as an `Answer`.
    fn run(&self, connection: &ConnectionName, run: RunId, statement: Statement) {
        let Some(session) = self.sessions.get(connection) else {
            tracing::warn!("no session for `{connection}`: the statement is dropped");
            return;
        };
        session.send(Job::Run(Queued { run, statement }));
    }

    /// Asks the session of `connection` for the next page of `run`; nothing comes when its portal is closed.
    fn next_page(&self, connection: &ConnectionName, run: RunId) {
        if let Some(session) = self.sessions.get(connection) {
            session.send(Job::NextPage { run });
        }
    }

    /// Cancels `run` on the session of `connection`; see `SessionLink::cancel`.
    fn cancel(&self, connection: &ConnectionName, run: RunId) {
        let Some(session) = self.sessions.get(connection) else {
            tracing::warn!("no session for `{connection}`: the cancel is dropped");
            return;
        };
        session.cancel(run);
    }

    /// Stops every session task at once, so quitting waits `STOP_WAIT` at most.
    pub(super) async fn stop(self) {
        future::join_all(self.sessions.into_values().map(SessionLink::stop)).await;
    }
}

impl SessionLink {
    /// Starts `task`, which opens its session and runs each statement in turn; its cancels come through `cancels`, and
    /// `running` says what it has on the server.
    fn start(task: SessionTask, cancels: UnboundedSender<RunId>, running: watch::Receiver<Option<CancelToken>>) -> Self {
        let (jobs, queue) = mpsc::unbounded_channel();
        Self { jobs, cancels, running, task: tokio::spawn(task.serve(queue)) }
    }

    /// Queues `job` behind the one running.
    fn send(&self, job: Job) {
        if self.jobs.send(job).is_err() {
            tracing::warn!("the session task is gone: a statement or an open is dropped");
        }
    }

    /// Cancels `run` if the session is on it, or keeps it from the server if it has not started; a later run is never touched.
    fn cancel(&self, run: RunId) {
        if self.cancels.send(run).is_err() {
            tracing::warn!("the session task is gone: the cancel is dropped");
        }
    }

    /// Cancels the running statement and lets the session task finish it, waiting `STOP_WAIT` at most;
    /// past it, the server is asked to cancel a statement or a page whose cancel did not reach it, and the task is
    /// aborted, which cuts the session's link.
    async fn stop(self) {
        let Self { jobs, cancels, running, mut task } = self;
        drop((jobs, cancels));
        match tokio::time::timeout(STOP_WAIT, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!("the session task failed: {err}"),
            Err(_) => {
                tracing::warn!("the session task did not stop within {STOP_WAIT:?}: it is aborted");
                let on_server = running.borrow().clone();
                if let Some(token) = on_server
                    && let Err(err) = token.cancel_within(CANCEL_WAIT).await
                {
                    tracing::warn!("the statement may still run on the server: {err:#}");
                }
                task.abort();
            }
        }
    }
}

/// What woke the session task.
enum Woken {
    Queued(Option<Job>),
    Closed(String),
    TunnelEnded(Died),
}

/// The task that owns the session and its tunnel: it opens them, runs each statement in turn, and says when they are lost.
struct SessionTask {
    connection: Connection,
    tunnel: Option<Tunnel>,
    session: Option<Session>,
    cancelled: UnboundedReceiver<RunId>,
    on_server: OnServer,
    answers: UnboundedSender<Answer>,
    history: Option<Store>,
    idle_limit: Duration,
}

/// Holds the cancel token of what a session task has on the server, a statement or a page, for its link's abort.
#[derive(Debug)]
struct OnServer(watch::Sender<Option<CancelToken>>);

impl OnServer {
    fn starts(&self, token: &CancelToken) {
        self.0.send_replace(Some(token.clone()));
    }

    fn ended(&self) {
        self.0.send_replace(None);
    }

    /// Cancels what is on the server as the session closes; once the server took it, an abort has nothing to cancel.
    async fn cancel_as_closing(&self, token: &CancelToken) {
        if cancel(token).await {
            self.ended();
        }
    }
}

/// A run's answer for the app, what came of it for the history, and its portal while more pages wait in it.
struct Answered<'s> {
    incoming: Incoming,
    outcome: Outcome,
    rest: Option<Pages<'s>>,
}

impl SessionTask {
    /// Opens the session at once, and again before a statement when the last try failed or the session was lost;
    /// once pgbg quits, a statement still queued is never sent.
    async fn serve(mut self, mut queue: UnboundedReceiver<Job>) {
        self.open_or_fail_queued(&mut queue).await;
        let mut pending = None;
        loop {
            let woken = match pending.take() {
                Some(woken) => woken,
                None => self.woken(&mut queue).await,
            };
            match woken {
                Woken::Queued(Some(_)) if queue.is_closed() => {}
                Woken::Queued(Some(Job::Run(queued))) => pending = self.run(queued, &mut queue).await,
                Woken::Queued(Some(Job::NextPage { .. })) => {}
                Woken::Queued(Some(Job::Open)) if self.session.is_none() => self.open_or_fail_queued(&mut queue).await,
                Woken::Queued(Some(Job::Open)) => {}
                Woken::Queued(None) => break,
                Woken::Closed(why) => self.lose(why),
                Woken::TunnelEnded(died) => self.lose_tunnel(&died),
            }
        }
        self.stop().await;
    }

    async fn woken(&mut self, queue: &mut UnboundedReceiver<Job>) -> Woken {
        let Self { session, tunnel, .. } = self;
        tokio::select! {
            queued = queue.recv() => Woken::Queued(queued),
            why = closed(session.as_ref()) => Woken::Closed(why),
            died = ended(tunnel.as_mut()) => Woken::TunnelEnded(died),
        }
    }

    /// Opens the session and says how it went; the error, when it did not open.
    async fn open(&mut self) -> Result<(), String> {
        match self.connect().await {
            Ok(session) => {
                self.session = Some(session);
                self.send(Incoming::Opened);
                Ok(())
            }
            Err(error) => {
                self.session = None;
                self.send(Incoming::NotOpened(error.clone()));
                Err(error)
            }
        }
    }

    async fn connect(&mut self) -> Result<Session, String> {
        let route = self.route().await.ok_or_else(|| TUNNEL_NOT_STARTED.to_owned())?;
        Session::open(&self.connection, route).await.map_err(|err| format!("{err:#}"))
    }

    /// Opens the session; when it does not open, every statement sent meanwhile fails with it, so a later open never sends one.
    async fn open_or_fail_queued(&mut self, queue: &mut UnboundedReceiver<Job>) {
        let Err(error) = self.open().await else { return };
        while let Ok(job) = queue.try_recv() {
            if let Job::Run(queued) = job {
                self.send(Incoming::Failed { run: queued.run, error: error.clone(), position: None });
            }
        }
    }

    /// Runs `queued`, then serves the pages of its result while its portal is open; returns what woke the task meanwhile
    /// that it still has to act on.
    async fn run(&mut self, queued: Queued, queue: &mut UnboundedReceiver<Job>) -> Option<Woken> {
        let (statement, started_at, clock) = (queued.statement.as_str().to_owned(), Utc::now(), Instant::now());
        if self.session.is_none()
            && let Err(error) = self.open().await
        {
            self.send(Incoming::Failed { run: queued.run, error, position: None });
            record(
                self.history.as_ref(),
                Entry { statement, connection: self.connection.name().clone(), started_at, took_ms: 0, outcome: Outcome::LinkError },
            );
            return None;
        }
        let session = self.session.as_mut()?;
        let run = queued.run;
        if was_cancelled(&mut self.cancelled, run) {
            self.send(Incoming::Failed { run, error: CANCELLED_BEFORE_SENT.to_owned(), position: None });
            return None;
        }
        let woken = {
            let token = session.cancel_token();
            let Answered { incoming, outcome, rest } =
                run_until_answered(session, self.connection.writes(), queued, &mut self.cancelled, &self.on_server).await;
            let took_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
            record(self.history.as_ref(), Entry { statement, connection: self.connection.name().clone(), started_at, took_ms, outcome });
            send(&self.answers, Answer::Session { connection: self.connection.name().clone(), incoming });
            if let Some(pages) = rest {
                let tunnel = self.tunnel.as_mut();
                let (connection, answers, idle_limit, on_server) =
                    (self.connection.name(), &self.answers, self.idle_limit, &self.on_server);
                let cancelled = &mut self.cancelled;
                serve_pages(Paging { pages, run, token, queue, cancelled, on_server, tunnel, connection, answers, idle_limit }).await
            } else {
                None
            }
        };
        self.lose_if_lost(woken)
    }

    /// Closes a session that can run no more statements, and drops a pending close it already says.
    fn lose_if_lost(&mut self, woken: Option<Woken>) -> Option<Woken> {
        let Some(why) = self.session.as_ref().and_then(Session::lost) else { return woken };
        self.lose(why);
        woken.filter(|woken| !matches!(woken, Woken::Closed(_)))
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
        self.tunnel = None;
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

    /// Closes the session, then stops its tunnel, warning when its port still answers.
    async fn stop(mut self) {
        self.session = None;
        if let Some(tunnel) = self.tunnel.as_mut()
            && let Err(err) = tunnel.stop().await
        {
            tracing::warn!("{err:#}");
        }
    }

    fn lose(&mut self, why: String) {
        self.session = None;
        self.send(Incoming::Closed(why));
    }

    fn send(&self, incoming: Incoming) {
        send(&self.answers, Answer::Session { connection: self.connection.name().clone(), incoming });
    }
}

/// Hands `answer` to the loop; one that comes after the loop ended is dropped with a warning.
fn send(answers: &UnboundedSender<Answer>, answer: Answer) {
    if answers.send(answer).is_err() {
        tracing::warn!("the loop is gone: an answer is dropped");
    }
}

/// Adds `entry` to the history in the background: the next statement never waits on the disk.
fn record(history: Option<&Store>, entry: Entry) {
    if let Some(store) = history {
        drop(store.record(entry));
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

/// Whether `run` was cancelled before it started, by its own cancel or by its session closing; a cancel for an
/// earlier run is dropped.
fn was_cancelled(cancelled: &mut UnboundedReceiver<RunId>, run: RunId) -> bool {
    loop {
        match cancelled.try_recv() {
            Ok(cancelled) if cancelled == run => return true,
            Ok(_) => {}
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => return true,
        }
    }
}

/// Runs `queued`, asking the server to cancel it when its cancel comes or its session closes; a cancel for another
/// run does nothing. `on_server` holds its token while it runs.
async fn run_until_answered<'s>(
    session: &'s mut Session,
    writes: WritePolicy,
    queued: Queued,
    cancelled: &mut UnboundedReceiver<RunId>,
    on_server: &OnServer,
) -> Answered<'s> {
    let token = session.cancel_token();
    let run = queued.run;
    on_server.starts(&token);
    let mut answer = pin!(answer_to(session, writes, queued));
    let mut is_closing = false;
    let answered = loop {
        tokio::select! {
            answer = &mut answer => break answer,
            cancel_of = cancelled.recv(), if !is_closing => match cancel_of {
                Some(cancelled) if cancelled == run => _ = cancel(&token).await,
                Some(_) => {}
                None => {
                    is_closing = true;
                    on_server.cancel_as_closing(&token).await;
                }
            },
        }
    };
    on_server.ended();
    answered
}

/// Whether the server took the cancel request.
async fn cancel(token: &CancelToken) -> bool {
    let sent = token.cancel().await;
    if let Err(err) = &sent {
        tracing::warn!("the cancel request failed: {err:#}");
    }
    sent.is_ok()
}

async fn answer_to(session: &mut Session, writes: WritePolicy, queued: Queued) -> Answered<'_> {
    let Queued { run, statement } = queued;
    let admitted = match guard::admit(statement, writes) {
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
    use crate::db::model::Value;
    use crate::fixtures;

    const SLEEP: &str = "SELECT pg_sleep(30)";

    fn connection() -> Connection {
        fixtures::test_database_as("shop")
    }

    fn shop() -> ConnectionName {
        connection().name().clone()
    }

    fn incoming(answer: Answer) -> Incoming {
        let Answer::Session { incoming, .. } = answer else { panic!("not a session's answer: {answer:?}") };
        incoming
    }

    fn started() -> (Backend, UnboundedReceiver<Answer>) {
        started_on(connection())
    }

    fn started_on(connection: Connection) -> (Backend, UnboundedReceiver<Answer>) {
        let (sender, answers) = mpsc::unbounded_channel();
        let mut backend = Backend::new(Roots::default(), PathBuf::from("config.toml"), sender);
        backend.open(connection);
        (backend, answers)
    }

    /// An empty table `name` in the test database, made again from scratch.
    async fn empty_table(name: &str) -> String {
        let mut session = Session::open(&connection(), Route::Direct).await.unwrap();
        for sql in [format!("DROP TABLE IF EXISTS {name}"), format!("CREATE TABLE {name} (n int)")] {
            session.run(&guard::admit(Statement::new(&sql), WritePolicy::Allowed).unwrap()).await.unwrap();
        }
        name.to_owned()
    }

    fn is_answer_to(run: RunId, answer: &Incoming) -> bool {
        matches!(answer, Incoming::Result { run: of, .. } | Incoming::Refused { run: of, .. } | Incoming::Failed { run: of, .. } if *of == run)
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_sent_while_the_session_fails_to_open_never_reaches_the_server() {
        let flags = tempfile::tempdir().unwrap();
        let started_once = flags.path().join("started");
        let fails_the_first_time = format!("[ -e {0:?} ] || {{ touch {0:?}; sleep 3; exit 1; }}", started_once.display().to_string());
        let table = empty_table("pgbg_sent_while_failing_to_open").await;
        let (backend, mut answers) =
            started_on(fixtures::tunnelled_after("pgbg-test-fails-once", &fails_the_first_time, WritePolicy::Allowed));
        backend.run(&shop(), RunId::FIRST, Statement::new(format!("INSERT INTO {table} VALUES (1)")));
        answers_until(&mut answers, |answer| is_answer_to(RunId::FIRST, answer)).await;
        let rows = on_a_side_session(&format!("SELECT n FROM {table}")).await;
        assert_eq!(rows.len(), 0, "{rows:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_still_queued_when_the_backend_stops_never_reaches_the_server() {
        let table = empty_table("pgbg_queued_at_stop").await;
        let (backend, _answers) = started_on(fixtures::tunnelled_after("pgbg-test-slow-start", "sleep 1", WritePolicy::Allowed));
        backend.run(&shop(), RunId::FIRST, Statement::new(format!("INSERT INTO {table} VALUES (1)")));
        backend.stop().await;
        let rows = on_a_side_session(&format!("SELECT n FROM {table}")).await;
        assert_eq!(rows.len(), 0, "{rows:?}");
    }

    async fn answers_until(answers: &mut UnboundedReceiver<Answer>, last: impl Fn(&Incoming) -> bool) -> Vec<Incoming> {
        let mut seen = vec![];
        while seen.last().is_none_or(|answer| !last(answer)) {
            seen.push(incoming(answers.recv().await.unwrap()));
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
    #[ignore = "starts a tunnel"]
    async fn a_tunnel_that_does_not_start_says_why_and_the_session_does_not_open() {
        let (_backend, mut answers) = started_on(fixtures::connection(
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n\
             tunnel = { command = [\"sh\", \"-c\", \"echo 'bastion.example.com: no route' >&2; exit 255\", \"{port}\"] }\n",
        ));
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::NotOpened(_))).await;
        assert_eq!(seen[0], Incoming::Tunnel(TunnelState::Starting));
        assert!(matches!(&seen[1], Incoming::Tunnel(TunnelState::Died(how)) if how.contains("no route")), "{seen:?}");
        assert_eq!(seen[2], Incoming::NotOpened(TUNNEL_NOT_STARTED.to_owned()));
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn opening_again_a_connection_that_failed_tries_again() {
        let shop = fixtures::connection(
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n\
             tunnel = { command = [\"sh\", \"-c\", \"exit 255\", \"{port}\"] }\n",
        );
        let (mut backend, mut answers) = started_on(shop.clone());
        answers_until(&mut answers, |answer| matches!(answer, Incoming::NotOpened(_))).await;
        backend.open(shop);
        let again = answers_until(&mut answers, |answer| matches!(answer, Incoming::NotOpened(_))).await;
        assert_eq!(again[0], Incoming::Tunnel(TunnelState::Starting));
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_statement_sent_while_the_tunnel_fails_to_start_fails_with_it() {
        let shop = fixtures::connection(
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n\
             tunnel = { command = [\"sh\", \"-c\", \"exit 255\", \"{port}\"] }\n",
        );
        let name = shop.name().clone();
        let (backend, mut answers) = started_on(shop);
        backend.run(&name, RunId::FIRST, Statement::new("SELECT 1"));
        let seen = answers_until(&mut answers, |answer| is_answer_to(RunId::FIRST, answer)).await;
        let failed = Incoming::Failed { run: RunId::FIRST, error: TUNNEL_NOT_STARTED.to_owned(), position: None };
        assert_eq!(seen.last(), Some(&failed));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_tunnel_that_dies_closes_its_session_and_says_why() {
        let marker = "pgbg-test-tunnel-that-dies";
        let (_backend, mut answers) = started_on(fixtures::tunnelled(marker));
        answers_until(&mut answers, |answer| *answer == Incoming::Opened).await;
        kill_tunnel_marked(marker).await;
        let mut seen = vec![];
        while !seen.iter().any(|answer| matches!(answer, Incoming::Closed(_))) || !seen.iter().any(is_tunnel_death) {
            seen.push(incoming(answers.recv().await.unwrap()));
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
        let (backend, mut answers) = started_on(fixtures::tunnelled(marker));
        answers_until(&mut answers, |answer| *answer == Incoming::Opened).await;
        kill_tunnel_marked(marker).await;
        answers_until(&mut answers, is_tunnel_death).await;
        backend.run(&shop(), RunId::FIRST, Statement::new("SELECT 1"));
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::Result { .. })).await;
        let seen: Vec<Incoming> = seen.into_iter().filter(|answer| !matches!(answer, Incoming::Closed(_))).collect();
        assert_eq!(seen[0], Incoming::Tunnel(TunnelState::Starting));
        assert!(matches!(seen[1], Incoming::Tunnel(TunnelState::Up(_))), "{seen:?}");
        assert_eq!(seen[2], Incoming::Opened);
    }

    async fn next_answer(answers: &mut UnboundedReceiver<Answer>) -> Incoming {
        loop {
            match incoming(answers.recv().await.unwrap()) {
                Incoming::Opened => {}
                answer => return answer,
            }
        }
    }

    async fn answer_to(backend: &Backend, answers: &mut UnboundedReceiver<Answer>, run: RunId, sql: &str) -> Incoming {
        backend.run(&shop(), run, Statement::new(sql));
        next_answer(answers).await
    }

    async fn on_a_side_session(sql: &str) -> Vec<Vec<Value>> {
        let mut session = Session::open(&connection(), Route::Direct).await.unwrap();
        session.run(&guard::admit(Statement::new(sql), WritePolicy::ReadOnly).unwrap()).await.unwrap().rows().to_vec()
    }

    async fn backend_pid(backend: &Backend, answers: &mut UnboundedReceiver<Answer>) -> Value {
        let Incoming::Result { result, .. } = answer_to(backend, answers, RunId::FIRST, "SELECT pg_backend_pid()").await else {
            panic!("no pid")
        };
        result.rows()[0][0].clone()
    }

    async fn until_sleeping() {
        let is_sleeping = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLEEP}' AND state = 'active'");
        while on_a_side_session(&is_sleeping).await.is_empty() {}
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_answer_names_the_connection_whose_session_gave_it() {
        let (mut backend, mut answers) = started();
        let copy = fixtures::test_database_as("shop-copy");
        backend.open(copy.clone());
        backend.run(copy.name(), RunId::FIRST, Statement::new("SELECT 1"));
        let connection = loop {
            if let Answer::Session { connection, incoming: Incoming::Result { .. } } = answers.recv().await.unwrap() {
                break connection;
            }
        };
        assert_eq!(&connection, copy.name());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn opening_a_connection_twice_keeps_its_one_session() {
        let (mut backend, mut answers) = started();
        let pid = backend_pid(&backend, &mut answers).await;
        backend.open(connection());
        let again = answer_to(&backend, &mut answers, RunId::FIRST.next(), "SELECT pg_backend_pid()").await;
        assert!(matches!(again, Incoming::Result { result, .. } if result.rows()[0][0] == pid));
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
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP));
        backend.cancel(&shop(), RunId::FIRST);
        let answer = next_answer(&mut answers).await;
        assert_eq!(answer, Incoming::Failed { run: RunId::FIRST, error: CANCELLED_BEFORE_SENT.to_owned(), position: None });
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cancel_for_an_earlier_run_leaves_the_running_one_alone() {
        let (backend, mut answers) = started();
        backend_pid(&backend, &mut answers).await;
        backend.cancel(&shop(), RunId::FIRST);
        let answer = answer_to(&backend, &mut answers, RunId::FIRST.next(), "SELECT pg_sleep(0.2)").await;
        assert!(matches!(answer, Incoming::Result { .. }), "{answer:?}");
    }

    async fn until_active(sql: &str) {
        let is_active = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{sql}' AND state = 'active'");
        while on_a_side_session(&is_active).await.is_empty() {}
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn closing_a_session_cancels_its_running_statement() {
        const SLEEP_AT_CLOSE: &str = "SELECT pg_sleep(31)";
        let (mut backend, mut answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP_AT_CLOSE));
        until_active(SLEEP_AT_CLOSE).await;
        backend.close(&shop());
        let answer = next_answer(&mut answers).await;
        assert!(matches!(&answer, Incoming::Failed { error, .. } if error.contains("canceling statement")), "{answer:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_write_running_when_its_session_closes_never_commits() {
        let table = empty_table("pgbg_write_at_close").await;
        let (mut backend, mut answers) = started_on(fixtures::test_database("local", "writes = \"allowed\"\n"));
        let insert = format!("INSERT INTO {table} SELECT 1 FROM pg_sleep(32)");
        backend.run(&shop(), RunId::FIRST, Statement::new(&insert));
        until_active(&insert).await;
        backend.close(&shop());
        next_answer(&mut answers).await;
        let rows = on_a_side_session(&format!("SELECT n FROM {table}")).await;
        assert_eq!(rows.len(), 0, "{rows:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn stopping_after_a_cancel_waits_for_the_cancelled_run_to_end() {
        let (backend, mut answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP));
        until_sleeping().await;
        backend.cancel(&shop(), RunId::FIRST);
        backend.stop().await;
        let answer = next_answer(&mut answers).await;
        assert!(matches!(&answer, Incoming::Failed { error, .. } if error.contains("canceling statement")), "{answer:?}");
    }
}
