//! A lookup: a read the app built, admitted read only whatever the connection allows, kept out of the history.
use super::run::{Cancels, OnServer, warn_if_still_running};
use crate::connection::WritePolicy;
use crate::db::guard::{self, Admitted};
use crate::db::model::{CancelToken, ResultSet};
use crate::failure::Failure;
use crate::keys::Lookup;
use crate::tui::app::{Incoming, LookId};

/// A lookup waiting for the session, and the id its answer carries.
#[derive(Debug)]
pub(in crate::tui::backend) struct QueuedLookup {
    pub(in crate::tui::backend) look: LookId,
    pub(in crate::tui::backend) lookup: Lookup,
}

/// Where a lookup runs and how a stop reaches it.
pub(in crate::tui::backend) struct OnSession<'a> {
    pub(in crate::tui::backend) token: &'a CancelToken,
    pub(in crate::tui::backend) cancelled: &'a mut Cancels,
    pub(in crate::tui::backend) on_server: &'a OnServer,
}

/// Answers `queued` with the rows `read` gives for it once the guard admitted it read only, so a write in it is refused
/// as on a read-only connection, under its own timeout when it has one; a stop ends it as every stop does.
pub(in crate::tui::backend) async fn look_up(
    queued: QueuedLookup,
    read: impl AsyncFnOnce(&Admitted) -> anyhow::Result<ResultSet>,
    on: OnSession<'_>,
) -> Incoming {
    let QueuedLookup { look, lookup } = queued;
    let admitted = match guard::admit(lookup.statement, lookup.params, WritePolicy::ReadOnly) {
        Ok(admitted) => admitted.with_own_timeout(lookup.own_timeout),
        Err(refusal) => return Incoming::LookedUp { look, rows: Err(Failure::Refused(refusal.to_string())) },
    };
    on.on_server.starts(on.token);
    let read = on.token.unless_stopped(read(&admitted), on.cancelled.until_stopping()).await;
    on.on_server.ended();
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
    use crate::query::params::ParamInput;
    use crate::tui::app::{RunId, SessionId};

    /// A fresh `table` of customers, whose key is `(id, region)`, holding customer 3 in `eu` and in `us`.
    async fn customers(table: &str) {
        on_a_side_session_writing(&format!("DROP TABLE IF EXISTS {table}")).await;
        on_a_side_session_writing(&format!("CREATE TABLE {table} (id int, region text, name text, PRIMARY KEY (id, region))")).await;
        on_a_side_session_writing(&format!("INSERT INTO {table} VALUES (3, 'eu', 'nina'), (3, 'us', 'noel')")).await;
    }

    /// The lookup of the customer in `table` that an order of customer 3 in `eu` points to, through a key of two columns.
    fn customer_of_an_order(table: &str) -> Lookup {
        let pair = |at, column: &str| KeyPair { at, from: format!("customer_{column}"), to: column.to_owned() };
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
        let lookup = Lookup { statement: Statement::new("DELETE FROM pgbg_follow_read"), params: Params::default(), own_timeout: None };
        backend.look_up(&shop(), QueuedLookup { look: LookId::FIRST, lookup });
        let answer = next_answer(&mut answers).await;
        assert!(matches!(answer, Incoming::LookedUp { rows: Err(Failure::Refused(_)), .. }), "{answer:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_lookup_failing_while_a_result_pages_leaves_its_next_page_waiting() {
        let (backend, mut answers) = started();
        let pages = Statement::new("SELECT n FROM generate_series(1, 1000) AS n");
        backend.run(&shop(), RunId::FIRST, pages, ParamInput::default(), None);
        next_answer(&mut answers).await;
        backend.look_up(&shop(), QueuedLookup { look: LookId::FIRST, lookup: customer_of_an_order("pgbg_follow_missing") });
        let looked_up = next_answer(&mut answers).await;
        assert!(matches!(looked_up, Incoming::LookedUp { rows: Err(Failure::Server { .. }), .. }), "{looked_up:?}");
        backend.next_page(&shop(), SessionId::FIRST, RunId::FIRST);
        let Incoming::Page { page, .. } = next_answer(&mut answers).await else { panic!("no page") };
        assert_eq!(page.rows.len(), 500);
    }
}
