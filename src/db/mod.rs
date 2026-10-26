//! The only module that sees `tokio_postgres`: connect, run one statement, decode its rows, cancel it.
use crate::connection::{Connection, StatementTimeout, Target, TimeoutChoice, Tls};
use anyhow::{Context, Result, anyhow};
use futures_util::{StreamExt, TryStreamExt, future};
use guard::{Admitted, Mode};
use model::{CancelToken, LocalPort, ResultSet, Route, ServerError};
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::timeout;
use tokio_postgres::config::SslMode;
use tokio_postgres::error::{ErrorPosition, SqlState};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{Client, Config, NoTls, RowStream, Socket, Statement as Prepared, Transaction};
use tokio_postgres_rustls::MakeRustlsConnect;
use url::Url;

mod decode;
pub(crate) mod guard;
pub(crate) mod model;
mod open;
#[cfg_attr(not(test), expect(dead_code, reason = "`pgbg schema`, the next step, prints it"))]
mod schema;
mod server;

pub(crate) use open::{NotOpened, Opened, open_along};

const NO_PARAMETERS: [&str; 0] = [];
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
const PROBE_SAVEPOINT: &str = "SAVEPOINT pgbg_probe";
const RELEASE_PROBE: &str = "RELEASE SAVEPOINT pgbg_probe";
const ROLLBACK_FAILED: &str = "a rollback failed, so the server may still hold its transaction";

/// An open link to one connection.
#[derive(Debug)]
pub(crate) struct Session {
    client: Client,
    tls: Tls,
    /// A `ROLLBACK` failed, so the server may still hold the transaction open.
    has_failed_rollback: bool,
    /// Why the link ended, set by the task that drives it.
    ended: watch::Receiver<Option<String>>,
}

impl Session {
    /// Logs in to `connection` along `route`, reading its password just for that.
    pub(crate) async fn open(connection: &Connection, route: Route) -> Result<Self> {
        let mut config = driver_config(connection.target(), route)?;
        if let Some(source) = connection.password() {
            config.password(source.read().await?.expose());
        }
        let tls = connection.tls();
        let link = match tls {
            Tls::On => open_client(config.ssl_mode(SslMode::Require), MakeRustlsConnect::with_webpki_roots()).await,
            Tls::Off => open_client(config.ssl_mode(SslMode::Disable), NoTls).await,
        };
        let Link { client, ended } = link.with_context(|| format!("connecting to {}", connection.name()))?;
        stop_statements_when_client_gone(&client).await;
        let timeout = statement_timeout_in_effect(&client, connection.statement_timeout()).await?;
        set_statement_timeout(&client, timeout).await?;
        Ok(Self { client, tls, has_failed_rollback: false, ended })
    }

    /// Waits until the link to the server ends, and says why.
    pub(crate) async fn closed(&self) -> String {
        let mut ended = self.ended.clone();
        match ended.wait_for(Option::is_some).await {
            Ok(reason) => reason.clone().unwrap_or_default(),
            Err(_) => LINK_TASK_GONE.to_owned(),
        }
    }

    /// Why this session can run no more statements, when it cannot: its link is gone, or a rollback failed.
    pub(crate) fn lost(&self) -> Option<String> {
        let ended = self.ended.borrow().clone();
        let is_link_gone = self.client.is_closed();
        let lost = ended.or_else(|| is_link_gone.then(|| SERVER_CLOSED.to_owned()));
        lost.or_else(|| self.has_failed_rollback.then(|| ROLLBACK_FAILED.to_owned()))
    }

    /// What cancels the statement this session is running, usable while `run` waits.
    pub(crate) fn cancel_token(&self) -> CancelToken {
        CancelToken { token: self.client.cancel_token(), tls: self.tls }
    }

    /// Runs `admitted` alone and returns its first `ROW_CAP` rows; a read-only one is always rolled back.
    pub(crate) async fn run(&mut self, admitted: &Admitted) -> Result<ResultSet> {
        let sql = admitted.statement.as_str();
        match admitted.mode {
            Mode::Writes => self.run_writes(sql).await,
            Mode::ReadOnly => self.run_read_only(sql).await,
        }
    }

