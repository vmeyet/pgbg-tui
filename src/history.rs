//! Every statement run, per connection: one JSON line each in `<data folder>/pgbg/history/<connection>.jsonl`, oldest first.
//! A line never holds a password or a row.
use crate::config;
use crate::connection::ConnectionName;
use crate::db::model::{ResultSet, Statement};
use crate::failure::Failure;
use crate::lock;
use crate::secret;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::fs::{DirBuilder, File};
use std::io::{self, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::task::JoinHandle;

const MAX_BYTES: u64 = 4 * 1024 * 1024;
const DIR_MODE: u32 = 0o700;

/// What came of a statement, without the server's words: they can quote a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Outcome {
    Rows {
        count: usize,
        /// More rows were left on the server than came back with the first answer: cut, or waiting for the grid.
        is_cut: bool,
    },
    /// pgbg kept it from the server (its guard, or parameters it could not fill or type), or the server refused the write
    /// of a statement read as a read, after what came before that write ran.
    Refused,
    ServerError,
    /// The tunnel, the link or the login failed before the server answered.
    LinkError,
}

impl Outcome {
    pub(crate) fn of(result: &anyhow::Result<ResultSet>) -> Self {
        match result {
            Ok(result) => Self::rows(result),
            Err(err) => Self::from(&Failure::from(err)),
        }
    }

    pub(crate) fn rows(result: &ResultSet) -> Self {
        Self::Rows { count: result.rows().len(), is_cut: !result.is_whole() }
    }
}

impl From<&Failure> for Outcome {
    fn from(failure: &Failure) -> Self {
        match failure {
            Failure::Refused(_) | Failure::TriedToWrite | Failure::CancelledBeforeSent => Self::Refused,
            Failure::Server { .. } => Self::ServerError,
            Failure::TunnelNotStarted(_) | Failure::Login(_) | Failure::Link(_) | Failure::StoppedWithItsSession => Self::LinkError,
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rows { count: 1, is_cut } => write!(f, "1 row{}", if *is_cut { ", cut" } else { "" }),
            Self::Rows { count, is_cut } => write!(f, "{count} rows{}", if *is_cut { ", cut" } else { "" }),
            Self::Refused => f.write_str("refused"),
            Self::ServerError => f.write_str("server error"),
            Self::LinkError => f.write_str("link error"),
        }
    }
}

/// One statement run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) statement: Statement,
    pub(crate) connection: ConnectionName,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) took_ms: u64,
    pub(crate) outcome: Outcome,
}

impl Entry {
    /// The run of `statement` on `connection` that began at `started_at` and was answered `took` later.
    pub(crate) fn of_run(
        statement: Statement,
        connection: ConnectionName,
        started_at: DateTime<Utc>,
        took: Duration,
        outcome: Outcome,
    ) -> Self {
        let took_ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX);
        Self { statement, connection, started_at, took_ms, outcome }
    }
}

/// A connection's history, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct History {
    pub(crate) entries: Vec<Entry>,
}

impl History {
    /// Each statement once, at its newest run, newest first: what `ctrl-r` searches.
    pub(crate) fn distinct(self) -> Vec<Entry> {
        let mut seen = HashSet::new();
        self.entries.into_iter().filter(|entry| seen.insert(entry.statement.clone())).collect()
    }
}

/// The history files, one per connection, each rolled over to `<connection>.jsonl.1` once past its cap.
#[derive(Clone, Debug)]
pub(crate) struct Store {
    dir: PathBuf,
    max_bytes: u64,
}

impl Store {
    /// The `history` folder in pgbg's data folder.
    pub(crate) fn from_env() -> Option<Self> {
        config::data_dir().map(|dir| Self { dir: dir.join("history"), max_bytes: MAX_BYTES })
    }

    /// Adds `entry` at the end of its connection's file; a statement that holds a password is kept as a note instead.
    /// The file is locked while it is rolled over and written, so two pgbg never lose each other's lines.
    pub(crate) fn append(&self, entry: &Entry) -> io::Result<()> {
        DirBuilder::new().recursive(true).mode(DIR_MODE).create(&self.dir)?;
        let line = serde_json::to_string(&kept(entry)).map_err(io::Error::other)? + "\n";
        let path = self.file(&entry.connection);
        let file = lock::open(&path)?;
        let is_full = file.metadata()?.len() > self.max_bytes;
        let mut file = if is_full { self.rolled_over(&path, &entry.connection)? } else { file };
        file.write_all(line.as_bytes())
    }

    /// Moves the full file at `path` to `.1` and gives a new one, locked; the full one stays locked until it is dropped.
    fn rolled_over(&self, path: &Path, connection: &ConnectionName) -> io::Result<File> {
        std::fs::rename(path, self.rolled(connection))?;
        lock::open(path)
    }

