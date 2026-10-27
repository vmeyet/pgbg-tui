//! A result shown a page at a time: its portal stays open between pages, in its `BEGIN READ ONLY` transaction.
use super::guard::{Admitted, Mode};
use super::model::{Column, Extent, Page, ResultSet};
use super::{FetchSize, Isolation, Marks, Session, begin_read_only, bind_portal, columns, fetch_page, roll_back, why_ended};
use anyhow::Result;
use std::time::Duration;
use tokio::sync::watch;
use tokio_postgres::{Portal, Transaction};

/// The rows of one page: the grid asks for the next page as it nears the end of the rows it has.
const PAGE_ROWS: FetchSize = FetchSize::new(500);

/// A statement's first page, and its portal when more rows wait in it.
pub(crate) struct Started<'s> {
    pub(crate) result: ResultSet,
    pub(crate) rest: Option<Pages<'s>>,
}

/// The next page, and the portal when more rows wait in it still.
pub(crate) struct Fetched<'s> {
    pub(crate) page: Page,
    pub(crate) rest: Option<Pages<'s>>,
}

/// The open portal of a result and the read-only transaction it lives in, holding its session until it is closed.
/// Dropped, it asks the server to roll back without waiting for it: `close` waits.
pub(crate) struct Pages<'s> {
    transaction: Transaction<'s>,
    portal: Portal,
    marks: &'s mut Marks,
    ended: &'s watch::Receiver<Option<String>>,
    idle_limit_on_server: Duration,
}

/// A statement bound to its portal, and the columns it returns.
struct Bound {
    columns: Vec<Column>,
    portal: Portal,
}

impl Session {
    /// Runs `admitted` alone and returns its first page; on a read-only connection the rest wait in its portal, kept
    /// open in `Started::rest`. On a connection that allows writes it runs as `run` does, its first `ROW_CAP` rows kept.
    pub(crate) async fn start(&mut self, admitted: &Admitted) -> Result<Started<'_>> {
        if admitted.mode == Mode::Writes {
            return Ok(Started { result: self.run(admitted).await?, rest: None });
        }
        let Self { client, limits, marks, ended, .. } = self;
        let idle_limit_on_server = limits.idle.on_server();
        let transaction = marks.during(begin_read_only(client, limits, Isolation::EachStatement)).await?;
        let bound = marks.during(bind(&transaction, admitted)).await;
        let Bound { columns, portal } = match bound {
            Ok(bound) => bound,
            Err(err) => {
                roll_back(transaction, marks).await?;
                return Err(err);
            }
        };
        let Fetched { page, rest } = Pages { transaction, portal, marks, ended, idle_limit_on_server }.next().await?;
        Ok(Started { result: ResultSet::new(columns, page), rest })
    }
}

impl<'s> Pages<'s> {
    /// Fetches the next `PAGE_ROWS` rows: the portal stays open while more wait in it, else its transaction ends, as it
    /// does when the server fails the fetch.
    pub(crate) async fn next(self) -> Result<Fetched<'s>> {
        let page = self.marks.during(fetch_page(&self.transaction, &self.portal, PAGE_ROWS)).await;
        match page {
            Ok(page) if page.extent == Extent::More => Ok(Fetched { page, rest: Some(self) }),
            Ok(page) => self.close().await.map(|()| Fetched { page, rest: None }),
            Err(err) => {
                self.close().await?;
                Err(err)
            }
        }
    }

    /// Ends the portal's transaction; the rows it did not send stay on the server.
    pub(crate) async fn close(self) -> Result<()> {
        roll_back(self.transaction, self.marks).await
    }

    /// How long the portal may wait for its next page before the server ends the whole session.
    pub(crate) fn idle_limit_on_server(&self) -> Duration {
        self.idle_limit_on_server
    }

    /// Waits until the link to the server ends, and says why.
    pub(crate) async fn closed(&self) -> String {
        why_ended(self.ended).await
    }
}

impl Marks {
    async fn during<T>(&mut self, work: impl Future<Output = T>) -> T {
        self.is_mid_statement = true;
        let done = work.await;
        self.is_mid_statement = false;
        done
    }
}

