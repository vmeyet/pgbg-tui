//! One session task per opened connection: it owns the session and its tunnel, runs each statement in turn, takes
//! cancels by `RunId`, and stops a statement the same way every stop in pgbg does.
use super::paging::{Paging, serve_pages};
use super::schema::SchemaReader;
use crate::db::Session;
use crate::failure::{Failure, Lost};
use crate::history::Outcome;
use crate::tui::app::{Incoming, RunId};
use crate::tunnel::Died;
use open::Opener;
use record::{Recorder, RunStart};
use run::{Answered, run_until_answered};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

mod link;
mod open;
mod record;
mod run;

pub(super) use link::{Job, Queued, SessionAnswers, SessionLink};
pub(super) use open::ended;
pub(super) use run::{Cancels, OnServer, warn_if_still_running};

/// What woke the session task.
pub(super) enum Woken {
    Queued(Option<Job>),
    Closed(Lost),
    TunnelEnded(Died),
}

/// What the session task does next.
pub(super) enum Next {
    /// Wait for a job, the session's close or the tunnel's end.
    Wait,
    /// Act on what woke the task while it served the pages of a run.
    Act(Woken),
    /// Stop, because pgbg quits.
    Stop,
}

/// The task that owns the session and its tunnel: it opens them, runs each statement in turn, and reports when they are
/// lost.
struct SessionTask {
    opener: Opener,
    mailbox: Mailbox,
    recorder: Recorder,
    /// How long a result's portal waits for its next page.
    idle_limit: Duration,
}

/// The task's channels with the app: jobs and cancels come in, answers go out, plus the statement on the server that a
/// cancel reaches.
pub(super) struct Mailbox {
    pub(super) queue: UnboundedReceiver<Job>,
    pub(super) cancelled: Cancels,
    pub(super) on_server: OnServer,
    pub(super) answers: SessionAnswers,
}

impl Mailbox {
    /// Fails every statement queued with `failure`, so a later open never sends one.
    fn fail_queued(&mut self, failure: &Failure) {
        while let Ok(job) = self.queue.try_recv() {
            if let Job::Run(queued) = job {
                self.answers.say(Incoming::Failed { run: queued.run, failure: failure.clone() });
            }
        }
    }
}

impl SessionTask {
    /// Opens the session at once, and again before a statement when the last try failed or the session was lost.
    /// Once pgbg quits, a statement still queued is never sent.
    async fn serve(mut self, reader: SchemaReader) {
        tokio::select! {
            () = self.serve_jobs() => {}
            () = reader.serve() => {}
        }
        self.opener.stop().await;
    }

    async fn serve_jobs(&mut self) {
        self.open_or_fail_queued().await;
        let mut next = Next::Wait;
        loop {
            let woken = match next {
                Next::Wait => self.woken().await,
                Next::Act(woken) => woken,
                Next::Stop => break,
            };
            next = self.act(woken).await;
        }
    }

    async fn act(&mut self, woken: Woken) -> Next {
        match woken {
            Woken::Queued(Some(_)) if self.mailbox.queue.is_closed() => {}
            Woken::Queued(Some(Job::Run(queued))) => return self.run(queued).await,
            Woken::Queued(Some(Job::NextPage { .. })) => {}
            Woken::Queued(Some(Job::Open)) if !self.opener.is_open() => self.open_or_fail_queued().await,
            Woken::Queued(Some(Job::Open)) => {}
            Woken::Queued(None) => return Next::Stop,
            Woken::Closed(why) => self.opener.lose(why, &self.mailbox.answers),
            Woken::TunnelEnded(died) => self.opener.lose_tunnel(&died, &self.mailbox.answers),
        }
        Next::Wait
    }

    async fn woken(&mut self) -> Woken {
        let Self { opener, mailbox, .. } = self;
        tokio::select! {
            queued = mailbox.queue.recv() => Woken::Queued(queued),
            ending = opener.ending() => ending,
        }
    }

    /// Opens the session; when it does not open, every statement sent meanwhile fails with it, so a later open never
    /// sends one.
    async fn open_or_fail_queued(&mut self) {
        if let Err(failure) = self.opener.open(&self.mailbox.answers).await {
            self.mailbox.fail_queued(&failure);
        }
    }

