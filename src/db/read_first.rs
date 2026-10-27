//! A read on a connection that allows writes runs with writes refused: a write hidden in it, as in a function that
//! writes, fails before anything is kept, and runs again as a write only once the user confirms it.
//! Inside the user's own transaction a read runs as a write would: their `COMMIT` or `ROLLBACK` decides what stays.
use super::guard::Admitted;
use super::model::{ResultSet, ServerError, TriedToWrite};
use super::run::Access;
use super::{Session, bind};
use anyhow::Result;

impl Session {
    /// Runs `admitted`, a read, in a read-only transaction of pgbg's, committed so a setting it changes stays; one the
    /// server refuses as a write fails as `TriedToWrite`.
    /// Inside the user's transaction it runs as a write would, unchecked: a savepoint around it would undo a lock, a
    /// cursor or a setting it made.
    pub(super) async fn run_read_first(&mut self, admitted: &Admitted) -> Result<ResultSet> {
        if self.is_in_user_transaction().await? {
            return self.run_writes(admitted).await;
        }
        let statement = bind::prepare(&self.client, admitted).await?;
        self.fetch_in_own_transaction(&statement, &admitted.params, Access::ReadOnly).await.map_err(as_tried_to_write)
    }
}

fn as_tried_to_write(err: anyhow::Error) -> anyhow::Error {
    let is_refused_write = err.downcast_ref::<ServerError>().is_some_and(|server| server.is_refused_write);
    if is_refused_write { TriedToWrite.into() } else { err }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::guard;
    use crate::db::model::{ParamValue, Params, Statement, TypeName, Value};
    use crate::fixtures::{run, run_on, session};

    /// A fresh `table` and a function `add_to_<table>()` that inserts a row into it, called by a `SELECT`.
    async fn table_and_writing_function(table: &str) -> String {
        run(WritePolicy::Allowed, &format!("DROP TABLE IF EXISTS {table}")).await.unwrap();
        run(WritePolicy::Allowed, &format!("CREATE TABLE {table} (id int)")).await.unwrap();
        let function = format!(
            "CREATE OR REPLACE FUNCTION add_to_{table}() RETURNS int LANGUAGE sql AS $$ INSERT INTO {table} VALUES (1) RETURNING id $$"
        );
        run(WritePolicy::Allowed, &function).await.unwrap();
        format!("SELECT add_to_{table}()")
    }

    async fn rows_of(table: &str) -> Vec<Vec<Value>> {
        run(WritePolicy::ReadOnly, &format!("SELECT id FROM {table} ORDER BY id")).await.unwrap().rows().to_vec()
    }

    fn admitted(sql: &str) -> Admitted {
        guard::admit(Statement::new(sql), Params::default(), WritePolicy::Allowed).unwrap()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_function_that_writes_fails_as_a_write_to_confirm() {
        let call = table_and_writing_function("orders_hidden").await;
        let err = run(WritePolicy::Allowed, &call).await.unwrap_err();
        assert_eq!(err.downcast_ref(), Some(&TriedToWrite), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_function_that_writes_keeps_nothing_before_it_is_confirmed() {
        let call = table_and_writing_function("orders_hidden_unkept").await;
        assert!(run(WritePolicy::Allowed, &call).await.is_err());
        assert_eq!(rows_of("orders_hidden_unkept").await, Vec::<Vec<Value>>::new());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_confirmed_function_that_writes_runs_once_and_is_committed() {
        let call = table_and_writing_function("orders_hidden_confirmed").await;
        assert_eq!(session(WritePolicy::Allowed).await.run(&admitted(&call).confirmed()).await.unwrap().rows(), [[Value::Int(1)]]);
        assert_eq!(rows_of("orders_hidden_confirmed").await, [[Value::Int(1)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_plain_read_runs_without_asking() {
        assert_eq!(run(WritePolicy::Allowed, "SELECT 41 + 1").await.unwrap().rows(), [[Value::Int(42)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_setting_a_read_changes_stays_on_the_session() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "SELECT set_config('application_name', 'nina_shop', false)").await;
        assert_eq!(run_on(&mut writer, "SHOW application_name").await.rows(), [[Value::Text("nina_shop".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_function_that_writes_inside_the_users_transaction_runs_without_asking() {
        let call = table_and_writing_function("orders_hidden_inside").await;
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        assert_eq!(run_on(&mut writer, &call).await.rows(), [[Value::Int(1)]]);
        run_on(&mut writer, "COMMIT").await;
        assert_eq!(rows_of("orders_hidden_inside").await, [[Value::Int(1)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_lock_a_read_takes_inside_the_users_transaction_stays_held() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "SELECT pg_advisory_xact_lock(161042)").await;
        let mut other = session(WritePolicy::Allowed).await;
        assert_eq!(run_on(&mut other, "SELECT pg_try_advisory_lock(161042)").await.rows(), [[Value::Bool(false)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_cursor_a_function_opens_inside_the_users_transaction_can_be_fetched() {
        let function = "CREATE OR REPLACE FUNCTION open_orders_cursor() RETURNS refcursor LANGUAGE plpgsql AS \
            $$ DECLARE found refcursor := 'nina_orders'; BEGIN OPEN found FOR SELECT 7; RETURN found; END $$";
        run(WritePolicy::Allowed, function).await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "SELECT open_orders_cursor()").await;
        assert_eq!(run_on(&mut writer, "FETCH ALL FROM nina_orders").await.rows(), [[Value::Int(7)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_setting_a_read_changes_inside_the_users_transaction_stays_until_it_ends() {
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "SELECT set_config('shop.owner', 'nina', true)").await;
        assert_eq!(run_on(&mut writer, "SELECT current_setting('shop.owner')").await.rows(), [[Value::Text("nina".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_failing_before_the_server_runs_it_leaves_the_users_transaction_writable() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_unknown_type").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_unknown_type (id int)").await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        let params = Params::new(vec![Some(ParamValue::new("7"))], vec![Some(TypeName::new("no_such_type"))]);
        let typed = guard::admit(Statement::new("SELECT $1"), params, WritePolicy::Allowed).unwrap();
        assert!(writer.run(&typed).await.is_err());
        run_on(&mut writer, "INSERT INTO orders_unknown_type VALUES (7)").await;
        run_on(&mut writer, "COMMIT").await;
        assert_eq!(rows_of("orders_unknown_type").await, [[Value::Int(7)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_inside_the_users_transaction_leaves_it_writable() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS orders_read_inside").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE orders_read_inside (id int)").await.unwrap();
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "SELECT count(*) FROM orders_read_inside").await;
        run_on(&mut writer, "INSERT INTO orders_read_inside VALUES (7)").await;
        run_on(&mut writer, "COMMIT").await;
        assert_eq!(rows_of("orders_read_inside").await, [[Value::Int(7)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_confirmed_function_inside_the_users_transaction_is_left_to_the_user() {
        let call = table_and_writing_function("orders_hidden_undone").await;
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        writer.run(&admitted(&call).confirmed()).await.unwrap();
        run_on(&mut writer, "ROLLBACK").await;
        assert_eq!(rows_of("orders_hidden_undone").await, Vec::<Vec<Value>>::new());
    }
}
