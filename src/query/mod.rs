//! Saved queries: the `.sql` files under the `.pgbg/queries/` nearest the working directory, found the way git finds
//! `.git`, and under `pgbg/queries/` in the user's config folder.
use crate::render;
use anyhow::{Context, Result, bail};
use std::fmt;
use std::fs::{self, DirEntry, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

const PROJECT_DIR: &str = ".pgbg";
const QUERIES_DIR: &str = "queries";
const USER_DIR: &str = "pgbg/queries";
const REPOSITORY_MARKER: &str = ".git";
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

    fn under(&self, root: &Path) -> PathBuf {
        self.0.split('/').fold(root.to_path_buf(), |path, part| path.join(part))
    }
}

/// Prints the path with its control characters and text-reversing marks escaped, as it is drawn.
impl fmt::Display for QueryPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&render::printable(&self.0))
    }
}

/// Which root a query is under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Source {
    /// The `.pgbg/queries/` nearest the working directory.
    Project,
    /// `pgbg/queries/` in the user's config folder.
    User,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Project => "project",
            Self::User => "user",
        })
    }
}

/// One saved query: its root and its path under it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Query {
    pub(crate) source: Source,
    pub(crate) path: QueryPath,
}

/// Where the roots are looked for: the walk up from `working_dir` stops at `home` or at the first repository root.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Places<'a> {
    pub(crate) working_dir: &'a Path,
    pub(crate) home: Option<&'a Path>,
    pub(crate) config_dir: Option<&'a Path>,
}

/// Why a folder is not taken as a root of queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootProblem {
    Unreadable,
    Link,
    NotAFolder,
    OwnedByAnotherUser,
}

impl fmt::Display for RootProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unreadable => "it cannot be read",
            Self::Link => "it is a link",
            Self::NotAFolder => "it is not a folder",
            Self::OwnedByAnotherUser => "another user owns it",
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

/// `root` when each of `dirs` is a folder of the user's own and none is a link, else none and a warning.
fn checked_root(root: PathBuf, dirs: &[&Path]) -> Option<PathBuf> {
    let owner = rustix::process::getuid().as_raw();
    let Some((dir, problem)) = dirs.iter().find_map(|dir| Some((dir, root_problem(dir, owner)?))) else { return Some(root) };
    tracing::warn!("the queries folder {} is skipped: {problem}", dir.display());
    None
}

/// The folders from `working_dir` up to `home` or the first one holding `.git`, both included.
fn walked<'a>(places: &Places<'a>) -> Vec<&'a Path> {
    let ancestors: Vec<&Path> = places.working_dir.ancestors().collect();
    let is_last = |dir: &&Path| places.home == Some(*dir) || dir.join(REPOSITORY_MARKER).exists();
    let kept = ancestors.iter().position(is_last).map_or(ancestors.len(), |last| last + 1);
    ancestors[..kept].to_vec()
}

fn project_root(places: &Places<'_>) -> Option<PathBuf> {
    let project = walked(places).into_iter().map(|dir| dir.join(PROJECT_DIR)).find(|dir| exists(&dir.join(QUERIES_DIR)))?;
    let root = project.join(QUERIES_DIR);
    checked_root(root.clone(), &[&project, &root])
}