    /// Runs `queued` on the open session, or on one opened for it; returns what the task does next.
    async fn run(&mut self, queued: Queued) -> Next {
        let start = RunStart::now(&queued);
        let mut session = match self.opener.take_or_open(&self.mailbox.answers).await {
            Ok(session) => session,
            Err(failure) => return self.fail_unopened(start, queued.run, failure),
        };
        if self.mailbox.cancelled.was_cancelled(queued.run) {
            return self.refuse_cancelled(session, queued.run);
        }
        let next = self.answer_and_page(&mut session, queued, start).await;
        self.keep_unless_lost(session, next)
    }

    /// Records and answers `run` as failed, since its session did not open.
    fn fail_unopened(&self, start: RunStart, run: RunId, failure: Failure) -> Next {
        self.recorder.record(start, Duration::ZERO, Outcome::from(&failure));
        self.send(Incoming::Failed { run, failure });
        Next::Wait
    }

    /// Keeps `session` and answers `run` as cancelled, since its cancel came before it was sent.
    fn refuse_cancelled(&mut self, session: Session, run: RunId) -> Next {
        self.opener.keep(session);
        self.send(Incoming::Failed { run, failure: Failure::CancelledBeforeSent });
        Next::Wait
    }

    /// Answers `queued` on `session`, then serves the pages of its result while its portal is open.
    async fn answer_and_page(&mut self, session: &mut Session, queued: Queued, start: RunStart) -> Next {
        let (run, token) = (queued.run, session.cancel_token());
        let writes = self.opener.connection().writes();
        let Answered { incoming, outcome, rest } =
            run_until_answered(session, writes, queued, &mut self.mailbox.cancelled, &self.mailbox.on_server).await;
        let took = start.elapsed();
        self.recorder.record(start, took, outcome);
        self.send(incoming);
        let Some(pages) = rest else { return Next::Wait };
        let (tunnel, mailbox, idle_limit) = (self.opener.tunnel(), &mut self.mailbox, self.idle_limit);
        serve_pages(Paging { pages, run, token, mailbox, tunnel, idle_limit }).await
    }

    /// Keeps `session` for the next run, or closes it when it can run no more statements.
    /// Closing it already reports the loss, so a pending `Closed` wake is then dropped.
    fn keep_unless_lost(&mut self, session: Session, next: Next) -> Next {
        let Some(lost) = session.lost() else {
            self.opener.keep(session);
            return next;
        };
        drop(session);
        self.opener.lose(lost, &self.mailbox.answers);
        match next {
            Next::Act(Woken::Closed(_)) => Next::Wait,
            next => next,
        }
    }

