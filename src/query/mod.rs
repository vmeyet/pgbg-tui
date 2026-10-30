//! Saved queries: the `.sql` files under `queries/<connection>/` in pgbg's data folder, never in a project.
use crate::config;
use crate::connection::ConnectionName;
use crate::render;
use anyhow::{Context, Result, bail};
use rustix::fs::{Mode, OFlags};
#[cfg(target_os = "macos")]
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirEntry, Metadata};
use std::io::{self, Read};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub(crate) use header::Header;
pub(crate) use save::Write;
pub(crate) use tree::{TreeRow, tree_rows};
pub(crate) use versions::{Version, Versions};

pub(crate) mod condition;
mod header;
pub(crate) mod params;
mod save;
mod tree;
mod versions;

const QUERIES_DIR: &str = "queries";
const EXTENSION: &str = ".sql";
/// The biggest query the editor takes: past it, the file is a dump, not a query.
const MAX_QUERY_BYTES: u64 = 1 << 20;
const NOT_A_FILE: &str = "it is not a plain file";

/// Where a query sits under its root, as `orders/daily.sql`: relative, without `..` and with a name, so it never leaves the root.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct QueryPath(String);

impl QueryPath {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let stays_inside = text.split('/').all(|part| !matches!(part, "" | "." | "..") && !part.contains(['\\', '\0']));
        let has_name = text.rsplit('/').next().is_some_and(|file| file.len() > EXTENSION.len());
        if !stays_inside || !has_name || !text.ends_with(EXTENSION) {
            bail!(
                "`{}` is not a query path: name a {EXTENSION} file under the queries folder, as orders/daily.sql",
                render::printable(text)
            );
        }
        Ok(Self(text.to_owned()))
    }

    fn parts(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    fn under(&self, root: &Path) -> PathBuf {
        self.parts().fold(root.to_path_buf(), |path, part| path.join(part))
    }
}

/// Prints the path with its control characters and text-reversing marks escaped, as it is drawn.
impl fmt::Display for QueryPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&render::printable(&self.0))
    }
}

/// One saved query: the queries folder it is under and its path there.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Query {
    /// The connection whose folder, `queries/<connection>/` in pgbg's data folder, holds the query.
    pub(crate) folder: ConnectionName,
    pub(crate) path: QueryPath,
}

/// Why a folder is not taken as a root of queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootProblem {
    Unreadable,
    Link,
    NotAFolder,
    OwnedByAnotherUser,
    LeadsElsewhere,
}

impl fmt::Display for RootProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unreadable => "it cannot be read",
            Self::Link => "it is a link",
            Self::NotAFolder => "it is not a folder",
            Self::OwnedByAnotherUser => "another user owns it",
            Self::LeadsElsewhere => "it leads somewhere else than when pgbg started",
        })
    }
}

fn root_problem(dir: &Path, owner: u32) -> Option<RootProblem> {
    let Ok(metadata) = fs::symlink_metadata(dir) else { return Some(RootProblem::Unreadable) };
    if metadata.file_type().is_symlink() {
        return Some(RootProblem::Link);
    }
    if !metadata.is_dir() {
        return Some(RootProblem::NotAFolder);
    }
    (metadata.uid() != owner).then_some(RootProblem::OwnedByAnotherUser)
}

/// A folder of queries as it was found: the folders checked on the way to it, and the real path it had then.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Root {
    dir: PathBuf,
    checked: Vec<PathBuf>,
    real: PathBuf,
}

impl Root {
    /// `dir` when each of `checked` is a folder of the user's own and none is a link, else none and a warning.
    fn checked(dir: PathBuf, checked: Vec<PathBuf>) -> Option<Self> {
        let real =
            fs::canonicalize(&dir).inspect_err(|err| tracing::warn!("the queries folder {} is skipped: {err}", dir.display())).ok()?;
        let root = Self { dir, checked, real };
        root.is_still_safe().then_some(root)
    }

    /// Whether the folder still passes the checks it passed when found, and leads where it led; a warning when not.
    fn is_still_safe(&self) -> bool {
        let Some((dir, problem)) = self.problem() else { return true };
        tracing::warn!("the queries folder {} is skipped: {problem}", dir.display());
        false
    }

