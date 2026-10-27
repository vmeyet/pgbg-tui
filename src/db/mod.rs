//! The only module that sees `tokio_postgres`: connect, run one statement, decode its rows, cancel it.
use crate::connection::{Connection, StatementTimeout, Target, TimeoutChoice, Tls, WritePolicy};
use anyhow::{Context, Result, anyhow};
use futures_util::{StreamExt, TryStreamExt, future};
use guard::{Admitted, Mode};
use model::{CancelToken, Column, Extent, Page, Params, ResultSet, Route, ServerError, Value};
use std::pin::pin;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::time::timeout;
use tokio_postgres::config::SslMode;
use tokio_postgres::error::{ErrorPosition, SqlState};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{Client, Config, IsolationLevel, NoTls, Portal, RowStream, Socket, Statement as Prepared, Transaction};
use tokio_postgres_rustls::MakeRustlsConnect;

mod bind;
mod cancel;
mod decode;
pub(crate) mod guard;
pub(crate) mod lex;
pub(crate) mod model;
mod open;
mod pages;
mod schema;
mod server;

pub(crate) use cancel::Interrupted;
pub(crate) use open::{Attempt, NotOpened, Opened, TunnelStep, open_along, open_once};
pub(crate) use pages::{Fetched, Pages, Started};
pub(crate) use schema::read_schema_along;

/// The rows pgbg keeps of a result; past it the result is cut, so a big table cannot fill the memory.
const ROW_CAP: usize = 10_000;
/// How long opening a session waits for the server, unless the URL's `connect_timeout` says otherwise.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Has the server stop a statement within a second of pgbg going away, instead of running it to its end.
const STOP_WHEN_CLIENT_GONE: &str = "SET client_connection_check_interval = '1s'";
const SERVER_CLOSED: &str = "the server closed the link";
const LINK_TASK_GONE: &str = "the link's task is gone";
/// The server's `statement_timeout` in milliseconds, whatever unit it was set in.
const SERVER_STATEMENT_TIMEOUT: &str = "SELECT setting::bigint FROM pg_settings WHERE name = 'statement_timeout'";
const SERVER_IDLE_IN_TRANSACTION_TIMEOUT: &str =
    "SELECT setting::bigint FROM pg_settings WHERE name = 'idle_in_transaction_session_timeout'";
/// The server ends the session of a read-only transaction left idle this long, should pgbg not close a portal waiting for
/// its next page itself, after 5 minutes.
const IDLE_IN_TRANSACTION_BACKSTOP: Duration = Duration::from_mins(6);
const PROBE_SAVEPOINT: &str = "SAVEPOINT pgbg_probe";
const RELEASE_PROBE: &str = "RELEASE SAVEPOINT pgbg_probe";
const ROLLBACK_FAILED: &str = "a rollback failed, so the server may still hold its transaction";

/// How many rows one fetch from a portal asks for: never 0, which the protocol reads as every row.
#[derive(Clone, Copy, Debug)]
struct FetchSize(i32);

impl FetchSize {
    const fn new(rows: i32) -> Self {
        assert!(rows > 0, "a fetch asks for at least one row");
        Self(rows)
    }
}

/// One row past the cap, so a result that has more says it is cut.
const CAPPED_FETCH: FetchSize = FetchSize::new(ROW_CAP as i32 + 1);

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

/// What every read-only transaction sets for itself with `SET LOCAL`: behind a pooler that hands each transaction any
/// server link, a session-wide `SET` would miss the next transaction and reach other clients.
#[derive(Debug)]
struct ReadLimits {
    statement_timeout: StatementTimeout,
    idle: IdleTimeout,
}

/// How long the server lets a read-only transaction stay idle before it ends the whole session.
#[derive(Clone, Copy, Debug)]
enum IdleTimeout {
    /// The server's own timeout, stricter than the backstop.
    Server(Duration),
    /// No stricter timeout on the server: pgbg sets `IDLE_IN_TRANSACTION_BACKSTOP`.
    Backstop,
}

