//! Stopping a running statement: the server's cancel request, and how pgbg leaves a statement before it ends.
use super::model::CancelToken;
use super::server_error;
use crate::connection::Tls;
use anyhow::{Context, Result};
use std::pin::pin;
use std::time::Duration;
use tokio::time::timeout;
use tokio_postgres::NoTls;
use tokio_postgres_rustls::MakeRustlsConnect;

/// How long pgbg waits for the server to accept a cancel request as it stops.
/// A cancel is one connect and one message, so a server out of reach never holds pgbg up for long.
const CANCEL_WAIT: Duration = Duration::from_secs(1);

/// What stopped a statement before it ended, and whether the server took the cancel sent then.
pub(crate) struct Interrupted<S> {
    pub(crate) by: S,
    pub(crate) cancel: Result<()>,
}

impl CancelToken {
    /// Asks the server to cancel the statement the session is running; that statement then fails with the server's
    /// error. With nothing running, the server ignores the request.
    pub(crate) async fn cancel(&self) -> Result<()> {
        let sent = match self.tls {
            Tls::On => self.token.cancel_query(MakeRustlsConnect::with_webpki_roots()).await,
            Tls::Off => self.token.cancel_query(NoTls).await,
        };
        sent.map_err(server_error)
    }

    /// Like `cancel`, but gives up after `CANCEL_WAIT`, so a stop never waits long on the server.
    pub(crate) async fn cancel_as_stopping(&self) -> Result<()> {
        timeout(CANCEL_WAIT, self.cancel())
            .await
            .with_context(|| format!("the server did not take the cancel within {}s", CANCEL_WAIT.as_secs()))?
    }

    /// Runs `work` to its end, unless `stop` ends first: then the server is asked to cancel what `work` runs, and `work`
    /// is dropped so it sends nothing more. A cancel that reaches the server between two round trips does nothing, so
    /// only the drop keeps the rest of `work` (a `COMMIT`) from the server. The session, left mid-statement, then cuts
    /// its link when dropped.
    pub(crate) async fn unless_stopped<T, S>(
        &self,
        work: impl Future<Output = T>,
        stop: impl Future<Output = S>,
    ) -> Result<T, Interrupted<S>> {
        let mut work = pin!(work);
        let by = tokio::select! {
            done = &mut work => return Ok(done),
            by = stop => by,
        };
        let cancel = self.cancel_as_stopping().await;
        Err(Interrupted { by, cancel })
    }
}
