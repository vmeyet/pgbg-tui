//! Running one statement alone and reading its first `ROW_CAP` rows; a read-only one is always rolled back.
use super::guard::{Admitted, Mode, TransactionStep};
use super::limits::ReadLimits;
use super::model::{Column, Extent, InsideUserTransaction, Page, Params, ResultSet, Value};
use super::{Isolation, Session, bind, decode, roll_back, server_error};
use anyhow::{Context, Result};
use futures_util::{StreamExt, TryStreamExt, future};
use std::pin::pin;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, IsolationLevel, Portal, RowStream, Statement as Prepared, Transaction};

/// The most rows pgbg keeps of a result; past it the result is cut, so a big table cannot fill the memory.
pub(super) const ROW_CAP: usize = 10_000;
const PROBE_SAVEPOINT: &str = "SAVEPOINT pgbg_probe";
const RELEASE_PROBE: &str = "RELEASE SAVEPOINT pgbg_probe";
/// The name prefix of the cursor that reads a result inside the user's transaction.
const CURSOR: &str = "pgbg_result";
/// The cursors open on the session with a name pgbg could pick, so it picks another.
const CURSORS_TAKEN: &str = r"SELECT name FROM pg_cursors WHERE name LIKE 'pgbg\_result\_%'";

/// How many rows one fetch from a portal asks for: never 0, which the protocol reads as every row.
#[derive(Clone, Copy, Debug)]
pub(super) struct FetchSize(i32);

impl FetchSize {
    pub(super) const fn new(rows: i32) -> Self {
        assert!(rows > 0, "a fetch asks for at least one row");
        Self(rows)
    }

    pub(super) const fn rows(self) -> i32 {
        self.0
    }
}

/// One row past the cap, so a result that has more says it is cut.
const CAPPED_FETCH: FetchSize = FetchSize::new(ROW_CAP as i32 + 1);

/// Whether a transaction the user began is open on the session.
/// Outside one, a write that returns rows commits a transaction of its own; inside one, it must never end the user's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum UserTransaction {
    #[default]
    None,
    Open,
    /// A statement that opens or ends one failed, or got no answer: only the server knows.
    Unsure,
}

/// How a connection that allows writes runs a statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WritePath {
    /// Sent as it is, and every row it returns is read.
    AsIs,
    /// Through a portal in a transaction of its own, committed once its first `ROW_CAP` rows are read.
    OwnTransaction,
    /// Inside the user's transaction, through a cursor that reads its first `ROW_CAP` rows.
    UsersCursor,
}

impl UserTransaction {
    /// The state once the server has run, or refused, a statement that takes `step`.
    fn after(self, step: TransactionStep, has_run: bool) -> Self {
        match (step, has_run) {
            (TransactionStep::Keeps, _) => self,
            (TransactionStep::Opens, true) => Self::Open,
            (TransactionStep::Ends, true) => Self::None,
            (TransactionStep::Opens | TransactionStep::Ends, false) | (TransactionStep::Unsure, _) => Self::Unsure,
        }
    }
}

impl Session {
    /// Runs `admitted` alone and returns its first `ROW_CAP` rows; a read-only one is always rolled back.
    pub(crate) async fn run(&mut self, admitted: &Admitted) -> Result<ResultSet> {
        self.marks.is_mid_statement = true;
        let result = match admitted.mode {
            Mode::Writes => self.run_writes(admitted).await,
            Mode::ReadOnly => self.run_read_only(admitted).await,
        };
        self.marks.is_mid_statement = false;
        result
    }

    /// Runs `admitted` on its `WritePath`, and keeps track of the transaction the user began; until the server answers
    /// a statement that opens or ends one, the session is unsure of it.
    async fn run_writes(&mut self, admitted: &Admitted) -> Result<ResultSet> {
        let step = admitted.transaction_step;
        if step != TransactionStep::Keeps {
            self.marks.user_transaction = UserTransaction::Unsure;
        }
        let result = async {
            let statement = bind::prepare(&self.client, admitted).await?;
            match self.write_path(&statement, admitted).await? {
                WritePath::AsIs => fetch(&self.client, &statement, &admitted.params).await,
                WritePath::OwnTransaction => self.fetch_in_own_transaction(&statement, &admitted.params).await,
                WritePath::UsersCursor => self.fetch_through_cursor(&statement, admitted).await,
            }
        };
        let result = result.await;
        self.marks.user_transaction = self.marks.user_transaction.after(step, result.is_ok());
        result
    }

