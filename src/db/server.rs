//! What the server says of a session: who it is, its version, its timeout, what its role can write; read in
//! a read-only transaction, so asking writes nothing.
use super::model::{RoleRights, ServerFacts};
use super::{Session, server_error};
use crate::connection::StatementTimeout;
use anyhow::{Context, Result};
use tokio_postgres::Row;

const SERVER_FACTS: &str = "SELECT current_user::text, current_setting('server_version'), setting::int8, reset_val::int8
    FROM pg_settings WHERE name = 'statement_timeout'";
const ROLE_FLAGS: &str = "SELECT rolsuper, rolcreaterole, rolcreatedb, rolbypassrls,
    coalesce(pg_has_role(current_user, to_regrole('pg_write_all_data'), 'MEMBER'), false)
    FROM pg_roles WHERE rolname = current_user";
const WRITABLE_TABLES: &str = "SELECT format('%I.%I', n.nspname, c.relname), count(*) OVER ()
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE c.relkind IN ('r', 'p', 'v', 'f') AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'
    AND has_schema_privilege(n.oid, 'USAGE')
    AND (has_table_privilege(c.oid, 'INSERT, UPDATE, DELETE, TRUNCATE') OR has_any_column_privilege(c.oid, 'INSERT, UPDATE'))
    ORDER BY 1 LIMIT 10";

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

    /// What the session's role can write: its flags, `pg_write_all_data`, and the first tables it may change.
    pub(crate) async fn role_rights(&mut self) -> Result<RoleRights> {
        self.read_only(async |transaction| {
            let flags = transaction.query_one(ROLE_FLAGS, &[]).await.map_err(server_error)?;
            let tables = transaction.query(WRITABLE_TABLES, &[]).await.map_err(server_error)?;
            let writable_tables = tables.iter().map(|row| row.try_get(0)).collect::<Result<_, _>>()?;
            let writable_table_count = tables.first().map_or(Ok(0), |row| row.try_get::<_, i64>(1))?;
            Ok(RoleRights {
                is_superuser: flags.try_get(0)?,
                can_create_role: flags.try_get(1)?,
                can_create_db: flags.try_get(2)?,
                can_bypass_rls: flags.try_get(3)?,
                can_write_all_data: flags.try_get(4)?,
                writable_tables,
                writable_table_count: u64::try_from(writable_table_count)?,
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
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::guard;
    use crate::db::model::{Route, Statement};
    use crate::fixtures;

    async fn session_as(user: Option<&str>, writes: &str) -> Session {
        let mut url = url::Url::parse(&fixtures::test_database_url()).unwrap();
        if let Some(user) = user {
            url.set_username(user).unwrap();
        }
        let connection = fixtures::connection(&format!("tier = \"local\"\nurl = \"{url}\"\nwrites = \"{writes}\"\n"));
        Session::open(&connection, Route::Direct).await.unwrap()
    }

    /// A login role `role` that may read `table`, and do `grant` on it too when set; safe to run twice.
    async fn role_on(role: &str, table: &str, grant: Option<&str>) -> Session {
        let mut owner = session_as(None, "allowed").await;
        let setup = [
            format!("DO $$ BEGIN CREATE ROLE {role} LOGIN; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"),
            format!("CREATE TABLE IF NOT EXISTS {table} (id int)"),
            format!("GRANT SELECT ON {table} TO {role}"),
        ];
        let granted = grant.map(|grant| format!("GRANT {grant} ON {table} TO {role}"));
        for sql in setup.into_iter().chain(granted) {
            owner.run(&guard::admit(Statement::new(sql), WritePolicy::Allowed).unwrap()).await.unwrap();
        }
        session_as(Some(role), "read-only").await
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
        assert!(rights.writable_tables.contains(&"public.doctor_written_orders".to_owned()), "{rights:?}");
        assert!(!rights.is_superuser);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_role_granted_an_update_on_one_column_can_write_that_table() {
        let rights = role_on("doctor_column_writer", "doctor_column_orders", Some("UPDATE (id)")).await.role_rights().await.unwrap();
        assert_eq!(rights.writable_tables, ["public.doctor_column_orders"]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_member_of_pg_write_all_data_says_so() {
        role_on("doctor_all_writer", "doctor_all_orders", None).await;
        let mut owner = session_as(None, "allowed").await;
        let grant = guard::admit(Statement::new("GRANT pg_write_all_data TO doctor_all_writer"), WritePolicy::Allowed).unwrap();
        owner.run(&grant).await.unwrap();
        let rights = session_as(Some("doctor_all_writer"), "read-only").await.role_rights().await.unwrap();
        assert!(rights.can_write_all_data && rights.writable_table_count >= 1, "{rights:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_superuser_says_so_and_counts_every_table() {
        role_on("doctor_owner", "doctor_owned_orders", None).await;
        let rights = session_as(None, "read-only").await.role_rights().await.unwrap();
        assert!(rights.is_superuser && rights.writable_table_count >= 1, "{rights:?}");
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
