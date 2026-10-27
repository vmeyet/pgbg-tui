//! One session task per connection opened: it owns the session and its tunnel, runs each statement in turn, takes
//! cancels by `RunId`, and stops a statement as every stop does.
use super::paging::{Paging, serve_pages};
use super::schema::SchemaReader;
use crate::connection::Connection;
use crate::db::model::Route;
use crate::db::{NotOpened, Opened, Session, TunnelStep, open_along};
use crate::history::{Entry, Outcome, Store};
use crate::tui::app::{Incoming, TunnelState};
use crate::tunnel::{Died, Tunnel};
use chrono::Utc;
use run::{Answered, CANCELLED_BEFORE_SENT, run_until_answered};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;
use tokio::time::Instant;

mod link;
mod run;

pub(super) use link::{Job, Queued, SessionAnswers, SessionLink};
pub(super) use run::{Cancels, OnServer, warn_if_still_running};

const TUNNEL_NOT_STARTED: &str = "its tunnel did not start";
const TUNNEL_DIED: &str = "its tunnel died";

/// What woke the session task.
pub(super) enum Woken {
    Queued(Option<Job>),
    Closed(String),
    TunnelEnded(Died),
}

/// The task that owns the session and its tunnel: it opens them, runs each statement in turn, and says when they are lost.
struct SessionTask {
    connection: Connection,
    tunnel: Option<Tunnel>,
    session: Option<Session>,
    /// The route of the open session, for the schema reader; none while it is not open.
    route: watch::Sender<Option<Route>>,
    cancelled: Cancels,
    on_server: OnServer,
    answers: SessionAnswers,
    history: Option<Store>,
    idle_limit: Duration,
}

impl SessionTask {
    /// Opens the session at once, and again before a statement when the last try failed or the session was lost;
    /// once pgbg quits, a statement still queued is never sent.
    async fn serve(mut self, queue: UnboundedReceiver<Job>, reader: SchemaReader) {
        tokio::select! {
            () = self.serve_jobs(queue) => {}
            () = reader.serve() => {}
        }
        self.stop().await;
    }