    /// Only a statement that returns rows and keeps the user's transaction as it is leaves the as-is path.
    /// A `CALL` stays on it: it may commit, which it cannot do inside a transaction pgbg opened.
    async fn write_path(&mut self, statement: &Prepared, admitted: &Admitted) -> Result<WritePath> {
        let returns_rows = !statement.columns().is_empty();
        if !returns_rows || admitted.transaction_step != TransactionStep::Keeps {
            return Ok(WritePath::AsIs);
        }
        if !self.is_in_user_transaction().await? {
            return Ok(WritePath::OwnTransaction);
        }
        Ok(if admitted.is_query { WritePath::UsersCursor } else { WritePath::AsIs })
    }

    async fn fetch_in_own_transaction(&mut self, statement: &Prepared, params: &Params) -> Result<ResultSet> {
        let transaction = self.client.transaction().await.map_err(server_error)?;
        let portal = bind_portal(&transaction, statement, params).await?;
        let first = fetch_capped(&transaction, &portal).await?;
        transaction.commit().await.map_err(server_error)?;
        Ok(ResultSet::new(columns(statement), first))
    }

    /// Reads through an SQL cursor: the driver binds a portal only inside a transaction handle of its own, whose start
    /// or drop would roll the user's transaction back.
    async fn fetch_through_cursor(&self, statement: &Prepared, admitted: &Admitted) -> Result<ResultSet> {
        let cursor = free_cursor_name(&self.cursors_taken().await?);
        let declare = format!("DECLARE {cursor} NO SCROLL CURSOR FOR {}", admitted.statement.as_str());
        let declare = self.client.prepare_typed(&declare, statement.params()).await.map_err(server_error)?;
        self.client.execute_raw(&declare, bind::as_text(&admitted.params)).await.map_err(server_error)?;
        let first = fetch_cursor(&self.client, &cursor).await;
        let closed = self.client.batch_execute(&format!("CLOSE {cursor}")).await.map_err(server_error);
        let first = first?;
        closed?;
        Ok(ResultSet::new(columns(statement), first))
    }

    /// Asked before each `DECLARE`: a name the user already holds would fail it and abort the user's transaction.
    async fn cursors_taken(&self) -> Result<Vec<String>> {
        let rows = self.client.query(CURSORS_TAKEN, &[]).await.map_err(server_error)?;
        rows.iter().map(|row| row.try_get(0).map_err(server_error)).collect()
    }

    /// Whether a `BEGIN` the user ran left a transaction open, asking the server only when the session is unsure.
    async fn is_in_user_transaction(&mut self) -> Result<bool> {
        match self.marks.user_transaction {
            UserTransaction::None => Ok(false),
            UserTransaction::Open => Ok(true),
            UserTransaction::Unsure => {
                let is_open = self.probe_user_transaction().await?;
                self.marks.user_transaction = if is_open { UserTransaction::Open } else { UserTransaction::None };
                Ok(is_open)
            }
        }
    }

    /// Whether a transaction is open, by a savepoint: it succeeds only inside one, and fails as "in failed transaction"
    /// only inside one too.
    async fn probe_user_transaction(&self) -> Result<bool> {
        match self.client.batch_execute(PROBE_SAVEPOINT).await {
            Ok(()) => self.client.batch_execute(RELEASE_PROBE).await.map(|()| true).map_err(server_error),
            Err(err) if err.code() == Some(&SqlState::NO_ACTIVE_SQL_TRANSACTION) => Ok(false),
            Err(err) if err.code() == Some(&SqlState::IN_FAILED_SQL_TRANSACTION) => Ok(true),
            Err(err) => Err(server_error(err)),
        }
    }

    async fn run_read_only(&mut self, admitted: &Admitted) -> Result<ResultSet> {
        self.read_only(Isolation::EachStatement, async |transaction| {
            let statement = bind::prepare(transaction, admitted).await?;
            let portal = bind_portal(transaction, &statement, &admitted.params).await?;
            Ok(ResultSet::new(columns(&statement), fetch_capped(transaction, &portal).await?))
        })
        .await
    }

    /// Runs `read` inside `BEGIN READ ONLY` at `isolation`, always rolled back; the session is mid-statement meanwhile,
    /// so dropping it then cuts its link. Refused inside the user's transaction, which its `ROLLBACK` would end.
    pub(super) async fn read_only<T>(&mut self, isolation: Isolation, read: impl AsyncFnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        if self.is_in_user_transaction().await? {
            return Err(InsideUserTransaction.into());
        }
        let was_mid_statement = std::mem::replace(&mut self.marks.is_mid_statement, true);
        let result = self.read_only_marked(isolation, read).await;
        self.marks.is_mid_statement = was_mid_statement;
        result
    }

