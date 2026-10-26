//! The saved queries on disk: listed and read off the loop, each answer sent back as `Incoming`.
use super::super::app::Incoming;
use crate::query::{QueryPath, Root};
use tokio::sync::mpsc::UnboundedSender;

/// The folder of saved queries, when there is one, and the loop's channel for what is read in it.
#[derive(Debug)]
pub(in crate::tui) struct QueryFiles {
    pub(super) root: Option<Root>,
    pub(super) answers: UnboundedSender<Incoming>,
}

impl QueryFiles {
    /// Lists every query on a blocking thread; the list comes back as `Incoming::Queries`.
    pub(in crate::tui) fn list(&self) {
        let Some(root) = self.root.clone() else { return };
        let answers = self.answers.clone();
        tokio::task::spawn_blocking(move || send(&answers, Incoming::Queries(root.list())));
    }

    /// Reads the query at `path`; its text comes back as `Incoming::QueryOpened`.
    pub(in crate::tui) fn open(&self, path: QueryPath) {
        let Some(root) = self.root.clone() else { return };
        let answers = self.answers.clone();
        tokio::spawn(async move {
            let answer = match root.read(&path).await {
                Ok(text) => Incoming::QueryOpened { path, text },
                Err(err) => Incoming::QueryNotOpened { path, error: format!("{err:#}") },
            };
            send(&answers, answer);
        });
    }
}

fn send(answers: &UnboundedSender<Incoming>, incoming: Incoming) {
    if answers.send(incoming).is_err() {
        tracing::warn!("the loop is gone: a saved query answer is dropped");
    }
}
