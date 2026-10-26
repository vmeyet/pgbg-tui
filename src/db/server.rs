//! What the server says of a session: who it is, its version, its timeout, what its role can write; read in
//! a read-only transaction, so asking writes nothing.
use super::model::{RoleFlag, RolePowers, RoleRights, ServerFacts};
use super::{Session, server_error};
use crate::connection::StatementTimeout;
use anyhow::{Context, Result};
use tokio_postgres::Row;

const SERVER_FACTS: &str = "SELECT current_user::text, current_setting('server_version'), setting::int8, reset_val::int8
    FROM pg_settings WHERE name = 'statement_timeout'";
/// Every power of the login role and of each role it is a member of, as `(role, power, object, count)`: the
/// first ten objects of each power, and how many it has. A superuser's memberships add nothing.
const ROLE_POWERS: &str = "WITH login AS (SELECT oid, rolsuper FROM pg_roles WHERE rolname = session_user),
member AS (
    SELECT r.oid, r.rolname::text AS role, r.rolsuper, r.rolcreaterole, r.rolcreatedb, r.rolbypassrls FROM pg_roles r, login
    WHERE r.oid = login.oid OR (NOT login.rolsuper AND pg_has_role(login.oid, r.oid, 'MEMBER'))
),
grantee AS (SELECT oid, role FROM member UNION ALL SELECT 0::oid, 'PUBLIC'),
relation AS (
    SELECT c.relkind, c.relacl, c.relowner, c.oid, format('%I.%I', n.nspname, c.relname) AS name
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'
),
held AS (
    SELECT role, 1 AS rank, 'superuser' AS power, NULL::text AS object FROM member WHERE rolsuper
    UNION SELECT role, 2, 'create_role', NULL FROM member WHERE rolcreaterole
    UNION SELECT role, 3, 'create_database', NULL FROM member WHERE rolcreatedb
    UNION SELECT role, 4, 'bypass_rls', NULL FROM member WHERE rolbypassrls
    UNION SELECT role, 5, 'write_all_data', NULL FROM member WHERE role = 'pg_write_all_data'
    UNION SELECT g.role, 6, 'create_schema', NULL
        FROM pg_database d, aclexplode(coalesce(d.datacl, acldefault('d', d.datdba))) a JOIN grantee g ON g.oid = a.grantee
        WHERE d.datname = current_database() AND a.privilege_type = 'CREATE'
    UNION SELECT g.role, 7, 'schema', n.nspname::text
        FROM pg_namespace n, aclexplode(coalesce(n.nspacl, acldefault('n', n.nspowner))) a JOIN grantee g ON g.oid = a.grantee
        WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' AND a.privilege_type = 'CREATE'
    UNION SELECT g.role, 8, 'table', r.name
        FROM relation r, aclexplode(coalesce(r.relacl, acldefault('r', r.relowner))) a JOIN grantee g ON g.oid = a.grantee
        WHERE r.relkind IN ('r', 'p', 'v', 'f') AND a.privilege_type IN ('INSERT', 'UPDATE', 'DELETE', 'TRUNCATE')
    UNION SELECT g.role, 8, 'table', r.name
        FROM relation r JOIN pg_attribute t ON t.attrelid = r.oid, aclexplode(t.attacl) a JOIN grantee g ON g.oid = a.grantee
        WHERE r.relkind IN ('r', 'p', 'v', 'f') AND a.privilege_type IN ('INSERT', 'UPDATE')
    UNION SELECT g.role, 9, 'sequence', r.name
        FROM relation r, aclexplode(coalesce(r.relacl, acldefault('s', r.relowner))) a JOIN grantee g ON g.oid = a.grantee
        WHERE r.relkind = 'S' AND a.privilege_type IN ('UPDATE', 'USAGE')
)
SELECT role, power, object, total FROM (
    SELECT held.*, count(*) OVER (PARTITION BY role, rank) AS total,
        row_number() OVER (PARTITION BY role, rank ORDER BY object) AS place
    FROM held
) listed
WHERE place <= 10
ORDER BY role <> session_user, role, rank, object";

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

    /// What the login role, and each role it may become, can write: its flags and the first objects it may change.
    pub(crate) async fn role_rights(&mut self) -> Result<RoleRights> {
        self.read_only(async |transaction| {
            let rows = transaction.query(ROLE_POWERS, &[]).await.map_err(server_error)?;
            Ok(RoleRights { roles: roles_holding(&rows)? })
        })
        .await
    }
}

/// One row of `ROLE_POWERS`: a power its role holds.
enum Held {
    Flag(RoleFlag),
    Schema(Listed),
    Table(Listed),
    Sequence(Listed),
}

/// One object of a kind, and how many of that kind the role holds.
struct Listed {
    name: String,
    count: u64,
}

