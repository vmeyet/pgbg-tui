//! The messages between the app and the backend: the actions the app asks for, and the answers that come back.
use super::Confirmed;
use super::versions::Listed;
use crate::ai::{AnswerError, CommandError, NotSent, Prompt};
use crate::complete::Index;
use crate::config::{AiSettings, ConnectionChange};
use crate::connection::{Connection, ConnectionName};
use crate::db::model::{Catalog, LocalPort, Page, ResultSet, Statement};
use crate::export::Format;
use crate::failure::{Failure, Lost};
use crate::history::Entry;
use crate::keys::Lookup;
use crate::query::params::ParamInput;
use crate::query::{Query, QueryPath};
use crate::secret::{KeychainItem, Secret};
use std::fmt;
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

/// Which ask an answer is about, so the answer to an ask dropped or replaced never shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct AskId(u64);

impl AskId {
    pub(in crate::tui) const FIRST: Self = Self(1);

    pub(in crate::tui) fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Which lookup an answer is about, so the row of a lookup the user moved away from never shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct LookId(u64);

impl LookId {
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
    /// Read the rows of `lookup` on the connection's session, read only whatever the connection allows, kept out of the
    /// history.
    LookUp { connection: ConnectionName, look: LookId, lookup: Lookup },
    /// Cancel this run if the connection's session is still running it.
    Cancel { connection: ConnectionName, run: RunId },
    /// List the saved queries of this connection's queries folder.
    ListQueries(ConnectionName),
    /// Read this query into the editor of the connection's sheet.
    OpenQuery { connection: ConnectionName, query: Query },
    /// Write the text of the connection's editor over the query it stands for.
    SaveQuery { connection: ConnectionName, query: Query, text: String },
    /// Write the text of the connection's editor as a new query at this path in `folder`, never over another one.
    SaveNewQuery { connection: ConnectionName, folder: ConnectionName, path: QueryPath, text: String },
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
    /// Read the versions of the query a save replaced, for `ctrl-v`, each compared with the editor's `text`.
    ListVersions { query: Query, text: String },
    /// Put `text` on the clipboard, then report `done`; the loop also sends it to the terminal, unless it is too big.
    Copy { text: String, done: String },
    /// Build the text of `result` in `format` off the loop, then copy it as `Copy` does.
    CopyResult { result: ResultSet, format: Format, done: String },
    /// Read the machine's clipboard off the loop, for `⌘v` where the terminal hands the key over instead of pasting.
    PasteClipboard,
    /// Ask the `[ai] command` of `settings` with `prompt`, killing the command of the ask before it.
    Ask { ask: AskId, prompt: Prompt, settings: AiSettings },
    /// Kill the command of `ask` if it still runs; its answer never comes.
    DropAsk(AskId),
    /// Log why the idle ask shows no ghost, which nobody asked for and so nothing on screen says.
    LogNoIdleGhost(NoIdleGhost),
    /// Log why the idle ask left out the comment above the statement and asked to complete the statement instead.
    LogCommentLeftOut(NotSent),
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
    /// About the saved queries: the list of a queries folder, or a query read for one sheet.
    Queries(QueryAnswer),
    /// About the change to the config that `id` names.
    Config { id: ConfigChangeId, answer: ConfigAnswer },
    /// The history of `connection`: each statement once, newest first.
    History { connection: ConnectionName, entries: Vec<Entry> },
    /// The versions of `query` a save replaced, newest first.
    Versions { query: Query, versions: Vec<Listed> },
    /// The outcome of a copy to the clipboard, in a few words.
    Copied(String),
    /// The text of a result that `CopyResult` asked for, to copy before reporting `done`.
    Exported { text: String, done: String },
    /// The machine's clipboard that `PasteClipboard` asked for, to paste as the terminal's paste is; `None` when no
    /// program read it.
    Pasted(Option<String>),
    /// The statements the `[ai] command` answered `ask` with, its literals put back, or why there are none.
    Asked { ask: AskId, answer: Result<Vec<Statement>, AskFailure> },
}

/// Why `ctrl-g` shows no answer.
#[derive(Debug, PartialEq)]
pub(in crate::tui) enum AskFailure {
    /// `[ai]` names no command.
    NoCommand,
    /// The connection says `ai = "off"`.
    Off,
    NotSent(NotSent),
    Command(CommandError),
    Answer(AnswerError),
    /// The editor changed while the ask was on screen, so the answer no longer fits the text it was asked about.
    EditorChanged,
}

/// Why the idle ask shows no ghost; it never holds the statement nor the answer.
#[derive(Debug, PartialEq)]
pub(in crate::tui) enum NoIdleGhost {
    Failed(AskFailure),
    /// The answer does not start with the statement as typed, blanks, comments and the case of words aside.
    NotContinued,
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
    /// The saved queries of `folder`, sorted.
    All {
        folder: ConnectionName,
        queries: Vec<Query>,
    },
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
    NotOpened(Failure),
    /// The open session was lost, and why; the next run opens a new one.
    Closed(Lost),
    Tunnel(TunnelState),
    /// The schema a second session read, or why it could not read it.
    Schema(Result<Arc<Catalog>, Failure>),
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
    /// The server failed to send the next page of the run's result; the result keeps the rows it has.
    PageFailed {
        run: RunId,
        failure: Failure,
    },
    /// The rows a lookup read, or why it read none.
    LookedUp {
        look: LookId,
        rows: Result<ResultSet, Failure>,
    },
    /// The statement gave no result: pgbg kept it from the server, or it failed on the server or on the link to it.
    Failed {
        run: RunId,
        failure: Failure,
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

impl fmt::Display for AskFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCommand => f.write_str("^g asks nothing: config.toml names no [ai] command"),
            Self::Off => f.write_str("^g asks nothing: this connection says ai = \"off\""),
            Self::NotSent(not_sent) => write!(f, "not asked: {not_sent}"),
            Self::Command(failed) => failed.fmt(f),
            Self::Answer(unread) => unread.fmt(f),
            Self::EditorChanged => f.write_str("the answer is dropped: the editor changed since ^g asked"),
        }
    }
}

impl fmt::Display for NoIdleGhost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(failure) => write!(f, "no idle ghost: {failure}"),
            Self::NotContinued => f.write_str("no idle ghost: the answer does not continue the statement as typed"),
        }
    }
}