    /// A statement that returns rows goes through a portal in its own committed transaction, so the
    /// server sends no row past the cap; one inside a transaction the user began, or one without rows, runs as is.
    async fn run_writes(&mut self, sql: &str) -> Result<ResultSet> {
        let statement = self.client.prepare(sql).await.map_err(server_error)?;
        if statement.columns().is_empty() || self.is_in_user_transaction().await? {
            return fetch(&self.client, &statement).await;
        }
        let transaction = self.client.transaction().await.map_err(server_error)?;
        let result = fetch_from_portal(&transaction, &statement).await?;
        transaction.commit().await.map_err(server_error)?;
        Ok(result)
    }

    /// Whether a `BEGIN` the user ran left a transaction open: only there does a savepoint take.
    async fn is_in_user_transaction(&self) -> Result<bool> {
        match self.client.batch_execute(PROBE_SAVEPOINT).await {
            Ok(()) => self.client.batch_execute(RELEASE_PROBE).await.map(|()| true).map_err(server_error),
            Err(err) if err.code() == Some(&SqlState::NO_ACTIVE_SQL_TRANSACTION) => Ok(false),
            Err(err) => Err(server_error(err)),
        }
    }

    async fn run_read_only(&mut self, sql: &str) -> Result<ResultSet> {
        self.read_only(async |transaction| prepare_and_fetch_from_portal(transaction, sql).await).await
    }

    /// `read` inside `BEGIN READ ONLY`, always rolled back.
    async fn read_only<T>(&mut self, read: impl AsyncFnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let transaction = self.client.build_transaction().read_only(true).start().await.map_err(server_error)?;
        let result = read(&transaction).await;
        let rolled_back = transaction.rollback().await;
        self.has_failed_rollback = rolled_back.is_err();
        rolled_back.map_err(server_error)?;
        result
    }
}

impl CancelToken {
    /// Asks the server to cancel the statement the session is running; that statement then fails
    /// with the server's words. With nothing running, the server does nothing.
    pub(crate) async fn cancel(&self) -> Result<()> {
        let sent = match self.tls {
            Tls::On => self.token.cancel_query(MakeRustlsConnect::with_webpki_roots()).await,
            Tls::Off => self.token.cancel_query(NoTls).await,
        };
        sent.map_err(server_error)
    }
}

/// The driver's config for `target` along `route`; `sslmode` is left out, since `Tls` decides it.
fn driver_config(target: &Target, route: Route) -> Result<Config> {
    let url = without_key(target.url(), "sslmode");
    let url = match route {
        Route::Direct => url,
        Route::Tunnel(port) => through_tunnel(&url, port),
    };
    url.as_str().trim_end_matches('?').parse().map_err(|err| anyhow!("`url` is not a PostgreSQL URL: {err}"))
}

/// How long opening a session waits, TLS and login included: the driver's own timeout covers only the TCP connect.
fn connect_wait(config: &Config) -> Duration {
    config.get_connect_timeout().copied().unwrap_or(CONNECT_TIMEOUT)
}

fn without_key(url: &Url, dropped: &str) -> Url {
    let mut url = url.clone();
    let kept: Vec<(String, String)> =
        url.query_pairs().filter(|(key, _)| key != dropped).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    url.query_pairs_mut().clear().extend_pairs(kept);
    url
}

/// `url` sent to the tunnel's port, with its host kept so TLS still checks the certificate against it.
fn through_tunnel(url: &Url, port: LocalPort) -> Url {
    let mut url = without_key(url, "port");
    url.query_pairs_mut().append_pair("hostaddr", &Ipv4Addr::LOCALHOST.to_string());
    if url.set_port(Some(port.get())).is_err() {
        url.query_pairs_mut().append_pair("port", &port.to_string());
    }
    url
}

