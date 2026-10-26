//! Saved queries: the `.sql` files under the `.pgbg/queries/` nearest the working directory, found the way git finds `.git`.
use anyhow::{Context, Result, bail};
use std::fmt;
use std::path::{Path, PathBuf};

const ROOT_DIR: &str = ".pgbg/queries";
const EXTENSION: &str = ".sql";

/// Where a query sits under its root, as `orders/daily.sql`: relative and without `..`, so it never leaves the root.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct QueryPath(String);

impl QueryPath {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let stays_inside = text.split('/').all(|part| !matches!(part, "" | "." | "..") && !part.contains(['\\', '\0']));
        if !stays_inside || !text.ends_with(EXTENSION) {
            bail!("`{text}` is not a query path: name a {EXTENSION} file under the queries folder, as orders/daily.sql");
        }
        Ok(Self(text.to_owned()))
    }

    fn under(&self, root: &Path) -> PathBuf {
        self.0.split('/').fold(root.to_path_buf(), |path, part| path.join(part))
    }
}

impl fmt::Display for QueryPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The folder saved queries live in.
#[derive(Clone, Debug)]
pub(crate) struct Root(PathBuf);

impl Root {
    /// The nearest `.pgbg/queries/` from `working_dir` up, if any; blocks on the file system.
    pub(crate) fn find(working_dir: &Path) -> Option<Self> {
        working_dir.ancestors().map(|dir| dir.join(ROOT_DIR)).find(|dir| dir.is_dir()).map(Self)
    }

    /// Every query under the root, sorted; a folder that cannot be read is skipped with a warning. Blocks on the file system.
    pub(crate) fn list(&self) -> Vec<QueryPath> {
        let mut paths = files_under(&self.0, "");
        paths.sort();
        paths
    }

    /// The text of the query at `path`, read without blocking.
    pub(crate) async fn read(&self, path: &QueryPath) -> Result<String> {
        tokio::fs::read_to_string(path.under(&self.0)).await.with_context(|| format!("reading {path}"))
    }
}

/// The `.sql` files under `dir`, as paths from `prefix`; a link is never followed, so nothing outside the root is listed.
fn files_under(dir: &Path, prefix: &str) -> Vec<QueryPath> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("reading the queries folder {}: {err}", dir.display());
            return vec![];
        }
    };
    let mut paths = vec![];
    for entry in entries.flatten() {
        let (Ok(kind), Some(name)) = (entry.file_type(), entry.file_name().to_str().map(|name| format!("{prefix}{name}"))) else {
            continue;
        };
        if kind.is_dir() {
            paths.extend(files_under(&entry.path(), &format!("{name}/")));
        } else if !kind.is_symlink() {
            paths.extend(QueryPath::parse(&name).ok());
        }
    }
    paths
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

    fn listed(root: &Path) -> Vec<String> {
        Root(root.into()).list().iter().map(QueryPath::to_string).collect()
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
        let root = Root::find(&home.path().join("shop/src")).unwrap();
        assert_eq!(root.0, home.path().join("shop/.pgbg/queries"));
    }

    #[test]
    fn listing_finds_every_sql_file_sorted() {
        let root = tempfile::tempdir().unwrap();
        for file in ["orders/daily.sql", "customers.sql", "notes.txt", "orders/weekly/top.sql"] {
            write(root.path(), file);
        }
        assert_eq!(listed(root.path()), ["customers.sql", "orders/daily.sql", "orders/weekly/top.sql"]);
    }

    #[cfg(unix)]
    #[test]
    fn listing_follows_no_link_out_of_the_root() {
        let (root, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(outside.path(), "secret.sql");
        std::os::unix::fs::symlink(outside.path(), root.path().join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.sql"), root.path().join("secret.sql")).unwrap();
        assert_eq!(listed(root.path()), Vec::<String>::new());
    }

    #[tokio::test]
    async fn reading_a_query_gives_its_text() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "orders/daily.sql");
        let text = Root(root.path().into()).read(&QueryPath::parse("orders/daily.sql").unwrap()).await.unwrap();
        assert_eq!(text, "SELECT 1");
    }
}
