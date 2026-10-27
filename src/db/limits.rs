//! What a session runs with: the statement timeout in effect, and the limits each read-only transaction sets.
use super::server_error;
use crate::connection::StatementTimeout;
use anyhow::{Context, Result};
use std::time::Duration;
use tokio_postgres::{Client, SimpleQueryMessage, SimpleQueryRow};

/// The server's `statement_timeout` and `idle_in_transaction_session_timeout` in milliseconds, whatever unit they were set
/// in, in one row.
const SERVER_TIMEOUTS: &str = "SELECT
    max(setting) FILTER (WHERE name = 'statement_timeout'),
    max(setting) FILTER (WHERE name = 'idle_in_transaction_session_timeout')
    FROM pg_settings WHERE name IN ('statement_timeout', 'idle_in_transaction_session_timeout')";
/// The server ends the session of a read-only transaction left idle this long, should pgbg not close a portal waiting for
/// its next page itself, after 5 minutes.
const IDLE_IN_TRANSACTION_BACKSTOP: Duration = Duration::from_mins(6);
/// The guard and the parameters read a backslash in a string as itself; the server must read it the same way.
const STANDARD_STRINGS: &str = "standard_conforming_strings = on";

/// What every read-only transaction sets for itself with `SET LOCAL`: behind a pooler that hands each transaction any
/// server link, a session-wide `SET` would miss the next transaction and reach other clients.
#[derive(Debug)]
pub(super) struct ReadLimits {
    pub(super) statement_timeout: StatementTimeout,
    pub(super) idle: IdleTimeout,
}

/// How long the server lets a read-only transaction stay idle before it ends the whole session.
#[derive(Clone, Copy, Debug)]
pub(super) enum IdleTimeout {
    /// The server's own timeout, stricter than the backstop.
    Server(Duration),
    /// No stricter timeout on the server: pgbg sets `IDLE_IN_TRANSACTION_BACKSTOP`.
    Backstop,
}

impl IdleTimeout {
    pub(super) fn on_server(self) -> Duration {
        match self {
            Self::Server(timeout) => timeout,
            Self::Backstop => IDLE_IN_TRANSACTION_BACKSTOP,
        }
    }
}

impl ReadLimits {
    pub(super) fn sql(&self) -> String {
        let statement = format!("SET LOCAL {STANDARD_STRINGS}; SET LOCAL statement_timeout = {}", self.statement_timeout.as_millis());
        match self.idle {
            IdleTimeout::Server(_) => statement,
            IdleTimeout::Backstop => {
                format!("{statement}; SET LOCAL idle_in_transaction_session_timeout = {}", IDLE_IN_TRANSACTION_BACKSTOP.as_millis())
            }
        }
    }
}

/// The timeouts the server applies already: a role's or a database's default, or the URL's `options`.
pub(super) struct ServerTimeouts {
    pub(super) statement: StatementTimeout,
    pub(super) idle: IdleTimeout,
}

/// Both timeouts of the server in one round trip, as the simple query protocol sends a statement and its rows at once.
pub(super) async fn read_server_timeouts(client: &Client) -> Result<ServerTimeouts> {
    let messages = client.simple_query(SERVER_TIMEOUTS).await.map_err(server_error)?;
    let row = messages.iter().find_map(only_row).context("the server gave no row of settings")?;
    let statement = millis(row.get(0)).context("reading the server's statement timeout")?;
    let idle = millis(row.get(1)).context("reading the server's idle timeout")?;
    Ok(ServerTimeouts { statement: StatementTimeout::from_millis(statement), idle: idle_timeout(idle) })
}

fn only_row(message: &SimpleQueryMessage) -> Option<&SimpleQueryRow> {
    match message {
        SimpleQueryMessage::Row(row) => Some(row),
        _ => None,
    }
}

/// A setting `pg_settings` gives in milliseconds.
fn millis(setting: Option<&str>) -> Result<u64> {
    let setting = setting.context("the setting is missing")?;
    setting.parse().with_context(|| format!("the setting is not a count of milliseconds: {setting}"))
}

