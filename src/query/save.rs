//! Writing a query back: into its connection's folder in pgbg's data folder, through a temporary file renamed over the old one.
use super::{Query, Root, Roots};
use crate::connection::ConnectionName;
use crate::files::{self, Readers};
use anyhow::{Context, Result, bail};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Whether a save may replace a file that is there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Write {
    /// The query the editor stands for: its file is replaced.
    Replace,
    /// A new query: a file already there with other text is kept, and the save refused.
    Create,
}

impl Roots {
    /// Writes `text` as `query`, making its folders and its connection's folder, the text it replaces kept as a version
    /// first; safe to run twice. Blocks on the file system.
    pub(crate) fn write(&self, query: &Query, text: &str, write: Write) -> Result<()> {
        self.make_dir(&query.folder).context("making your queries folder")?;
        if write == Write::Replace
            && let Ok(old) = self.read_blocking(query)
        {
            self.keep(query, &old, text);
        }
        let root = self.existing_root(&query.folder)?;
        write_inside(&root, query, text, write).with_context(|| format!("saving {}", query.path))
    }

    /// Makes the folder of `folder` under pgbg's data folder; a link or a file in the way is refused. Safe to run twice.
    fn make_dir(&self, folder: &ConnectionName) -> Result<()> {
        let (Some(data_dir), Some(dir)) = (&self.data_dir, self.dir(folder)) else {
            bail!("there is no data folder: set $PGBG_DATA_DIR");
        };
        files::make_private_dir(data_dir)?;
        make_folders(files::make_private_dir, data_dir, &dir)
    }
}

impl Roots {
    /// The file of `query`, for a program of the user's to open: a plain file inside its root. Blocks on the file system.
    pub(crate) fn file(&self, query: &Query) -> Result<PathBuf> {
        let root = self.existing_root(&query.folder)?;
        if !root.is_still_safe() {
            bail!("its queries folder changed since pgbg started");
        }
        let path = query.path.under(&root.dir);
        if !fs::canonicalize(&path)?.starts_with(&root.real) {
            bail!("{} leads out of the queries folder", query.path);
        }
        if !fs::symlink_metadata(&path)?.is_file() {
            bail!("{} is not a plain file", query.path);
        }
        Ok(path)
    }
}

fn write_inside(root: &Root, query: &Query, text: &str, write: Write) -> Result<()> {
    if !root.is_still_safe() {
        bail!("its queries folder changed since pgbg started");
    }
    let target = query.path.under(&root.dir);
    let folder = target.parent().unwrap_or(&root.dir);
    make_folders(files::make_private_dir, &root.dir, folder)?;
    if !fs::canonicalize(folder)?.starts_with(&root.real) {
        bail!("it leads out of the queries folder");
    }
    if let Ok(old) = fs::symlink_metadata(&target) {
        check_replaceable(&target, &old, text, write)?;
    }
    Ok(files::write_whole(&target, text, Readers::AsReplaced)?)
}

/// Makes each folder from `root` down to `folder` with `make`; one that is a link or a file is refused, so the write
/// stays in the root.
fn make_folders(make: fn(&Path) -> io::Result<()>, root: &Path, folder: &Path) -> Result<()> {
    let below = folder.strip_prefix(root)?;
    let mut at = root.to_path_buf();
    for part in below.components() {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => bail!("{} is not a folder", at.display()),
            Err(_) => make(&at)?,
        }
    }
    Ok(())
}

