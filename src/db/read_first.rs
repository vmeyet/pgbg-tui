//! A read on a connection that allows writes runs with writes refused: a write hidden in it, as in a function that
//! writes, fails before anything is kept, and runs again as a write only once the user confirms it.
use super::guard::Admitted;
use super::model::{ResultSet, ServerError, TriedToWrite};
use super::run::{Access, fetch};
use super::{Session, bind, server_error};
use anyhow::Result;

/// A transaction cannot turn writable again once it has read, so the read-only setting lives in a savepoint, and only
/// rolling back to it ends the setting.
const READ_ONLY_SAVEPOINT: &str = "SAVEPOINT pgbg_read; SET LOCAL transaction_read_only = on";
const LEAVE_SAVEPOINT: &str = "ROLLBACK TO SAVEPOINT pgbg_read; RELEASE SAVEPOINT pgbg_read";

impl Session {
    /// Runs `admitted`, a read, with writes refused; one the server refuses as a write fails as `TriedToWrite`.
    /// Outside the user's transaction it runs in a read-only one of pgbg's, committed so a setting it changes stays.
    pub(super) async fn run_read_first(&mut self, admitted: &Admitted) -> Result<ResultSet> {
        if self.is_in_user_transaction().await? {
            return self.read_in_savepoint(admitted).await;
        }
        let statement = bind::prepare(&self.client, admitted).await?;
        self.fetch_in_own_transaction(&statement, &admitted.params, Access::ReadOnly).await.map_err(as_tried_to_write)
    }

    /// Runs `admitted` in a read-only savepoint of the user's transaction, then rolls back to it: a cursor a function
    /// opened or a setting it changed goes with it.
    /// A read that fails for another reason leaves the user's transaction failed, as any failed statement does.
    async fn read_in_savepoint(&self, admitted: &Admitted) -> Result<ResultSet> {
        self.client.batch_execute(READ_ONLY_SAVEPOINT).await.map_err(server_error)?;
        let read = self.read_in_users_transaction(admitted).await.map_err(as_tried_to_write);
        let is_clean_end = match &read {
            Ok(_) => true,
            Err(err) => err.is::<TriedToWrite>(),
        };
        if is_clean_end {
            self.client.batch_execute(LEAVE_SAVEPOINT).await.map_err(server_error)?;
        }
        read
    }

    async fn read_in_users_transaction(&self, admitted: &Admitted) -> Result<ResultSet> {
        let statement = bind::prepare(&self.client, admitted).await?;
        if admitted.is_query {
            return self.fetch_through_cursor(&statement, admitted).await;
        }
        fetch(&self.client, &statement, &admitted.params).await
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
    use crate::db::model::{Params, Statement, Value};
    use crate::fixtures::{run, run_on, session, try_on};

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
    async fn a_function_that_writes_inside_the_users_transaction_fails_as_a_write_to_confirm() {
        let call = table_and_writing_function("orders_hidden_inside").await;
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        let err = try_on(&mut writer, &call).await.unwrap_err();
        assert_eq!(err.downcast_ref(), Some(&TriedToWrite), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_write_refused_inside_the_users_transaction_leaves_it_open_and_writable() {
        let call = table_and_writing_function("orders_hidden_kept").await;
        let mut writer = session(WritePolicy::Allowed).await;
        run_on(&mut writer, "BEGIN").await;
        run_on(&mut writer, "INSERT INTO orders_hidden_kept VALUES (7)").await;
        assert!(try_on(&mut writer, &call).await.is_err());
        run_on(&mut writer, "INSERT INTO orders_hidden_kept VALUES (8)").await;
        run_on(&mut writer, "COMMIT").await;
        assert_eq!(rows_of("orders_hidden_kept").await, [[Value::Int(7)], [Value::Int(8)]]);
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