    fn problem(&self) -> Option<(&Path, RootProblem)> {
        let owner = rustix::process::getuid().as_raw();
        let checked = self.checked.iter().find_map(|dir| Some((dir.as_path(), root_problem(dir, owner)?)));
        let leads_elsewhere = fs::canonicalize(&self.dir).ok().as_ref() != Some(&self.real);
        checked.or_else(|| leads_elsewhere.then_some((self.dir.as_path(), RootProblem::LeadsElsewhere)))
    }
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The folder in `data_dir` that holds the saved queries, one folder per connection.
pub(crate) fn user_queries_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(QUERIES_DIR)
}

/// The folders queries live in: one of each connection, under pgbg's data folder.
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(Default))]
pub(crate) struct Roots {
    /// pgbg's data folder, where a connection's own folder is made at its first save; none keeps no query of the user's.
    data_dir: Option<PathBuf>,
    /// Where a save keeps the text it replaces; none keeps no version.
    versions: Option<Versions>,
}

impl Roots {
    /// The folders under pgbg's data folder; the working directory plays no part, so a project never gives a query.
    pub(crate) fn from_env() -> Self {
        Self { data_dir: config::data_dir(), versions: None }
    }

    /// The query at `path` in `folder`, `.sql` added when it lacks it.
    pub(crate) async fn named(&self, path: &str, folder: &ConnectionName) -> Result<Query> {
        let with_extension = if path.ends_with(EXTENSION) { path.to_owned() } else { format!("{path}{EXTENSION}") };
        let path = QueryPath::parse(&with_extension)?;
        let (roots, folder) = (self.clone(), folder.clone());
        let listed = tokio::task::spawn_blocking(move || roots.list(&folder)).await?;
        listed
            .into_iter()
            .find(|query| query.path == path)
            .with_context(|| format!("no saved query {path}: `pgbg` lists them in its sidebar"))
    }

    /// The folder of the connection `folder`, `queries/<connection>/` in pgbg's data folder.
    fn dir(&self, folder: &ConnectionName) -> Option<PathBuf> {
        Some(user_queries_dir(self.data_dir.as_ref()?).join(folder.to_string()))
    }

    /// The root of `folder`, when it exists and passes the checks. Blocks on the file system.
    fn root(&self, folder: &ConnectionName) -> Option<Root> {
        let root = self.dir(folder).filter(|dir| exists(dir))?;
        let queries = root.parent()?.to_path_buf();
        Root::checked(root.clone(), vec![queries, root])
    }

    /// Every query of `folder`, sorted; a folder that cannot be read, or that changed since it was found, is skipped
    /// with a warning. Blocks on the file system.
    pub(crate) fn list(&self, folder: &ConnectionName) -> Vec<Query> {
        let Some(root) = self.root(folder).filter(Root::is_still_safe) else { return vec![] };
        let mut queries: Vec<Query> = files_under(&root.dir, "").into_iter().map(|path| Query { folder: folder.clone(), path }).collect();
        queries.sort();
        queries
    }

    /// The text of `query`, read without blocking: a plain file inside its root, as the root was found, `MAX_QUERY_BYTES` at most.
    pub(crate) async fn read(&self, query: &Query) -> Result<String> {
        let (roots, owned) = (self.clone(), query.clone());
        let read = tokio::task::spawn_blocking(move || roots.read_blocking(&owned)).await?;
        read.with_context(|| format!("reading {}", query.path))
    }

    fn read_blocking(&self, query: &Query) -> Result<String> {
        let root = self.existing_root(&query.folder)?;
        read_checked(&root, &query.path.under(&root.real))
    }

    /// The root of `folder`, or an error naming it when it does not exist. Blocks on the file system.
    fn existing_root(&self, folder: &ConnectionName) -> Result<Root> {
        self.root(folder).with_context(|| format!("no queries folder for {folder}"))
    }
}

/// The text at `path` once opened, when `root` is still safe and the opened file is a plain one inside it. Blocks.
fn read_checked(root: &Root, path: &Path) -> Result<String> {
    if !root.is_still_safe() {
        bail!("its queries folder changed since pgbg started");
    }
    let file = open_unlinked(path)?;
    check_inside(&root.real, &opened_path(&file)?)?;
    check_readable(&file.metadata()?)?;
    let mut text = String::new();
    file.take(MAX_QUERY_BYTES + 1).read_to_string(&mut text)?;
    check_size(text.len() as u64)?;
    Ok(text)
}

/// Opens `path` for reading unless it is a link; a pipe opens without waiting for a writer, then fails the checks.
fn open_unlinked(path: &Path) -> Result<fs::File> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    Ok(fs::File::from(rustix::fs::open(path, flags, Mode::empty())?))
}