/// A client, and why its link ended once it has.
struct Link {
    client: Client,
    ended: watch::Receiver<Option<String>>,
}

async fn open_client<T>(config: &Config, tls: T) -> Result<Link>
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
    tokio::spawn(async move {
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
    Ok(Link { client, ended })
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
    let row = client.query_one(SERVER_STATEMENT_TIMEOUT, &[]).await.map_err(server_error)?;
    let ms: i64 = row.try_get(0).map_err(server_error)?;
    let server = StatementTimeout::from_millis(u64::try_from(ms).context("the server's statement timeout is negative")?);
    Ok(choice.in_effect(server))
}

/// Not best effort: a production session never runs without its timeout.
async fn set_statement_timeout(client: &Client, timeout: StatementTimeout) -> Result<()> {
    let sql = format!("SET statement_timeout = {}", timeout.as_millis());
    client.batch_execute(&sql).await.map_err(server_error).context("setting the statement timeout")
}

/// Streams the rows of `statement`, prepared first so a result without rows still has its columns.
async fn fetch(client: &Client, statement: &Prepared) -> Result<ResultSet> {
    let rows = client.query_raw(statement, NO_PARAMETERS).await.map_err(server_error)?;
    first_rows(statement, rows).await
}

/// Like `fetch`, through a portal: the server sends no row past the cap, so ending the transaction does not wait on them.
async fn fetch_from_portal(transaction: &Transaction<'_>, statement: &Prepared) -> Result<ResultSet> {
    let portal = transaction.bind(statement, &[]).await.map_err(server_error)?;
    let rows = transaction.query_portal_raw(&portal, (ROW_CAP + 1) as i32).await.map_err(server_error)?;
    first_rows(statement, rows).await
}

async fn prepare_and_fetch_from_portal(transaction: &Transaction<'_>, sql: &str) -> Result<ResultSet> {
    let statement = transaction.prepare(sql).await.map_err(server_error)?;
    fetch_from_portal(transaction, &statement).await
}

/// The first `ROW_CAP` rows of `rows`, decoded; one more says the result is cut.
async fn first_rows(statement: &Prepared, rows: RowStream) -> Result<ResultSet> {
    let columns = statement.columns().iter().map(decode::column).collect();
    let mut rows: Vec<_> =
        rows.take(ROW_CAP + 1).and_then(|row| future::ready(decode::row(&row))).try_collect().await.map_err(server_error)?;
    let is_cut = rows.len() > ROW_CAP;
    rows.truncate(ROW_CAP);
    Ok(ResultSet { columns, rows, is_cut })
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
    use crate::connection::{Entry, WritePolicy};
    use model::{Statement, Value};
    use std::time::Duration;

    const DATABASE_URL: &str = "PGBG_TEST_DATABASE_URL";

    async fn session(writes: WritePolicy) -> Session {
        let url = std::env::var(DATABASE_URL).unwrap();
        let writes = if writes == WritePolicy::Allowed { "allowed" } else { "read-only" };
        let entry: Entry = toml::from_str(&format!("tier = \"local\"\nurl = \"{url}\"\nwrites = \"{writes}\"\n")).unwrap();
        Session::open(&Connection::new("shop", entry).unwrap(), Route::Direct).await.unwrap()
    }

    async fn run(writes: WritePolicy, sql: &str) -> Result<ResultSet> {
        let admitted = guard::admit(Statement::new(sql), writes).unwrap();
        session(writes).await.run(&admitted).await
    }

    #[test]
    fn the_driver_config_drops_sslmode_and_keeps_the_rest() {
        let entry: Entry = toml::from_str(
            "tier = \"staging\"\nurl = \"postgres://nina@db.example.com:6432/shop?sslmode=verify-full&application_name=pgbg\"\n",
        )
        .unwrap();
        let config = driver_config(Connection::new("shop", entry).unwrap().target(), Route::Direct).unwrap();
        assert_eq!(config.get_application_name(), Some("pgbg"));
        assert_eq!(config.get_dbname(), Some("shop"));
        assert_eq!(config.get_ports(), [6432]);
    }

    fn driver_config_of(url: &str) -> Config {
        let entry: Entry = toml::from_str(&format!("tier = \"local\"\nurl = \"{url}\"\n")).unwrap();
        driver_config(Connection::new("shop", entry).unwrap().target(), Route::Direct).unwrap()
    }

    #[test]
    fn a_port_in_the_query_is_kept_without_a_tunnel() {
        assert!(driver_config_of("postgres://nina@localhost/shop?port=6432").get_ports().contains(&6432));
    }

    fn tunnelled_config_of(url: &str) -> Config {
        let entry: Entry = toml::from_str(&format!("tier = \"staging\"\nurl = \"{url}\"\n")).unwrap();
        driver_config(Connection::new("shop", entry).unwrap().target(), Route::Tunnel(LocalPort::new(49213))).unwrap()
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
        let entry: Entry = toml::from_str(&format!("tier = \"local\"\nurl = \"postgres://nina@localhost:{port}/shop\"\n")).unwrap();
        let err = Session::open(&Connection::new("shop", entry).unwrap(), Route::Direct).await.unwrap_err();
        assert!(format!("{err:#}").contains("did not answer within 10s"), "{err:#}");
    }

    fn numbers_up_to(count: usize) -> String {
        format!("SELECT generate_series(1, {count})")
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_past_the_row_cap_keeps_its_first_rows_and_says_it_is_cut() {
        let result = run(WritePolicy::ReadOnly, &numbers_up_to(ROW_CAP + 1)).await.unwrap();
        assert_eq!((result.rows.len(), result.is_cut), (ROW_CAP, true));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_of_exactly_the_row_cap_is_whole() {
        let result = run(WritePolicy::ReadOnly, &numbers_up_to(ROW_CAP)).await.unwrap();
        assert_eq!((result.rows.len(), result.is_cut), (ROW_CAP, false));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_on_a_connection_that_allows_writes_is_cut_too() {
        let result = run(WritePolicy::Allowed, &numbers_up_to(ROW_CAP + 1)).await.unwrap();
        assert_eq!((result.rows.len(), result.is_cut), (ROW_CAP, true));
    }

    async fn run_on(session: &mut Session, sql: &str) -> ResultSet {
        session.run(&guard::admit(Statement::new(sql), WritePolicy::Allowed).unwrap()).await.unwrap()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cut_result_on_a_connection_that_allows_writes_leaves_the_server_idle() {
        let mut writer = session(WritePolicy::Allowed).await;
        let pid = run_on(&mut writer, "SELECT pg_backend_pid()").await.rows[0][0].to_string();
        run_on(&mut writer, &numbers_up_to(100_000_000)).await;
        let state = format!("SELECT state FROM pg_stat_activity WHERE pid = {pid}");
        let watched = run(WritePolicy::ReadOnly, &state).await.unwrap();
        assert_eq!(watched.rows, [[Value::Text("idle".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_write_that_returns_rows_is_committed() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_returned").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_returned (id int)").await.unwrap();
        run(WritePolicy::Allowed, "INSERT INTO orders_returned VALUES (41) RETURNING id").await.unwrap();
        let seen = run(WritePolicy::ReadOnly, "SELECT count(*)::int4 FROM orders_returned WHERE id = 41").await.unwrap();
        assert_eq!(seen.rows, [[Value::Int(1)]]);
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
        assert_eq!(seen.rows, [[Value::Int(0)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_statement_that_cannot_run_in_a_transaction_runs_when_writes_are_allowed() {
        assert!(run(WritePolicy::Allowed, "VACUUM").await.is_ok());
    }

    async fn session_on(tier: &str, extra: &str) -> Session {
        let url = std::env::var(DATABASE_URL).unwrap();
        let entry: Entry = toml::from_str(&format!("tier = \"{tier}\"\nurl = \"{url}\"\n{extra}")).unwrap();
        Session::open(&Connection::new("shop", entry).unwrap(), Route::Direct).await.unwrap()
    }

    async fn on(mut session: Session, sql: &str) -> Result<ResultSet> {
        session.run(&guard::admit(Statement::new(sql), WritePolicy::ReadOnly).unwrap()).await
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_runs_with_the_statement_timeout_of_its_tier() {
        let shown = on(session_on("staging", "").await, "SHOW statement_timeout").await.unwrap();
        assert_eq!(shown.rows, [[Value::Text("1min".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_runs_with_the_statement_timeout_of_its_connection() {
        let shown = on(session_on("staging", "statement_timeout = \"90s\"").await, "SHOW statement_timeout").await.unwrap();
        assert_eq!(shown.rows, [[Value::Text("90s".into())]]);
    }

    async fn shown_timeout_with_a_server_default(tier: &str, extra: &str) -> Vec<Vec<Value>> {
        let mut url = Url::parse(&std::env::var(DATABASE_URL).unwrap()).unwrap();
        url.query_pairs_mut().append_pair("options", "-cstatement_timeout=5s");
        let entry: Entry = toml::from_str(&format!("tier = \"{tier}\"\nurl = \"{url}\"\n{extra}")).unwrap();
        let session = Session::open(&Connection::new("shop", entry).unwrap(), Route::Direct).await.unwrap();
        on(session, "SHOW statement_timeout").await.unwrap().rows
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
        assert_eq!(result.rows, [[Value::Text("1s".into())]]);
    }

    async fn ended_on_the_server() -> Session {
        let mut doomed = session(WritePolicy::ReadOnly).await;
        let pid =
            &doomed.run(&guard::admit(Statement::new("SELECT pg_backend_pid()"), WritePolicy::ReadOnly).unwrap()).await.unwrap().rows[0][0];
        run(WritePolicy::ReadOnly, &format!("SELECT pg_terminate_backend({pid}, 5000)")).await.unwrap();
        doomed
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_whose_server_side_ended_is_lost() {
        let mut doomed = ended_on_the_server().await;
        let lost = doomed.run(&guard::admit(Statement::new("SELECT 1"), WritePolicy::ReadOnly).unwrap()).await;
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
        let names: Vec<&str> = result.columns.iter().map(|column| column.name.as_str()).collect();
        assert_eq!(names, ["id", "name", "paid", "total", "note", "day", "cart", "ratio", "raw"]);
        let shown: Vec<String> = result.rows[0].iter().map(Value::to_string).collect();
        assert_eq!(shown, ["7", "nina", "true", "12.50", "NULL", "2026-10-26", "{\"items\":2}", "1.5", "\\x0102"]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn infinite_dates_and_special_floats_decode_in_their_row() {
        let sql = "SELECT 'infinity'::date, '-infinity'::timestamptz, 'NaN'::float8, pg_sleep(0), 7";
        let result = run(WritePolicy::ReadOnly, sql).await.unwrap();
        let shown: Vec<String> = result.rows[0].iter().map(Value::to_string).collect();
        assert_eq!(shown, ["infinity", "-infinity", "NaN", "NULL", "7"]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_without_rows_keeps_its_columns() {
        let result = run(WritePolicy::ReadOnly, "SELECT 1 AS id WHERE false").await.unwrap();
        assert_eq!(result.columns.len(), 1);
        assert_eq!(result.rows, Vec::<Vec<Value>>::new());
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
        let running = tokio::spawn(async move { sleeper.run(&guard::admit(Statement::new(SLEEP), WritePolicy::ReadOnly).unwrap()).await });
        let mut watcher = session(WritePolicy::ReadOnly).await;
        let is_sleeping = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLEEP}' AND state = 'active'");
        while watcher.run(&guard::admit(Statement::new(&is_sleeping), WritePolicy::ReadOnly).unwrap()).await.unwrap().rows.is_empty() {}
        token.cancel().await.unwrap();
        let err = running.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("canceling statement due to user request"), "{err}");
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