    fn send(&self, incoming: Incoming) {
        self.mailbox.answers.say(incoming);
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        SLEEP, answer_to, answers_until, backend_pid, empty_table, next_answer, on_a_side_session, on_a_side_session_writing, shop,
        started, started_on, until_sleeping,
    };
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::model::{Statement, Value};
    use crate::fixtures;
    use crate::query::params::ParamInput;
    use crate::tui::app::{Answer, Confirmed, RunId, SessionId};

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_still_queued_when_the_backend_stops_never_reaches_the_server() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let table = empty_table("pgbg_queued_at_stop").await;
        let (backend, _answers) = started_on(fixtures::tunnelled_after("pgbg-test-slow-start", "sleep 1", WritePolicy::Allowed));
        backend.run(
            &shop(),
            RunId::FIRST,
            Statement::new(format!("INSERT INTO {table} VALUES (1)")),
            ParamInput::default(),
            Some(Confirmed::FOR_TESTS),
        );
        backend.stop().await;
        let rows = on_a_side_session(&format!("SELECT n FROM {table}")).await;
        assert_eq!(rows.len(), 0, "{rows:?}");
    }

    /// A fresh `table` and a function `add_to_<table>()` that inserts a row into it; gives the `SELECT` calling it.
    async fn table_and_writing_function(table: &str) -> Statement {
        empty_table(table).await;
        on_a_side_session_writing(&format!(
            "CREATE OR REPLACE FUNCTION add_to_{table}() RETURNS int LANGUAGE sql AS $$ INSERT INTO {table} VALUES (1) RETURNING n $$"
        ))
        .await;
        Statement::new(format!("SELECT add_to_{table}()"))
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_function_that_writes_answers_that_it_tried_to_write() {
        let call = table_and_writing_function("pgbg_hidden_write").await;
        let (backend, mut answers) = started_on(fixtures::test_database("local", "writes = \"allowed\"\n"));
        backend.run(&shop(), RunId::FIRST, call, ParamInput::default(), None);
        assert_eq!(next_answer(&mut answers).await, Incoming::Failed { run: RunId::FIRST, failure: Failure::TriedToWrite });
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_confirmed_function_that_writes_runs() {
        let call = table_and_writing_function("pgbg_hidden_write_confirmed").await;
        let (backend, mut answers) = started_on(fixtures::test_database("local", "writes = \"allowed\"\n"));
        backend.run(&shop(), RunId::FIRST, call, ParamInput::default(), Some(Confirmed::FOR_TESTS));
        next_answer(&mut answers).await;
        assert_eq!(on_a_side_session("SELECT n FROM pgbg_hidden_write_confirmed").await, [[Value::Int(1)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_answer_names_the_connection_whose_session_gave_it() {
        let (mut backend, mut answers) = started();
        let copy = fixtures::test_database_as("shop-copy");
        backend.open(copy.clone(), SessionId::FIRST);
        backend.run(copy.name(), RunId::FIRST, Statement::new("SELECT 1"), ParamInput::default(), None);
        let connection = loop {
            if let Answer::Session { connection, incoming: Incoming::Result { .. }, .. } = answers.recv().await.unwrap() {
                break connection;
            }
        };
        assert_eq!(&connection, copy.name());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cancel_that_comes_before_its_run_keeps_it_from_the_server() {
        let (backend, mut answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP), ParamInput::default(), None);
        backend.cancel(&shop(), RunId::FIRST);
        let answer = next_answer(&mut answers).await;
        assert_eq!(answer, Incoming::Failed { run: RunId::FIRST, failure: Failure::CancelledBeforeSent });
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
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP_AT_STOP), ParamInput::default(), None);
        until_active(SLEEP_AT_STOP).await;
        backend.stop().await;
        let is_active = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLEEP_AT_STOP}' AND state = 'active'");
        while !on_a_side_session(&is_active).await.is_empty() {}
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_running_when_its_session_stops_fails_with_it() {
        let (backend, mut answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP), ParamInput::default(), None);
        until_sleeping().await;
        backend.stop().await;
        let failed = Incoming::Failed { run: RunId::FIRST, failure: Failure::StoppedWithItsSession };
        assert_eq!(next_answer(&mut answers).await, failed);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_write_running_when_its_session_closes_never_commits() {
        let table = empty_table("pgbg_write_at_close").await;
        let (mut backend, mut answers) = started_on(fixtures::test_database("local", "writes = \"allowed\"\n"));
        let insert = format!("INSERT INTO {table} SELECT 1 FROM pg_sleep(32)");
        backend.run(&shop(), RunId::FIRST, Statement::new(&insert), ParamInput::default(), Some(Confirmed::FOR_TESTS));
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
        backend.run(
            &shop(),
            RunId::FIRST,
            Statement::new(format!("UPDATE {table} SET n = 2")),
            ParamInput::default(),
            Some(Confirmed::FOR_TESTS),
        );
        running.wait_for(Option::is_some).await.unwrap();
        backend.stop().await;
        let rows = on_a_side_session(&format!("SELECT n FROM {table}")).await;
        assert_eq!(rows, [[Value::Int(1)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_run_cancelled_then_stopped_is_answered_as_failed() {
        let (backend, mut answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP), ParamInput::default(), None);
        until_sleeping().await;
        backend.cancel(&shop(), RunId::FIRST);
        backend.stop().await;
        let answer = next_answer(&mut answers).await;
        assert!(matches!(&answer, Incoming::Failed { run, .. } if *run == RunId::FIRST), "{answer:?}");
    }
}
