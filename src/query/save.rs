//! Writing a query back: into a root of the user's own, through a temporary file renamed over the old one.
use super::{Project, Query, QueryPath, Root, Roots, Source, queries_dir};
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write as _;
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
    /// Where a new query at `path` goes: under the user's root when only that one exists, else under the project's,
    /// which the write makes when missing. Blocks on the file system.
    pub(crate) fn new_query(&self, path: QueryPath) -> Query {
        let only_user = self.root(Source::Project).is_none() && self.user.is_some();
        Query { source: if only_user { Source::User } else { Source::Project }, path }
    }

    /// Writes `text` as `query`, making its folders and the project's `.pgbg/queries/`, the text it replaces kept as a
    /// version first; safe to run twice. Blocks on the file system.
    pub(crate) fn write(&self, query: &Query, text: &str, write: Write) -> Result<()> {
        if query.source == Source::Project {
            self.project.make().context("making the project's queries folder")?;
        }
        if write == Write::Replace
            && let Ok(old) = self.read_blocking(query)
        {
            self.keep(query, &old, text);
        }
        let root = self.existing_root(query.source)?;
        write_inside(&root, query, text, write).with_context(|| format!("saving {}", query.path))
    }
}

impl Project {
    /// Makes `.pgbg/queries/` when it was missing as pgbg started; a link or a file in the way is refused. Safe to run
    /// twice.
    fn make(&self) -> Result<()> {
        if self.root.is_some() {
            return Ok(());
        }
        make_folders(&self.dir, &queries_dir(&self.dir))
    }
}

impl Roots {
    /// The file of `query`, for a program of the user's to open: a plain file inside its root. Blocks on the file system.
    pub(crate) fn file(&self, query: &Query) -> Result<PathBuf> {
        let root = self.existing_root(query.source)?;
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
    make_folders(&root.dir, folder)?;
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

/// Makes each folder from `root` down to `folder`; one that is a link or a file is refused, so the write stays in the root.
fn make_folders(root: &Path, folder: &Path) -> Result<()> {
    let below = folder.strip_prefix(root)?;
    let mut at = root.to_path_buf();
    for part in below.components() {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => bail!("{} is not a folder", at.display()),
            Err(_) => fs::create_dir(&at)?,
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

    fn project(root: &Path) -> Roots {
        Roots { project: Project { dir: root.into(), root: Root::checked(root.into(), vec![root.into()]) }, ..Roots::default() }
    }

    fn found_from(working_dir: &Path, config_dir: Option<&Path>) -> Roots {
        Roots::find(&Places { working_dir, home: None, config_dir })
    }

    fn query(path: &str) -> Query {
        Query { source: Source::Project, path: QueryPath::parse(path).unwrap() }
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
    fn a_new_query_goes_to_the_project_before_the_user_folder() {
        let (root, user) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let both = Roots { user: Root::checked(user.path().into(), vec![user.path().into()]), ..project(root.path()) };
        assert_eq!(both.new_query(QueryPath::parse("x.sql").unwrap()).source, Source::Project);
    }

    #[test]
    fn a_new_query_goes_to_the_user_folder_when_only_that_one_exists() {
        let (project, user) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::create_dir_all(user.path().join("queries")).unwrap();
        let roots = found_from(project.path(), Some(user.path()));
        assert_eq!(roots.new_query(QueryPath::parse("x.sql").unwrap()).source, Source::User);
    }

    #[test]
    fn saving_into_a_project_without_a_queries_folder_makes_it() {
        let project = tempfile::tempdir().unwrap();
        let roots = found_from(project.path(), None);
        let query = roots.new_query(QueryPath::parse("orders/daily.sql").unwrap());
        for _ in 0..2 {
            roots.write(&query, "SELECT 1", Write::Create).unwrap();
        }
        assert_eq!(fs::read_to_string(project.path().join(".pgbg/queries/orders/daily.sql")).unwrap(), "SELECT 1");
        assert_eq!(roots.list(), [query]);
    }

    #[test]
    fn a_missing_queries_folder_is_made_at_the_repository_root() {
        let repository = tempfile::tempdir().unwrap();
        fs::create_dir_all(repository.path().join(".git")).unwrap();
        fs::create_dir_all(repository.path().join("src/orders")).unwrap();
        let roots = found_from(&repository.path().join("src/orders"), None);
        roots.write(&roots.new_query(QueryPath::parse("daily.sql").unwrap()), "SELECT 1", Write::Create).unwrap();
        assert!(repository.path().join(".pgbg/queries/daily.sql").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_queries_folder_is_not_made_through_a_link() {
        let (project, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::os::unix::fs::symlink(outside.path(), project.path().join(".pgbg")).unwrap();
        let roots = found_from(project.path(), None);
        assert!(roots.write(&roots.new_query(QueryPath::parse("daily.sql").unwrap()), "SELECT 1", Write::Create).is_err());
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