    /// Appends `entry` on a blocking thread, logging a failure: the history is best effort and never stops a run.
    pub(crate) fn record(&self, entry: Entry) -> JoinHandle<()> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(err) = store.append(&entry) {
                tracing::warn!("the history was not written: {err}");
            }
        })
    }

    /// The history of `connection`, its rolled-over file included, newest first; a line pgbg cannot read is left out.
    pub(crate) fn read(&self, connection: &ConnectionName) -> io::Result<History> {
        let text = read_or_empty(self.rolled(connection))? + &read_or_empty(self.file(connection))?;
        let mut entries: Vec<Entry> = text.lines().filter_map(parsed).collect();
        entries.reverse();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.started_at));
        Ok(History { entries })
    }

    fn file(&self, connection: &ConnectionName) -> PathBuf {
        self.dir.join(format!("{connection}.jsonl"))
    }

    fn rolled(&self, connection: &ConnectionName) -> PathBuf {
        self.dir.join(format!("{connection}.jsonl.1"))
    }
}

fn kept(entry: &Entry) -> Entry {
    let statement = Statement::new(secret::kept_or_note(entry.statement.as_str()));
    Entry { statement, ..entry.clone() }
}

fn read_or_empty(path: PathBuf) -> io::Result<String> {
    match std::fs::read_to_string(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        read => read,
    }
}

