//! What the runner calls: one session per opened connection, each owned by its own task, and the saved queries.
use super::app::{Action, Answer, Confirmed, RunId, SessionId};
use crate::connection::{Connection, ConnectionName};
use crate::db::model::Statement;
use crate::history::{History, Store};
use crate::query::params::ParamInput;
use crate::query::{Query, Roots};
use config::ConfigFile;
use futures_util::future;
use paging::PAGES_IDLE_LIMIT;
use queries::QueryFiles;
use session::{Job, Queued, SessionAnswers, SessionLink};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

mod clipboard;
mod config;
#[cfg(test)]
mod fixtures;

pub(super) use clipboard::reaches_the_terminal;
mod paging;
mod queries;
mod schema;
mod session;

/// The handles to the session tasks, one per opened connection, kept until pgbg quits; answers go out on the loop's
/// channel.
#[derive(Debug)]
pub(super) struct Backend {
    sessions: BTreeMap<ConnectionName, SessionLink>,
    answers: UnboundedSender<Answer>,
    queries: QueryFiles,
    config: ConfigFile,
    /// Where each run is recorded and where `ctrl-r` reads from; none keeps no history.
    history: Option<Store>,
    /// How long a result's portal waits for its next page.
    idle_limit: Duration,
}

impl Backend {
    /// The backend with no session yet; saved queries are read under `roots`, and the config is changed at `config`.
    pub(super) fn new(roots: Roots, config: PathBuf, answers: UnboundedSender<Answer>) -> Self {
        let queries = QueryFiles { roots, answers: answers.clone() };
        let config = ConfigFile::start(config, answers.clone());
        Self { sessions: BTreeMap::new(), answers, queries, config, history: None, idle_limit: PAGES_IDLE_LIMIT }
    }

    /// The backend recording each run in `history`.
    pub(super) fn with_history(self, history: Option<Store>) -> Self {
        Self { history, ..self }
    }

    /// The file of `query` for the user's editor; blocks on the file system, so the loop calls it with the terminal lent.
    pub(super) fn query_file(&self, query: &Query) -> anyhow::Result<PathBuf> {
        self.queries.file(query)
    }

    /// Hands `action` over without waiting for it; the loop never waits on the database.
    pub(super) fn dispatch(&mut self, action: Action) {
        match action {
            Action::Open { connection, session } => self.open(connection, session),
            Action::Run { connection, run, statement, input, confirmed } => self.run(&connection, run, statement, input, confirmed),
            Action::NextPage { connection, session, run } => self.next_page(&connection, session, run),
            Action::ReadSchema { connection, session } => self.read_schema(&connection, session),
            Action::Cancel { connection, run } => self.cancel(&connection, run),
            Action::ListQueries => self.queries.list(),
            Action::OpenQuery { connection, query } => self.queries.open(connection, query),
            Action::SaveQuery { connection, query, text } => self.queries.save(connection, query, text),
            Action::SaveNewQuery { connection, path, text } => self.queries.save_new(connection, path, text),
            Action::EditOutside { query, .. } => {
                tracing::warn!("{} goes to the loop, which lends the terminal, not to the backend", query.path);
            }
            Action::Close(connection) => self.close(&connection),
            Action::Reopen { connection, session } => {
                self.close(connection.name());
                self.open(connection, session);
            }
            Action::RemoveConnection { id, connection } => self.config.remove(id, connection),
            Action::SaveConnection(save) => self.config.save(save),
            Action::ListHistory(connection) => self.list_history(connection),
            Action::Copy { text, done } => clipboard::copy(text, done, self.answers.clone()),
            Action::CopyResult { result, format, done } => clipboard::export(result, format, done, self.answers.clone()),
        }
    }

    /// Reads the history of `connection` on a blocking thread; a history that cannot be read is logged and comes back
    /// empty.
    fn list_history(&self, connection: ConnectionName) {
        let (history, answers) = (self.history.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || {
            let read = history.map(|store| store.read(&connection)).transpose();
            let entries = read.unwrap_or_else(|err| {
                tracing::warn!("the history of `{connection}` was not read: {err}");
                None
            });
            send(&answers, Answer::History { connection, entries: entries.map(History::distinct).unwrap_or_default() });
        });
    }

    /// Stops the session task of `connection` in the background, which drops its session and kills its tunnel.
    fn close(&mut self, connection: &ConnectionName) {
        if let Some(session) = self.sessions.remove(connection) {
            tokio::spawn(session.stop());
        }
    }

    /// Starts the task that opens the session `session` on `connection`, or asks the task already serving it to open
    /// it again unless it is open; a task serving an older session of `connection` stops first.
    fn open(&mut self, connection: Connection, session: SessionId) {
        match self.sessions.get(connection.name()) {
            Some(link) if link.session == session => return link.send(Job::Open),
            Some(_) => self.close(connection.name()),
            None => {}
        }
        let name = connection.name().clone();
        let answers = SessionAnswers { connection: name.clone(), session, answers: self.answers.clone() };
        let link = SessionLink::start(connection, answers, self.history.clone(), self.idle_limit);
        _ = self.sessions.insert(name, link);
    }

    /// Queues `statement` on the session of `connection`, a write only when `confirmed`; its answer comes back as an
    /// `Answer`.
    fn run(&self, connection: &ConnectionName, run: RunId, statement: Statement, input: ParamInput, confirmed: Option<Confirmed>) {
        let Some(session) = self.sessions.get(connection) else {
            tracing::warn!("no session for `{connection}`: the statement is dropped");
            return;
        };
        session.send(Job::Run(Queued { run, statement, input, confirmed }));
    }

    /// Asks the session of `connection` for the next page of `run`; nothing comes back when its portal is closed.
    fn next_page(&self, connection: &ConnectionName, session: SessionId, run: RunId) {
        if let Some(link) = self.sessions.get(connection).filter(|link| link.session == session) {
            link.send(Job::NextPage { run });
        }
    }

    /// Reads the schema of `connection` along the same route as its session `session`; it comes back as an `Answer`.
    fn read_schema(&self, connection: &ConnectionName, session: SessionId) {
        if let Some(link) = self.sessions.get(connection).filter(|link| link.session == session) {
            link.read_schema();
        }
    }

    /// Cancels `run` on the session of `connection`; see `SessionLink::cancel`.
    fn cancel(&self, connection: &ConnectionName, run: RunId) {
        let Some(session) = self.sessions.get(connection) else {
            tracing::warn!("no session for `{connection}`: the cancel is dropped");
            return;
        };
        session.cancel(run);
    }

    /// Stops every session task at once, so quitting waits `STOP_WAIT` at most.
    pub(super) async fn stop(self) {
        future::join_all(self.sessions.into_values().map(SessionLink::stop)).await;
    }
}

/// Hands `answer` to the loop; an answer sent after the loop ended is dropped with a warning.
fn send(answers: &UnboundedSender<Answer>, answer: Answer) {
    if answers.send(answer).is_err() {
        tracing::warn!("the loop is gone: an answer is dropped");
    }
}