    async fn read_only_marked<T>(&mut self, isolation: Isolation, read: impl AsyncFnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let transaction = begin_read_only(&mut self.client, &self.limits, isolation).await?;
        let result = read(&transaction).await;
        roll_back(transaction, &mut self.marks).await?;
        result
    }
}

/// The first `pgbg_result_<n>` that `taken` does not hold.
fn free_cursor_name(taken: &[String]) -> String {
    let mut n = 1;
    while taken.contains(&format!("{CURSOR}_{n}")) {
        n += 1;
    }
    format!("{CURSOR}_{n}")
}

/// Opens a `BEGIN READ ONLY` transaction and sets its own limits.
pub(super) async fn begin_read_only<'c>(client: &'c mut Client, limits: &ReadLimits, isolation: Isolation) -> Result<Transaction<'c>> {
    let level = match isolation {
        Isolation::EachStatement => IsolationLevel::ReadCommitted,
        Isolation::OneSnapshot => IsolationLevel::RepeatableRead,
    };
    let transaction = client.build_transaction().read_only(true).isolation_level(level).start().await.map_err(server_error)?;
    transaction.batch_execute(&limits.sql()).await.map_err(server_error).context("setting the transaction's limits")?;
    Ok(transaction)
}

/// `statement` bound to a portal with `params`, each in text.
pub(super) async fn bind_portal(transaction: &Transaction<'_>, statement: &Prepared, params: &Params) -> Result<Portal> {
    let values = bind::as_text(params);
    transaction.bind(statement, &bind::borrowed(&values)).await.map_err(server_error)
}

/// Streams the rows of `statement` run with `params`, prepared first so a result without rows still has its columns.
async fn fetch(client: &Client, statement: &Prepared, params: &Params) -> Result<ResultSet> {
    let rows = client.query_raw(statement, bind::as_text(params)).await.map_err(server_error)?;
    Ok(ResultSet::new(columns(statement), first_rows(rows).await?))
}

/// The first `ROW_CAP` rows of `cursor`; one more says the result is cut.
async fn fetch_cursor(client: &Client, cursor: &str) -> Result<Page> {
    let fetch = format!("FETCH FORWARD {} FROM {cursor}", CAPPED_FETCH.rows());
    first_rows(client.query_raw(&fetch, std::iter::empty::<&str>()).await.map_err(server_error)?).await
}

pub(super) fn columns(statement: &Prepared) -> Vec<Column> {
    statement.columns().iter().map(decode::column).collect()
}

/// The next `size` rows of `portal`, decoded: all of them when the portal ran to its end, else more wait in it.
pub(super) async fn fetch_page(transaction: &Transaction<'_>, portal: &Portal, size: FetchSize) -> Result<Page> {
    let mut stream = pin!(transaction.query_portal_raw(portal, size.rows()).await.map_err(server_error)?);
    let rows = stream.as_mut().and_then(|row| future::ready(decode::row(&row))).try_collect().await.map_err(server_error)?;
    let has_ended = stream.rows_affected().is_some();
    Ok(Page { rows, extent: if has_ended { Extent::All } else { Extent::More } })
}

/// The first `ROW_CAP` rows of `portal`; one more says the result is cut, and the server sends no row past it, so
/// ending the transaction does not wait on them.
async fn fetch_capped(transaction: &Transaction<'_>, portal: &Portal) -> Result<Page> {
    let Page { rows, .. } = fetch_page(transaction, portal, CAPPED_FETCH).await?;
    Ok(capped(rows))
}

/// The first `ROW_CAP` rows of `rows`, decoded; one more says the result is cut.
async fn first_rows(rows: RowStream) -> Result<Page> {
    let rows: Vec<_> = rows.take(ROW_CAP + 1).and_then(|row| future::ready(decode::row(&row))).try_collect().await.map_err(server_error)?;
    Ok(capped(rows))
}

