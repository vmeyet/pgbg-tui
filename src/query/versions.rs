//! The texts a save replaced, kept outside the project in pgbg's data folder: one file each in
//! `versions/<hash of the query's real path>/<when it was replaced>.sql`, the newest `MAX_VERSIONS` of each query.
use super::{Query, Roots};
use crate::config;
use anyhow::Result;
use chrono::{DateTime, NaiveDateTime, Utc};
use std::fs::{self, DirBuilder};
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

/// How many versions a query keeps, the oldest dropped past it: a save every few minutes keeps about a working day,
/// and a query of 1 MiB, the most the editor takes, never fills more than 50 MiB.
const MAX_VERSIONS: usize = 50;
/// Only the user reads the folder: a query can quote a value its author did not mean to share.
pub(super) const DIR_MODE: u32 = 0o700;
const EXTENSION: &str = ".sql";
/// When the version was replaced, in UTC to the millisecond, so the names sort as the versions do.
const NAME_FORMAT: &str = "%Y%m%dT%H%M%S%.3fZ";

/// A text a save replaced, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Version {
    pub(crate) replaced_at: DateTime<Utc>,
    pub(crate) text: String,
}

/// The versions of every saved query.
#[derive(Clone, Debug)]
pub(crate) struct Versions {
    dir: PathBuf,
}

impl Roots {
    /// The roots keeping in `versions` the text each save of a query replaces; none keeps no version.
    pub(crate) fn with_versions(self, versions: Option<Versions>) -> Self {
        Self { versions, ..self }
    }

    /// The versions of `query`, newest first. Blocks on the file system.
    pub(crate) fn versions(&self, query: &Query) -> Result<Vec<Version>> {
        let Some(versions) = &self.versions else { return Ok(vec![]) };
        Ok(versions.list(&self.real_file(query)?)?)
    }

    /// Runs `edit` on the file of `query`, as the user's editor does, then keeps the text it replaced. Blocks.
    pub(crate) fn edit_kept(&self, query: &Query, edit: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
        let path = self.file(query)?;
        let before = self.read_blocking(query);
        edit(&path)?;
        match (before, self.read_blocking(query)) {
            (Ok(before), Ok(after)) => self.keep(query, &before, &after),
            (Err(err), _) | (_, Err(err)) => tracing::warn!("the version the editor replaced was not kept: {err:#}"),
        }
        Ok(())
    }

    /// Keeps `old`, the text of `query` that `new` replaces, best effort: a version not kept is logged and never stops
    /// a save. Blocks on the file system.
    pub(super) fn keep(&self, query: &Query, old: &str, new: &str) {
        let Some(versions) = self.versions.as_ref().filter(|_| old != new) else { return };
        let kept = self.real_file(query).and_then(|file| Ok(versions.keep(&file, old, Utc::now())?));
        if let Err(err) = kept {
            tracing::warn!("the version a save replaced was not kept: {err:#}");
        }
    }

    /// The real path of the file of `query`, which names its versions.
    fn real_file(&self, query: &Query) -> Result<PathBuf> {
        Ok(query.path.under(&self.existing_root(&query.source)?.real))
    }
}

impl Versions {
    /// The `versions` folder in pgbg's data folder.
    pub(crate) fn from_env() -> Option<Self> {
        config::data_dir().map(|dir| Self { dir: dir.join("versions") })
    }

