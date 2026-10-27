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

/// What the session could write, through the role it logged in as or any role it may become with `SET ROLE`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RoleRights {
    /// Each role that holds a power, the login role first; `PUBLIC` holds what every role has.
    pub(crate) roles: Vec<RolePowers>,
}

/// What one role may write itself, not counting the roles it is a member of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RolePowers {
    pub(crate) role: String,
    pub(crate) flags: Vec<RoleFlag>,
    /// The schemas it may create objects in.
    pub(crate) schemas: Named,
    /// The tables it may insert into, update, delete from or truncate, as `schema.table`.
    pub(crate) tables: Named,
    /// The sequences it may advance or set, as `schema.sequence`.
    pub(crate) sequences: Named,
}

/// A power a role holds on the whole server or database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RoleFlag {
    Superuser,
    CreateRole,
    CreateDatabase,
    BypassRls,
    /// `pg_write_all_data` itself, which writes every table.
    WriteAllData,
    /// `CREATE` on the session's database.
    CreateSchema,
}

/// The first objects of a kind, by name, and how many there are past those too.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Named {
    pub(crate) first: Vec<String>,
    pub(crate) count: u64,
}

/// What the session's role can do that a read-only transaction does not stop: through the login role or any role it
/// may become with `SET ROLE`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReadOnlyGaps {
    /// It may cancel or end other sessions, through `pg_signal_backend`.
    pub(crate) signals_sessions: bool,
    /// The `dblink` functions it may call, each named once whatever its overloads: each opens a session of its own that
    /// may write.
    pub(crate) dblink_functions: Vec<String>,
    /// The settings only a superuser could set, granted it with `GRANT SET ON PARAMETER`.
    pub(crate) granted_settings: Vec<String>,
}
