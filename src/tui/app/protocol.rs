//! The messages between the app and the backend: the actions the app asks for, and the answers that come back.
use super::Confirmed;
use crate::complete::Index;
use crate::config::ConnectionChange;
use crate::connection::{Connection, ConnectionName};
use crate::db::model::{Catalog, LocalPort, Page, ResultSet, Statement};
use crate::export::Format;
use crate::history::Entry;
use crate::query::params::ParamInput;
use crate::query::{Query, QueryPath};
use crate::secret::{KeychainItem, Secret};
use std::sync::Arc;
use std::time::Duration;

/// Which run of a statement an answer or a cancel is about, so a late answer never lands on the next run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

/// Which of the sessions opened on a connection an answer comes from, so answers from a closed session never reach
/// the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct SessionId(u64);

impl SessionId {
    pub(in crate::tui) const FIRST: Self = Self(1);

    pub(in crate::tui) fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// What the app asks the runner to do.
#[derive(Debug, PartialEq)]
pub(in crate::tui) enum Action {
    /// Open the session `session` on this connection; a session already open is kept as it is.
    Open { connection: Connection, session: SessionId },
    /// Run this statement on the connection's session; a write runs only when `confirmed`.
    Run { connection: ConnectionName, run: RunId, statement: Statement, input: ParamInput, confirmed: Option<Confirmed> },
    /// Fetch the next page of this run's result, if its portal is still open on the session `session`.
    NextPage { connection: ConnectionName, session: SessionId, run: RunId },
    /// Read the connection's schema on a second session that takes the same route as `session`, once that one is open.
    ReadSchema { connection: ConnectionName, session: SessionId },
    /// Cancel this run if the connection's session is still running it.
    Cancel { connection: ConnectionName, run: RunId },
    /// List every saved query.
    ListQueries,
    /// Read this query into the editor of the connection's sheet.
    OpenQuery { connection: ConnectionName, query: Query },
    /// Write the text of the connection's editor over the query it stands for.
    SaveQuery { connection: ConnectionName, query: Query, text: String },
    /// Write the text of the connection's editor as a new query at this path, never over another one.
    SaveNewQuery { connection: ConnectionName, path: QueryPath, text: String },
    /// Lend the terminal to the user's editor on this query's file, then read the file again into the connection's sheet.
    /// The loop runs it, not the backend.
    EditOutside { connection: ConnectionName, query: Query },
    /// Close the connection's session and its tunnel; does nothing when it has no session.
    Close(ConnectionName),
    /// Close the connection's session and open `session` on it as the config now says.
    Reopen { connection: Connection, session: SessionId },
    /// Remove the connection from the config, and its keychain item when no other connection names it.
    RemoveConnection { id: ConfigChangeId, connection: ConnectionName },
    /// Write a connection the form filled in.
    SaveConnection(SaveConnection),
    /// Read the history of the connection, for `ctrl-r`.
    ListHistory(ConnectionName),
    /// Put `text` on the clipboard, then report `done`; the loop also sends it to the terminal, unless it is too big.
    Copy { text: String, done: String },
    /// Build the text of `result` in `format` off the loop, then copy it as `Copy` does.
    CopyResult { result: ResultSet, format: Format, done: String },
}

/// A connection to add or edit: the keys it sets, and the password typed for its keychain item.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::tui) struct SaveConnection {
    pub(in crate::tui) id: ConfigChangeId,
    pub(in crate::tui) name: ConnectionName,
    pub(in crate::tui) saving: Saving,
    pub(in crate::tui) change: ConnectionChange,
    /// The typed password, written to its keychain item once the config has taken the change.
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
    /// From the session `session` of `connection`, so an answer from one session never lands on another.
    Session { connection: ConnectionName, session: SessionId, incoming: Incoming },
    /// About the saved queries: the list every connection shares, or a query read for one sheet.
    Queries(QueryAnswer),
    /// About the change to the config that `id` names.
    Config { id: ConfigChangeId, answer: ConfigAnswer },
    /// The history of `connection`: each statement once, newest first.
    History { connection: ConnectionName, entries: Vec<Entry> },
    /// The outcome of a copy to the clipboard, in a few words.
    Copied(String),
    /// The text of a result that `CopyResult` asked for, to copy before reporting `done`.
    Exported { text: String, done: String },
}

/// The outcome of a change to the config.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::tui) enum ConfigAnswer {
    /// The connections read back from the file after the change, and, for the status line, what failed after the
    /// file took the change (the keychain step of a save).
    Changed { connections: Vec<Connection>, warning: Option<String> },
    /// The change was refused or failed, with the error's message; the file did not change, and the typed password
    /// goes back to the form.
    Failed { why: String, password: Option<Secret> },
}

/// The outcome of reading or writing the saved queries.
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
    /// `text` is now saved in the file of `query`, for the sheet of `connection`, which saved it.
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
    /// The session could not open, with the error's message.
    NotOpened(String),
    /// The open session was lost, and why; the next run opens a new one.
    Closed(String),
    Tunnel(TunnelState),
    /// The schema a second session read, or why it could not read it.
    Schema(Result<Arc<Catalog>, String>),
    /// The completion index built from `catalog`, which a later schema read may have replaced.
    Index {
        catalog: Arc<Catalog>,
        index: Arc<Index>,
    },
    Result {
        run: RunId,
        result: ResultSet,
        took: Duration,
    },
    /// The next page of the run's result; an empty `Cut` page means its portal closed before the last page.
    Page {
        run: RunId,
        page: Page,
    },
    /// The server failed to send the next page of the run's result, with its message; the result keeps the rows it has.
    PageFailed {
        run: RunId,
        error: String,
    },
    /// The guard stopped the statement before it reached the server, and why.
    Refused {
        run: RunId,
        why: String,
    },
    /// The statement, which the guard read as a read, tried to write: the server refused it, so nothing of it ran.
    TriedToWrite {
        run: RunId,
    },
    /// The statement failed on the server or on the link to it; the server may say where in the statement.
    Failed {
        run: RunId,
        error: String,
        /// A character position in the statement sent, counted from 1 as PostgreSQL counts.
        position: Option<usize>,
    },
}

/// Where the connection's tunnel stands, as the status line says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum TunnelState {
    Starting,
    Up(LocalPort),
    /// It did not start, or it stopped, and how.
    Died(String),
}