    /// Keeps `text` as the version of `file` replaced `at`, unless it is already the newest one, then drops the oldest
    /// past `MAX_VERSIONS`; safe to run twice. Blocks on the file system.
    fn keep(&self, file: &Path, text: &str, at: DateTime<Utc>) -> io::Result<()> {
        let folder = self.folder(file);
        let names = names_newest_first(&folder)?;
        if let Some(newest) = names.first()
            && fs::read_to_string(folder.join(newest)).is_ok_and(|newest| newest == text)
        {
            return Ok(());
        }
        DirBuilder::new().recursive(true).mode(DIR_MODE).create(&folder)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&folder)?;
        temporary.write_all(text.as_bytes())?;
        let kept = name(at);
        temporary.persist(folder.join(&kept)).map_err(|err| err.error)?;
        for oldest in names.iter().filter(|name| **name != kept).skip(MAX_VERSIONS - 1) {
            fs::remove_file(folder.join(oldest))?;
        }
        Ok(())
    }

    /// The versions of `file`, newest first; a file pgbg cannot read is left out.
    fn list(&self, file: &Path) -> io::Result<Vec<Version>> {
        let folder = self.folder(file);
        let names = names_newest_first(&folder)?;
        Ok(names.iter().filter_map(|name| version(&folder, name)).collect())
    }

    fn folder(&self, file: &Path) -> PathBuf {
        self.dir.join(format!("{:016x}", fnv1a(file.as_os_str().as_bytes())))
    }
}

/// FNV-1a: a hash that stays the same across Rust releases, as the folder it names must.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(PRIME))
}

fn name(at: DateTime<Utc>) -> String {
    format!("{}{EXTENSION}", at.format(NAME_FORMAT))
}

fn replaced_at(name: &str) -> Option<DateTime<Utc>> {
    let stem = name.strip_suffix(EXTENSION)?;
    NaiveDateTime::parse_from_str(stem, NAME_FORMAT).ok().map(|at| at.and_utc())
}

/// The names of the versions in `folder`, newest first; none when the query has none yet.
fn names_newest_first(folder: &Path) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(folder) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        entries => entries?,
    };
    let mut names: Vec<String> =
        entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok()).filter(|name| replaced_at(name).is_some()).collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    Ok(names)
}

fn version(folder: &Path, name: &str) -> Option<Version> {
    let text = fs::read_to_string(folder.join(name)).inspect_err(|err| tracing::warn!("a version pgbg cannot read: {err}")).ok()?;
    Some(Version { replaced_at: replaced_at(name)?, text })
}

#[cfg(test)]
mod tests {
    use super::super::{QueryPath, Root, Source, Write};
    use super::*;
    use chrono::Duration;
    use std::os::unix::fs::PermissionsExt;

    const DAILY: &str = "/Users/nina/shop/.pgbg/queries/daily.sql";

    fn store(dir: &tempfile::TempDir) -> Versions {
        Versions { dir: dir.path().join("versions") }
    }

    fn at(minute: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-26T09:00:00Z").unwrap().to_utc() + Duration::minutes(minute)
    }

    fn texts(versions: &Versions) -> Vec<String> {
        versions.list(Path::new(DAILY)).unwrap().into_iter().map(|version| version.text).collect()
    }

    /// A project whose queries live in `queries` and whose versions live in `data`.
    fn kept(queries: &tempfile::TempDir, data: &tempfile::TempDir) -> Roots {
        let project = Root::checked(queries.path().into(), vec![queries.path().into()]);
        let roots = Roots { project, ..Roots::default() };
        roots.with_versions(Some(store(data)))
    }

    fn daily() -> Query {
        Query { source: Source::Project, path: QueryPath::parse("daily.sql").unwrap() }
    }

    fn version_texts(roots: &Roots) -> Vec<String> {
        roots.versions(&daily()).unwrap().into_iter().map(|version| version.text).collect()
    }

