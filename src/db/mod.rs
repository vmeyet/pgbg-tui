//! The only module that sees `tokio_postgres`: connect, run one statement, decode its rows, cancel it.
use crate::connection::{Connection, Target, Tls};
use anyhow::{Context, Result, anyhow};
use futures_util::{StreamExt, TryStreamExt, future};
use guard::{Admitted, Mode};
use model::{CancelToken, LocalPort, ResultSet, Route, ServerError};
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::sync::watch;
use tokio_postgres::config::SslMode;
use tokio_postgres::error::ErrorPosition;
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{Client, Config, NoTls, RowStream, Socket, Statement as Prepared, Transaction};
use tokio_postgres_rustls::MakeRustlsConnect;
use url::Url;

mod decode;
pub(crate) mod guard;
pub(crate) mod model;

const NO_PARAMETERS: [&str; 0] = [];
/// The rows pgbg keeps of a result; past it the result is cut, so a big table cannot fill the memory.
const ROW_CAP: usize = 10_000;
/// How long opening a session waits for the server, unless the URL's `connect_timeout` says otherwise.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Has the server stop a statement within a second of pgbg going away, instead of running it to its end.
const STOP_WHEN_CLIENT_GONE: &str = "SET client_connection_check_interval = '1s'";
const SERVER_CLOSED: &str = "the server closed the link";
const LINK_TASK_GONE: &str = "the link's task is gone";
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
            Mode::Writes => fetch(&self.client, sql).await,
            Mode::ReadOnly => self.run_read_only(sql).await,
        }
    }

    async fn run_read_only(&mut self, sql: &str) -> Result<ResultSet> {
        let transaction = self.client.build_transaction().read_only(true).start().await.map_err(server_error)?;
        let result = fetch_from_portal(&transaction, sql).await;
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
    let mut config: Config = url.as_str().trim_end_matches('?').parse().map_err(|err| anyhow!("`url` is not a PostgreSQL URL: {err}"))?;
    if config.get_connect_timeout().is_none() {
        config.connect_timeout(CONNECT_TIMEOUT);
    }
    Ok(config)
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
    let (client, connection) = config.connect(tls).await.map_err(server_error)?;
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

/// Prepares `sql` first, so a result without rows still has its columns, then streams the rows.
async fn fetch(client: &Client, sql: &str) -> Result<ResultSet> {
    let statement = client.prepare(sql).await.map_err(server_error)?;
    let rows = client.query_raw(&statement, NO_PARAMETERS).await.map_err(server_error)?;
    first_rows(&statement, rows).await
}

/// Like `fetch`, through a portal: the server sends no row past the cap, so the rollback does not wait on them.
async fn fetch_from_portal(transaction: &Transaction<'_>, sql: &str) -> Result<ResultSet> {
    let statement = transaction.prepare(sql).await.map_err(server_error)?;
    let portal = transaction.bind(&statement, &[]).await.map_err(server_error)?;
    let rows = transaction.query_portal_raw(&portal, (ROW_CAP + 1) as i32).await.map_err(server_error)?;
    first_rows(&statement, rows).await
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
        assert_eq!(driver_config_of("postgres://nina@localhost/shop").get_connect_timeout(), Some(&Duration::from_secs(10)));
    }

    #[test]
    fn the_connect_timeout_of_the_url_wins() {
        assert_eq!(
            driver_config_of("postgres://nina@localhost/shop?connect_timeout=3").get_connect_timeout(),
            Some(&Duration::from_secs(3))
        );
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
        let watcher = session(WritePolicy::ReadOnly).await;
        let is_sleeping = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLEEP}' AND state = 'active'");
        while fetch(&watcher.client, &is_sleeping).await.unwrap().rows.is_empty() {}
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