#[cfg(target_os = "macos")]
fn opened_path(file: &fs::File) -> Result<PathBuf> {
    Ok(PathBuf::from(OsString::from_vec(rustix::fs::getpath(file)?.into_bytes())))
}

#[cfg(target_os = "linux")]
fn opened_path(file: &fs::File) -> Result<PathBuf> {
    Ok(fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?)
}

/// Refuses a file outside the root whose real path is `real_root`: a link swapped in after the listing can lead there.
fn check_inside(real_root: &Path, opened: &Path) -> Result<()> {
    if !opened.starts_with(real_root) {
        bail!("it leads out of the queries folder");
    }
    Ok(())
}

fn check_readable(metadata: &Metadata) -> Result<()> {
    if !metadata.is_file() {
        bail!(NOT_A_FILE);
    }
    check_size(metadata.len())
}

fn check_size(bytes: u64) -> Result<()> {
    if bytes > MAX_QUERY_BYTES {
        bail!("it is too big for the editor: {} KiB at most", MAX_QUERY_BYTES >> 10);
    }
    Ok(())
}

/// The `.sql` files under `dir`, as paths from `prefix`; a link is never followed, so nothing outside the root is listed.
fn files_under(dir: &Path, prefix: &str) -> Vec<QueryPath> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("reading the queries folder {}: {err}", dir.display());
            return vec![];
        }
    };
    let mut paths = vec![];
    for entry in entries {
        match entry.and_then(|entry| entry_paths(&entry, prefix)) {
            Ok(found) => paths.extend(found),
            Err(err) => tracing::warn!("reading an entry of the queries folder {}: {err}", dir.display()),
        }
    }
    paths
}

