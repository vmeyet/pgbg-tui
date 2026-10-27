//! The saved queries on disk: listed and read off the loop, each answer sent back as `Answer::Queries`.
use super::super::app::{Answer, QueryAnswer};
use super::send;
use crate::connection::ConnectionName;
use crate::query::{Query, QueryPath, Roots, Write};
use tokio::sync::mpsc::UnboundedSender;

/// The folders of saved queries, and the loop's channel for what is read from them.
#[derive(Debug)]
pub(in crate::tui) struct QueryFiles {
    pub(super) roots: Roots,
    pub(super) answers: UnboundedSender<Answer>,
}

impl QueryFiles {
    /// Lists every query on a blocking thread; the list comes back as `QueryAnswer::All`.
    pub(in crate::tui) fn list(&self) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || send(&answers, Answer::Queries(QueryAnswer::All(roots.list()))));
    }

    /// Reads `query` for the sheet of `connection`; its text comes back as `QueryAnswer::Opened`.
    pub(in crate::tui) fn open(&self, connection: ConnectionName, query: Query) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::spawn(async move {
            let answer = match roots.read(&query).await {
                Ok(text) => QueryAnswer::Opened { connection, query, text },
                Err(err) => QueryAnswer::NotOpened { connection, query, error: format!("{err:#}") },
            };
            send(&answers, Answer::Queries(answer));
        });
    }

    /// The file of `query` for the user's editor; blocks on the file system.
    pub(super) fn file(&self, query: &Query) -> anyhow::Result<std::path::PathBuf> {
        self.roots.file(query)
    }

    /// Writes `text` over `query` on a blocking thread; `QueryAnswer::Saved` comes back for the sheet of `connection`,
    /// followed by the new list.
    pub(in crate::tui) fn save(&self, connection: ConnectionName, query: Query, text: String) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || {
            let path = query.path.clone();
            let saved = roots.write(&query, &text, Write::Replace).map(|()| query);
            answer_saved(&answers, saved_answer(Saving { connection, path, saved, text }), &roots);
        });
    }

    /// Writes `text` as a new query at `path`, never over another one; answers as `save` does.
    pub(in crate::tui) fn save_new(&self, connection: ConnectionName, path: QueryPath, text: String) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || {
            let saved = roots.new_query(path.clone()).and_then(|query| roots.write(&query, &text, Write::Create).map(|()| query));
            answer_saved(&answers, saved_answer(Saving { connection, path, saved, text }), &roots);
        });
    }
}

/// The outcome of saving `text` at `path` for the sheet of `connection`.
struct Saving {
    connection: ConnectionName,
    path: QueryPath,
    saved: anyhow::Result<Query>,
    text: String,
}

fn saved_answer(Saving { connection, path, saved, text }: Saving) -> QueryAnswer {
    match saved {
        Ok(query) => QueryAnswer::Saved { connection, query, text },
        Err(err) => QueryAnswer::NotSaved { connection, path, error: format!("{err:#}") },
    }
}

/// Sends `answer`, then the list of queries again, so a new file shows in the sidebar and the finder.
fn answer_saved(answers: &UnboundedSender<Answer>, answer: QueryAnswer, roots: &Roots) {
    send(answers, Answer::Queries(answer));
    send(answers, Answer::Queries(QueryAnswer::All(roots.list())));
}