fn held_at(row: &Row) -> Result<Held> {
    let power: &str = row.try_get(1)?;
    let flag = match power {
        "superuser" => RoleFlag::Superuser,
        "create_role" => RoleFlag::CreateRole,
        "create_database" => RoleFlag::CreateDatabase,
        "bypass_rls" => RoleFlag::BypassRls,
        "write_all_data" => RoleFlag::WriteAllData,
        "create_schema" => RoleFlag::CreateSchema,
        "schema" => return Ok(Held::Schema(listed_at(row)?)),
        "table" => return Ok(Held::Table(listed_at(row)?)),
        "sequence" => return Ok(Held::Sequence(listed_at(row)?)),
        other => anyhow::bail!("the server named an unknown power `{other}`"),
    };
    Ok(Held::Flag(flag))
}

fn listed_at(row: &Row) -> Result<Listed> {
    Ok(Listed { name: row.try_get(2)?, count: u64::try_from(row.try_get::<_, i64>(3)?)? })
}

/// The rows of `ROLE_POWERS`, gathered per role in their order.
fn roles_holding(rows: &[Row]) -> Result<Vec<RolePowers>> {
    let mut roles: Vec<RolePowers> = Vec::new();
    for row in rows {
        let role: String = row.try_get(0)?;
        if roles.last().is_none_or(|last| last.role != role) {
            roles.push(RolePowers { role, ..RolePowers::default() });
        }
        let powers = roles.last_mut().context("a role was just pushed")?;
        let (named, listed) = match held_at(row)? {
            Held::Flag(flag) => {
                powers.flags.push(flag);
                continue;
            }
            Held::Schema(listed) => (&mut powers.schemas, listed),
            Held::Table(listed) => (&mut powers.tables, listed),
            Held::Sequence(listed) => (&mut powers.sequences, listed),
        };
        named.first.push(listed.name);
        named.count = listed.count;
    }
    Ok(roles)
}

