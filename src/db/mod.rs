//! The only module that sees `tokio_postgres`: connect, run one statement, decode its rows, cancel it.
use crate::connection::Tls;
use crate::failure::Lost;
use anyhow::Result;
use limits::ReadLimits;
use model::{CancelToken, ServerError};
use run::UserTransaction;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio_postgres::error::{ErrorPosition, SqlState};
use tokio_postgres::{Client, Transaction};

mod bind;
mod cancel;
mod connect;
mod decode;
pub(crate) mod explain;
pub(crate) mod guard;
pub(crate) mod lex;
mod limits;
pub(crate) mod model;
mod open;
mod pages;
pub(crate) mod preview;
mod read_first;
mod run;
mod schema;
mod server;

pub(crate) use cancel::Interrupted;
pub(crate) use open::{Attempt, NotOpened, Opened, TunnelStep, open_along, open_once};
pub(crate) use pages::{Fetched, Pages, Started};
pub(crate) use schema::read_schema_along;

/// An open link to one connection.
#[derive(Debug)]
pub(crate) struct Session {
    client: Client,
    tls: Tls,
    limits: ReadLimits,
    /// Why the link ended, set by the task that drives it.
    ended: watch::Receiver<Option<Lost>>,
    link: AbortHandle,
    marks: Marks,
}

/// What a read-only transaction sees: each statement the database as it is when it starts, or all of them one snapshot.
#[derive(Clone, Copy, Debug)]
enum Isolation {
    EachStatement,
    OneSnapshot,
}

/// What the session's own statements leave behind.
#[derive(Debug, Default)]
struct Marks {
    /// A `ROLLBACK` failed, so the server may still hold the transaction open.
    has_failed_rollback: bool,
    /// A statement or a page is running on the server: dropping the session then cuts its link, so the server stops
    /// that work instead of the link running it to its end.
    is_mid_statement: bool,
    user_transaction: UserTransaction,
}

impl Session {
    /// Waits until the link to the server ends, and says why.
    pub(crate) async fn closed(&self) -> Lost {
        why_ended(&self.ended).await
    }

    /// Why this session can run no more statements, when it cannot: its link is gone, or a rollback failed.
    pub(crate) fn lost(&self) -> Option<Lost> {
        let ended = self.ended.borrow().clone();
        let is_link_gone = self.client.is_closed();
        let lost = ended.or_else(|| is_link_gone.then_some(Lost::ServerClosed));
        lost.or_else(|| self.marks.has_failed_rollback.then_some(Lost::RollbackFailed))
    }

    /// Whether only the link went: it ended with no transaction of the user's on it, so a read sent again on a new
    /// session sees the same database.
    pub(crate) fn lost_only_its_link(&self) -> bool {
        let is_link_gone = self.lost().is_some_and(|lost| lost != Lost::RollbackFailed);
        is_link_gone && self.marks.user_transaction == UserTransaction::None
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

/// Waits until the link that `ended` watches ends, and says why.
async fn why_ended(ended: &watch::Receiver<Option<Lost>>) -> Lost {
    let mut ended = ended.clone();
    match ended.wait_for(Option::is_some).await {
        Ok(lost) => lost.clone().unwrap_or(Lost::LinkTaskGone),
        Err(_) => Lost::LinkTaskGone,
    }
}

/// Rolls `transaction` back, and marks the session when that failed.
async fn roll_back(transaction: Transaction<'_>, marks: &mut Marks) -> Result<()> {
    let rolled_back = transaction.rollback().await;
    marks.has_failed_rollback = rolled_back.is_err();
    rolled_back.map_err(server_error)
}

/// A `ServerError` when the server answered with one, the driver's error otherwise.
/// It says no position: the text it failed on is pgbg's own, or holds more than the statement.
fn server_error(err: tokio_postgres::Error) -> anyhow::Error {
    from_server(err, None)
}

/// `server_error` for a failure of the statement sent exactly as it was admitted, so its position counts in that text.
fn statement_error(err: tokio_postgres::Error) -> anyhow::Error {
    let position = err.as_db_error().and_then(|db| match db.position() {
        Some(ErrorPosition::Original(position)) => Some(*position as usize),
        Some(ErrorPosition::Internal { .. }) | None => None,
    });
    from_server(err, position)
}

fn from_server(err: tokio_postgres::Error, position: Option<usize>) -> anyhow::Error {
    let Some(db) = err.as_db_error() else { return anyhow::Error::new(err) };
    anyhow::Error::new(ServerError {
        severity: db.severity().to_owned(),
        message: db.message().to_owned(),
        detail: db.detail().map(str::to_owned),
        hint: db.hint().map(str::to_owned),
        position,
        is_refused_write: db.code() == &SqlState::READ_ONLY_SQL_TRANSACTION,
        code: model::ErrorCode::parse(db.code().code()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::fixtures::{run, run_on, session, try_on};
    use model::{Params, Statement};
    use std::time::Duration;
    use tokio::time::timeout;

    fn position_of(err: &anyhow::Error) -> Option<usize> {
        err.downcast_ref::<ServerError>().and_then(|server| server.position)
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
        assert!(doomed.closed().await.to_string().contains("terminating connection"));
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
        assert_eq!(position_of(&err), Some(15));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_server_position_counts_characters_and_the_comments_above() {
        let err = run(WritePolicy::ReadOnly, "-- zoë\nSELECT 'é', nope").await.unwrap_err();
        assert_eq!(position_of(&err), Some(20));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_failing_inside_a_transaction_of_the_user_says_where_in_the_statement_not_in_its_cursor() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        let err = try_on(&mut writer, "SELECT nope").await.unwrap_err();
        assert_eq!(position_of(&err), Some(8));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_cut_short_fails_just_past_its_last_character() {
        let err = run(WritePolicy::ReadOnly, "SELECT * FROM").await.unwrap_err();
        assert_eq!(position_of(&err), Some(14));
    }
}
