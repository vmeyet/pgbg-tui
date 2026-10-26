//! What the server says of a session: who it is, its version, its timeout, read in one read-only transaction.
use super::model::ServerFacts;
use super::{Session, server_error};
use crate::connection::StatementTimeout;
use anyhow::{Context, Result};
use tokio_postgres::Row;

const SERVER_FACTS: &str = "SELECT current_user::text, current_setting('server_version'), setting::int8, reset_val::int8
    FROM pg_settings WHERE name = 'statement_timeout'";

impl Session {
    /// Who the session is, the server's version, and the timeout in effect beside the one the server applied before pgbg's.
    pub(crate) async fn server_facts(&mut self) -> Result<ServerFacts> {
        self.read_only(async |transaction| {
            let row = transaction.query_one(SERVER_FACTS, &[]).await.map_err(server_error)?;
            Ok(ServerFacts {
                user: row.try_get(0)?,
                version: row.try_get(1)?,
                statement_timeout: timeout_at(&row, 2)?,
                server_statement_timeout: timeout_at(&row, 3)?,
            })
        })
        .await
    }
}

fn timeout_at(row: &Row, index: usize) -> Result<StatementTimeout> {
    let ms: i64 = row.try_get(index)?;
    Ok(StatementTimeout::from_millis(u64::try_from(ms).context("the server's statement timeout is negative")?))
}

#[cfg(test)]
mod tests {
    use crate::connection::{Connection, Entry};
    use crate::db::Session;
    use crate::db::model::Route;

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_facts_name_the_user_the_version_and_the_timeout_in_effect() {
        let mut url = url::Url::parse(&std::env::var("PGBG_TEST_DATABASE_URL").unwrap()).unwrap();
        url.query_pairs_mut().append_pair("options", "-cstatement_timeout=2min");
        let entry: Entry = toml::from_str(&format!("tier = \"staging\"\ntls = \"off\"\nurl = \"{url}\"\n")).unwrap();
        let mut session = Session::open(&Connection::new("shop", entry).unwrap(), Route::Direct).await.unwrap();
        let facts = session.server_facts().await.unwrap();
        assert_eq!(facts.user, url.username());
        assert_eq!((facts.statement_timeout.as_millis(), facts.server_statement_timeout.as_millis()), (60_000, 120_000));
        assert!(facts.version.starts_with(|c: char| c.is_ascii_digit()), "{}", facts.version);
    }
}