impl IdleTimeout {
    fn on_server(self) -> Duration {
        match self {
            Self::Server(timeout) => timeout,
            Self::Backstop => IDLE_IN_TRANSACTION_BACKSTOP,
        }
    }
}

impl ReadLimits {
    fn sql(&self) -> String {
        let statement = format!("SET LOCAL statement_timeout = {}", self.statement_timeout.as_millis());
        match self.idle {
            IdleTimeout::Server(_) => statement,
            IdleTimeout::Backstop => {
                format!("{statement}; SET LOCAL idle_in_transaction_session_timeout = {}", IDLE_IN_TRANSACTION_BACKSTOP.as_millis())
            }
        }
    }
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
    /// Logs in to `connection` along `route`, reading its password just for that.
    pub(crate) async fn open(connection: &Connection, route: Route) -> Result<Self> {
        let mut config = driver_config(connection.target(), route)?;
        if let Some(source) = connection.password() {
            config.password(source.read().await?.expose());
        }
        let tls = connection.tls();
        let connected = match tls {
            Tls::On => open_client(config.ssl_mode(SslMode::Require), MakeRustlsConnect::with_webpki_roots()).await,
            Tls::Off => open_client(config.ssl_mode(SslMode::Disable), NoTls).await,
        };
        let Connected { client, ended, link } = connected.with_context(|| format!("connecting to {}", connection.name()))?;
        stop_statements_when_client_gone(&client).await;
        let statement_timeout = statement_timeout_in_effect(&client, connection.statement_timeout()).await?;
        if connection.writes() == WritePolicy::Allowed {
            set_statement_timeout(&client, statement_timeout).await?;
        }
        let limits = ReadLimits { statement_timeout, idle: idle_timeout(&client).await? };
        Ok(Self { client, tls, limits, ended, link, marks: Marks::default() })
    }

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
    async fn read_only<T>(&mut self, isolation: Isolation, read: impl AsyncFnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
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

/// The driver's config for `target` along `route`; `sslmode` is left out, since `Tls` decides it.
fn driver_config(target: &Target, route: Route) -> Result<Config> {
    let url = target.along(route);
    url.as_str().trim_end_matches('?').parse().map_err(|err| anyhow!("`url` is not a PostgreSQL URL: {err}"))
}

/// How long opening a session waits, TLS and login included: the driver's own timeout covers only the TCP connect.
fn connect_wait(config: &Config) -> Duration {
    config.get_connect_timeout().copied().unwrap_or(CONNECT_TIMEOUT)
}

/// A client, why its link ended once it has, and the task that drives the link.
struct Connected {
    client: Client,
    ended: watch::Receiver<Option<String>>,
    link: AbortHandle,
}

async fn open_client<T>(config: &Config, tls: T) -> Result<Connected>
where
    T: MakeTlsConnect<Socket>,
    T::Stream: Send + 'static,
{
    let wait = connect_wait(config);
    let (client, connection) = timeout(wait, config.connect(tls))
        .await
        .with_context(|| format!("the server did not answer within {}s", wait.as_secs()))?
        .map_err(server_error)?;
    let (end, ended) = watch::channel(None);
    let link = tokio::spawn(async move {
        let reason = match connection.await {
            Ok(()) => SERVER_CLOSED.to_owned(),
            Err(err) => {
                let reason = format!("{:#}", server_error(err));
                tracing::warn!("the connection closed: {reason}");
                reason
            }
        };
        end.send_replace(Some(reason));
    });
    Ok(Connected { client, ended, link: link.abort_handle() })
}

/// Best effort: a server older than PostgreSQL 14, or not on Linux, cannot check.
async fn stop_statements_when_client_gone(client: &Client) {
    if let Err(err) = client.batch_execute(STOP_WHEN_CLIENT_GONE).await {
        tracing::warn!("the server will not stop a statement when pgbg is gone: {err}");
    }
}

/// The timeout a session runs with: the one the connection chose, else the stricter of its tier's and the one the
/// server applies already (a role's or a database's default, or the URL's `options`).
async fn statement_timeout_in_effect(client: &Client, choice: TimeoutChoice) -> Result<StatementTimeout> {
    let server = server_millis(client, SERVER_STATEMENT_TIMEOUT).await.context("reading the server's statement timeout")?;
    Ok(choice.in_effect(StatementTimeout::from_millis(server)))
}

/// A server setting in milliseconds, read by `sql`.
async fn server_millis(client: &Client, sql: &str) -> Result<u64> {
    let row = client.query_one(sql, &[]).await.map_err(server_error)?;
    let ms: i64 = row.try_get(0).map_err(server_error)?;
    u64::try_from(ms).context("the setting is negative")
}

/// The idle timeout a read-only transaction runs with: the server's own when stricter than the backstop, which then
/// ends a portal left waiting for its next page should pgbg not close it itself.
async fn idle_timeout(client: &Client) -> Result<IdleTimeout> {
    let server = server_millis(client, SERVER_IDLE_IN_TRANSACTION_TIMEOUT).await.context("reading the server's idle timeout")?;
    let is_stricter_on_server = server != 0 && u128::from(server) <= IDLE_IN_TRANSACTION_BACKSTOP.as_millis();
    Ok(if is_stricter_on_server { IdleTimeout::Server(Duration::from_millis(server)) } else { IdleTimeout::Backstop })
}

/// `BEGIN READ ONLY`, then the transaction's own limits.
async fn begin_read_only<'c>(client: &'c mut Client, limits: &ReadLimits, isolation: Isolation) -> Result<Transaction<'c>> {
    let level = match isolation {
        Isolation::EachStatement => IsolationLevel::ReadCommitted,
        Isolation::OneSnapshot => IsolationLevel::RepeatableRead,
    };
    let transaction = client.build_transaction().read_only(true).isolation_level(level).start().await.map_err(server_error)?;
    transaction.batch_execute(&limits.sql()).await.map_err(server_error).context("setting the transaction's limits")?;
    Ok(transaction)
}

