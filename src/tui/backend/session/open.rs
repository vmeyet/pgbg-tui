//! The session of one connection and its tunnel: opened when needed, kept between runs, and lost when its link or its
//! tunnel ends.
use super::Woken;
use super::link::SessionAnswers;
use crate::connection::Connection;
use crate::db::model::Route;
use crate::db::{NotOpened, Opened, Session, TunnelStep, open_along};
use crate::failure::{Failure, Lost};
use crate::tui::app::{Incoming, TunnelState};
use crate::tunnel::{Died, Tunnel};
use tokio::sync::watch;

/// Opens the session of `connection` through its tunnel, keeps it between runs, and gives its route to the schema
/// reader.
pub(super) struct Opener {
    connection: Connection,
    tunnel: Option<Tunnel>,
    /// The open session between runs; a run takes it out while it uses it.
    session: Option<Session>,
    /// The route of the open session, for the schema reader; none while it is not open.
    route: watch::Sender<Option<Route>>,
}

impl Opener {
    pub(super) fn new(connection: Connection, route: watch::Sender<Option<Route>>) -> Self {
        Self { connection, tunnel: None, session: None, route }
    }

    pub(super) fn connection(&self) -> &Connection {
        &self.connection
    }

    pub(super) fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// The tunnel the session runs through, for the pages of a result to hear it end.
    pub(super) fn tunnel(&mut self) -> Option<&mut Tunnel> {
        self.tunnel.as_mut()
    }

    /// Opens the session and keeps it; returns why when it did not open.
    pub(super) async fn open(&mut self, answers: &SessionAnswers) -> Result<(), Failure> {
        let session = self.opened(answers).await?;
        self.session = Some(session);
        Ok(())
    }

    /// The open session, taken out for a run, or one opened for it.
    pub(super) async fn take_or_open(&mut self, answers: &SessionAnswers) -> Result<Session, Failure> {
        match self.session.take() {
            Some(session) => Ok(session),
            None => self.opened(answers).await,
        }
    }

    /// Keeps `session` for the next run.
    pub(super) fn keep(&mut self, session: Session) {
        self.session = Some(session);
    }

    /// What ends the open session: its link closing, or its tunnel ending; never resolves while neither is there.
    pub(super) async fn ending(&mut self) -> Woken {
        let Self { session, tunnel, .. } = self;
        tokio::select! {
            why = closed(session.as_ref()) => Woken::Closed(why),
            died = ended(tunnel.as_mut()) => Woken::TunnelEnded(died),
        }
    }

    /// Closes the session, and tells the schema reader there is no route.
    pub(super) fn lose(&mut self, lost: Lost, answers: &SessionAnswers) {
        self.session = None;
        self.route.send_replace(None);
        answers.say(Incoming::Closed(lost));
    }

    /// Closes the session that a dead tunnel carried; the next statement starts both again.
    pub(super) fn lose_tunnel(&mut self, died: &Died, answers: &SessionAnswers) {
        self.tunnel = None;
        answers.say(Incoming::Tunnel(TunnelState::Died(died.to_string())));
        if self.session.is_some() {
            self.lose(Lost::TunnelDied, answers);
        }
    }

    /// Closes the session, then stops its tunnel, warning when its port still answers.
    pub(super) async fn stop(mut self) {
        self.session = None;
        if let Some(tunnel) = self.tunnel.as_mut()
            && let Err(err) = tunnel.stop().await
        {
            tracing::warn!("{err:#}");
        }
    }

    /// Opens a session, gives the schema reader its route and reports the outcome; returns why when it did not open.
    async fn opened(&mut self, answers: &SessionAnswers) -> Result<Session, Failure> {
        let opened = self.connect(answers).await;
        let route = opened.as_ref().ok().map(|_| self.tunnel.as_ref().map_or(Route::Direct, Tunnel::route));
        self.route.send_replace(route);
        match &opened {
            Ok(_) => answers.say(Incoming::Opened),
            Err(failure) => answers.say(Incoming::NotOpened(failure.clone())),
        }
        opened
    }