fn timeout_at(row: &Row, index: usize) -> Result<StatementTimeout> {
    let ms: i64 = row.try_get(index)?;
    Ok(StatementTimeout::from_millis(u64::try_from(ms).context("the server's statement timeout is negative")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::guard;
    use crate::db::model::{Route, Statement};
    use crate::fixtures;

    async fn session_as(user: Option<&str>, writes: &str) -> Session {
        let writes = format!("writes = \"{writes}\"\n");
        let connection = match user {
            Some(user) => fixtures::test_database_by(user, &writes),
            None => fixtures::test_database("local", &writes),
        };
        Session::open(&connection, Route::Direct).await.unwrap()
    }

    /// Runs `setup` as the test database's owner; each statement is safe to run twice.
    async fn as_owner(setup: &[String]) {
        let mut owner = session_as(None, "allowed").await;
        for sql in setup {
            owner.run(&guard::admit(Statement::new(sql.clone()), WritePolicy::Allowed).unwrap()).await.unwrap();
        }
    }

    fn create_role(role: &str, options: &str) -> String {
        format!("DO $$ BEGIN CREATE ROLE {role} {options}; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$")
    }

    /// A login role `role` that may read `table`, and do `grant` on it too when set.
    async fn role_on(role: &str, table: &str, grant: Option<&str>) -> Session {
        let setup = [
            create_role(role, "LOGIN"),
            format!("CREATE TABLE IF NOT EXISTS {table} (id int)"),
            format!("GRANT SELECT ON {table} TO {role}"),
        ];
        let granted = grant.map(|grant| format!("GRANT {grant} ON {table} TO {role}"));
        as_owner(&setup.into_iter().chain(granted).collect::<Vec<_>>()).await;
        session_as(Some(role), "read-only").await
    }

    async fn rights_of(role: &str) -> RoleRights {
        session_as(Some(role), "read-only").await.role_rights().await.unwrap()
    }

    fn powers_of(rights: &RoleRights, role: &str) -> RolePowers {
        rights.roles.iter().find(|powers| powers.role == role).cloned().unwrap_or_default()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_role_that_only_reads_can_write_nothing() {
        let rights = role_on("doctor_reader", "doctor_read_orders", None).await.role_rights().await.unwrap();
        assert_eq!(rights, RoleRights::default());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_role_granted_a_write_on_a_table_lists_that_table() {
        let rights = role_on("doctor_writer", "doctor_written_orders", Some("INSERT")).await.role_rights().await.unwrap();
        let powers = powers_of(&rights, "doctor_writer");
        assert_eq!(powers.tables.first, ["public.doctor_written_orders"], "{rights:?}");
        assert_eq!(powers.flags, []);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_role_granted_an_update_on_one_column_can_write_that_table() {
        let rights = role_on("doctor_column_writer", "doctor_column_orders", Some("UPDATE (id)")).await.role_rights().await.unwrap();
        assert_eq!(powers_of(&rights, "doctor_column_writer").tables.first, ["public.doctor_column_orders"]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_member_of_pg_write_all_data_names_it() {
        role_on("doctor_all_writer", "doctor_all_orders", None).await;
        as_owner(&["GRANT pg_write_all_data TO doctor_all_writer".to_owned()]).await;
        let rights = rights_of("doctor_all_writer").await;
        assert_eq!(powers_of(&rights, "pg_write_all_data").flags, [RoleFlag::WriteAllData], "{rights:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_superuser_says_so() {
        let rights = session_as(None, "read-only").await.role_rights().await.unwrap();
        assert_eq!(rights.roles[0].flags.first(), Some(&RoleFlag::Superuser), "{rights:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_superuser_login_that_sets_a_reading_role_still_says_it_is_a_superuser() {
        role_on("doctor_set_reader", "doctor_set_orders", None).await;
        let mut url = url::Url::parse(&fixtures::test_database_url()).unwrap();
        url.query_pairs_mut().append_pair("options", "-crole=doctor_set_reader");
        let connection = fixtures::connection(&format!("tier = \"local\"\nurl = \"{url}\"\n"));
        let rights = Session::open(&connection, Route::Direct).await.unwrap().role_rights().await.unwrap();
        assert_eq!(rights.roles[0].flags.first(), Some(&RoleFlag::Superuser), "{rights:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_login_that_inherits_nothing_names_the_writing_role_it_may_set() {
        role_on("doctor_unset_writer", "doctor_unset_orders", Some("INSERT")).await;
        as_owner(&[create_role("doctor_noinherit", "LOGIN NOINHERIT"), "GRANT doctor_unset_writer TO doctor_noinherit".to_owned()]).await;
        let rights = rights_of("doctor_noinherit").await;
        assert_eq!(powers_of(&rights, "doctor_unset_writer").tables.first, ["public.doctor_unset_orders"], "{rights:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_membership_granted_without_inherit_names_the_writing_role_it_may_set() {
        role_on("doctor_set_writer", "doctor_set_written_orders", Some("DELETE")).await;
        as_owner(&[
            create_role("doctor_unheriting", "LOGIN"),
            "GRANT doctor_set_writer TO doctor_unheriting WITH INHERIT FALSE".to_owned(),
        ])
        .await;
        let rights = rights_of("doctor_unheriting").await;
        assert_eq!(powers_of(&rights, "doctor_set_writer").tables.first, ["public.doctor_set_written_orders"], "{rights:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_role_that_may_create_in_a_schema_names_that_schema() {
        let setup = [create_role("doctor_schema_creator", "LOGIN"), "CREATE SCHEMA IF NOT EXISTS doctor_billing".to_owned()];
        as_owner(&[&setup[..], &["GRANT CREATE ON SCHEMA doctor_billing TO doctor_schema_creator".to_owned()]].concat()).await;
        let rights = rights_of("doctor_schema_creator").await;
        assert_eq!(powers_of(&rights, "doctor_schema_creator").schemas.first, ["doctor_billing"], "{rights:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_role_that_may_advance_a_sequence_names_that_sequence() {
        let setup = [
            create_role("doctor_sequence_user", "LOGIN"),
            "CREATE SEQUENCE IF NOT EXISTS doctor_order_ids".to_owned(),
            "GRANT USAGE ON SEQUENCE doctor_order_ids TO doctor_sequence_user".to_owned(),
        ];
        as_owner(&setup).await;
        let rights = rights_of("doctor_sequence_user").await;
        assert_eq!(powers_of(&rights, "doctor_sequence_user").sequences.first, ["public.doctor_order_ids"], "{rights:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_role_that_may_create_schemas_in_the_database_says_so() {
        let grant = "DO $$ BEGIN EXECUTE format('GRANT CREATE ON DATABASE %I TO doctor_database_creator', current_database()); END $$";
        as_owner(&[create_role("doctor_database_creator", "LOGIN"), grant.to_owned()]).await;
        let rights = rights_of("doctor_database_creator").await;
        assert_eq!(powers_of(&rights, "doctor_database_creator").flags, [RoleFlag::CreateSchema], "{rights:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_facts_name_the_user_the_version_and_the_timeout_in_effect() {
        let mut url = url::Url::parse(&fixtures::test_database_url()).unwrap();
        url.query_pairs_mut().append_pair("options", "-cstatement_timeout=2min");
        let connection = fixtures::connection(&format!("tier = \"staging\"\ntls = \"off\"\nurl = \"{url}\"\n"));
        let mut session = Session::open(&connection, Route::Direct).await.unwrap();
        let facts = session.server_facts().await.unwrap();
        assert_eq!(facts.user, url.username());
        assert_eq!((facts.statement_timeout.as_millis(), facts.server_statement_timeout.as_millis()), (60_000, 120_000));
        assert!(facts.version.starts_with(|c: char| c.is_ascii_digit()), "{}", facts.version);
    }
}
