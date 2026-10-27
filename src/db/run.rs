//! Running one statement alone and reading its first `ROW_CAP` rows; a read-only one is always rolled back.
use super::guard::{Admitted, Mode};
use super::limits::ReadLimits;
use super::model::{Column, Extent, Page, Params, ResultSet, Value};
use super::{Isolation, Session, bind, decode, roll_back, server_error};
use anyhow::{Context, Result};
use futures_util::{StreamExt, TryStreamExt, future};
use std::pin::pin;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, IsolationLevel, Portal, RowStream, Statement as Prepared, Transaction};

/// The rows pgbg keeps of a result; past it the result is cut, so a big table cannot fill the memory.
pub(super) const ROW_CAP: usize = 10_000;
const PROBE_SAVEPOINT: &str = "SAVEPOINT pgbg_probe";
const RELEASE_PROBE: &str = "RELEASE SAVEPOINT pgbg_probe";

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

    /// A statement that returns rows goes through a portal in its own committed transaction, so the
    /// server sends no row past the cap; one inside a transaction the user began, or one without rows, runs as is.
    async fn run_writes(&mut self, admitted: &Admitted) -> Result<ResultSet> {
        let statement = bind::prepare(&self.client, admitted).await?;
        if statement.columns().is_empty() || self.is_in_user_transaction().await? {
            return fetch(&self.client, &statement, &admitted.params).await;
        }
        let transaction = self.client.transaction().await.map_err(server_error)?;
        let portal = bind_portal(&transaction, &statement, &admitted.params).await?;
        let first = fetch_capped(&transaction, &portal).await?;
        transaction.commit().await.map_err(server_error)?;
        Ok(ResultSet::new(columns(&statement), first))
    }

    /// Whether a `BEGIN` the user ran left a transaction open: only there does a savepoint take.
    async fn is_in_user_transaction(&self) -> Result<bool> {
        match self.client.batch_execute(PROBE_SAVEPOINT).await {
            Ok(()) => self.client.batch_execute(RELEASE_PROBE).await.map(|()| true).map_err(server_error),
            Err(err) if err.code() == Some(&SqlState::NO_ACTIVE_SQL_TRANSACTION) => Ok(false),
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

    /// `read` inside `BEGIN READ ONLY` at `isolation`, always rolled back; the session is mid-statement meanwhile, so
    /// one dropped then cuts its link.
    pub(super) async fn read_only<T>(&mut self, isolation: Isolation, read: impl AsyncFnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
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

/// `BEGIN READ ONLY`, then the transaction's own limits.
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
    use crate::db::model::{self, Route, Statement};
    use crate::fixtures;

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

    async fn session(writes: WritePolicy) -> Session {
        let writes = if writes == WritePolicy::Allowed { "allowed" } else { "read-only" };
        Session::open(&fixtures::test_database("local", &format!("writes = \"{writes}\"\n")), Route::Direct).await.unwrap()
    }

    async fn run(writes: WritePolicy, sql: &str) -> Result<ResultSet> {
        let admitted = guard::admit(Statement::new(sql), Params::default(), writes).unwrap();
        session(writes).await.run(&admitted).await
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

    async fn run_on(session: &mut Session, sql: &str) -> ResultSet {
        session.run(&guard::admit(Statement::new(sql), Params::default(), WritePolicy::Allowed).unwrap()).await.unwrap()
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
    async fn infinite_dates_and_special_floats_decode_in_their_row() {
        let sql = "SELECT 'infinity'::date, '-infinity'::timestamptz, 'NaN'::float8, pg_sleep(0), 7";
        let result = run(WritePolicy::ReadOnly, sql).await.unwrap();
        let shown: Vec<String> = result.rows()[0].iter().map(Value::to_string).collect();
        assert_eq!(shown, ["infinity", "-infinity", "NaN", "NULL", "7"]);
    }

    async fn run_with(writes: WritePolicy, sql: &str, values: &[Option<&str>]) -> Result<ResultSet> {
        let params = Params::new(values.iter().map(|value| value.map(str::to_owned)).collect(), vec![]);
        session(writes).await.run(&guard::admit(Statement::new(sql), params, writes).unwrap()).await
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_value_is_read_by_the_server_as_the_type_it_gives_its_parameter() {
        let result = run_with(WritePolicy::ReadOnly, "SELECT $1::int4 + 1, $2::date", &[Some("41"), Some("2024-01-01")]).await.unwrap();
        assert_eq!(result.rows()[0].iter().map(Value::to_string).collect::<Vec<_>>(), ["42", "2024-01-01"]);
    }

    async fn run_typed(writes: WritePolicy, sql: &str, value: &str, type_name: &str) -> Result<ResultSet> {
        let params = Params::new(vec![Some(value.to_owned())], vec![Some(model::TypeName::new(type_name))]);
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
    async fn a_write_the_guard_lets_through_is_refused_by_the_read_only_transaction() {
        run(WritePolicy::Allowed, "CREATE SEQUENCE IF NOT EXISTS order_numbers").await.unwrap();
        let err = run(WritePolicy::ReadOnly, "SELECT nextval('order_numbers')").await.unwrap_err();
        assert!(err.to_string().contains("read-only transaction"), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn two_statements_are_refused_by_the_server_even_when_writes_are_allowed() {
        let err = run(WritePolicy::Allowed, "SELECT 1; SELECT 2").await.unwrap_err();
        assert!(err.to_string().contains("multiple commands"), "{err}");
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