async fn bind(transaction: &Transaction<'_>, admitted: &Admitted) -> Result<Bound> {
    let statement = super::bind::prepare(transaction, admitted).await?;
    let portal = bind_portal(transaction, &statement, &admitted.params).await?;
    Ok(Bound { columns: columns(&statement), portal })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::model::{Params, Route, Statement, Value};
    use crate::db::{ROW_CAP, guard};
    use crate::fixtures;

    async fn session(extra: &str) -> Session {
        Session::open(&fixtures::test_database("local", extra), Route::Direct).await.unwrap()
    }

    fn read_only(sql: &str) -> Admitted {
        guard::admit(Statement::new(sql), Params::default(), WritePolicy::ReadOnly).unwrap()
    }

    fn numbers_up_to(count: usize) -> String {
        format!("SELECT generate_series(1, {count})")
    }

    async fn on_a_side_session(sql: &str) -> Vec<Vec<Value>> {
        session("").await.run(&read_only(sql)).await.unwrap().rows().to_vec()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_read_only_result_starts_with_its_first_page_and_keeps_its_portal_open() {
        let mut session = session("").await;
        let Started { result, rest } = session.start(&read_only(&numbers_up_to(1200))).await.unwrap();
        assert_eq!((result.rows().len(), result.extent()), (PAGE_ROWS.0 as usize, Extent::More));
        assert!(rest.is_some());
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_next_pages_follow_the_first_until_the_last_closes_the_portal() {
        let mut session = session("").await;
        let started = session.start(&read_only(&numbers_up_to(1200))).await.unwrap();
        let second = started.rest.unwrap().next().await.unwrap();
        assert_eq!((second.page.rows[0][0].to_string(), second.page.extent), ("501".to_owned(), Extent::More));
        let last = second.rest.unwrap().next().await.unwrap();
        assert_eq!((last.page.rows.len(), last.page.extent, last.rest.is_none()), (200, Extent::All, true));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_paged_result_binds_its_values() {
        let mut session = session("").await;
        let admitted = guard::admit(
            Statement::new("SELECT generate_series(1, $1::int4)"),
            Params::new(vec![Some("700".into())], vec![]),
            WritePolicy::ReadOnly,
        );
        let Started { rest, .. } = session.start(&admitted.unwrap()).await.unwrap();
        let last = rest.unwrap().next().await.unwrap();
        assert_eq!((last.page.rows.len(), last.page.extent), (200, Extent::All));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_of_one_page_keeps_no_portal() {
        let mut session = session("").await;
        let Started { result, rest } = session.start(&read_only(&numbers_up_to(12))).await.unwrap();
        assert_eq!((result.rows().len(), result.extent(), rest.is_none()), (12, Extent::All, true));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_result_on_a_connection_that_allows_writes_is_cut_at_the_row_cap_and_keeps_no_portal() {
        let mut session = session("writes = \"allowed\"\n").await;
        let admitted = guard::admit(Statement::new(numbers_up_to(ROW_CAP + 1)), Params::default(), WritePolicy::Allowed).unwrap();
        let Started { result, rest } = session.start(&admitted).await.unwrap();
        assert_eq!((result.rows().len(), result.extent(), rest.is_none()), (ROW_CAP, Extent::Cut, true));
    }

    async fn idle_limit_of_a_portal(url: &str) -> Duration {
        let connection = fixtures::connection(&format!("tier = \"local\"\nurl = \"{url}\"\n"));
        let mut session = Session::open(&connection, Route::Direct).await.unwrap();
        let started = session.start(&read_only(&numbers_up_to(1200))).await.unwrap();
        started.rest.unwrap().idle_limit_on_server()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_portal_waits_on_the_server_until_the_backstop() {
        assert_eq!(idle_limit_of_a_portal(&fixtures::test_database_url()).await, Duration::from_mins(6));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_portal_waits_on_the_server_no_longer_than_its_stricter_idle_timeout() {
        let mut url = url::Url::parse(&fixtures::test_database_url()).unwrap();
        url.query_pairs_mut().append_pair("options", "-cidle_in_transaction_session_timeout=1min");
        assert_eq!(idle_limit_of_a_portal(url.as_str()).await, Duration::from_mins(1));
    }

    async fn backend_pid(session: &mut Session) -> Value {
        session.run(&read_only("SELECT pg_backend_pid()")).await.unwrap().rows()[0][0].clone()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn closing_the_pages_ends_their_transaction_on_the_server() {
        let mut session = session("").await;
        let pid = backend_pid(&mut session).await;
        let started = session.start(&read_only(&numbers_up_to(1200))).await.unwrap();
        started.rest.unwrap().close().await.unwrap();
        let state = on_a_side_session(&format!("SELECT state FROM pg_stat_activity WHERE pid = {pid}")).await;
        assert_eq!(state, [[Value::Text("idle".into())]]);
    }

    const SLOW_AFTER_THE_FIRST_PAGE: &str = "SELECT n, pg_sleep(CASE WHEN n > 500 THEN 30 ELSE 0 END) FROM generate_series(1, 1000) AS n";

    async fn until_active(sql: &str) {
        let is_active = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{sql}' AND state = 'active'");
        while on_a_side_session(&is_active).await.is_empty() {}
    }

    /// Fetches the second page of a slow result on `session`, cancelled while the server works on it.
    async fn cancelled_mid_page(session: &mut Session) -> anyhow::Error {
        let token = session.cancel_token();
        let started = session.start(&read_only(SLOW_AFTER_THE_FIRST_PAGE)).await.unwrap();
        let cancel = async {
            until_active(SLOW_AFTER_THE_FIRST_PAGE).await;
            token.cancel().await.unwrap();
        };
        let (fetched, ()) = tokio::join!(started.rest.unwrap().next(), cancel);
        fetched.err().unwrap()
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_page_cancelled_mid_fetch_fails_with_the_servers_words() {
        let err = cancelled_mid_page(&mut session("").await).await;
        assert!(err.to_string().contains("canceling statement due to user request"), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_runs_on_after_a_page_was_cancelled() {
        let mut session = session("").await;
        cancelled_mid_page(&mut session).await;
        assert_eq!(session.run(&read_only("SELECT 7")).await.unwrap().rows(), [[Value::Int(7)]]);
    }
}