fn capped(mut rows: Vec<Vec<Value>>) -> Page {
    let is_cut = rows.len() > ROW_CAP;
    rows.truncate(ROW_CAP);
    Page { rows, extent: if is_cut { Extent::Cut } else { Extent::All } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::guard;
    use crate::db::model::{self, ParamValue, Statement};
    use crate::fixtures::{run, run_on, session, try_on};

    #[test]
    fn a_cursor_takes_the_first_name_no_open_cursor_holds() {
        assert_eq!(free_cursor_name(&[]), "pgbg_result_1");
        assert_eq!(free_cursor_name(&["pgbg_result_1".to_owned(), "pgbg_result_3".to_owned()]), "pgbg_result_2");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_on_one_snapshot_runs_in_repeatable_read() {
        let mut session = session(WritePolicy::ReadOnly).await;
        let isolation = session
            .read_only(Isolation::OneSnapshot, async |transaction| {
                Ok(transaction.query_one("SHOW transaction_isolation", &[]).await?.get::<_, String>(0))
            })
            .await;
        assert_eq!(isolation.unwrap(), "repeatable read");
    }

    fn numbers_up_to(count: usize) -> String {
        format!("SELECT generate_series(1, {count})")
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_past_the_row_cap_keeps_its_first_rows_and_says_it_is_cut() {
        let result = run(WritePolicy::ReadOnly, &numbers_up_to(ROW_CAP + 1)).await.unwrap();
        assert_eq!((result.rows().len(), result.is_whole()), (ROW_CAP, false));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_of_exactly_the_row_cap_is_whole() {
        let result = run(WritePolicy::ReadOnly, &numbers_up_to(ROW_CAP)).await.unwrap();
        assert_eq!((result.rows().len(), result.is_whole()), (ROW_CAP, true));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_on_a_connection_that_allows_writes_is_cut_too() {
        let result = run(WritePolicy::Allowed, &numbers_up_to(ROW_CAP + 1)).await.unwrap();
        assert_eq!((result.rows().len(), result.is_whole()), (ROW_CAP, false));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cut_result_on_a_connection_that_allows_writes_leaves_the_server_idle() {
        let mut writer = session(WritePolicy::Allowed).await;
        let pid = run_on(&mut writer, "SELECT pg_backend_pid()").await.rows()[0][0].to_string();
        run_on(&mut writer, &numbers_up_to(100_000_000)).await;
        let state = format!("SELECT state FROM pg_stat_activity WHERE pid = {pid}");
        let watched = run(WritePolicy::ReadOnly, &state).await.unwrap();
        assert_eq!(watched.rows(), [[Value::Text("idle".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cut_result_inside_a_transaction_the_user_began_leaves_the_server_idle_in_it() {
        let mut writer = session(WritePolicy::Allowed).await;
        let pid = run_on(&mut writer, "SELECT pg_backend_pid()").await.rows()[0][0].to_string();
        run_on(&mut writer, "BEGIN").await;
        let result = run_on(&mut writer, &numbers_up_to(100_000_000)).await;
        let state = format!("SELECT state FROM pg_stat_activity WHERE pid = {pid}");
        let watched = run(WritePolicy::ReadOnly, &state).await.unwrap();
        assert!(!result.is_whole());
        assert_eq!(watched.rows(), [[Value::Text("idle in transaction".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_write_that_returns_rows_inside_a_transaction_the_user_began_is_left_to_the_user() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_returned_undone").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_returned_undone (id int)").await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        assert_eq!(run_on(&mut writer, "INSERT INTO orders_returned_undone VALUES (41) RETURNING id").await.rows(), [[Value::Int(41)]]);
        run_on(&mut writer, "ROLLBACK").await;
        let seen = run(WritePolicy::ReadOnly, "SELECT count(*)::int4 FROM orders_returned_undone").await.unwrap();
        assert_eq!(seen.rows(), [[Value::Int(0)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_that_fails_inside_a_transaction_the_user_began_leaves_it_to_the_user() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        let failed = writer.run(&guard::admit(Statement::new("SELECT 1 / 0"), Params::default(), WritePolicy::Allowed).unwrap()).await;
        assert!(failed.is_err());
        run_on(&mut writer, "ROLLBACK").await;
        assert_eq!(writer.marks.user_transaction, UserTransaction::None);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_write_that_returns_rows_is_committed() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_returned").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_returned (id int)").await.unwrap();
        run(WritePolicy::Allowed, "INSERT INTO orders_returned VALUES (41) RETURNING id").await.unwrap();
        let seen = run(WritePolicy::ReadOnly, "SELECT count(*)::int4 FROM orders_returned WHERE id = 41").await.unwrap();
        assert_eq!(seen.rows(), [[Value::Int(1)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_select_inside_a_transaction_the_user_began_leaves_it_to_the_user() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_undone").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_undone (id int)").await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "INSERT INTO orders_undone VALUES (7)").await;
        run_on(&mut writer, "SELECT id FROM orders_undone").await;
        run_on(&mut writer, "ROLLBACK").await;
        let seen = run(WritePolicy::ReadOnly, "SELECT count(*)::int4 FROM orders_undone").await.unwrap();
        assert_eq!(seen.rows(), [[Value::Int(0)]]);
    }

    /// Runs `opening` then, on a fresh `table`, an insert and a select that returns rows, and the user's `ROLLBACK`;
    /// gives the rows the table keeps.
    async fn kept_after_the_users_rollback(table: &str, opening: &[&str]) -> Vec<Vec<Value>> {
        run(WritePolicy::Allowed, &format!("DROP TABLE IF EXISTS {table}")).await.unwrap();
        run(WritePolicy::Allowed, &format!("CREATE TABLE {table} (id int)")).await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        for sql in opening {
            run_on(&mut writer, sql).await;
        }
        run_on(&mut writer, &format!("INSERT INTO {table} VALUES (7)")).await;
        run_on(&mut writer, &format!("SELECT id FROM {table}")).await;
        run_on(&mut writer, "ROLLBACK").await;
        run(WritePolicy::ReadOnly, &format!("SELECT count(*)::int4 FROM {table}")).await.unwrap().rows().to_vec()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_select_after_start_transaction_leaves_it_to_the_user() {
        assert_eq!(kept_after_the_users_rollback("orders_started", &["START TRANSACTION"]).await, [[Value::Int(0)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_select_after_commit_and_chain_leaves_the_next_transaction_to_the_user() {
        assert_eq!(kept_after_the_users_rollback("orders_chained", &["BEGIN", "COMMIT AND CHAIN"]).await, [[Value::Int(0)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_select_after_rollback_and_chain_leaves_the_next_transaction_to_the_user() {
        assert_eq!(kept_after_the_users_rollback("orders_rechained", &["BEGIN", "ROLLBACK AND CHAIN"]).await, [[Value::Int(0)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_select_after_a_rollback_to_a_savepoint_leaves_it_to_the_user() {
        let opening = ["BEGIN", "SAVEPOINT before_orders", "ROLLBACK TO SAVEPOINT before_orders"];
        assert_eq!(kept_after_the_users_rollback("orders_saved", &opening).await, [[Value::Int(0)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_unsure_of_the_users_transaction_asks_the_server() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_unsure").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_unsure (id int)").await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "INSERT INTO orders_unsure VALUES (7)").await;
        writer.marks.user_transaction = UserTransaction::Unsure;
        run_on(&mut writer, "SELECT id FROM orders_unsure").await;
        run_on(&mut writer, "ROLLBACK").await;
        let seen = run(WritePolicy::ReadOnly, "SELECT count(*)::int4 FROM orders_unsure").await.unwrap();
        assert_eq!(seen.rows(), [[Value::Int(0)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_select_in_a_failed_transaction_of_the_user_fails_with_the_servers_words() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        assert!(writer.run(&guard::admit(Statement::new("SELECT 1 / 0"), Params::default(), WritePolicy::Allowed).unwrap()).await.is_err());
        writer.marks.user_transaction = UserTransaction::Unsure;
        let admitted = guard::admit(Statement::new("SELECT 1"), Params::default(), WritePolicy::Allowed).unwrap();
        let err = writer.run(&admitted).await.unwrap_err();
        assert!(err.to_string().contains("current transaction is aborted"), "{err}");
    }

    /// Runs, on a fresh `table`, the user's `BEGIN`, an insert, a statement that fails, two selects, a second insert
    /// and the user's `COMMIT`; gives the rows the table keeps and the state the server leaves the session in.
    async fn kept_after_a_failed_transaction_of_the_user(table: &str) -> (Vec<Vec<Value>>, Vec<Vec<Value>>) {
        run(WritePolicy::Allowed, &format!("DROP TABLE IF EXISTS {table}")).await.unwrap();
        run(WritePolicy::Allowed, &format!("CREATE TABLE {table} (id int)")).await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        let pid = run_on(&mut writer, "SELECT pg_backend_pid()").await.rows()[0][0].to_string();
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, &format!("INSERT INTO {table} VALUES (1)")).await;
        let select = format!("SELECT id FROM {table}");
        for sql in ["SELECT 1 / 0", &select, &select, &format!("INSERT INTO {table} VALUES (2)")] {
            assert!(try_on(&mut writer, sql).await.is_err(), "{sql}");
        }
        run_on(&mut writer, "COMMIT").await;
        let kept = run(WritePolicy::ReadOnly, &format!("SELECT id FROM {table}")).await.unwrap().rows().to_vec();
        let state = run(WritePolicy::ReadOnly, &format!("SELECT state FROM pg_stat_activity WHERE pid = {pid}")).await.unwrap();
        (kept, state.rows().to_vec())
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_transaction_of_the_user_that_failed_keeps_nothing_once_committed() {
        assert_eq!(kept_after_a_failed_transaction_of_the_user("orders_failed").await.0, Vec::<Vec<Value>>::new());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_commit_of_a_transaction_of_the_user_that_failed_leaves_none_open() {
        let (_, state) = kept_after_a_failed_transaction_of_the_user("orders_failed_ended").await;
        assert_eq!(state, [[Value::Text("idle".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_read_inside_a_failed_transaction_of_the_user_never_ends_it() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_still_failed").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_still_failed (id int)").await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "INSERT INTO orders_still_failed VALUES (1)").await;
        let select = guard::admit(Statement::new("SELECT id FROM orders_still_failed"), Params::default(), WritePolicy::Allowed).unwrap();
        let prepared = bind::prepare(&writer.client, &select).await.unwrap();
        assert!(try_on(&mut writer, "SELECT 1 / 0").await.is_err());
        assert!(writer.fetch_through_cursor(&prepared, &select).await.is_err());
        assert!(try_on(&mut writer, "INSERT INTO orders_still_failed VALUES (2)").await.is_err());
        run_on(&mut writer, "COMMIT").await;
        let kept = run(WritePolicy::ReadOnly, "SELECT id FROM orders_still_failed").await.unwrap();
        assert_eq!(kept.rows(), Vec::<Vec<Value>>::new());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_inside_a_transaction_of_the_user_binds_its_values() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        let params = Params::new(vec![Some(ParamValue::new("41"))], vec![]);
        let admitted = guard::admit(Statement::new("SELECT $1::int4 + 1"), params, WritePolicy::Allowed).unwrap();
        assert_eq!(writer.run(&admitted).await.unwrap().rows(), [[Value::Int(42)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_inside_a_transaction_of_the_user_leaves_the_users_own_cursors_alone() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "DECLARE pgbg_result CURSOR FOR SELECT 1").await;
        assert_eq!(run_on(&mut writer, "SELECT 2").await.rows(), [[Value::Int(2)]]);
        assert_eq!(run_on(&mut writer, "FETCH 1 FROM pgbg_result").await.rows(), [[Value::Int(1)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_inside_a_transaction_of_the_user_takes_a_cursor_name_the_user_does_not_hold() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "DECLARE pgbg_result_1 CURSOR FOR SELECT 1").await;
        assert_eq!(run_on(&mut writer, "SELECT 2").await.rows(), [[Value::Int(2)]]);
        assert_eq!(run_on(&mut writer, "FETCH 1 FROM pgbg_result_1").await.rows(), [[Value::Int(1)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_inside_a_transaction_of_the_user_leaves_no_cursor_open() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "SELECT 2").await;
        run_on(&mut writer, "SELECT 3").await;
        run_on(&mut writer, "DECLARE open_cursors CURSOR FOR SELECT count(*)::int4 FROM pg_cursors WHERE name LIKE 'pgbg%'").await;
        assert_eq!(run_on(&mut writer, "FETCH 1 FROM open_cursors").await.rows(), [[Value::Int(0)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_with_that_changes_rows_inside_a_transaction_of_the_user_returns_them() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_moved").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_moved (id int)").await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        let moved = run_on(&mut writer, "WITH added AS (INSERT INTO orders_moved VALUES (7) RETURNING id) SELECT id FROM added").await;
        assert_eq!(moved.rows(), [[Value::Int(7)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_procedure_with_an_inout_parameter_that_commits_runs_when_writes_are_allowed() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_counted").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_counted (id int)").await.unwrap();
        let procedure = "CREATE OR REPLACE PROCEDURE count_orders(INOUT total int) LANGUAGE plpgsql AS $$ \
                         BEGIN INSERT INTO orders_counted VALUES (1); COMMIT; SELECT count(*) INTO total FROM orders_counted; END $$";
        run(WritePolicy::Allowed, procedure).await.unwrap();
        let result = run(WritePolicy::Allowed, "CALL count_orders(NULL)").await.unwrap();
        assert_eq!(result.rows(), [[Value::Int(1)]]);
    }

    async fn after_the_user_ends(ending: &str) -> UserTransaction {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, ending).await;
        writer.marks.user_transaction
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_begin_tells_the_session_the_user_has_a_transaction_open() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        assert_eq!(writer.marks.user_transaction, UserTransaction::Open);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_commit_tells_the_session_no_transaction_of_the_user_is_open() {
        assert_eq!(after_the_user_ends("COMMIT").await, UserTransaction::None);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_end_tells_the_session_no_transaction_of_the_user_is_open() {
        assert_eq!(after_the_user_ends("END").await, UserTransaction::None);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_rollback_tells_the_session_no_transaction_of_the_user_is_open() {
        assert_eq!(after_the_user_ends("ROLLBACK").await, UserTransaction::None);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_abort_tells_the_session_no_transaction_of_the_user_is_open() {
        assert_eq!(after_the_user_ends("ABORT").await, UserTransaction::None);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_procedure_that_commits_has_the_session_ask_the_server_next() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_closed").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_closed (id int)").await.unwrap();
        let procedure = "CREATE OR REPLACE PROCEDURE close_orders() LANGUAGE plpgsql AS $$ \
                         BEGIN INSERT INTO orders_closed VALUES (1); COMMIT; INSERT INTO orders_closed VALUES (2); END $$";
        run(WritePolicy::Allowed, procedure).await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "CALL close_orders()").await;
        assert_eq!(writer.marks.user_transaction, UserTransaction::Unsure);
        run_on(&mut writer, "SELECT id FROM orders_closed").await;
        assert_eq!(writer.marks.user_transaction, UserTransaction::None);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_of_pgbg_is_refused_inside_a_transaction_of_the_user() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        let err = writer.read_only(Isolation::EachStatement, async |_| Ok(())).await.unwrap_err();
        assert_eq!(err.downcast_ref(), Some(&InsideUserTransaction));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_of_pgbg_unsure_of_the_users_transaction_asks_the_server_first() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        writer.marks.user_transaction = UserTransaction::Unsure;
        let err = writer.read_only(Isolation::EachStatement, async |_| Ok(())).await.unwrap_err();
        assert_eq!(err.downcast_ref(), Some(&InsideUserTransaction));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_of_pgbg_after_a_do_block_runs_once_the_server_says_no_transaction_is_open() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "DO $$ BEGIN PERFORM 1; END $$").await;
        assert!(writer.read_only(Isolation::EachStatement, async |_| Ok(())).await.is_ok());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_failed_ending_leaves_the_session_unsure() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        let admitted = guard::admit(Statement::new("COMMIT AND MAYBE"), Params::default(), WritePolicy::Allowed).unwrap();
        assert!(writer.run(&admitted).await.is_err());
        assert_eq!(writer.marks.user_transaction, UserTransaction::Unsure);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_that_cannot_run_in_a_transaction_runs_when_writes_are_allowed() {
        assert!(run(WritePolicy::Allowed, "VACUUM").await.is_ok());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_select_returns_its_columns_and_decoded_values() {
        let sql = "SELECT 7::int4 AS id, 'nina' AS name, true AS paid, 12.50::numeric AS total, NULL::text AS note, \
                   '2026-10-26'::date AS day, '{\"items\": 2}'::jsonb AS cart, 1.5::float8 AS ratio, '\\x0102'::bytea AS raw";
        let result = run(WritePolicy::ReadOnly, sql).await.unwrap();
        let names: Vec<&str> = result.columns().iter().map(|column| column.name.as_str()).collect();
        assert_eq!(names, ["id", "name", "paid", "total", "note", "day", "cart", "ratio", "raw"]);
        let shown: Vec<String> = result.rows()[0].iter().map(Value::to_string).collect();
        assert_eq!(shown, ["7", "nina", "true", "12.50", "NULL", "2026-10-26", "{\"items\":2}", "1.5", "\\x0102"]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn json_keeps_its_keys_in_the_order_the_server_gives_them() {
        let result = run(WritePolicy::ReadOnly, r#"SELECT '{"z": 1, "a": 2}'::json, '{"aa": 1, "b": 2}'::jsonb"#).await.unwrap();
        let shown: Vec<String> = result.rows()[0].iter().map(Value::to_string).collect();
        assert_eq!(shown, [r#"{"z":1,"a":2}"#, r#"{"b":2,"aa":1}"#]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn infinite_dates_and_special_floats_decode_in_their_row() {
        let sql = "SELECT 'infinity'::date, '-infinity'::timestamptz, 'NaN'::float8, pg_sleep(0), 7";
        let result = run(WritePolicy::ReadOnly, sql).await.unwrap();
        let shown: Vec<String> = result.rows()[0].iter().map(Value::to_string).collect();
        assert_eq!(shown, ["infinity", "-infinity", "NaN", "NULL", "7"]);
    }

    async fn run_with(writes: WritePolicy, sql: &str, values: &[Option<&str>]) -> Result<ResultSet> {
        let params = Params::new(values.iter().map(|value| value.map(ParamValue::new)).collect(), vec![]);
        session(writes).await.run(&guard::admit(Statement::new(sql), params, writes).unwrap()).await
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_value_is_read_by_the_server_as_the_type_it_gives_its_parameter() {
        let result = run_with(WritePolicy::ReadOnly, "SELECT $1::int4 + 1, $2::date", &[Some("41"), Some("2024-01-01")]).await.unwrap();
        assert_eq!(result.rows()[0].iter().map(Value::to_string).collect::<Vec<_>>(), ["42", "2024-01-01"]);
    }

    async fn run_typed(writes: WritePolicy, sql: &str, value: &str, type_name: &str) -> Result<ResultSet> {
        let params = Params::new(vec![Some(ParamValue::new(value))], vec![Some(model::TypeName::new(type_name))]);
        session(writes).await.run(&guard::admit(Statement::new(sql), params, writes).unwrap()).await
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_parameter_takes_the_type_its_header_names() {
        let result = run_typed(WritePolicy::ReadOnly, "SELECT $1 + 1", "41", "int4").await.unwrap();
        assert_eq!(result.rows(), [[Value::Int(42)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_type_of_several_words_is_looked_up_on_writes_too() {
        let result = run_typed(WritePolicy::Allowed, "SELECT pg_typeof($1)::text", "2024-01-01", "timestamp with time zone").await.unwrap();
        assert_eq!(result.rows(), [[Value::Text("timestamp with time zone".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_type_the_server_does_not_know_stops_the_run() {
        let err = run_typed(WritePolicy::ReadOnly, "SELECT $1", "1", "no_such_kind").await.unwrap_err();
        assert_eq!(err.downcast_ref(), Some(&model::UnknownType(model::TypeName::new("no_such_kind"))), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_value_is_never_read_as_sql() {
        let result = run_with(WritePolicy::ReadOnly, "SELECT $1", &[Some("1; DROP TABLE orders")]).await.unwrap();
        assert_eq!(result.rows(), [[Value::Text("1; DROP TABLE orders".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_missing_value_binds_null() {
        let result = run_with(WritePolicy::ReadOnly, "SELECT $1::text IS NULL", &[None]).await.unwrap();
        assert_eq!(result.rows(), [[Value::Bool(true)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_value_its_type_cannot_read_fails_with_the_servers_words() {
        let err = run_with(WritePolicy::ReadOnly, "SELECT $1::date", &[Some("soon")]).await.unwrap_err();
        assert!(err.to_string().contains("invalid input syntax for type date"), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_connection_that_allows_writes_binds_values_too() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_bound").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_bound (id int)").await.unwrap();
        run_with(WritePolicy::Allowed, "INSERT INTO orders_bound VALUES ($1)", &[Some("41")]).await.unwrap();
        let returned = run_with(WritePolicy::Allowed, "UPDATE orders_bound SET id = $1 RETURNING id", &[Some("42")]).await.unwrap();
        assert_eq!(returned.rows(), [[Value::Int(42)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_without_rows_keeps_its_columns() {
        let result = run(WritePolicy::ReadOnly, "SELECT 1 AS id WHERE false").await.unwrap();
        assert_eq!(result.columns().len(), 1);
        assert_eq!(result.rows(), Vec::<Vec<Value>>::new());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_explain_analyze_of_a_read_shows_the_plan_as_it_ran() {
        let result = run(WritePolicy::ReadOnly, "EXPLAIN (ANALYZE, FORMAT TEXT) SELECT generate_series(1, 3)").await.unwrap();
        let plan: Vec<String> = result.rows().iter().map(|row| row[0].to_string()).collect();
        assert!(plan.iter().any(|line| line.contains("actual")), "{plan:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_write_the_guard_lets_through_is_refused_by_the_read_only_transaction() {
        run(WritePolicy::Allowed, "CREATE SEQUENCE IF NOT EXISTS order_numbers").await.unwrap();
        let err = run(WritePolicy::ReadOnly, "SELECT nextval('order_numbers')").await.unwrap_err();
        assert!(err.to_string().contains("read-only transaction"), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cancelled_statement_fails_with_the_servers_words() {
        const SLEEP: &str = "SELECT pg_sleep(30)";
        let mut sleeper = session(WritePolicy::ReadOnly).await;
        let token = sleeper.cancel_token();
        let running = tokio::spawn(async move {
            sleeper.run(&guard::admit(Statement::new(SLEEP), Params::default(), WritePolicy::ReadOnly).unwrap()).await
        });
        let mut watcher = session(WritePolicy::ReadOnly).await;
        let is_sleeping = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLEEP}' AND state = 'active'");
        while watcher
            .run(&guard::admit(Statement::new(&is_sleeping), Params::default(), WritePolicy::ReadOnly).unwrap())
            .await
            .unwrap()
            .rows()
            .is_empty()
        {}
        token.cancel().await.unwrap();
        let err = running.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("canceling statement due to user request"), "{err}");
    }
}
