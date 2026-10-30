//! The files pgbg kept in the macOS folders before it followed XDG on every system: it no longer reads them, so it says
//! where each one goes now.
use std::fmt;
use std::path::{Path, PathBuf};

const OLD_SUPPORT_FOLDER: &str = "Library/Application Support/pgbg";
const OLD_CACHE_FOLDER: &str = "Library/Caches/pgbg";

/// A file or folder in an old macOS folder, and where pgbg reads it now.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LeftBehind {
    old: PathBuf,
    new: PathBuf,
}

/// Where pgbg reads what it kept in the old macOS folders.
pub(crate) struct Places<'a> {
    pub(crate) config: &'a Path,
    pub(crate) history: &'a Path,
    pub(crate) usage: &'a Path,
}

impl fmt::Display for LeftBehind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} sits in an old macOS folder pgbg no longer reads: move it to {}", self.old.display(), self.new.display())
    }
}

/// What still sits in the old macOS folders under `home`, the config, the history and the usage counts, each with the
/// place in `places` it goes; one that is already that place is left out.
pub(crate) fn left_behind(home: &Path, places: &Places<'_>) -> Vec<LeftBehind> {
    let (support, cache) = (home.join(OLD_SUPPORT_FOLDER), home.join(OLD_CACHE_FOLDER));
    let moves = [
        (support.join("config.toml"), places.config),
        (support.join("history"), places.history),
        (cache.join("usage.jsonl"), places.usage),
    ];
    moves.into_iter().filter(|(old, new)| old != new && old.exists()).map(|(old, new)| LeftBehind { old, new: new.to_owned() }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(home: &Path) -> (PathBuf, PathBuf, PathBuf) {
        (home.join(".config/pgbg/config.toml"), home.join(".local/share/pgbg/history"), home.join(".local/share/pgbg/usage.jsonl"))
    }

    fn found(home: &Path) -> Vec<LeftBehind> {
        let (config, history, usage) = places(home);
        left_behind(home, &Places { config: &config, history: &history, usage: &usage })
    }

    fn make(home: &Path, file: &str) -> PathBuf {
        let path = home.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        path
    }

    #[test]
    fn nothing_in_the_old_folders_finds_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(found(home.path()), []);
    }

    #[test]
    fn a_config_in_application_support_says_where_it_goes() {
        let home = tempfile::tempdir().unwrap();
        let old = make(home.path(), "Library/Application Support/pgbg/config.toml");
        let [left] = &*found(home.path()) else { panic!("{:?}", found(home.path())) };
        let expected =
            format!("{} sits in an old macOS folder pgbg no longer reads: move it to {}", old.display(), places(home.path()).0.display());
        assert_eq!(left.to_string(), expected);
    }

    #[test]
    fn the_history_and_the_usage_counts_are_found_too() {
        let home = tempfile::tempdir().unwrap();
        make(home.path(), "Library/Application Support/pgbg/history/shop.jsonl");
        make(home.path(), "Library/Caches/pgbg/usage.jsonl");
        let (_, history, usage) = places(home.path());
        let news: Vec<PathBuf> = found(home.path()).into_iter().map(|left| left.new).collect();
        assert_eq!(news, [history, usage]);
    }

    #[test]
    fn an_old_folder_pgbg_still_reads_is_not_left_behind() {
        let home = tempfile::tempdir().unwrap();
        let config = make(home.path(), "Library/Application Support/pgbg/config.toml");
        let (_, history, usage) = places(home.path());
        assert_eq!(left_behind(home.path(), &Places { config: &config, history: &history, usage: &usage }), []);
    }
}