    async fn serve_jobs(&mut self, mut queue: UnboundedReceiver<Job>) {
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
                self.set_session(Some(session));
                self.send(Incoming::Opened);
                Ok(())
            }
            Err(error) => {
                self.set_session(None);
                self.send(Incoming::NotOpened(error.clone()));
                Err(error)
            }
        }
    }

    /// Opens the session along the tunnel kept, or one started again, as `pgbg run` does; the tunnel stays for the
    /// next try when the login fails, and its state goes out as it changes.
    async fn connect(&mut self) -> Result<Session, String> {
        let answers = &self.answers;
        let on_tunnel = |step| answers.say(Incoming::Tunnel(tunnel_state(step)));
        match open_along(&self.connection, self.tunnel.take(), on_tunnel).await {
            Ok(Opened { session, tunnel }) => {
                self.tunnel = tunnel;
                Ok(session)
            }
            Err(NotOpened::Tunnel(err)) => {
                self.send(Incoming::Tunnel(TunnelState::Died(format!("{err:#}"))));
                Err(TUNNEL_NOT_STARTED.to_owned())
            }
            Err(NotOpened::Login { error, tunnel }) => {
                self.tunnel = tunnel.map(|tunnel| *tunnel);
                Err(format!("{error:#}"))
            }
        }
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
        if self.cancelled.was_cancelled(run) {
            self.send(Incoming::Failed { run, error: CANCELLED_BEFORE_SENT.to_owned(), position: None });
            return None;
        }
        let woken = {
            let token = session.cancel_token();
            let Answered { incoming, outcome, rest } =
                run_until_answered(session, self.connection.writes(), queued, &mut self.cancelled, &self.on_server).await;
            let took_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
            record(self.history.as_ref(), Entry { statement, connection: self.connection.name().clone(), started_at, took_ms, outcome });
            self.answers.say(incoming);
            if let Some(pages) = rest {
                let (tunnel, cancelled) = (self.tunnel.as_mut(), &mut self.cancelled);
                let (answers, idle_limit, on_server) = (&self.answers, self.idle_limit, &self.on_server);
                let paging = Paging { pages, run, token, queue, cancelled, on_server, tunnel, answers, idle_limit };
                serve_pages(paging).await
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
        self.set_session(None);
        self.send(Incoming::Closed(why));
    }

    /// Keeps `session`, and tells the schema reader the route it takes.
    fn set_session(&mut self, session: Option<Session>) {
        let route = session.as_ref().map(|_| self.tunnel.as_ref().map_or(Route::Direct, Tunnel::route));
        self.session = session;
        self.route.send_replace(route);
    }

    fn send(&self, incoming: Incoming) {
        self.answers.say(incoming);
    }
}

fn tunnel_state(step: TunnelStep) -> TunnelState {
    match step {
        TunnelStep::Starting => TunnelState::Starting,
        TunnelStep::Up(port) => TunnelState::Up(port),
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
pub(super) async fn ended(tunnel: Option<&mut Tunnel>) -> Died {
    match tunnel {
        Some(tunnel) => tunnel.ended().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::super::Backend;
    use super::run::STOPPED_WITH_ITS_SESSION;
    use super::*;
    use crate::connection::{ConnectionName, WritePolicy};
    use crate::db::guard;
    use crate::db::model::Statement;
    use crate::db::model::{Params, Route, Value};
    use crate::fixtures;
    use crate::query::Roots;
    use crate::query::params::ParamInput;
    use crate::tui::app::{Answer, RunId, SessionId};
    use std::path::PathBuf;
    use tokio::sync::mpsc;

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
        backend.open(connection, SessionId::FIRST);
        (backend, answers)
    }

    /// An empty table `name` in the test database, made again from scratch.
    async fn empty_table(name: &str) -> String {
        on_a_side_session_writing(&format!("DROP TABLE IF EXISTS {name}")).await;
        on_a_side_session_writing(&format!("CREATE TABLE {name} (n int)")).await;
        name.to_owned()
    }

    async fn on_a_side_session_writing(sql: &str) {
        let mut session = Session::open(&connection(), Route::Direct).await.unwrap();
        session.run(&guard::admit(Statement::new(sql), Params::default(), WritePolicy::Allowed).unwrap()).await.unwrap();
    }

    fn is_answer_to(run: RunId, answer: &Incoming) -> bool {
        matches!(answer, Incoming::Result { run: of, .. } | Incoming::Refused { run: of, .. } | Incoming::Failed { run: of, .. } if *of == run)
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_sent_while_the_session_fails_to_open_never_reaches_the_server() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let flags = tempfile::tempdir().unwrap();
        let started_once = flags.path().join("started");
        let fails_the_first_time = format!("[ -e {0:?} ] || {{ touch {0:?}; sleep 3; exit 1; }}", started_once.display().to_string());
        let table = empty_table("pgbg_sent_while_failing_to_open").await;
        let (backend, mut answers) =
            started_on(fixtures::tunnelled_after("pgbg-test-fails-once", &fails_the_first_time, WritePolicy::Allowed));
        backend.run(&shop(), RunId::FIRST, Statement::new(format!("INSERT INTO {table} VALUES (1)")), ParamInput::default());
        answers_until(&mut answers, |answer| is_answer_to(RunId::FIRST, answer)).await;
        let rows = on_a_side_session(&format!("SELECT n FROM {table}")).await;
        assert_eq!(rows.len(), 0, "{rows:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_still_queued_when_the_backend_stops_never_reaches_the_server() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let table = empty_table("pgbg_queued_at_stop").await;
        let (backend, _answers) = started_on(fixtures::tunnelled_after("pgbg-test-slow-start", "sleep 1", WritePolicy::Allowed));
        backend.run(&shop(), RunId::FIRST, Statement::new(format!("INSERT INTO {table} VALUES (1)")), ParamInput::default());
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
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
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
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let shop = fixtures::connection(
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n\
             tunnel = { command = [\"sh\", \"-c\", \"exit 255\", \"{port}\"] }\n",
        );
        let (mut backend, mut answers) = started_on(shop.clone());
        answers_until(&mut answers, |answer| matches!(answer, Incoming::NotOpened(_))).await;
        backend.open(shop, SessionId::FIRST);
        let again = answers_until(&mut answers, |answer| matches!(answer, Incoming::NotOpened(_))).await;
        assert_eq!(again[0], Incoming::Tunnel(TunnelState::Starting));
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_statement_sent_while_the_tunnel_fails_to_start_fails_with_it() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let shop = fixtures::connection(
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n\
             tunnel = { command = [\"sh\", \"-c\", \"exit 255\", \"{port}\"] }\n",
        );
        let name = shop.name().clone();
        let (backend, mut answers) = started_on(shop);
        backend.run(&name, RunId::FIRST, Statement::new("SELECT 1"), ParamInput::default());
        let seen = answers_until(&mut answers, |answer| is_answer_to(RunId::FIRST, answer)).await;
        let failed = Incoming::Failed { run: RunId::FIRST, error: TUNNEL_NOT_STARTED.to_owned(), position: None };
        assert_eq!(seen.last(), Some(&failed));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_tunnel_that_dies_closes_its_session_and_says_why() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
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
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let marker = "pgbg-test-tunnel-started-again";
        let (backend, mut answers) = started_on(fixtures::tunnelled(marker));
        answers_until(&mut answers, |answer| *answer == Incoming::Opened).await;
        kill_tunnel_marked(marker).await;
        answers_until(&mut answers, is_tunnel_death).await;
        backend.run(&shop(), RunId::FIRST, Statement::new("SELECT 1"), ParamInput::default());
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
        backend.run(&shop(), run, Statement::new(sql), ParamInput::default());
        next_answer(answers).await
    }

    async fn on_a_side_session(sql: &str) -> Vec<Vec<Value>> {
        let mut session = Session::open(&connection(), Route::Direct).await.unwrap();
        session.run(&guard::admit(Statement::new(sql), Params::default(), WritePolicy::ReadOnly).unwrap()).await.unwrap().rows().to_vec()
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
        backend.open(copy.clone(), SessionId::FIRST);
        backend.run(copy.name(), RunId::FIRST, Statement::new("SELECT 1"), ParamInput::default());
        let connection = loop {
            if let Answer::Session { connection, incoming: Incoming::Result { .. }, .. } = answers.recv().await.unwrap() {
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
        backend.open(connection(), SessionId::FIRST);
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
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP), ParamInput::default());
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
    async fn stopping_a_session_ends_its_running_statement_on_the_server() {
        const SLEEP_AT_STOP: &str = "SELECT pg_sleep(31)";
        let (backend, _answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP_AT_STOP), ParamInput::default());
        until_active(SLEEP_AT_STOP).await;
        backend.stop().await;
        let is_active = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLEEP_AT_STOP}' AND state = 'active'");
        while !on_a_side_session(&is_active).await.is_empty() {}
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_running_when_its_session_stops_fails_with_it() {
        let (backend, mut answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP), ParamInput::default());
        until_sleeping().await;
        backend.stop().await;
        let failed = Incoming::Failed { run: RunId::FIRST, error: STOPPED_WITH_ITS_SESSION.to_owned(), position: None };
        assert_eq!(next_answer(&mut answers).await, failed);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_write_running_when_its_session_closes_never_commits() {
        let table = empty_table("pgbg_write_at_close").await;
        let (mut backend, mut answers) = started_on(fixtures::test_database("local", "writes = \"allowed\"\n"));
        let insert = format!("INSERT INTO {table} SELECT 1 FROM pg_sleep(32)");
        backend.run(&shop(), RunId::FIRST, Statement::new(&insert), ParamInput::default());
        until_active(&insert).await;
        backend.close(&shop());
        next_answer(&mut answers).await;
        let rows = on_a_side_session(&format!("SELECT n FROM {table}")).await;
        assert_eq!(rows.len(), 0, "{rows:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_write_whose_session_stops_between_its_round_trips_never_commits() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let table = empty_table("pgbg_write_between_round_trips").await;
        on_a_side_session_writing(&format!("INSERT INTO {table} VALUES (1)")).await;
        let (backend, mut answers) = started_on(fixtures::tunnelled_slowly("pgbg-test-slow-write", WritePolicy::Allowed));
        answers_until(&mut answers, |answer| *answer == Incoming::Opened).await;
        let mut running = backend.sessions[&shop()].running.clone();
        backend.run(&shop(), RunId::FIRST, Statement::new(format!("UPDATE {table} SET n = 2")), ParamInput::default());
        running.wait_for(Option::is_some).await.unwrap();
        backend.stop().await;
        let rows = on_a_side_session(&format!("SELECT n FROM {table}")).await;
        assert_eq!(rows, [[Value::Int(1)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_is_read_while_a_statement_runs() {
        let (backend, mut answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP), ParamInput::default());
        until_sleeping().await;
        backend.read_schema(&shop(), SessionId::FIRST);
        let answer = next_answer(&mut answers).await;
        assert!(matches!(&answer, Incoming::Schema(Ok(catalog)) if !catalog.search_path.is_empty()), "{answer:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_completion_index_of_a_schema_read_follows_it() {
        let (backend, mut answers) = started();
        backend.read_schema(&shop(), SessionId::FIRST);
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::Index { .. })).await;
        let read = seen.iter().find_map(|answer| if let Incoming::Schema(Ok(catalog)) = answer { Some(catalog) } else { None });
        let is_built_from_it =
            matches!((read, seen.last()), (Some(read), Some(Incoming::Index { catalog, .. })) if std::sync::Arc::ptr_eq(read, catalog));
        assert!(is_built_from_it, "{seen:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_is_read_through_the_tunnel_of_the_session() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let marker = "pgbg-test-schema-through-the-tunnel";
        let (backend, mut answers) = started_on(fixtures::tunnelled(marker));
        backend.read_schema(&shop(), SessionId::FIRST);
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::Schema(_))).await;
        let tunnels_started = seen.iter().filter(|answer| **answer == Incoming::Tunnel(TunnelState::Starting)).count();
        assert!(matches!(seen.last(), Some(Incoming::Schema(Ok(_)))) && tunnels_started == 1, "{seen:?}");
    }

    /// `shop` on the test database, its password from a shell line that `$1` names a file for.
    fn with_password_command(line: &str, file: &std::path::Path) -> Connection {
        let command = format!("password = {{ command = [\"sh\", \"-c\", {line:?}, \"sh\", {:?}] }}\n", file.display().to_string());
        fixtures::test_database("local", &command)
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_session_reads_the_password_again() {
        let dir = tempfile::tempdir().unwrap();
        let reads = dir.path().join("reads");
        let (backend, mut answers) = started_on(with_password_command("echo read >> \"$1\"; echo nina-password", &reads));
        backend.read_schema(&shop(), SessionId::FIRST);
        answers_until(&mut answers, |answer| matches!(answer, Incoming::Schema(_))).await;
        assert_eq!(std::fs::read_to_string(&reads).unwrap(), "read\nread\n");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_schema_read_that_fails_leaves_the_session_open() {
        let dir = tempfile::tempdir().unwrap();
        let once = "[ -e \"$1\" ] && exit 1; touch \"$1\"; echo nina-password";
        let (backend, mut answers) = started_on(with_password_command(once, &dir.path().join("read")));
        backend.read_schema(&shop(), SessionId::FIRST);
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::Schema(_))).await;
        assert!(matches!(seen.last(), Some(Incoming::Schema(Err(_)))), "{seen:?}");
        let after = answer_to(&backend, &mut answers, RunId::FIRST, "SELECT 1").await;
        assert!(matches!(after, Incoming::Result { .. }), "{after:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_run_cancelled_then_stopped_is_answered_as_failed() {
        let (backend, mut answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP), ParamInput::default());
        until_sleeping().await;
        backend.cancel(&shop(), RunId::FIRST);
        backend.stop().await;
        let answer = next_answer(&mut answers).await;
        assert!(matches!(&answer, Incoming::Failed { run, .. } if *run == RunId::FIRST), "{answer:?}");
    }
}
