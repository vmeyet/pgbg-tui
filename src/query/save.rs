//! Writing a query back: into a root of the user's own, through a temporary file renamed over the old one.
use super::versions::DIR_MODE;
use super::{Query, QueryPath, Root, Roots, Source};
use crate::connection::ConnectionName;
use anyhow::{Context, Result, bail};
use std::fs::{self, DirBuilder};
use std::io::Write as _;
use std::os::unix::fs::DirBuilderExt;
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
    /// Where a new query at `path` goes: under the project's root when it has one, else under the user's own `folder`;
    /// pgbg never makes a project's folder.
    pub(crate) fn new_query(&self, path: QueryPath, folder: &ConnectionName) -> Query {
        let source = if self.project.is_some() { Source::Project } else { Source::User(folder.clone()) };
        Query { source, path }
    }

    /// Writes `text` as `query`, making its folders and the user's own folder, the text it replaces kept as a version
    /// first; safe to run twice. Blocks on the file system.
    pub(crate) fn write(&self, query: &Query, text: &str, write: Write) -> Result<()> {
        if let Source::User(folder) = &query.source {
            self.make_user_dir(folder).context("making your queries folder")?;
        }
        if write == Write::Replace
            && let Ok(old) = self.read_blocking(query)
        {
            self.keep(query, &old, text);
        }
        let root = self.existing_root(&query.source)?;
        write_inside(&root, query, text, write).with_context(|| format!("saving {}", query.path))
    }

    /// Makes the user's own `folder` under pgbg's data folder; a link or a file in the way is refused. Safe to run twice.
    fn make_user_dir(&self, folder: &ConnectionName) -> Result<()> {
        let (Some(data_dir), Some(user_dir)) = (&self.data_dir, self.user_dir(folder)) else {
            bail!("there is no data folder: set $PGBG_DATA_DIR");
        };
        DirBuilder::new().recursive(true).mode(DIR_MODE).create(data_dir)?;
        make_folders(DirBuilder::new().mode(DIR_MODE), data_dir, &user_dir)
    }
}