/// Only where writes are allowed, whose own `BEGIN` and statements run outside pgbg's transactions; behind a
/// transaction pooler it holds only on the server link it reached. Not best effort: a production session never runs
/// without its timeout.
async fn set_statement_timeout(client: &Client, timeout: StatementTimeout) -> Result<()> {
    let sql = format!("SET statement_timeout = {}", timeout.as_millis());
    client.batch_execute(&sql).await.map_err(server_error).context("setting the statement timeout")
}

/// `statement` bound to a portal with `params`, each in text.
async fn bind_portal(transaction: &Transaction<'_>, statement: &Prepared, params: &Params) -> Result<Portal> {
    let values = bind::as_text(params);
    transaction.bind(statement, &bind::borrowed(&values)).await.map_err(server_error)
}

/// Streams the rows of `statement` run with `params`, prepared first so a result without rows still has its columns.
async fn fetch(client: &Client, statement: &Prepared, params: &Params) -> Result<ResultSet> {
    let rows = client.query_raw(statement, bind::as_text(params)).await.map_err(server_error)?;
    Ok(ResultSet::new(columns(statement), first_rows(rows).await?))
}

fn columns(statement: &Prepared) -> Vec<Column> {
    statement.columns().iter().map(decode::column).collect()
}

/// The next `size` rows of `portal`, decoded: all of them when the portal ran to its end, else more wait in it.
async fn fetch_page(transaction: &Transaction<'_>, portal: &Portal, size: FetchSize) -> Result<Page> {
    let mut stream = pin!(transaction.query_portal_raw(portal, size.0).await.map_err(server_error)?);
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
    use model::LocalPort;
    use model::{Params, Statement};
    use std::net::Ipv4Addr;
    use std::time::Duration;
    use url::Url;

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

    #[test]
    fn the_driver_config_drops_sslmode_and_keeps_the_rest() {
        let connection = fixtures::connection(
            "tier = \"staging\"\nurl = \"postgres://nina@db.example.com:6432/shop?sslmode=verify-full&application_name=pgbg\"\n",
        );
        let config = driver_config(connection.target(), Route::Direct).unwrap();
        assert_eq!(config.get_application_name(), Some("pgbg"));
        assert_eq!(config.get_dbname(), Some("shop"));
        assert_eq!(config.get_ports(), [6432]);
    }

    fn driver_config_of(url: &str) -> Config {
        let connection = fixtures::connection(&format!("tier = \"local\"\nurl = \"{url}\"\n"));
        driver_config(connection.target(), Route::Direct).unwrap()
    }

    #[test]
    fn a_port_in_the_query_is_kept_without_a_tunnel() {
        assert!(driver_config_of("postgres://nina@localhost/shop?port=6432").get_ports().contains(&6432));
    }

    fn tunnelled_config_of(url: &str) -> Config {
        let connection = fixtures::connection(&format!("tier = \"staging\"\nurl = \"{url}\"\n"));
        driver_config(connection.target(), Route::Tunnel(LocalPort::new(49213))).unwrap()
    }

    #[test]
    fn through_a_tunnel_the_link_goes_to_its_local_port() {
        let config = tunnelled_config_of("postgres://nina@db.example.com:6432/shop?sslmode=require");
        assert_eq!(config.get_hostaddrs(), [Ipv4Addr::LOCALHOST]);
        assert_eq!(config.get_ports(), [49213]);
    }

    #[test]
    fn through_a_tunnel_the_host_stays_for_the_certificate_check() {
        let config = tunnelled_config_of("postgres://nina@db.example.com/shop?sslmode=require");
        assert_eq!(config.get_hosts(), [tokio_postgres::config::Host::Tcp("db.example.com".to_owned())]);
    }

    #[test]
    fn through_a_tunnel_a_host_named_in_the_query_goes_to_its_local_port_too() {
        let config = tunnelled_config_of("postgres:///shop?host=db.example.com&sslmode=require");
        assert_eq!((config.get_hostaddrs().len(), config.get_ports()), (1, [49213].as_slice()));
    }

    #[test]
    fn through_a_tunnel_a_port_in_the_query_gives_way_to_the_tunnels() {
        assert_eq!(tunnelled_config_of("postgres://nina@db.example.com/shop?sslmode=require&port=6432").get_ports(), [49213]);
    }

    #[test]
    fn options_with_an_encoded_space_reach_the_driver_as_typed() {
        let config = driver_config_of("postgres://nina@localhost/shop?sslmode=disable&options=-c%20role%3Dnina");
        assert_eq!(config.get_options(), Some("-c role=nina"));
    }

    #[test]
    fn an_application_name_with_an_encoded_space_reaches_the_driver_as_typed() {
        let config = driver_config_of("postgres://nina@localhost/shop?application_name=my%20app&sslmode=disable");
        assert_eq!(config.get_application_name(), Some("my app"));
    }

    #[test]
    fn through_a_tunnel_options_with_an_encoded_space_reach_the_driver_as_typed() {
        let config = tunnelled_config_of("postgres://nina@db.example.com/shop?options=-c%20role%3Dnina&port=6432&sslmode=require");
        assert_eq!(config.get_options(), Some("-c role=nina"));
    }

    #[test]
    fn a_session_gives_up_connecting_after_ten_seconds() {
        assert_eq!(connect_wait(&driver_config_of("postgres://nina@localhost/shop")), Duration::from_secs(10));
    }

    #[test]
    fn the_connect_timeout_of_the_url_wins() {
        assert_eq!(connect_wait(&driver_config_of("postgres://nina@localhost/shop?connect_timeout=3")), Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn a_server_that_accepts_but_never_answers_is_given_up() {
        let silent = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = silent.local_addr().unwrap().port();
        let connection = fixtures::connection(&format!("tier = \"local\"\nurl = \"postgres://nina@localhost:{port}/shop\"\n"));
        let err = Session::open(&connection, Route::Direct).await.unwrap_err();
        assert!(format!("{err:#}").contains("did not answer within 10s"), "{err:#}");
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

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_asks_the_server_to_stop_its_statement_once_pgbg_is_gone() {
        let result = run(WritePolicy::ReadOnly, "SHOW client_connection_check_interval").await.unwrap();
        assert_eq!(result.rows(), [[Value::Text("1s".into())]]);
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
        assert!(err.to_string().contains("the server knows no type `no_such_kind`"), "{err}");
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
