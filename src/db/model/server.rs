use crate::connection::StatementTimeout;

/// What the server says of the session pgbg opened on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerFacts {
    /// The role the session logged in as.
    pub(crate) user: String,
    /// `17.2`, as `server_version` spells it.
    pub(crate) version: String,
    /// The `statement_timeout` the session runs with.
    pub(crate) statement_timeout: StatementTimeout,
    /// The one the server applies before pgbg sets its own: a role's or a database's default, or the URL's `options`.
    pub(crate) server_statement_timeout: StatementTimeout,
}
