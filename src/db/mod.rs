//! The only module that sees `tokio_postgres`: connect, run one statement, decode its rows, cancel it.
use crate::connection::Tls;
use anyhow::Result;
use limits::ReadLimits;
use model::{CancelToken, ServerError};
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio_postgres::error::ErrorPosition;
use tokio_postgres::{Client, Transaction};

mod bind;
mod cancel;
mod connect;
mod decode;
pub(crate) mod guard;
pub(crate) mod lex;
mod limits;
pub(crate) mod model;
mod open;
mod pages;
mod run;
mod schema;
mod server;

pub(crate) use cancel::Interrupted;
pub(crate) use open::{Attempt, NotOpened, Opened, TunnelStep, open_along, open_once};
pub(crate) use pages::{Fetched, Pages, Started};
pub(crate) use schema::read_schema_along;

const SERVER_CLOSED: &str = "the server closed the link";
const LINK_TASK_GONE: &str = "the link's task is gone";
const ROLLBACK_FAILED: &str = "a rollback failed, so the server may still hold its transaction";

/// An open link to one connection.
#[derive(Debug)]
pub(crate) struct Session {
    client: Client,
    tls: Tls,
    limits: ReadLimits,
    /// Why the link ended, set by the task that drives it.
    ended: watch::Receiver<Option<String>>,
    link: AbortHandle,
    marks: Marks,
}

/// What a read-only transaction sees: each statement the database as it is then, or all of them one snapshot.
#[derive(Clone, Copy, Debug)]
enum Isolation {
    EachStatement,
    OneSnapshot,
}

/// What the session's own statements left behind them.
#[derive(Debug, Default)]
struct Marks {
    /// A `ROLLBACK` failed, so the server may still hold the transaction open.
    has_failed_rollback: bool,
    /// A statement or a page is on the server: a session dropped then cuts its link, so the server stops it instead of
    /// the link finishing it.
    is_mid_statement: bool,
}

impl Session {
    /// Waits until the link to the server ends, and says why.
    pub(crate) async fn closed(&self) -> String {
        why_ended(&self.ended).await
    }

    /// Why this session can run no more statements, when it cannot: its link is gone, or a rollback failed.
    pub(crate) fn lost(&self) -> Option<String> {
        let ended = self.ended.borrow().clone();
        let is_link_gone = self.client.is_closed();
        let lost = ended.or_else(|| is_link_gone.then(|| SERVER_CLOSED.to_owned()));
        lost.or_else(|| self.marks.has_failed_rollback.then(|| ROLLBACK_FAILED.to_owned()))
    }

    /// What cancels the statement this session is running, usable while `run` waits.
    pub(crate) fn cancel_token(&self) -> CancelToken {
        CancelToken { token: self.client.cancel_token(), tls: self.tls }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.marks.is_mid_statement {
            self.link.abort();
        }
    }
}

/// Why the link `ended` watches ended, once it has.
async fn why_ended(ended: &watch::Receiver<Option<String>>) -> String {
    let mut ended = ended.clone();
    match ended.wait_for(Option::is_some).await {
        Ok(reason) => reason.clone().unwrap_or_default(),
        Err(_) => LINK_TASK_GONE.to_owned(),
    }
}

/// Rolls `transaction` back, and marks the session when that failed.
async fn roll_back(transaction: Transaction<'_>, marks: &mut Marks) -> Result<()> {
    let rolled_back = transaction.rollback().await;
    marks.has_failed_rollback = rolled_back.is_err();
    rolled_back.map_err(server_error)
}

/// A `ServerError` when the server answered with one, the driver's error otherwise.
fn server_error(err: tokio_postgres::Error) -> anyhow::Error {
    let Some(db) = err.as_db_error() else { return anyhow::Error::new(err) };
    let position = match db.position() {
        Some(ErrorPosition::Original(position)) => Some(*position as usize),
        Some(ErrorPosition::Internal { .. }) | None => None,
    };
    anyhow::Error::new(ServerError {
        severity: db.severity().to_owned(),
        message: db.message().to_owned(),
        detail: db.detail().map(str::to_owned),
        hint: db.hint().map(str::to_owned),
        position,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::fixtures;
    use model::{Params, ResultSet, Route, Statement};
    use std::time::Duration;
    use tokio::time::timeout;

    async fn session(writes: WritePolicy) -> Session {
        let writes = if writes == WritePolicy::Allowed { "allowed" } else { "read-only" };
        Session::open(&fixtures::test_database("local", &format!("writes = \"{writes}\"\n")), Route::Direct).await.unwrap()
    }

    async fn run(writes: WritePolicy, sql: &str) -> Result<ResultSet> {
        let admitted = guard::admit(Statement::new(sql), Params::default(), writes).unwrap();
        session(writes).await.run(&admitted).await
    }

    async fn ended_on_the_server() -> Session {
        let mut doomed = session(WritePolicy::ReadOnly).await;
        let admitted = guard::admit(Statement::new("SELECT pg_backend_pid()"), Params::default(), WritePolicy::ReadOnly).unwrap();
        let pid = doomed.run(&admitted).await.unwrap().rows()[0][0].to_string();
        run(WritePolicy::ReadOnly, &format!("SELECT pg_terminate_backend({pid}, 5000)")).await.unwrap();
        doomed
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_whose_server_side_ended_is_lost() {
        let mut doomed = ended_on_the_server().await;
        let lost = doomed.run(&guard::admit(Statement::new("SELECT 1"), Params::default(), WritePolicy::ReadOnly).unwrap()).await;
        assert!(lost.is_err());
        assert!(doomed.lost().is_some());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_whose_server_side_ended_says_why() {
        let doomed = ended_on_the_server().await;
        assert!(doomed.closed().await.contains("terminating connection"));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_dropped_mid_statement_ends_it_on_the_server() {
        const SLEEP: &str = "SELECT pg_sleep(600)";
        let mut sleeper = session(WritePolicy::ReadOnly).await;
        let running = tokio::spawn(async move {
            sleeper.run(&guard::admit(Statement::new(SLEEP), Params::default(), WritePolicy::ReadOnly).unwrap()).await
        });
        let mut watcher = session(WritePolicy::ReadOnly).await;
        let is_sleeping = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLEEP}'");
        let is_sleeping = guard::admit(Statement::new(&is_sleeping), Params::default(), WritePolicy::ReadOnly).unwrap();
        while watcher.run(&is_sleeping).await.unwrap().rows().is_empty() {}
        running.abort();
        let ended = async { while !watcher.run(&is_sleeping).await.unwrap().rows().is_empty() {} };
        assert!(timeout(Duration::from_secs(10), ended).await.is_ok(), "the statement still runs on the server");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_server_error_says_what_the_server_said() {
        let err = run(WritePolicy::ReadOnly, "SELECT * FROM no_such_orders").await.unwrap_err();
        assert!(err.to_string().contains("relation \"no_such_orders\" does not exist"), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_server_error_says_where_in_the_statement_it_stopped() {
        let err = run(WritePolicy::ReadOnly, "SELECT * FROM no_such_orders").await.unwrap_err();
        assert_eq!(err.downcast_ref::<ServerError>().and_then(|server| server.position), Some(15));
    }
}