fn user_root(config_dir: &Path) -> Option<PathBuf> {
    let root = config_dir.join(USER_DIR);
    if !exists(&root) {
        return None;
    }
    checked_root(root.clone(), &[&root])
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The folders queries live in, each when it exists.
#[derive(Clone, Debug, Default)]
pub(crate) struct Roots {
    project: Option<PathBuf>,
    user: Option<PathBuf>,
}

impl Roots {
    /// The nearest `.pgbg/queries/` from the working directory up, and `pgbg/queries/` under the config folder; a root
    /// that is a link, or that another user owns, is skipped. Blocks on the file system.
    pub(crate) fn find(places: &Places<'_>) -> Self {
        Self { project: project_root(places), user: places.config_dir.and_then(user_root) }
    }

    fn dir(&self, source: Source) -> Option<&Path> {
        match source {
            Source::Project => self.project.as_deref(),
            Source::User => self.user.as_deref(),
        }
    }

    /// Every query under both roots, sorted; a folder that cannot be read is skipped with a warning. Blocks on the file system.
    pub(crate) fn list(&self) -> Vec<Query> {
        let found = [Source::Project, Source::User].into_iter().filter_map(|source| Some((source, self.dir(source)?)));
        let mut queries: Vec<Query> =
            found.flat_map(|(source, dir)| files_under(dir, "").into_iter().map(move |path| Query { source, path })).collect();
        queries.sort();
        queries
    }

    /// The text of `query`, read without blocking: a plain file inside its root, `MAX_QUERY_BYTES` at most.
    pub(crate) async fn read(&self, query: &Query) -> Result<String> {
        let root = self.dir(query.source).with_context(|| format!("no {} queries folder", query.source))?;
        read_inside(root, &query.path.under(root)).await.with_context(|| format!("reading {}", query.path))
    }
}

async fn read_inside(root: &Path, path: &Path) -> Result<String> {
    check_inside(root, path).await?;
    check_readable(&tokio::fs::symlink_metadata(path).await?)?;
    let mut text = String::new();
    tokio::fs::File::open(path).await?.take(MAX_QUERY_BYTES + 1).read_to_string(&mut text).await?;
    check_size(text.len() as u64)?;
    Ok(text)
}

/// Refuses a path that a link, swapped in after the listing, takes out of `root`.
async fn check_inside(root: &Path, path: &Path) -> Result<()> {
    let (root, path) = (tokio::fs::canonicalize(root).await?, tokio::fs::canonicalize(path).await?);
    if !path.starts_with(&root) {
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

    fn project(root: &Path) -> Roots {
        Roots { project: Some(root.into()), user: None }
    }

    fn listed(roots: &Roots) -> Vec<String> {
        roots.list().iter().map(|query| format!("{} {}", query.source, query.path)).collect()
    }

    #[test]
    fn a_query_path_names_a_sql_file_inside_its_root() {
        assert_eq!(QueryPath::parse("orders/daily.sql").unwrap().under(Path::new("/q")), Path::new("/q/orders/daily.sql"));
        for refused in ["/etc/passwd.sql", "../shop.sql", "orders/../../x.sql", "orders//x.sql", "./x.sql", "a\\..\\x.sql", "x.txt"] {
            assert!(QueryPath::parse(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn the_root_is_the_nearest_one_up_from_the_working_directory() {
        let home = tempfile::tempdir().unwrap();
        write(home.path(), ".pgbg/queries/top.sql");
        write(home.path(), "shop/.pgbg/queries/near.sql");
        fs::create_dir_all(home.path().join("shop/src")).unwrap();
        assert_eq!(find_from(&home.path().join("shop/src"), None), Some(home.path().join("shop/.pgbg/queries")));
    }

    #[test]
    fn the_user_root_is_under_the_config_folder_when_it_exists() {
        let config = tempfile::tempdir().unwrap();
        let places = Places { working_dir: config.path(), home: None, config_dir: Some(config.path()) };
        assert_eq!(Roots::find(&places).user, None);
        write(config.path(), "pgbg/queries/mine.sql");
        assert_eq!(Roots::find(&places).user, Some(config.path().join("pgbg/queries")));
    }

    #[test]
    fn listing_finds_every_sql_file_of_both_roots_sorted() {
        let (root, user) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for file in ["orders/daily.sql", "customers.sql", "notes.txt", "orders/weekly/top.sql"] {
            write(root.path(), file);
        }
        write(user.path(), "mine.sql");
        let roots = Roots { user: Some(user.path().into()), ..project(root.path()) };
        assert_eq!(listed(&roots), ["project customers.sql", "project orders/daily.sql", "project orders/weekly/top.sql", "user mine.sql"]);
    }

    #[cfg(unix)]
    #[test]
    fn listing_follows_no_link_out_of_the_root() {
        let (root, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(outside.path(), "secret.sql");
        std::os::unix::fs::symlink(outside.path(), root.path().join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.sql"), root.path().join("secret.sql")).unwrap();
        assert_eq!(listed(&project(root.path())), Vec::<String>::new());
    }

    fn find_from(working_dir: &Path, home: Option<&Path>) -> Option<PathBuf> {
        Roots::find(&Places { working_dir, home, config_dir: None }).project
    }

    fn daily() -> Query {
        Query { source: Source::Project, path: QueryPath::parse("orders/daily.sql").unwrap() }
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

    #[test]
    fn the_walk_stops_at_the_repository_root() {
        let home = tempfile::tempdir().unwrap();
        write(home.path(), ".pgbg/queries/top.sql");
        fs::create_dir_all(home.path().join("shop/.git")).unwrap();
        fs::create_dir_all(home.path().join("shop/src")).unwrap();
        assert_eq!(find_from(&home.path().join("shop/src"), None), None);
    }

    #[test]
    fn the_walk_stops_at_the_home_folder() {
        let machine = tempfile::tempdir().unwrap();
        write(machine.path(), ".pgbg/queries/shared.sql");
        let home = machine.path().join("nina");
        fs::create_dir_all(home.join("shop")).unwrap();
        assert_eq!(find_from(&home.join("shop"), Some(&home)), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_queries_folder_that_is_a_link_is_refused() {
        let (project, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::create_dir_all(project.path().join(".pgbg")).unwrap();
        std::os::unix::fs::symlink(outside.path(), project.path().join(".pgbg/queries")).unwrap();
        assert_eq!(find_from(project.path(), None), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_pgbg_folder_that_is_a_link_is_refused() {
        let (project, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(outside.path(), "queries/secret.sql");
        std::os::unix::fs::symlink(outside.path(), project.path().join(".pgbg")).unwrap();
        assert_eq!(find_from(project.path(), None), None);
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
        let root = tempfile::tempdir().unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(root.path().join("socket.sql")).unwrap();
        assert_eq!(listed(&project(root.path())), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reading_refuses_what_is_not_a_regular_file() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("orders")).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(root.path().join("orders/daily.sql")).unwrap();
        assert!(project(root.path()).read(&daily()).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reading_refuses_a_query_replaced_by_a_link() {
        let (root, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(outside.path(), "secret.sql");
        fs::create_dir_all(root.path().join("orders")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.sql"), root.path().join("orders/daily.sql")).unwrap();
        assert!(project(root.path()).read(&daily()).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reading_refuses_a_query_under_a_folder_replaced_by_a_link() {
        let (root, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(outside.path(), "daily.sql");
        std::os::unix::fs::symlink(outside.path(), root.path().join("orders")).unwrap();
        assert!(project(root.path()).read(&daily()).await.is_err());
    }

    #[tokio::test]
    async fn reading_refuses_a_query_too_big_for_the_editor() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "orders/daily.sql");
        fs::write(root.path().join("orders/daily.sql"), "-".repeat(MAX_QUERY_BYTES as usize + 1)).unwrap();
        let error = project(root.path()).read(&daily()).await.unwrap_err();
        assert!(format!("{error:#}").contains("too big"), "{error:#}");
    }

    #[tokio::test]
    async fn reading_a_query_gives_its_text() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "orders/daily.sql");
        let query = Query { source: Source::Project, path: QueryPath::parse("orders/daily.sql").unwrap() };
        let text = project(root.path()).read(&query).await.unwrap();
        assert_eq!(text, "SELECT 1");
    }
}
