//! The saved queries on disk: listed and read off the loop, each answer sent back as `Answer::Queries`.
use super::super::app::{Answer, Listed, QueryAnswer};
use super::send;
use crate::connection::ConnectionName;
use crate::query::{Query, QueryPath, Roots, Write};
use std::path::Path;
use tokio::sync::mpsc::UnboundedSender;

/// The folders of saved queries, and the loop's channel for what is read from them.
#[derive(Debug)]
pub(in crate::tui) struct QueryFiles {
    pub(super) roots: Roots,
    pub(super) answers: UnboundedSender<Answer>,
}

impl QueryFiles {
    /// Lists the project's queries and the user's own of `folder` on a blocking thread; the list comes back as
    /// `QueryAnswer::All`.
    pub(in crate::tui) fn list(&self, folder: ConnectionName) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || {
            let queries = roots.list(&folder);
            send(&answers, Answer::Queries(QueryAnswer::All { folder, queries }));
        });
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

    /// Runs `edit` on the file of `query`, as the user's editor does, then keeps the text it replaced; blocks on the
    /// file system.
    pub(super) fn edit(&self, query: &Query, edit: impl FnOnce(&Path) -> anyhow::Result<()>) -> anyhow::Result<()> {
        self.roots.edit_kept(query, edit)
    }

    /// Lists the versions of `query` on a blocking thread, each compared with the editor's `text`; they come back as
    /// `Answer::Versions`, none when they cannot be read.
    pub(in crate::tui) fn list_versions(&self, query: Query, text: String) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || {
            let versions = roots.versions(&query).unwrap_or_else(|err| {
                tracing::warn!("the versions of {} were not read: {err:#}", query.path);
                vec![]
            });
            let versions = versions.into_iter().map(|version| Listed::against(version, &text)).collect();
            send(&answers, Answer::Versions { query, versions });
        });
    }

    /// Writes `text` over `query` on a blocking thread, the text it replaces kept as a version first;
    /// `QueryAnswer::Saved` comes back for the sheet of `connection`.
    pub(in crate::tui) fn save(&self, connection: ConnectionName, query: Query, text: String) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || {
            let path = query.path.clone();
            let saved = roots.write(&query, &text, Write::Replace).map(|()| query);
            send(&answers, Answer::Queries(saved_answer(Saving { connection, path, saved, text })));
        });
    }

    /// Writes `text` as a new query at `path`, never over another one: under the project's queries folder when it has
    /// one, else under the user's own `folder`; answers as `save` does.
    pub(in crate::tui) fn save_new(&self, connection: ConnectionName, folder: ConnectionName, path: QueryPath, text: String) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || {
            let query = roots.new_query(path.clone(), &folder);
            let saved = roots.write(&query, &text, Write::Create).map(|()| query);
            send(&answers, Answer::Queries(saved_answer(Saving { connection, path, saved, text })));
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
