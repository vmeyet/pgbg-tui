//! A lookup: a read the app built, admitted read only whatever the connection allows, kept out of the history.
use super::run::{Cancels, OnServer, warn_if_still_running};
use crate::connection::WritePolicy;
use crate::db::Session;
use crate::db::guard;
use crate::failure::Failure;
use crate::keys::Lookup;
use crate::tui::app::{Incoming, LookId};

/// A lookup waiting for the session, and the id its answer carries.
#[derive(Debug)]
pub(in crate::tui::backend) struct QueuedLookup {
    pub(in crate::tui::backend) look: LookId,
    pub(in crate::tui::backend) lookup: Lookup,
}

/// Answers `queued` with the rows it reads on `session` once the guard admitted it read only, so a write in it is
/// refused as on a read-only connection; a stop ends it as every stop does.
pub(super) async fn look_up(session: &mut Session, queued: QueuedLookup, cancelled: &mut Cancels, on_server: &OnServer) -> Incoming {
    let QueuedLookup { look, lookup } = queued;
    let admitted = match guard::admit(lookup.statement, lookup.params, WritePolicy::ReadOnly) {
        Ok(admitted) => admitted,
        Err(refusal) => return Incoming::LookedUp { look, rows: Err(Failure::Refused(refusal.to_string())) },
    };
    let token = session.cancel_token();
    on_server.starts(&token);
    let read = token.unless_stopped(session.run(&admitted), cancelled.until_stopping()).await;
    on_server.ended();
    let rows = match read {
        Ok(rows) => rows.map_err(|err| Failure::from(&err)),
        Err(interrupted) => {
            warn_if_still_running(&interrupted.cancel);
            Err(Failure::StoppedWithItsSession)
        }
    };
    Incoming::LookedUp { look, rows }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures::{next_answer, on_a_side_session_writing, shop, started, started_on};
    use super::*;
    use crate::db::model::{Params, Statement, Value};
    use crate::fixtures;
    use crate::keys::{KeyPair, Target};

    /// A fresh `table` of customers, whose key is `(id, region)`, holding customer 3 in `eu` and in `us`.
    async fn customers(table: &str) {
        on_a_side_session_writing(&format!("DROP TABLE IF EXISTS {table}")).await;
        on_a_side_session_writing(&format!("CREATE TABLE {table} (id int, region text, name text, PRIMARY KEY (id, region))")).await;
        on_a_side_session_writing(&format!("INSERT INTO {table} VALUES (3, 'eu', 'nina'), (3, 'us', 'noel')")).await;
    }

    /// The lookup of the customer in `table` that an order of customer 3 in `eu` points to, through a key of two columns.
    fn customer_of_an_order(table: &str) -> Lookup {
        let pair = |at, column: &str| KeyPair { at, column: column.to_owned() };
        let target = Target { schema: "public".to_owned(), table: table.to_owned(), pairs: vec![pair(0, "id"), pair(1, "region")] };
        target.lookup(&[Value::Int(3), Value::Text("eu".into())]).unwrap()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_lookup_reads_the_row_a_key_of_two_columns_points_to() {
        customers("pgbg_follow_read").await;
        let (backend, mut answers) = started();
        backend.look_up(&shop(), QueuedLookup { look: LookId::FIRST, lookup: customer_of_an_order("pgbg_follow_read") });
        let Incoming::LookedUp { rows: Ok(read), .. } = next_answer(&mut answers).await else { panic!("no rows") };
        assert_eq!(read.rows(), [[Value::Int(3), Value::Text("eu".into()), Value::Text("nina".into())]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_lookup_that_writes_is_refused_on_a_connection_that_allows_writes() {
        let (backend, mut answers) = started_on(fixtures::test_database("local", "writes = \"allowed\"\n"));
        let lookup = Lookup { statement: Statement::new("DELETE FROM pgbg_follow_read"), params: Params::default() };
        backend.look_up(&shop(), QueuedLookup { look: LookId::FIRST, lookup });
        let answer = next_answer(&mut answers).await;
        assert!(matches!(answer, Incoming::LookedUp { rows: Err(Failure::Refused(_)), .. }), "{answer:?}");
    }
}