    /// Opens the session through the kept tunnel, or a restarted one, as `pgbg run` does.
    /// The tunnel stays for the next try when the login fails, and its state is sent each time it changes.
    async fn connect(&mut self, answers: &SessionAnswers) -> Result<Session, Failure> {
        let on_tunnel = |step| answers.say(Incoming::Tunnel(tunnel_state(step)));
        match open_along(&self.connection, self.tunnel.take(), on_tunnel).await {
            Ok(Opened { session, tunnel }) => {
                self.tunnel = tunnel;
                Ok(session)
            }
            Err(NotOpened::Tunnel(err)) => {
                let how = format!("{err:#}");
                answers.say(Incoming::Tunnel(TunnelState::Died(how.clone())));
                Err(Failure::TunnelNotStarted(how))
            }
            Err(NotOpened::Login { error, tunnel }) => {
                self.tunnel = tunnel.map(|tunnel| *tunnel);
                Err(Failure::Login(format!("{error:#}")))
            }
        }
    }
}

fn tunnel_state(step: TunnelStep) -> TunnelState {
    match step {
        TunnelStep::Starting => TunnelState::Starting,
        TunnelStep::Up(port) => TunnelState::Up(port),
    }
}

/// Why `session` ended, once it has; never resolves when there is no session.
async fn closed(session: Option<&Session>) -> Lost {
    match session {
        Some(session) => session.closed().await,
        None => std::future::pending().await,
    }
}

/// How `tunnel` ended, once it has; never resolves when there is no tunnel.
pub(in crate::tui::backend) async fn ended(tunnel: Option<&mut Tunnel>) -> Died {
    match tunnel {
        Some(tunnel) => tunnel.ended().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures::{
        answer_to, answers_until, backend_pid, connection, empty_table, incoming, is_answer_to, next_answer, on_a_side_session, shop,
        started, started_on,
    };
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::model::Statement;
    use crate::fixtures;
    use crate::query::params::ParamInput;
    use crate::tui::app::{Confirmed, RunId, SessionId};

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
        backend.run(
            &shop(),
            RunId::FIRST,
            Statement::new(format!("INSERT INTO {table} VALUES (1)")),
            ParamInput::default(),
            Some(Confirmed::FOR_TESTS),
        );
        answers_until(&mut answers, |answer| is_answer_to(RunId::FIRST, answer)).await;
        let rows = on_a_side_session(&format!("SELECT n FROM {table}")).await;
        assert_eq!(rows.len(), 0, "{rows:?}");
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
        assert!(matches!(&seen[2], Incoming::NotOpened(Failure::TunnelNotStarted(how)) if how.contains("no route")), "{seen:?}");
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
    async fn a_statement_sent_while_the_tunnel_fails_to_start_fails_with_how_it_ended() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let shop = fixtures::connection(
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n\
             tunnel = { command = [\"sh\", \"-c\", \"echo 'bastion.example.com: no route' >&2; exit 255\", \"{port}\"] }\n",
        );
        let name = shop.name().clone();
        let (backend, mut answers) = started_on(shop);
        backend.run(&name, RunId::FIRST, Statement::new("SELECT 1"), ParamInput::default(), None);
        let seen = answers_until(&mut answers, |answer| is_answer_to(RunId::FIRST, answer)).await;
        let is_how_it_ended = |failure: &Failure| matches!(failure, Failure::TunnelNotStarted(how) if how.contains("no route"));
        assert!(matches!(seen.last(), Some(Incoming::Failed { failure, .. }) if is_how_it_ended(failure)), "{seen:?}");
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
        backend.run(&shop(), RunId::FIRST, Statement::new("SELECT 1"), ParamInput::default(), None);
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::Result { .. })).await;
        let seen: Vec<Incoming> = seen.into_iter().filter(|answer| !matches!(answer, Incoming::Closed(_))).collect();
        assert_eq!(seen[0], Incoming::Tunnel(TunnelState::Starting));
        assert!(matches!(seen[1], Incoming::Tunnel(TunnelState::Up(_))), "{seen:?}");
        assert_eq!(seen[2], Incoming::Opened);
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
        assert!(matches!(&answer, Incoming::Closed(lost) if lost.to_string().contains("terminating connection")), "{answer:?}");
    }
}
