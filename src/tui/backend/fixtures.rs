//! Invented fixtures the backend's tests share: a backend started on `shop`, its answers, and side sessions on the test
//! database.
use super::Backend;
use crate::connection::{Connection, ConnectionName, WritePolicy};
use crate::db::model::{Params, Route, Statement, Value};
use crate::db::{Session, guard};
use crate::fixtures;
use crate::query::Roots;
use crate::query::params::ParamInput;
use crate::tui::app::{Answer, Confirmed, Incoming, RunId, SessionId};
use std::path::PathBuf;
use tokio::sync::mpsc::{self, UnboundedReceiver};

pub(super) const SLEEP: &str = "SELECT pg_sleep(30)";

pub(super) fn connection() -> Connection {
    fixtures::test_database_as("shop")
}

pub(super) fn shop() -> ConnectionName {
    connection().name().clone()
}

pub(super) fn incoming(answer: Answer) -> Incoming {
    let Answer::Session { incoming, .. } = answer else { panic!("not a session's answer: {answer:?}") };
    incoming
}

pub(super) fn started() -> (Backend, UnboundedReceiver<Answer>) {
    started_on(connection())
}

pub(super) fn started_on(connection: Connection) -> (Backend, UnboundedReceiver<Answer>) {
    let (sender, answers) = mpsc::unbounded_channel();
    let mut backend = Backend::new(Roots::default(), PathBuf::from("config.toml"), sender);
    backend.open(connection, SessionId::FIRST);
    (backend, answers)
}

/// An empty table `name` in the test database, created again from scratch.
pub(super) async fn empty_table(name: &str) -> String {
    on_a_side_session_writing(&format!("DROP TABLE IF EXISTS {name}")).await;
    on_a_side_session_writing(&format!("CREATE TABLE {name} (n int)")).await;
    name.to_owned()
}

pub(super) async fn on_a_side_session_writing(sql: &str) {
    let _changing = fixtures::changing_the_catalog().await;
    let mut session = Session::open(&connection(), Route::Direct).await.unwrap();
    session.run(&guard::admit(Statement::new(sql), Params::default(), WritePolicy::Allowed).unwrap()).await.unwrap();
}

pub(super) fn is_answer_to(run: RunId, answer: &Incoming) -> bool {
    matches!(
        answer,
        Incoming::Result { run: of, .. }
            | Incoming::Failed { run: of, .. }
            if *of == run
    )
}

pub(super) async fn answers_until(answers: &mut UnboundedReceiver<Answer>, last: impl Fn(&Incoming) -> bool) -> Vec<Incoming> {
    let mut seen = vec![];
    while seen.last().is_none_or(|answer| !last(answer)) {
        seen.push(incoming(answers.recv().await.unwrap()));
    }
    seen
}

pub(super) async fn next_answer(answers: &mut UnboundedReceiver<Answer>) -> Incoming {
    loop {
        match incoming(answers.recv().await.unwrap()) {
            Incoming::Opened => {}
            answer => return answer,
        }
    }
}

pub(super) async fn answer_to(backend: &Backend, answers: &mut UnboundedReceiver<Answer>, run: RunId, sql: &str) -> Incoming {
    backend.run(&shop(), run, Statement::new(sql), ParamInput::default(), Some(Confirmed::FOR_TESTS));
    next_answer(answers).await
}

pub(super) async fn on_a_side_session(sql: &str) -> Vec<Vec<Value>> {
    let mut session = Session::open(&connection(), Route::Direct).await.unwrap();
    session.run(&guard::admit(Statement::new(sql), Params::default(), WritePolicy::ReadOnly).unwrap()).await.unwrap().rows().to_vec()
}

pub(super) async fn backend_pid(backend: &Backend, answers: &mut UnboundedReceiver<Answer>) -> Value {
    let Incoming::Result { result, .. } = answer_to(backend, answers, RunId::FIRST, "SELECT pg_backend_pid()").await else {
        panic!("no pid")
    };
    result.rows()[0][0].clone()
}

pub(super) async fn until_sleeping() {
    let is_sleeping = format!("SELECT 1 FROM pg_stat_activity WHERE query = '{SLEEP}' AND state = 'active'");
    while on_a_side_session(&is_sleeping).await.is_empty() {}
}