/// The idle timeout a read-only transaction runs with: the server's own when stricter than the backstop, which then
/// ends a portal left waiting for its next page should pgbg not close it itself.
fn idle_timeout(server_ms: u64) -> IdleTimeout {
    let is_stricter_on_server = server_ms != 0 && u128::from(server_ms) <= IDLE_IN_TRANSACTION_BACKSTOP.as_millis();
    if is_stricter_on_server { IdleTimeout::Server(Duration::from_millis(server_ms)) } else { IdleTimeout::Backstop }
}

/// Only where writes are allowed, whose own `BEGIN` and statements run outside pgbg's transactions; behind a
/// transaction pooler it holds only on the server link it reached. Not best effort: a production session never runs
/// without its timeout.
pub(super) async fn set_session_limits(client: &Client, timeout: StatementTimeout) -> Result<()> {
    let sql = format!("SET {STANDARD_STRINGS}; SET statement_timeout = {}", timeout.as_millis());
    client.batch_execute(&sql).await.map_err(server_error).context("setting the session's limits")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{Connection, WritePolicy};
    use crate::db::model::{Params, ResultSet, Route, Statement, Value};
    use crate::db::{Session, guard};
    use crate::fixtures;
    use url::Url;

    async fn session(writes: WritePolicy) -> Session {
        let writes = if writes == WritePolicy::Allowed { "allowed" } else { "read-only" };
        Session::open(&fixtures::test_database("local", &format!("writes = \"{writes}\"\n")), Route::Direct).await.unwrap()
    }

    #[test]
    fn no_idle_timeout_on_the_server_leaves_the_backstop() {
        assert!(matches!(idle_timeout(0), IdleTimeout::Backstop));
    }

    #[test]
    fn an_idle_timeout_on_the_server_looser_than_the_backstop_leaves_the_backstop() {
        assert!(matches!(idle_timeout(3_600_000), IdleTimeout::Backstop));
    }

    #[test]
    fn an_idle_timeout_on_the_server_stricter_than_the_backstop_is_kept() {
        assert!(matches!(idle_timeout(60_000), IdleTimeout::Server(timeout) if timeout == Duration::from_mins(1)));
    }

    #[test]
    fn a_setting_in_milliseconds_reads_as_a_number() {
        assert_eq!(millis(Some("360000")).unwrap(), 360_000);
    }

    #[test]
    fn a_negative_setting_is_refused() {
        assert!(millis(Some("-1")).is_err());
    }

    #[test]
    fn a_missing_setting_is_refused() {
        assert!(millis(None).is_err());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn one_read_gives_both_timeouts_of_the_server() {
        let session = session(WritePolicy::ReadOnly).await;
        let ServerTimeouts { statement, idle } = read_server_timeouts(&session.client).await.unwrap();
        assert_eq!((statement, idle.on_server()), (StatementTimeout::from_millis(0), IDLE_IN_TRANSACTION_BACKSTOP));
    }

    /// `shop` on the test database, its server set to read a backslash in a string as an escape.
    fn reading_backslashes_as_escapes(writes: WritePolicy) -> Connection {
        let database = fixtures::test_database_url();
        let separator = if database.contains('?') { '&' } else { '?' };
        let url = format!("{database}{separator}options=-c%20standard_conforming_strings%3Doff");
        let writes = if writes == WritePolicy::Allowed { "allowed" } else { "read-only" };
        fixtures::connection(&format!("tier = \"local\"\nurl = \"{url}\"\nwrites = \"{writes}\"\n"))
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_server_set_to_read_backslashes_as_escapes_reads_a_string_as_the_guard_does() {
        for writes in [WritePolicy::ReadOnly, WritePolicy::Allowed] {
            let mut session = Session::open(&reading_backslashes_as_escapes(writes), Route::Direct).await.unwrap();
            let admitted = guard::admit(Statement::new(r"SELECT '\' AS mark"), Params::default(), writes).unwrap();
            let result = session.run(&admitted).await.unwrap();
            assert_eq!(result.rows(), [[Value::Text(r"\".into())]], "{writes:?}");
        }
    }

    async fn run_on(session: &mut Session, sql: &str) -> ResultSet {
        session.run(&guard::admit(Statement::new(sql), Params::default(), WritePolicy::Allowed).unwrap()).await.unwrap()
    }

    async fn session_on(tier: &str, extra: &str) -> Session {
        Session::open(&fixtures::test_database(tier, extra), Route::Direct).await.unwrap()
    }

    async fn on(mut session: Session, sql: &str) -> Result<ResultSet> {
        session.run(&guard::admit(Statement::new(sql), Params::default(), WritePolicy::ReadOnly).unwrap()).await
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_runs_with_the_statement_timeout_of_its_tier() {
        let shown = on(session_on("staging", "").await, "SHOW statement_timeout").await.unwrap();
        assert_eq!(shown.rows(), [[Value::Text("1min".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_runs_with_the_statement_timeout_of_its_connection() {
        let shown = on(session_on("staging", "statement_timeout = \"90s\"").await, "SHOW statement_timeout").await.unwrap();
        assert_eq!(shown.rows(), [[Value::Text("90s".into())]]);
    }

    async fn shown_timeout_with_a_server_default(tier: &str, extra: &str) -> Vec<Vec<Value>> {
        let mut url = Url::parse(&fixtures::test_database_url()).unwrap();
        url.query_pairs_mut().append_pair("options", "-cstatement_timeout=5s");
        let connection = fixtures::connection(&format!("tier = \"{tier}\"\nurl = \"{url}\"\n{extra}"));
        let session = Session::open(&connection, Route::Direct).await.unwrap();
        on(session, "SHOW statement_timeout").await.unwrap().rows().to_vec()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_stricter_timeout_on_the_server_wins_over_the_tier() {
        assert_eq!(shown_timeout_with_a_server_default("staging", "").await, [[Value::Text("5s".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_local_session_keeps_the_timeout_of_the_server() {
        assert_eq!(shown_timeout_with_a_server_default("local", "").await, [[Value::Text("5s".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_timeout_the_connection_chose_wins_over_the_server() {
        let shown = shown_timeout_with_a_server_default("staging", "statement_timeout = \"90s\"").await;
        assert_eq!(shown, [[Value::Text("90s".into())]]);
    }

    async fn shown_idle_timeout(url: String, extra: &str) -> Vec<Vec<Value>> {
        let connection = fixtures::connection(&format!("tier = \"local\"\nurl = \"{url}\"\n{extra}"));
        on(Session::open(&connection, Route::Direct).await.unwrap(), "SHOW idle_in_transaction_session_timeout")
            .await
            .unwrap()
            .rows()
            .to_vec()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_only_session_has_the_server_end_a_transaction_left_idle_six_minutes() {
        assert_eq!(shown_idle_timeout(fixtures::test_database_url(), "").await, [[Value::Text("6min".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_transaction_the_user_began_stays_idle_as_long_as_the_user_wants() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        let shown = run_on(&mut writer, "SHOW idle_in_transaction_session_timeout").await;
        assert_eq!(shown.rows(), [[Value::Text("0".into())]]);
    }

    /// `setting` as the link holds it outside any transaction pgbg begins, as a pooler hands it to its next client.
    async fn shown_on_the_link(session: &Session, setting: &str) -> String {
        session.client.query_one(&format!("SHOW {setting}"), &[]).await.unwrap().get(0)
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_only_session_leaves_the_idle_timeout_of_its_link_alone() {
        let session = session_on("staging", "").await;
        assert_eq!(shown_on_the_link(&session, "idle_in_transaction_session_timeout").await, "0");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_only_session_leaves_the_statement_timeout_of_its_link_alone() {
        let session = session_on("staging", "").await;
        assert_eq!(shown_on_the_link(&session, "statement_timeout").await, "0");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_that_allows_writes_sets_its_statement_timeout_on_its_link() {
        let session = session_on("staging", "writes = \"allowed\"\n").await;
        assert_eq!(shown_on_the_link(&session, "statement_timeout").await, "1min");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_stricter_idle_timeout_on_the_server_wins_over_the_backstop() {
        let mut url = Url::parse(&fixtures::test_database_url()).unwrap();
        url.query_pairs_mut().append_pair("options", "-cidle_in_transaction_session_timeout=1min");
        assert_eq!(shown_idle_timeout(url.to_string(), "").await, [[Value::Text("1min".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_past_its_timeout_is_cancelled_by_the_server() {
        let err = on(session_on("local", "statement_timeout = \"100ms\"").await, "SELECT pg_sleep(5)").await.unwrap_err();
        assert!(err.to_string().contains("statement timeout"), "{err}");
    }
}
