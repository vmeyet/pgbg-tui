use crate::connection::Tls;
use std::fmt;

/// What cancels the statement a session is running, from another task: it opens its own link to
/// the server, over the session's TLS.
#[derive(Clone)]
pub(crate) struct CancelToken {
    pub(in crate::db) token: tokio_postgres::CancelToken,
    pub(in crate::db) tls: Tls,
}

/// Leaves out the backend's key: it is all a cancel request needs.
impl fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelToken").field("tls", &self.tls).finish_non_exhaustive()
    }
}
