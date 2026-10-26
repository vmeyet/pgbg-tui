//! The only module that sees `tokio_postgres`: connect, run one statement, decode its rows.
use crate::connection::{Connection, Target, Tls};
use anyhow::{Context, Result, anyhow};
use futures_util::{TryStreamExt, future};
use guard::{Admitted, Mode};
use model::ResultSet;
use tokio_postgres::config::SslMode;
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{Client, Config, GenericClient, NoTls, Socket};
use tokio_postgres_rustls::MakeRustlsConnect;

mod decode;
pub(crate) mod guard;
pub(crate) mod model;

const NO_PARAMETERS: [&str; 0] = [];

/// An open link to one connection.
#[derive(Debug)]
pub(crate) struct Session {
    client: Client,
}

impl Session {
    /// Logs in to `connection`, reading its password just for that.
    pub(crate) async fn open(connection: &Connection) -> Result<Self> {
        let mut config = driver_config(connection.target())?;
        if let Some(source) = connection.password() {
            config.password(source.read().await?.expose());
        }
        let client = match connection.tls() {
            Tls::On => open_client(config.ssl_mode(SslMode::Require), MakeRustlsConnect::with_webpki_roots()).await,
            Tls::Off => open_client(config.ssl_mode(SslMode::Disable), NoTls).await,
        };
        Ok(Self { client: client.with_context(|| format!("connecting to {}", connection.name()))? })
    }

    /// Runs `admitted` alone and returns everything it printed; a read-only one is always rolled back.
    pub(crate) async fn run(&mut self, admitted: &Admitted) -> Result<ResultSet> {
        let sql = admitted.statement.as_str();
        match admitted.mode {
            Mode::Writes => fetch(&self.client, sql).await,
            Mode::ReadOnly => {
                let transaction = self.client.build_transaction().read_only(true).start().await.map_err(server_error)?;
                let result = fetch(&transaction, sql).await;
                transaction.rollback().await.map_err(server_error)?;
                result
            }
        }
    }
}

/// The driver's config for `target`; `sslmode` is left out, since `Tls` decides it.
fn driver_config(target: &Target) -> Result<Config> {
    let mut url = target.url().clone();
    let kept: Vec<(String, String)> =
        url.query_pairs().filter(|(key, _)| key != "sslmode").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    url.query_pairs_mut().clear().extend_pairs(kept);
    url.as_str().trim_end_matches('?').parse().map_err(|err| anyhow!("`url` is not a PostgreSQL URL: {err}"))
}

async fn open_client<T>(config: &Config, tls: T) -> Result<Client>
where
    T: MakeTlsConnect<Socket>,
    T::Stream: Send + 'static,
{
    let (client, connection) = config.connect(tls).await.map_err(server_error)?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::warn!("the connection closed: {err}");
        }
    });
    Ok(client)
}

/// Prepares `sql` first, so a result without rows still has its columns, then streams the rows.
async fn fetch(client: &impl GenericClient, sql: &str) -> Result<ResultSet> {
    let statement = client.prepare(sql).await.map_err(server_error)?;
    let columns = statement.columns().iter().map(decode::column).collect();
    let rows = client.query_raw(&statement, NO_PARAMETERS).await.map_err(server_error)?;
    let rows = rows.and_then(|row| future::ready(decode::row(&row))).try_collect().await.map_err(server_error)?;
    Ok(ResultSet { columns, rows })
}

/// The server's own words, `ERROR: relation "orders" does not exist`, with its detail and hint.
fn server_error(err: tokio_postgres::Error) -> anyhow::Error {
    let Some(db) = err.as_db_error() else { return anyhow::Error::new(err) };
    let lines = [Some(format!("{}: {}", db.severity(), db.message())), db.detail().map(str::to_owned), db.hint().map(str::to_owned)];
    anyhow!(lines.into_iter().flatten().collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{Entry, WritePolicy};
    use model::{Statement, Value};

    const DATABASE_URL: &str = "PGBG_TEST_DATABASE_URL";

    async fn session(writes: WritePolicy) -> Session {
        let url = std::env::var(DATABASE_URL).unwrap();
        let writes = if writes == WritePolicy::Allowed { "allowed" } else { "read-only" };
        let entry: Entry = toml::from_str(&format!("tier = \"local\"\nurl = \"{url}\"\nwrites = \"{writes}\"\n")).unwrap();
        Session::open(&Connection::new("shop", entry).unwrap()).await.unwrap()
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
        let config = driver_config(Connection::new("shop", entry).unwrap().target()).unwrap();
        assert_eq!(config.get_application_name(), Some("pgbg"));
        assert_eq!(config.get_dbname(), Some("shop"));
        assert_eq!(config.get_ports(), [6432]);
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
    async fn a_server_error_says_what_the_server_said() {
        let err = run(WritePolicy::ReadOnly, "SELECT * FROM no_such_orders").await.unwrap_err();
        assert!(err.to_string().contains("relation \"no_such_orders\" does not exist"), "{err}");
    }
}
