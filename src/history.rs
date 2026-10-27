//! Every statement run, per connection: one JSON line each in `<data folder>/pgbg/history/<connection>.jsonl`, oldest first.
//! A line never holds a password or a row.
use crate::config;
use crate::connection::ConnectionName;
use crate::db::guard::Refusal;
use crate::db::model::{ResultSet, ServerError, UnknownType};
use crate::query::params::ParamError;
use chrono::{DateTime, Utc};
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use tokio::task::JoinHandle;

const PATH_VARIABLE: &str = "PGBG_DATA_DIR";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
/// How many times a writer opens the file again when another one rolled it over while it waited for the lock.
/// Without a cap, a file system whose files never stay the same (some FUSE and SMB mounts) would loop forever.
const LOCK_ATTEMPTS: usize = 8;
const KEPT_CHANGING: &str = "the history file kept changing while pgbg waited for its lock";
const PASSWORD_WORD: &str = "password";
const SCHEME_END: &str = "://";
const PASSWORD_KEPT_AS: &str = "-- not kept: it holds a password";

/// What came of a statement, without the server's words: they can quote a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Outcome {
    Rows {
        count: usize,
        /// More rows were left on the server than came back with the first answer: cut, or waiting for the grid.
        is_cut: bool,
    },
    /// pgbg kept it from the server: its guard, or parameters it could not fill or type.
    Refused,
    ServerError,
    /// The tunnel, the link or the login failed before the server answered.
    LinkError,
}

