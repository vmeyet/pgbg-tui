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
