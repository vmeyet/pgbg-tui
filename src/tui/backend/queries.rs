//! The saved queries on disk: listed and read off the loop, each answer sent back as `Incoming`.
use super::super::app::Incoming;
use crate::query::{Query, Roots};
use tokio::sync::mpsc::UnboundedSender;

/// The folders of saved queries, and the loop's channel for what is read in them.
#[derive(Debug)]
pub(in crate::tui) struct QueryFiles {
    pub(super) roots: Roots,
    pub(super) answers: UnboundedSender<Incoming>,
}

impl QueryFiles {
    /// Lists every query on a blocking thread; the list comes back as `Incoming::Queries`.
    pub(in crate::tui) fn list(&self) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || send(&answers, Incoming::Queries(roots.list())));
    }

    /// Reads `query`; its text comes back as `Incoming::QueryOpened`.
    pub(in crate::tui) fn open(&self, query: Query) {
        let (roots, answers) = (self.roots.clone(), self.answers.clone());
        tokio::spawn(async move {
            let answer = match roots.read(&query).await {
                Ok(text) => Incoming::QueryOpened { query, text },
                Err(err) => Incoming::QueryNotOpened { query, error: format!("{err:#}") },
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
