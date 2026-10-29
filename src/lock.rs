//! A file pgbg's writers take turns on: an exclusive `flock`, so two pgbg never lose each other's writes.
use rustix::fs::{FlockOperation, flock};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const FILE_MODE: u32 = 0o600;
/// How many times a writer opens the file again when another one replaced it while it waited for the lock.
/// Without a cap, a file system whose files never stay the same (some FUSE and SMB mounts) would loop forever.
const ATTEMPTS: usize = 8;
const KEPT_CHANGING: &str = "the file kept changing while pgbg waited for its lock";

/// The file at `path`, made `0600` when missing, opened to append and locked for this process alone until it is
/// dropped; opened again, a few times at most, when another writer moved or replaced it while this one waited.
pub(crate) fn open(path: &Path) -> io::Result<File> {
    first_some(ATTEMPTS, || {
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

/// `file` once locked, while it is still the one at `path`; `None` when it was moved or replaced before the lock was taken.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_file_that_keeps_changing_under_the_lock_is_an_error_not_a_loop() {
        let err = first_some(3, || Ok(None::<File>)).unwrap_err();
        assert!(err.to_string().contains("kept"), "{err}");
    }

    #[test]
    fn a_file_moved_away_by_another_writer_is_not_locked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shop.jsonl");
        let opened_before = OpenOptions::new().create(true).append(true).open(&path).unwrap();
        std::fs::rename(&path, dir.path().join("shop.jsonl.1")).unwrap();
        assert!(locked_if_current(opened_before, &path).unwrap().is_none());
    }

    #[test]
    fn a_file_replaced_by_another_writer_is_not_locked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        let opened_before = OpenOptions::new().create(true).append(true).open(&path).unwrap();
        std::fs::write(dir.path().join("new"), "").unwrap();
        std::fs::rename(dir.path().join("new"), &path).unwrap();
        assert!(locked_if_current(opened_before, &path).unwrap().is_none());
    }

    #[test]
    fn only_the_user_can_read_a_file_it_makes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shop.jsonl");
        open(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, FILE_MODE);
    }
}