impl Roots {
    /// The file of `query`, for a program of the user's to open: a plain file inside its root. Blocks on the file system.
    pub(crate) fn file(&self, query: &Query) -> Result<PathBuf> {
        let root = self.existing_root(&query.source)?;
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
    make_folders(&folder_builder(&query.source), &root.dir, folder)?;
    if !fs::canonicalize(folder)?.starts_with(&root.real) {
        bail!("it leads out of the queries folder");
    }
    if let Ok(old) = fs::symlink_metadata(&target) {
        check_replaceable(&target, &old, text, write)?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(folder)?;
    temporary.write_all(text.as_bytes())?;
    if let Ok(old) = fs::metadata(&target) {
        fs::set_permissions(temporary.path(), old.permissions())?;
    }
    temporary.persist(&target)?;
    Ok(())
}

/// How pgbg makes a folder under `source`: the user's own are for the user alone, a project's follow its umask.
fn folder_builder(source: &Source) -> DirBuilder {
    let mut builder = DirBuilder::new();
    if let Source::User(_) = source {
        builder.mode(DIR_MODE);
    }
    builder
}

/// Makes each folder from `root` down to `folder` with `builder`; one that is a link or a file is refused, so the write
/// stays in the root.
fn make_folders(builder: &DirBuilder, root: &Path, folder: &Path) -> Result<()> {
    let below = folder.strip_prefix(root)?;
    let mut at = root.to_path_buf();
    for part in below.components() {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => bail!("{} is not a folder", at.display()),
            Err(_) => builder.create(&at)?,
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
    use crate::query::Places;
    use std::os::unix::fs::PermissionsExt;

    fn project(root: &Path) -> Roots {
        Roots { project: Root::checked(root.into(), vec![root.into()]), ..Roots::default() }
    }

    fn found_from(working_dir: &Path, data_dir: Option<&Path>) -> Roots {
        Roots::find(&Places { working_dir, home: None, data_dir })
    }

    fn query(path: &str) -> Query {
        Query { source: Source::Project, path: QueryPath::parse(path).unwrap() }
    }

    fn shop() -> ConnectionName {
        "shop".parse().unwrap()
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn new_daily(roots: &Roots) -> Query {
        roots.new_query(QueryPath::parse("orders/daily.sql").unwrap(), &shop())
    }

    #[test]
    fn saving_makes_the_folders_and_writes_the_text() {
        let root = tempfile::tempdir().unwrap();
        project(root.path()).write(&query("orders/daily/top.sql"), "SELECT 1", Write::Create).unwrap();
        assert_eq!(fs::read_to_string(root.path().join("orders/daily/top.sql")).unwrap(), "SELECT 1");
    }

    #[test]
    fn saving_twice_changes_nothing_and_fails_nothing() {
        let root = tempfile::tempdir().unwrap();
        for write in [Write::Create, Write::Create, Write::Replace] {
            project(root.path()).write(&query("daily.sql"), "SELECT 1", write).unwrap();
        }
        assert_eq!(fs::read_to_string(root.path().join("daily.sql")).unwrap(), "SELECT 1");
    }

    #[test]
    fn a_new_query_never_overwrites_another_one() {
        let root = tempfile::tempdir().unwrap();
        project(root.path()).write(&query("daily.sql"), "SELECT 1", Write::Create).unwrap();
        assert!(project(root.path()).write(&query("daily.sql"), "SELECT 2", Write::Create).is_err());
        project(root.path()).write(&query("daily.sql"), "SELECT 2", Write::Replace).unwrap();
        assert_eq!(fs::read_to_string(root.path().join("daily.sql")).unwrap(), "SELECT 2");
    }

    #[cfg(unix)]
    #[test]
    fn saving_follows_no_link_out_of_the_root() {
        let (root, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::os::unix::fs::symlink(outside.path(), root.path().join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked.sql")).unwrap();
        assert!(project(root.path()).write(&query("elsewhere/x.sql"), "SELECT 1", Write::Replace).is_err());
        assert!(project(root.path()).write(&query("linked.sql"), "SELECT 1", Write::Replace).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_new_query_goes_to_the_project_when_it_has_a_queries_folder() {
        let (root, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = Roots { data_dir: Some(data.path().into()), ..project(root.path()) };
        assert_eq!(new_daily(&roots).source, Source::Project);
    }

    #[test]
    fn a_new_query_lands_in_the_connections_own_folder_when_the_project_has_none() {
        let (project, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = found_from(project.path(), Some(&data.path().join("pgbg")));
        let query = new_daily(&roots);
        for _ in 0..2 {
            roots.write(&query, "SELECT 1", Write::Create).unwrap();
        }
        assert_eq!(fs::read_to_string(data.path().join("pgbg/queries/shop/orders/daily.sql")).unwrap(), "SELECT 1");
        assert_eq!(roots.list(&shop()), [query]);
    }

    #[test]
    fn saving_never_makes_a_project_queries_folder() {
        let (repository, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::create_dir_all(repository.path().join(".git")).unwrap();
        let roots = found_from(repository.path(), Some(data.path()));
        roots.write(&new_daily(&roots), "SELECT 1", Write::Create).unwrap();
        assert!(!repository.path().join(".pgbg").exists());
    }

    #[test]
    fn only_the_user_can_open_the_folders_of_their_own_queries() {
        let (project, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = found_from(project.path(), Some(&data.path().join("pgbg")));
        roots.write(&new_daily(&roots), "SELECT 1", Write::Create).unwrap();
        for folder in ["pgbg", "pgbg/queries", "pgbg/queries/shop", "pgbg/queries/shop/orders"] {
            assert_eq!(mode(&data.path().join(folder)), 0o700, "{folder}");
        }
    }

    #[test]
    fn a_projects_folders_follow_its_umask() {
        let root = tempfile::tempdir().unwrap();
        project(root.path()).write(&query("orders/daily.sql"), "SELECT 1", Write::Create).unwrap();
        fs::create_dir(root.path().join("plain")).unwrap();
        assert_eq!(mode(&root.path().join("orders")), mode(&root.path().join("plain")));
    }

    #[cfg(unix)]
    #[test]
    fn the_connections_own_folder_is_not_made_through_a_link() {
        let (project, data, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::os::unix::fs::symlink(outside.path(), data.path().join("queries")).unwrap();
        let roots = found_from(project.path(), Some(data.path()));
        assert!(roots.write(&new_daily(&roots), "SELECT 1", Write::Create).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_for_the_users_editor_stays_inside_the_root() {
        let (root, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::write(outside.path().join("secret.sql"), "SELECT 1").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.sql"), root.path().join("linked.sql")).unwrap();
        project(root.path()).write(&query("daily.sql"), "SELECT 1", Write::Create).unwrap();
        assert_eq!(project(root.path()).file(&query("daily.sql")).unwrap(), root.path().join("daily.sql"));
        assert!(project(root.path()).file(&query("linked.sql")).is_err());
    }
}
