//! How pgbg makes its own folders and writes a whole file: the history, the log, the usage counts, the versions and
//! the saved queries all go through here.
use std::fs::{self, DirBuilder, Permissions};
use std::io::{self, Write as _};
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

const PRIVATE_DIR_MODE: u32 = 0o700;
const TEMP_PREFIX: &str = ".tmp-";

/// Who may read a file [`write_whole`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Readers {
    /// Its owner only.
    Owner,
    /// Those who could read the file it replaces; its owner only when there was none.
    AsReplaced,
}

/// Makes `dir` and each missing folder above it, each one for its owner alone (`0700`); safe to run twice.
pub(crate) fn make_private_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(PRIVATE_DIR_MODE).create(dir)
}

/// Writes `text` as the whole file at `path` through a temp file beside it, synced to disk before a rename, so the
/// file is always the old one or the new one; its folder must exist.
pub(crate) fn write_whole(path: &Path, text: &str, readers: Readers) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other(format!("{} has no folder", path.display())))?;
    let mut temp = tempfile::Builder::new().prefix(TEMP_PREFIX).tempfile_in(dir)?;
    temp.write_all(text.as_bytes())?;
    if let Some(permissions) = replaced_permissions(path, readers) {
        fs::set_permissions(temp.path(), permissions)?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

/// The permissions of the file at `path` when `readers` keeps them and there is one.
fn replaced_permissions(path: &Path, readers: Readers) -> Option<Permissions> {
    match readers {
        Readers::Owner => None,
        Readers::AsReplaced => fs::metadata(path).ok().map(|old| old.permissions()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_private_dir_and_each_folder_made_above_it_are_for_their_owner_alone() {
        let dir = tempfile::tempdir().unwrap();
        make_private_dir(&dir.path().join("pgbg/history")).unwrap();
        assert_eq!((mode(&dir.path().join("pgbg")), mode(&dir.path().join("pgbg/history"))), (0o700, 0o700));
    }

    #[test]
    fn making_a_private_dir_twice_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        make_private_dir(&dir.path().join("pgbg")).unwrap();
        make_private_dir(&dir.path().join("pgbg")).unwrap();
        assert_eq!(mode(&dir.path().join("pgbg")), 0o700);
    }

    #[test]
    fn a_whole_write_replaces_the_text_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        write_whole(&path, "old", Readers::Owner).unwrap();
        write_whole(&path, "new", Readers::Owner).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_file_for_its_owner_is_read_by_its_owner_only_whatever_it_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap();
        write_whole(&path, "new", Readers::Owner).unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn a_file_written_as_replaced_keeps_the_readers_of_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daily.sql");
        fs::write(&path, "SELECT 1").unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap();
        write_whole(&path, "SELECT 2", Readers::AsReplaced).unwrap();
        assert_eq!(mode(&path), 0o644);
    }

    #[test]
    fn a_new_file_written_as_replaced_is_for_its_owner_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daily.sql");
        write_whole(&path, "SELECT 1", Readers::AsReplaced).unwrap();
        assert_eq!(mode(&path), 0o600);
    }
}