/// The queries `entry` holds: itself when it is a plain `.sql` file, every one under it when it is a folder.
#[expect(clippy::filetype_is_file, reason = "only a regular file is a query: a pipe or a socket would block the read")]
fn entry_paths(entry: &DirEntry, prefix: &str) -> io::Result<Vec<QueryPath>> {
    let kind = entry.file_type()?;
    let Some(name) = entry.file_name().to_str().map(|name| format!("{prefix}{name}")) else {
        tracing::warn!("a query whose name is not UTF-8 is skipped: {}", entry.path().display());
        return Ok(vec![]);
    };
    if kind.is_dir() {
        return Ok(files_under(&entry.path(), &format!("{name}/")));
    }
    let is_query_file = kind.is_file() && name.ends_with(EXTENSION);
    if !is_query_file {
        return Ok(vec![]);
    }
    match QueryPath::parse(&name) {
        Ok(path) => Ok(vec![path]),
        Err(err) => {
            tracing::warn!("a query is skipped: {err}");
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, file: &str) {
        let at = root.join(file);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(at, "SELECT 1").unwrap();
    }

    fn name(text: &str) -> ConnectionName {
        text.parse().unwrap()
    }

    fn in_data(data: &Path) -> Roots {
        Roots { data_dir: Some(data.into()), ..Roots::default() }
    }

    /// The folder of `shop` under the data folder `data`, made empty.
    fn shop_dir(data: &Path) -> PathBuf {
        let dir = data.join("queries/shop");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn listed(roots: &Roots) -> Vec<String> {
        roots.list(&name("shop")).iter().map(|query| query.path.to_string()).collect()
    }

    fn daily() -> Query {
        Query { folder: name("shop"), path: QueryPath::parse("orders/daily.sql").unwrap() }
    }

    #[test]
    fn a_query_path_names_a_sql_file_inside_its_root() {
        assert_eq!(QueryPath::parse("orders/daily.sql").unwrap().under(Path::new("/q")), Path::new("/q/orders/daily.sql"));
        for refused in ["/etc/passwd.sql", "../shop.sql", "orders/../../x.sql", "orders//x.sql", "./x.sql", "a\\..\\x.sql", "x.txt"] {
            assert!(QueryPath::parse(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn listing_finds_every_sql_file_of_the_connections_folder_sorted() {
        let data = tempfile::tempdir().unwrap();
        for file in ["orders/daily.sql", "customers.sql", "notes.txt", "orders/weekly/top.sql"] {
            write(&shop_dir(data.path()), file);
        }
        assert_eq!(listed(&in_data(data.path())), ["customers.sql", "orders/daily.sql", "orders/weekly/top.sql"]);
    }

    #[test]
    fn a_connections_own_query_is_not_listed_on_another_connection() {
        let data = tempfile::tempdir().unwrap();
        write(data.path(), "queries/staging/mine.sql");
        let roots = in_data(data.path());
        assert_eq!(roots.list(&name("shop")), []);
        assert_eq!(roots.list(&name("staging")).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_connections_own_folder_that_is_a_link_is_refused() {
        let (data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(outside.path(), "mine.sql");
        fs::create_dir_all(data.path().join("queries")).unwrap();
        std::os::unix::fs::symlink(outside.path(), data.path().join("queries/shop")).unwrap();
        assert_eq!(listed(&in_data(data.path())), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn a_queries_folder_that_is_a_link_is_refused() {
        let (data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(outside.path(), "shop/mine.sql");
        std::os::unix::fs::symlink(outside.path(), data.path().join("queries")).unwrap();
        assert_eq!(listed(&in_data(data.path())), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn listing_follows_no_link_out_of_the_root() {
        let (data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = shop_dir(data.path());
        write(outside.path(), "secret.sql");
        std::os::unix::fs::symlink(outside.path(), root.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.sql"), root.join("secret.sql")).unwrap();
        assert_eq!(listed(&in_data(data.path())), Vec::<String>::new());
    }

    #[test]
    fn a_query_path_without_a_name_is_refused() {
        for refused in [".sql", "orders/.sql"] {
            assert!(QueryPath::parse(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_query_path_prints_its_control_characters_escaped() {
        assert_eq!(QueryPath::parse("orders/\u{202e}lqs.sql").unwrap().to_string(), "orders/\\u{202e}lqs.sql");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reading_refuses_a_connections_folder_that_is_a_link() {
        let (data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(outside.path(), "orders/daily.sql");
        fs::create_dir_all(data.path().join("queries")).unwrap();
        std::os::unix::fs::symlink(outside.path(), data.path().join("queries/shop")).unwrap();
        assert!(in_data(data.path()).read(&daily()).await.is_err());
    }

    #[test]
    fn a_queries_folder_another_user_owns_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let owner = rustix::process::getuid().as_raw();
        assert_eq!(root_problem(root.path(), owner), None);
        assert_eq!(root_problem(root.path(), owner + 1), Some(RootProblem::OwnedByAnotherUser));
    }

    #[cfg(unix)]
    #[test]
    fn listing_skips_what_is_not_a_regular_file() {
        let data = tempfile::tempdir().unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(shop_dir(data.path()).join("socket.sql")).unwrap();
        assert_eq!(listed(&in_data(data.path())), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reading_refuses_what_is_not_a_regular_file() {
        let data = tempfile::tempdir().unwrap();
        let root = shop_dir(data.path());
        fs::create_dir_all(root.join("orders")).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(root.join("orders/daily.sql")).unwrap();
        assert!(in_data(data.path()).read(&daily()).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reading_refuses_a_query_replaced_by_a_link() {
        let (data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = shop_dir(data.path());
        write(outside.path(), "secret.sql");
        fs::create_dir_all(root.join("orders")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.sql"), root.join("orders/daily.sql")).unwrap();
        assert!(in_data(data.path()).read(&daily()).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reading_refuses_a_query_under_a_folder_replaced_by_a_link() {
        let (data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(outside.path(), "daily.sql");
        std::os::unix::fs::symlink(outside.path(), shop_dir(data.path()).join("orders")).unwrap();
        assert!(in_data(data.path()).read(&daily()).await.is_err());
    }

    #[tokio::test]
    async fn reading_refuses_a_query_too_big_for_the_editor() {
        let data = tempfile::tempdir().unwrap();
        let root = shop_dir(data.path());
        write(&root, "orders/daily.sql");
        fs::write(root.join("orders/daily.sql"), "-".repeat(MAX_QUERY_BYTES as usize + 1)).unwrap();
        let error = in_data(data.path()).read(&daily()).await.unwrap_err();
        assert!(format!("{error:#}").contains("too big"), "{error:#}");
    }

    #[tokio::test]
    async fn reading_a_query_gives_its_text() {
        let data = tempfile::tempdir().unwrap();
        write(&shop_dir(data.path()), "orders/daily.sql");
        assert_eq!(in_data(data.path()).read(&daily()).await.unwrap(), "SELECT 1");
    }
}
