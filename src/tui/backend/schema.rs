//! The schema of a connection, read on a second, short session along the route of its open one: the session task polls
//! it beside its statements, so they never wait on it, and drops it when the task stops.
use super::session::SessionAnswers;
use crate::complete::Index;
use crate::connection::Connection;
use crate::db::model::{Catalog, Route};
use crate::db::read_schema_along;
use crate::tui::app::Incoming;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;

/// What reads the schema each time it is asked, along the route the session task says its open session takes.
#[derive(Debug)]
pub(super) struct SchemaReader {
    pub(super) connection: Connection,
    pub(super) answers: SessionAnswers,
    pub(super) asks: UnboundedReceiver<()>,
    pub(super) route: watch::Receiver<Option<Route>>,
}

impl SchemaReader {
    /// Reads the schema at each ask once the session is open, the asks that came meanwhile served by the same read; a
    /// failed read is answered in its words, and the session stays as it is. Never ends: the session task drops it.
    pub(super) async fn serve(mut self) {
        while self.asks.recv().await.is_some() {
            let Some(route) = self.open_route().await else { break };
            while self.asks.try_recv().is_ok() {}
            let read = answer(&self.connection, read_schema_along(&self.connection, route).await);
            let catalog = read.as_ref().ok().map(Arc::clone);
            self.answers.say(Incoming::Schema(read));
            if let Some(catalog) = catalog {
                self.say_index(catalog).await;
            }
        }
        std::future::pending::<()>().await;
    }

    /// Builds the completion index of `catalog` on a blocking thread, and says it.
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

fn answer(connection: &Connection, read: anyhow::Result<Catalog>) -> Result<Arc<Catalog>, String> {
    read.map(Arc::new).map_err(|err| {
        tracing::warn!("the schema of `{}` was not read: {err:#}", connection.name());
        format!("{err:#}")
    })
}
