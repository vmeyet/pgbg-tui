//! The schema of a connection, read on a second, short session that takes the same route as the open one.
//! The session task polls the reader next to its statements, so they never wait on it, and drops it when the task stops.
use super::session::SessionAnswers;
use crate::complete::Index;
use crate::connection::Connection;
use crate::db::model::{Catalog, Route};
use crate::db::read_schema_along;
use crate::failure::Failure;
use crate::tui::app::Incoming;
use futures_util::FutureExt;
use std::sync::Arc;
use tokio::sync::{Notify, watch};

/// Reads the schema each time it is asked, along the route the session task reports for its open session.
#[derive(Debug)]
pub(super) struct SchemaReader {
    pub(super) connection: Connection,
    pub(super) answers: SessionAnswers,
    /// Shared with the session link, which wakes the reader at each ask.
    pub(super) asks: Arc<Notify>,
    pub(super) route: watch::Receiver<Option<Route>>,
}

impl SchemaReader {
    /// Reads the schema at each ask once the session is open; asks that arrive during a read are served by that read.
    /// A failed read answers why, and the session stays as it is.
    /// It never ends: the session task drops it.
    pub(super) async fn serve(mut self) {
        loop {
            self.asks.notified().await;
            let Some(route) = self.open_route().await else { break };
            self.drop_asks_served_by_this_read();
            let read = answer(&self.connection, read_schema_along(&self.connection, route).await);
            let catalog = read.as_ref().ok().map(Arc::clone);
            self.answers.say(Incoming::Schema(read));
            if let Some(catalog) = catalog {
                self.say_index(catalog).await;
            }
        }
        std::future::pending::<()>().await;
    }

    /// Forgets an ask that came while the session was opening: the read about to start serves it.
    fn drop_asks_served_by_this_read(&self) {
        self.asks.notified().now_or_never();
    }

    /// Builds the completion index of `catalog` on a blocking thread, and sends it.
    async fn say_index(&self, catalog: Arc<Catalog>) {
        let read = Arc::clone(&catalog);
        match tokio::task::spawn_blocking(move || Index::new(&read)).await {
            Ok(index) => self.answers.say(Incoming::Index { catalog, index: Arc::new(index) }),
            Err(err) => tracing::warn!("the completion index of `{}` was not built: {err}", self.connection.name()),
        }
    }

    /// The route of the session once it is open; none once the session task is gone.
    async fn open_route(&mut self) -> Option<Route> {
        *self.route.wait_for(Option::is_some).await.ok()?
    }
}

fn answer(connection: &Connection, read: anyhow::Result<Catalog>) -> Result<Arc<Catalog>, Failure> {
    read.map(Arc::new).map_err(|err| {
        tracing::warn!("the schema of `{}` was not read: {err:#}", connection.name());
        Failure::from(&err)
    })
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{SLEEP, answer_to, answers_until, next_answer, shop, started, started_on, until_sleeping};
    use super::*;
    use crate::db::model::Statement;
    use crate::fixtures;
    use crate::query::params::ParamInput;
    use crate::tui::app::{RunId, SessionId, TunnelState};

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_is_read_while_a_statement_runs() {
        let (backend, mut answers) = started();
        backend.run(&shop(), RunId::FIRST, Statement::new(SLEEP), ParamInput::default(), None);
        until_sleeping().await;
        let _reading = fixtures::reading_the_catalog().await;
        backend.read_schema(&shop(), SessionId::FIRST);
        let answer = next_answer(&mut answers).await;
        assert!(matches!(&answer, Incoming::Schema(Ok(catalog)) if !catalog.search_path.is_empty()), "{answer:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_completion_index_of_a_schema_read_follows_it() {
        let (backend, mut answers) = started();
        let _reading = fixtures::reading_the_catalog().await;
        backend.read_schema(&shop(), SessionId::FIRST);
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::Index { .. })).await;
        let read = seen.iter().find_map(|answer| if let Incoming::Schema(Ok(catalog)) = answer { Some(catalog) } else { None });
        let is_built_from_it =
            matches!((read, seen.last()), (Some(read), Some(Incoming::Index { catalog, .. })) if Arc::ptr_eq(read, catalog));
        assert!(is_built_from_it, "{seen:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_is_read_through_the_tunnel_of_the_session() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let marker = "pgbg-test-schema-through-the-tunnel";
        let (backend, mut answers) = started_on(fixtures::tunnelled(marker));
        let _reading = fixtures::reading_the_catalog().await;
        backend.read_schema(&shop(), SessionId::FIRST);
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::Schema(_))).await;
        let tunnels_started = seen.iter().filter(|answer| **answer == Incoming::Tunnel(TunnelState::Starting)).count();
        assert!(matches!(seen.last(), Some(Incoming::Schema(Ok(_)))) && tunnels_started == 1, "{seen:?}");
    }

    /// `shop` on the test database, its password from the shell line `line`, which gets `file` as `$1`.
    fn with_password_command(line: &str, file: &std::path::Path) -> Connection {
        let command = format!("password = {{ command = [\"sh\", \"-c\", {line:?}, \"sh\", {:?}] }}\n", file.display().to_string());
        fixtures::test_database("local", &command)
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_session_reads_the_password_again() {
        let dir = tempfile::tempdir().unwrap();
        let reads = dir.path().join("reads");
        let (backend, mut answers) = started_on(with_password_command("echo read >> \"$1\"; echo nina-password", &reads));
        let _reading = fixtures::reading_the_catalog().await;
        backend.read_schema(&shop(), SessionId::FIRST);
        answers_until(&mut answers, |answer| matches!(answer, Incoming::Schema(_))).await;
        assert_eq!(std::fs::read_to_string(&reads).unwrap(), "read\nread\n");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_schema_read_that_fails_leaves_the_session_open() {
        let dir = tempfile::tempdir().unwrap();
        let once = "[ -e \"$1\" ] && exit 1; touch \"$1\"; echo nina-password";
        let (backend, mut answers) = started_on(with_password_command(once, &dir.path().join("read")));
        let _reading = fixtures::reading_the_catalog().await;
        backend.read_schema(&shop(), SessionId::FIRST);
        let seen = answers_until(&mut answers, |answer| matches!(answer, Incoming::Schema(_))).await;
        assert!(matches!(seen.last(), Some(Incoming::Schema(Err(_)))), "{seen:?}");
        let after = answer_to(&backend, &mut answers, RunId::FIRST, "SELECT 1").await;
        assert!(matches!(after, Incoming::Result { .. }), "{after:?}");
    }
}
