//! Opening a session: the driver's config, the TLS it logs in with, and the task that drives its link.
use super::limits::{ReadLimits, ServerTimeouts, read_server_timeouts, set_session_limits};
use super::model::Route;
use super::{Marks, SERVER_CLOSED, Session, server_error};
use crate::connection::{Connection, Target, Tls, WritePolicy};
use anyhow::{Context, Result, anyhow};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::time::timeout;
use tokio_postgres::config::SslMode;
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{Client, Config, NoTls, Socket};
use tokio_postgres_rustls::MakeRustlsConnect;

/// How long opening a session waits for the server, unless the URL's `connect_timeout` says otherwise.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Has the server stop a statement within a second of pgbg going away, instead of running it to its end.
const STOP_WHEN_CLIENT_GONE: &str = "SET client_connection_check_interval = '1s'";

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
        let ServerTimeouts { statement, idle } = read_server_timeouts(&client).await?;
        let statement_timeout = connection.statement_timeout().in_effect(statement);
        if connection.writes() == WritePolicy::Allowed {
            set_session_limits(&client, statement_timeout).await?;
        }
        let limits = ReadLimits { statement_timeout, idle };
        Ok(Self { client, tls, limits, ended, link, marks: Marks::default() })
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::{LocalPort, Value};
    use crate::fixtures::{self, run};
    use std::net::Ipv4Addr;

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

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_asks_the_server_to_stop_its_statement_once_pgbg_is_gone() {
        let result = run(WritePolicy::ReadOnly, "SHOW client_connection_check_interval").await.unwrap();
        assert_eq!(result.rows(), [[Value::Text("1s".into())]]);
    }
}