impl Outcome {
    pub(crate) fn of(result: &anyhow::Result<ResultSet>) -> Self {
        match result {
            Ok(result) => Self::Rows { count: result.rows().len(), is_cut: !result.is_whole() },
            Err(err) if err.is::<Refusal>() || err.is::<ParamError>() || err.is::<UnknownType>() => Self::Refused,
            Err(err) if err.is::<ServerError>() => Self::ServerError,
            Err(_) => Self::LinkError,
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
    pub(crate) statement: String,
    pub(crate) connection: ConnectionName,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) took_ms: u64,
    pub(crate) outcome: Outcome,
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
    /// The `history` folder in `$PGBG_DATA_DIR`, else `pgbg/history` in the user's data folder.
    pub(crate) fn from_env() -> Option<Self> {
        dir_from(std::env::var_os(PATH_VARIABLE), dirs::data_dir()).map(|dir| Self { dir, max_bytes: MAX_BYTES })
    }

    /// Adds `entry` at the end of its connection's file; a statement that holds a password is kept as a note instead.
    /// The file is locked while it is rolled over and written, so two pgbg never lose each other's lines.
    pub(crate) fn append(&self, entry: &Entry) -> io::Result<()> {
        DirBuilder::new().recursive(true).mode(DIR_MODE).create(&self.dir)?;
        let line = serde_json::to_string(&kept(entry)).map_err(io::Error::other)? + "\n";
        let path = self.file(&entry.connection);
        let file = locked(&path)?;
        let is_full = file.metadata()?.len() > self.max_bytes;
        let mut file = if is_full { self.rolled_over(&path, &entry.connection)? } else { file };
        file.write_all(line.as_bytes())
    }

    /// Moves the full file at `path` to `.1` and gives a new one, locked; the full one stays locked until it is dropped.
    fn rolled_over(&self, path: &Path, connection: &ConnectionName) -> io::Result<File> {
        std::fs::rename(path, self.rolled(connection))?;
        locked(path)
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

fn dir_from(variable: Option<OsString>, data_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(config::named_path(variable).or_else(|| data_dir.map(|dir| dir.join("pgbg")))?.join("history"))
}

/// The file at `path`, opened to append and locked for this process alone; opened again, a few times at most, when
/// another writer rolled it over while this one waited.
fn locked(path: &Path) -> io::Result<File> {
    first_some(LOCK_ATTEMPTS, || {
        let file = OpenOptions::new().create(true).append(true).mode(FILE_MODE).open(path)?;
        locked_if_current(file, path)
    })
}

/// What the first of `attempts` tries gives, else an error.
fn first_some<T>(attempts: usize, mut attempt: impl FnMut() -> io::Result<Option<T>>) -> io::Result<T> {
    for _ in 0..attempts {
        if let Some(found) = attempt()? {
            return Ok(found);
        }
    }
    Err(io::Error::other(KEPT_CHANGING))
}

/// `file` once locked, while it is still the one at `path`; `None` when it was rolled over before the lock was taken.
fn locked_if_current(file: File, path: &Path) -> io::Result<Option<File>> {
    flock(&file, FlockOperation::LockExclusive)?;
    let held = file.metadata()?;
    let current = match std::fs::metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        current => current?,
    };
    let is_current = held.dev() == current.dev() && held.ino() == current.ino();
    Ok(is_current.then_some(file))
}

fn kept(entry: &Entry) -> Entry {
    let statement = if holds_password(&entry.statement) { PASSWORD_KEPT_AS.to_owned() } else { entry.statement.clone() };
    Entry { statement, ..entry.clone() }
}

/// Whether `password` stands as a word of its own, as in `ALTER ROLE nina PASSWORD '...'`, or a URL holds one, as in
/// `postgresql://nina:...@db.example.com`; `password_hash` is another word.
fn holds_password(statement: &str) -> bool {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let has_password_word = statement.split(|c: char| !is_word_char(c)).any(|word| word.eq_ignore_ascii_case(PASSWORD_WORD));
    has_password_word || statement.match_indices(SCHEME_END).any(|(at, _)| is_url_with_password(&statement[..at], &statement[at..]))
}

/// Whether a scheme ends `before` and `after` opens on `://user:password@`.
fn is_url_with_password(before: &str, after: &str) -> bool {
    let has_scheme = before.ends_with(|c: char| c.is_alphanumeric() || c == '_');
    let Some((user, password)) = after[SCHEME_END.len()..].split_once('@').and_then(|(user_info, _)| user_info.split_once(':')) else {
        return false;
    };
    let is_user = !user.is_empty() && !user.contains(|c: char| c == '/' || c.is_whitespace());
    let is_password = !password.is_empty() && !password.contains(char::is_whitespace);
    has_scheme && is_user && is_password
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
    use crate::db::model::{Extent, Page, TypeName};
    use std::os::unix::fs::PermissionsExt;

    fn store(dir: &tempfile::TempDir) -> Store {
        Store { dir: dir.path().join("history"), max_bytes: MAX_BYTES }
    }

    fn entry(connection: &str, statement: &str, minute: u32) -> Entry {
        Entry {
            statement: statement.to_owned(),
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
    fn a_file_that_keeps_changing_under_the_lock_is_an_error_not_a_loop() {
        let err = first_some(3, || Ok(None::<File>)).unwrap_err();
        assert!(err.to_string().contains("kept"), "{err}");
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
        assert_eq!(mode(dir.path().join("history/shop.jsonl")), FILE_MODE);
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
        assert_eq!(statements(&store(&dir).read(&shop()).unwrap()), ["SELECT password_hash FROM customers", PASSWORD_KEPT_AS]);
    }

    #[test]
    fn a_statement_quoting_a_url_with_a_password_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let dblink = "SELECT * FROM dblink('postgresql://nina:hunter2@db.example.com/shop', 'SELECT 1')";
        store(&dir).append(&entry("shop", dblink, 1)).unwrap();
        store(&dir).append(&entry("shop", "SELECT 'https://db.example.com/nina@shop'", 2)).unwrap();
        assert_eq!(statements(&store(&dir).read(&shop()).unwrap()), ["SELECT 'https://db.example.com/nina@shop'", PASSWORD_KEPT_AS]);
    }

    #[test]
    fn a_file_rolled_over_by_another_writer_is_not_locked() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).append(&entry("shop", "SELECT 1", 1)).unwrap();
        let path = dir.path().join("history/shop.jsonl");
        let opened_before = OpenOptions::new().append(true).open(&path).unwrap();
        std::fs::rename(&path, dir.path().join("history/shop.jsonl.1")).unwrap();
        assert!(locked_if_current(opened_before, &path).unwrap().is_none());
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
    fn the_folder_is_the_variable_else_pgbg_in_the_data_folder() {
        assert_eq!(dir_from(Some("/tmp/pgbg-data".into()), Some("/data".into())), Some(PathBuf::from("/tmp/pgbg-data/history")));
        assert_eq!(dir_from(Some("".into()), Some("/data".into())), Some(PathBuf::from("/data/pgbg/history")));
        assert_eq!(dir_from(None, None), None);
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
        let server = ServerError { severity: "ERROR".into(), message: "nina".into(), detail: None, hint: None, position: None };
        assert_eq!(Outcome::of(&Err(server.into())), Outcome::ServerError);
        assert_eq!(Outcome::of(&Err(anyhow::anyhow!("connection refused"))), Outcome::LinkError);
        assert_eq!(Outcome::of(&Err(Refusal::Multiple.into())), Outcome::Refused);
        assert_eq!(serde_json::to_string(&Outcome::ServerError).unwrap(), r#"{"kind":"server_error"}"#);
    }

    #[test]
    fn a_parameter_that_cannot_be_filled_or_typed_is_refused() {
        assert_eq!(Outcome::of(&Err(ParamError::Mixed.into())), Outcome::Refused);
        assert_eq!(Outcome::of(&Err(UnknownType(TypeName::new("dat")).into())), Outcome::Refused);
    }

    #[test]
    fn an_outcome_of_rows_says_how_many() {
        let result = ResultSet::new(vec![], Page { rows: vec![vec![]], extent: Extent::Cut });
        assert_eq!(Outcome::of(&Ok(result)).to_string(), "1 row, cut");
    }
}