    #[test]
    fn a_version_is_read_back_with_when_it_was_replaced() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).keep(Path::new(DAILY), "SELECT 1", at(0)).unwrap();
        assert_eq!(store(&dir).list(Path::new(DAILY)).unwrap(), [Version { replaced_at: at(0), text: "SELECT 1".into() }]);
    }

    #[test]
    fn the_newest_version_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).keep(Path::new(DAILY), "SELECT 1", at(0)).unwrap();
        store(&dir).keep(Path::new(DAILY), "SELECT 2", at(1)).unwrap();
        assert_eq!(texts(&store(&dir)), ["SELECT 2", "SELECT 1"]);
    }

    #[test]
    fn keeping_a_version_twice_keeps_it_once() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).keep(Path::new(DAILY), "SELECT 1", at(0)).unwrap();
        store(&dir).keep(Path::new(DAILY), "SELECT 1", at(1)).unwrap();
        assert_eq!(texts(&store(&dir)), ["SELECT 1"]);
    }

    #[test]
    fn past_the_cap_the_oldest_version_goes() {
        let dir = tempfile::tempdir().unwrap();
        for minute in 0..=MAX_VERSIONS {
            store(&dir).keep(Path::new(DAILY), &format!("SELECT {minute}"), at(minute.try_into().unwrap())).unwrap();
        }
        let kept = texts(&store(&dir));
        assert_eq!((kept.len(), kept.last().map(String::as_str)), (MAX_VERSIONS, Some("SELECT 1")));
    }

    #[test]
    fn each_query_has_its_own_versions() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).keep(Path::new("/Users/nina/shop/.pgbg/queries/weekly.sql"), "SELECT 7", at(0)).unwrap();
        assert_eq!(texts(&store(&dir)), Vec::<String>::new());
    }

    #[test]
    fn only_the_user_can_read_the_versions_folder() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).keep(Path::new(DAILY), "SELECT 1", at(0)).unwrap();
        let folder = store(&dir).folder(Path::new(DAILY));
        assert_eq!(fs::metadata(folder).unwrap().permissions().mode() & 0o777, DIR_MODE);
    }

    #[test]
    fn a_file_that_is_not_a_version_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir).keep(Path::new(DAILY), "SELECT 1", at(0)).unwrap();
        fs::write(store(&dir).folder(Path::new(DAILY)).join("notes.txt"), "SELECT 9").unwrap();
        assert_eq!(texts(&store(&dir)), ["SELECT 1"]);
    }

    #[test]
    fn the_hash_naming_a_folder_gives_the_published_fnv1a_values() {
        assert_eq!((fnv1a(b""), fnv1a(b"a")), (0xcbf2_9ce4_8422_2325, 0xaf63_dc4c_8601_ec8c));
    }

    #[test]
    fn a_save_over_a_query_keeps_the_text_it_replaced() {
        let (queries, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = kept(&queries, &data);
        roots.write(&daily(), "SELECT 1", Write::Create).unwrap();
        roots.write(&daily(), "SELECT 2", Write::Replace).unwrap();
        assert_eq!(version_texts(&roots), ["SELECT 1"]);
    }

    #[test]
    fn saving_the_same_text_keeps_nothing() {
        let (queries, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = kept(&queries, &data);
        for _ in 0..2 {
            roots.write(&daily(), "SELECT 1", Write::Replace).unwrap();
        }
        assert_eq!(version_texts(&roots), Vec::<String>::new());
    }

    #[test]
    fn a_version_is_never_kept_in_the_project() {
        let (queries, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = kept(&queries, &data);
        roots.write(&daily(), "SELECT 1", Write::Create).unwrap();
        roots.write(&daily(), "SELECT 2", Write::Replace).unwrap();
        assert_eq!(fs::read_dir(queries.path()).unwrap().count(), 1);
    }

    #[test]
    fn an_edit_in_the_users_editor_keeps_the_text_it_replaced() {
        let (queries, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = kept(&queries, &data);
        roots.write(&daily(), "SELECT 1", Write::Create).unwrap();
        roots.edit_kept(&daily(), |path| Ok(fs::write(path, "SELECT 2")?)).unwrap();
        assert_eq!(version_texts(&roots), ["SELECT 1"]);
    }

    #[test]
    fn an_edit_that_changes_nothing_keeps_nothing() {
        let (queries, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = kept(&queries, &data);
        roots.write(&daily(), "SELECT 1", Write::Create).unwrap();
        roots.edit_kept(&daily(), |_| Ok(())).unwrap();
        assert_eq!(version_texts(&roots), Vec::<String>::new());
    }
}