fn parsed(line: &str) -> Option<Entry> {
    serde_json::from_str(line).inspect_err(|err| tracing::warn!("a history line pgbg cannot read: {err}")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::guard::Refusal;
    use crate::db::model::{Extent, Page, ServerError, TriedToWrite, TypeName, UnknownType};
    use crate::query::params::ParamError;
    use std::os::unix::fs::PermissionsExt;

    fn store(dir: &tempfile::TempDir) -> Store {
        Store { dir: dir.path().join("history"), max_bytes: MAX_BYTES }
    }

    fn entry(connection: &str, statement: &str, minute: u32) -> Entry {
        Entry {
            statement: Statement::new(statement),
            connection: connection.parse().unwrap(),
            started_at: DateTime::parse_from_rfc3339(&format!("2026-10-26T09:{minute:02}:00Z")).unwrap().to_utc(),
            took_ms: 38,
            outcome: Outcome::Rows { count: 4, is_cut: false },
        }
    }

    fn statements(history: &History) -> Vec<&str> {
        history.entries.iter().map(|entry| entry.statement.as_str()).collect()
    }

    fn shop() -> ConnectionName {
        "shop".parse().unwrap()
    }

    #[test]
    fn an_entry_is_one_json_line_of_plain_fields() {
        let line = r#"{"statement":"SELECT 1","connection":"shop","started_at":"2026-10-26T09:00:00Z","took_ms":38,"outcome":{"kind":"rows","count":4,"is_cut":false}}"#;
        assert_eq!(serde_json::to_string(&entry("shop", "SELECT 1", 0)).unwrap(), line);
        assert_eq!(serde_json::from_str::<Entry>(line).unwrap(), entry("shop", "SELECT 1", 0));
    }

    #[test]
    fn a_run_keeps_how_long_it_took_in_milliseconds() {
        let start = entry("shop", "SELECT 1", 0);
        let ran = Entry::of_run(start.statement.clone(), shop(), start.started_at, Duration::from_millis(38), start.outcome);
        assert_eq!(ran, start);
    }

    #[test]
    fn a_run_too_long_to_count_keeps_the_longest_time() {
        let start = entry("shop", "SELECT 1", 0);
        let ran = Entry::of_run(start.statement, shop(), start.started_at, Duration::MAX, start.outcome);
        assert_eq!(ran.took_ms, u64::MAX);
    }

    #[test]
    fn an_entry_appended_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).append(&entry("shop", "SELECT 1", 1)).unwrap();
        assert_eq!(store(&dir).read(&shop()).unwrap().entries, [entry("shop", "SELECT 1", 1)]);
    }

    #[test]
    fn the_newest_entry_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        for (minute, statement) in ["SELECT 1", "SELECT 2"].iter().enumerate() {
            store(&dir).append(&entry("shop", statement, minute as u32)).unwrap();
        }
        assert_eq!(statements(&store(&dir).read(&shop()).unwrap()), ["SELECT 2", "SELECT 1"]);
    }

    #[test]
    fn each_connection_has_its_own_file() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).append(&entry("prod", "SELECT 1", 1)).unwrap();
        assert_eq!(store(&dir).read(&shop()).unwrap().entries, []);
    }

    #[test]
    fn only_the_user_can_read_the_folder_and_the_file() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).append(&entry("shop", "SELECT 1", 1)).unwrap();
        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.path().join("history")), DIR_MODE);
        assert_eq!(mode(dir.path().join("history/shop.jsonl")), 0o600);
    }

    #[test]
    fn a_file_past_its_cap_rolls_over_and_is_still_read() {
        let dir = tempfile::tempdir().unwrap();
        let small = Store { max_bytes: 10, ..store(&dir) };
        for (minute, statement) in ["SELECT 1", "SELECT 2", "SELECT 3"].iter().enumerate() {
            small.append(&entry("shop", statement, minute as u32)).unwrap();
        }
        assert!(dir.path().join("history/shop.jsonl.1").exists());
        assert_eq!(statements(&small.read(&shop()).unwrap()), ["SELECT 3", "SELECT 2"]);
    }

    #[test]
    fn a_statement_that_sets_a_password_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).append(&entry("shop", "ALTER ROLE nina PASSWORD 'hunter2'", 1)).unwrap();
        store(&dir).append(&entry("shop", "SELECT password_hash FROM customers", 2)).unwrap();
        assert_eq!(statements(&store(&dir).read(&shop()).unwrap()), ["SELECT password_hash FROM customers", secret::NOT_KEPT]);
    }

    #[test]
    fn a_statement_quoting_a_url_with_a_password_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let dblink = "SELECT * FROM dblink('postgresql://nina:hunter2@db.example.com/shop', 'SELECT 1')";
        store(&dir).append(&entry("shop", dblink, 1)).unwrap();
        store(&dir).append(&entry("shop", "SELECT 'https://db.example.com/nina@shop'", 2)).unwrap();
        assert_eq!(statements(&store(&dir).read(&shop()).unwrap()), ["SELECT 'https://db.example.com/nina@shop'", secret::NOT_KEPT]);
    }

    #[test]
    fn a_statement_naming_a_secret_connection_key_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).append(&entry("shop", "SELECT dblink_connect('host=db.example.com sslpassword=hunter2')", 1)).unwrap();
        store(&dir)
            .append(&entry("shop", "SELECT dblink('postgresql://db.example.com/shop?oauth_client_secret=hunter2', 'SELECT 1')", 2))
            .unwrap();
        assert_eq!(statements(&store(&dir).read(&shop()).unwrap()), [secret::NOT_KEPT, secret::NOT_KEPT]);
    }

    #[test]
    fn a_line_pgbg_cannot_read_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).append(&entry("shop", "SELECT 1", 1)).unwrap();
        let path = dir.path().join("history/shop.jsonl");
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + "{not json\n").unwrap();
        assert_eq!(statements(&store(&dir).read(&shop()).unwrap()), ["SELECT 1"]);
    }

    #[test]
    fn a_connection_never_run_has_no_history() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(store(&dir).read(&shop()).unwrap(), History::default());
    }

    #[test]
    fn a_statement_run_twice_is_listed_once_at_its_newest() {
        let history = History { entries: vec![entry("shop", "SELECT 1", 3), entry("shop", "SELECT 2", 2), entry("shop", "SELECT 1", 1)] };
        let minutes: Vec<u32> = history.distinct().iter().map(|entry| chrono::Timelike::minute(&entry.started_at)).collect();
        assert_eq!(minutes, [3, 2]);
    }

    #[test]
    fn entries_written_out_of_order_are_read_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).append(&entry("shop", "SELECT 2", 2)).unwrap();
        store(&dir).append(&entry("shop", "SELECT 1", 1)).unwrap();
        assert_eq!(statements(&store(&dir).read(&shop()).unwrap()), ["SELECT 2", "SELECT 1"]);
    }

    #[tokio::test]
    async fn a_recorded_entry_is_appended() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).record(entry("shop", "SELECT 1", 1)).await.unwrap();
        assert_eq!(statements(&store(&dir).read(&shop()).unwrap()), ["SELECT 1"]);
    }

    #[test]
    fn an_outcome_says_what_failed_without_the_servers_words() {
        let server = ServerError {
            severity: "ERROR".into(),
            message: "nina".into(),
            detail: None,
            hint: None,
            position: None,
            is_refused_write: false,
        };
        assert_eq!(Outcome::of(&Err(server.into())), Outcome::ServerError);
        assert_eq!(Outcome::of(&Err(anyhow::anyhow!("connection refused"))), Outcome::LinkError);
        assert_eq!(Outcome::of(&Err(Refusal::Multiple.into())), Outcome::Refused);
        assert_eq!(serde_json::to_string(&Outcome::ServerError).unwrap(), r#"{"kind":"server_error"}"#);
    }

    #[test]
    fn a_statement_that_tried_to_write_unconfirmed_is_refused() {
        assert_eq!(Outcome::of(&Err(TriedToWrite.into())), Outcome::Refused);
    }

    #[test]
    fn a_parameter_that_cannot_be_filled_or_typed_is_refused() {
        assert_eq!(Outcome::of(&Err(ParamError::Mixed.into())), Outcome::Refused);
        assert_eq!(Outcome::of(&Err(UnknownType(TypeName::new("dat")).into())), Outcome::Refused);
    }

    #[test]
    fn a_run_whose_tunnel_did_not_start_is_a_link_error() {
        assert_eq!(Outcome::from(&Failure::TunnelNotStarted("exit status: 255".into())), Outcome::LinkError);
    }

    #[test]
    fn an_outcome_of_rows_says_how_many() {
        let result = ResultSet::new(vec![], Page { rows: vec![vec![]], extent: Extent::Cut });
        assert_eq!(Outcome::of(&Ok(result)).to_string(), "1 row, cut");
    }
}
