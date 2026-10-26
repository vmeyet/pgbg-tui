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

/// What the session's role could write, whatever transaction it runs in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RoleRights {
    pub(crate) is_superuser: bool,
    pub(crate) can_create_role: bool,
    pub(crate) can_create_db: bool,
    pub(crate) can_bypass_rls: bool,
    /// A member of `pg_write_all_data`, which writes every table.
    pub(crate) can_write_all_data: bool,
    /// The first tables it may insert into, update, delete from or truncate, as `schema.table`.
    pub(crate) writable_tables: Vec<String>,
    /// How many tables it may write, past the ones listed too.
    pub(crate) writable_table_count: u64,
}
