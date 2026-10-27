//! What the app and the backend say to each other: the actions it asks for and the answers that come back.
use crate::config::ConnectionChange;
use crate::connection::{Connection, ConnectionName};
use crate::db::model::{LocalPort, Page, ResultSet, Statement};
use crate::history::Entry;
use crate::query::{Query, QueryPath};
use crate::secret::{KeychainItem, Secret};
use std::time::Duration;

/// Which run of a statement an answer or a cancel is about, so a late one never lands on the next run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct RunId(u64);

impl RunId {
    pub(in crate::tui) const FIRST: Self = Self(1);

    pub(in crate::tui) fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Which change to the config an answer is about, so only the form or the picker that asked for it reacts to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct ConfigChangeId(u64);

impl ConfigChangeId {
    pub(in crate::tui) const FIRST: Self = Self(1);

    pub(in crate::tui) fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// What the app asks the runner to do.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::tui) enum Action {
    /// Open a session on this connection; one already open stays as it is.
    Open(Connection),
    /// Run this statement on the connection's session.
    Run { connection: ConnectionName, run: RunId, statement: Statement },
    /// Fetch the next page of this run's result, if its portal is still open.
    NextPage { connection: ConnectionName, run: RunId },
    /// Cancel this run if the connection's session is still on it.
    Cancel { connection: ConnectionName, run: RunId },
    /// List every saved query.
    ListQueries,
    /// Read this query into the editor of the connection's sheet.
    OpenQuery { connection: ConnectionName, query: Query },
    /// Write the text of the connection's editor over the query it stands for.
    SaveQuery { connection: ConnectionName, query: Query, text: String },
    /// Write the text of the connection's editor as a new query at this path, never over another one.
    SaveNewQuery { connection: ConnectionName, path: QueryPath, text: String },
    /// Lend the terminal to the user's editor on this query's file, then read it again into the connection's sheet; the
    /// loop runs it, not the backend.
    EditOutside { connection: ConnectionName, query: Query },
    /// Close the connection's session and its tunnel; nothing when it has none.
    Close(ConnectionName),
    /// Close the connection's session and open one on it as the config now says.
    Reopen(Connection),
    /// Remove the connection from the config, with the keychain item no other connection names.
    RemoveConnection { id: ConfigChangeId, connection: ConnectionName },
    /// Write a connection the form filled in.
    SaveConnection(SaveConnection),
    /// Read the history of the connection, for `ctrl-r`.
    ListHistory(ConnectionName),
    /// Put `text` on the clipboard, then say `done`; the loop sends it to the terminal too.
    Copy { text: String, done: String },
}

/// A connection to add or edit: the keys it sets, and the password typed for its keychain item.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::tui) struct SaveConnection {
    pub(in crate::tui) id: ConfigChangeId,
    pub(in crate::tui) name: ConnectionName,
    pub(in crate::tui) saving: Saving,
    pub(in crate::tui) change: ConnectionChange,
    /// Written to its keychain item once the config took the change, when one was typed.
    pub(in crate::tui) password: Option<KeychainPassword>,
}

/// A password typed for the keychain item the connection names, whether that item is new or already in the config.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::tui) struct KeychainPassword {
    pub(in crate::tui) item: KeychainItem,
    pub(in crate::tui) password: Secret,
}

/// Whether the form adds a connection or changes one already there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Saving {
    Add,
    Edit,
}

/// What the backend answers.
#[derive(Debug, PartialEq)]
pub(in crate::tui) enum Answer {
    /// From the session of `connection`, so an answer from one never lands on another.
    Session { connection: ConnectionName, incoming: Incoming },
    /// About the saved queries: the list every connection shares, or a query read for one sheet.
    Queries(QueryAnswer),
    /// About the change to the config `id` names.
    Config { id: ConfigChangeId, answer: ConfigAnswer },
    /// The history of `connection`: each statement once, newest first.
    History { connection: ConnectionName, entries: Vec<Entry> },
    /// What a copy to the clipboard came to, in a few words.
    Copied(String),
}

/// What changing the config came to.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::tui) enum ConfigAnswer {
    /// Its connections once changed, read back from the file.
    Changed(Vec<Connection>),
    /// The change was refused or failed, in the error's words; the file is as it was, and the password typed for it
    /// comes back to the form.
    Failed { why: String, password: Option<Secret> },
}

/// What reading the saved queries came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum QueryAnswer {
    /// Every saved query, sorted.
    All(Vec<Query>),
    /// The text of `query`, for the sheet of `connection`, which asked for it.
    Opened {
        connection: ConnectionName,
        query: Query,
        text: String,
    },
    NotOpened {
        connection: ConnectionName,
        query: Query,
        error: String,
    },
    /// `text` is now the file of `query`, for the sheet of `connection`, which saved it.
    Saved {
        connection: ConnectionName,
        query: Query,
        text: String,
    },
    NotSaved {
        connection: ConnectionName,
        path: QueryPath,
        error: String,
    },
}

/// What a session answers.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum Incoming {
    Opened,
    /// The session could not open, in the error's words.
    NotOpened(String),
    /// The open session was lost, and why; the next run opens a new one.
    Closed(String),
    Tunnel(TunnelState),
    Result {
        run: RunId,
        result: ResultSet,
        took: Duration,
    },
    /// The next page of the run's result; an empty one that is `Cut` says its portal closed before the last page.
    Page {
        run: RunId,
        page: Page,
    },
    /// The server failed the next page of the run's result, in its words; the result stays at the rows it has.
    PageFailed {
        run: RunId,
        error: String,
    },
    /// The guard kept the statement from the server, and why.
    Refused {
        run: RunId,
        why: String,
    },
    /// The server, or the link to it, failed the statement; the server may say where in it.
    Failed {
        run: RunId,
        error: String,
        /// A character count from 1 into the statement sent, as PostgreSQL counts.
        position: Option<usize>,
    },
}

/// Where the connection's tunnel stands, as the status line says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum TunnelState {
    Starting,
    Up(LocalPort),
    /// It did not start, or it ended, and how.
    Died(String),
}