fn check_replaceable(target: &Path, old: &fs::Metadata, text: &str, write: Write) -> Result<()> {
    if !old.is_file() {
        bail!("{} is not a plain file", target.display());
    }
    let is_same_text = fs::read_to_string(target).is_ok_and(|old_text| old_text == text);
    if write == Write::Create && !is_same_text {
        bail!("a query is already there: open it to change it");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::QueryPath;
    use std::os::unix::fs::PermissionsExt;

    fn in_data(data: &Path) -> Roots {
        Roots { data_dir: Some(data.into()), ..Roots::default() }
    }

    fn query(path: &str) -> Query {
        Query { folder: "shop".parse().unwrap(), path: QueryPath::parse(path).unwrap() }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn saving_makes_the_folders_and_writes_the_text() {
        let data = tempfile::tempdir().unwrap();
        in_data(data.path()).write(&query("orders/daily/top.sql"), "SELECT 1", Write::Create).unwrap();
        assert_eq!(fs::read_to_string(data.path().join("queries/shop/orders/daily/top.sql")).unwrap(), "SELECT 1");
    }

    #[test]
    fn saving_twice_changes_nothing_and_fails_nothing() {
        let data = tempfile::tempdir().unwrap();
        for write in [Write::Create, Write::Create, Write::Replace] {
            in_data(data.path()).write(&query("daily.sql"), "SELECT 1", write).unwrap();
        }
        assert_eq!(fs::read_to_string(data.path().join("queries/shop/daily.sql")).unwrap(), "SELECT 1");
        assert_eq!(in_data(data.path()).list(&"shop".parse().unwrap()), [query("daily.sql")]);
    }

    #[test]
    fn a_new_query_never_overwrites_another_one() {
        let data = tempfile::tempdir().unwrap();
        let roots = in_data(data.path());
        roots.write(&query("daily.sql"), "SELECT 1", Write::Create).unwrap();
        assert!(roots.write(&query("daily.sql"), "SELECT 2", Write::Create).is_err());
        roots.write(&query("daily.sql"), "SELECT 2", Write::Replace).unwrap();
        assert_eq!(fs::read_to_string(data.path().join("queries/shop/daily.sql")).unwrap(), "SELECT 2");
    }

    #[cfg(unix)]
    #[test]
    fn saving_follows_no_link_out_of_the_root() {
        let (data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = data.path().join("queries/shop");
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linked.sql")).unwrap();
        assert!(in_data(data.path()).write(&query("elsewhere/x.sql"), "SELECT 1", Write::Replace).is_err());
        assert!(in_data(data.path()).write(&query("linked.sql"), "SELECT 1", Write::Replace).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_pgbg_queries_folder_in_the_working_directory_is_never_listed_nor_written() {
        let (repository, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::create_dir_all(repository.path().join(".pgbg/queries")).unwrap();
        fs::write(repository.path().join(".pgbg/queries/customers.sql"), "SELECT 1").unwrap();
        let roots = in_data(data.path());
        roots.write(&query("orders/daily.sql"), "SELECT 1", Write::Create).unwrap();
        assert_eq!(roots.list(&"shop".parse().unwrap()), [query("orders/daily.sql")]);
        assert_eq!(fs::read_dir(repository.path().join(".pgbg/queries")).unwrap().count(), 1);
    }

    #[test]
    fn only_the_user_can_open_the_folders_of_their_own_queries() {
        let data = tempfile::tempdir().unwrap();
        in_data(&data.path().join("pgbg")).write(&query("orders/daily.sql"), "SELECT 1", Write::Create).unwrap();
        for folder in ["pgbg", "pgbg/queries", "pgbg/queries/shop", "pgbg/queries/shop/orders"] {
            assert_eq!(mode(&data.path().join(folder)), 0o700, "{folder}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_connections_own_folder_is_not_made_through_a_link() {
        let (data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::os::unix::fs::symlink(outside.path(), data.path().join("queries")).unwrap();
        assert!(in_data(data.path()).write(&query("orders/daily.sql"), "SELECT 1", Write::Create).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_for_the_users_editor_stays_inside_the_root() {
        let (data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = in_data(data.path());
        roots.write(&query("daily.sql"), "SELECT 1", Write::Create).unwrap();
        let root = data.path().join("queries/shop");
        fs::write(outside.path().join("secret.sql"), "SELECT 1").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.sql"), root.join("linked.sql")).unwrap();
        assert_eq!(roots.file(&query("daily.sql")).unwrap(), root.join("daily.sql"));
        assert!(roots.file(&query("linked.sql")).is_err());
    }
}
