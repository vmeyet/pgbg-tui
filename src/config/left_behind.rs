//! The files an earlier pgbg kept where this one no longer reads them: the macOS folders before pgbg followed XDG on
//! every system, the config folder before saved queries moved to the data folder, the cache folder before the usage
//! counts did; pgbg says where each one goes now.
use std::fmt;
use std::path::{Path, PathBuf};

const OLD_SUPPORT_FOLDER: &str = "Library/Application Support/pgbg";
const OLD_CACHE_FOLDER: &str = "Library/Caches/pgbg";
const QUERIES: &str = "queries";
const USAGE_FILE: &str = "usage.jsonl";
const CONNECTION_PLACEHOLDER: &str = "<connection>";

/// A file or folder pgbg no longer reads, and where pgbg reads it now.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LeftBehind {
    old: PathBuf,
    new: PathBuf,
}

/// Where pgbg reads now what an earlier pgbg kept elsewhere, and the folders it kept them in that still exist.
pub(crate) struct Places<'a> {
    pub(crate) config: &'a Path,
    pub(crate) config_dir: &'a Path,
    pub(crate) cache_dir: &'a Path,
    pub(crate) history: &'a Path,
    /// The folder holding the user's own queries, one folder per connection.
    pub(crate) queries: &'a Path,
    pub(crate) usage: &'a Path,
}

impl fmt::Display for LeftBehind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} sits in a folder pgbg no longer reads: move it to {}", self.old.display(), self.new.display())
    }
}

/// What still sits where an earlier pgbg kept it, under `home` or in the folders `places` names: the config, the
/// history, the saved queries and the usage counts, each with the place it goes; one already there is left out.
pub(crate) fn left_behind(home: &Path, places: &Places<'_>) -> Vec<LeftBehind> {
    let (support, cache) = (home.join(OLD_SUPPORT_FOLDER), home.join(OLD_CACHE_FOLDER));
    let queries = places.queries.join(CONNECTION_PLACEHOLDER);
    let moves = [
        (support.join("config.toml"), places.config.to_owned()),
        (support.join("history"), places.history.to_owned()),
        (support.join(QUERIES), queries.clone()),
        (places.config_dir.join(QUERIES), queries),
        (cache.join(USAGE_FILE), places.usage.to_owned()),
        (places.cache_dir.join(USAGE_FILE), places.usage.to_owned()),
    ];
    moves.into_iter().filter(|(old, new)| old != new && old.exists()).map(|(old, new)| LeftBehind { old, new }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Owned {
        config: PathBuf,
        config_dir: PathBuf,
        cache_dir: PathBuf,
        history: PathBuf,
        queries: PathBuf,
        usage: PathBuf,
    }

    fn owned(home: &Path) -> Owned {
        let data = home.join(".local/share/pgbg");
        Owned {
            config: home.join(".config/pgbg/config.toml"),
            config_dir: home.join(".config/pgbg"),
            cache_dir: home.join(".cache/pgbg"),
            history: data.join("history"),
            queries: data.join("queries"),
            usage: data.join("usage.jsonl"),
        }
    }

    fn found(home: &Path) -> Vec<LeftBehind> {
        let o = owned(home);
        let places = Places {
            config: &o.config,
            config_dir: &o.config_dir,
            cache_dir: &o.cache_dir,
            history: &o.history,
            queries: &o.queries,
            usage: &o.usage,
        };
        left_behind(home, &places)
    }

    fn make(home: &Path, file: &str) -> PathBuf {
        let path = home.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        path
    }

    fn news(home: &Path) -> Vec<PathBuf> {
        found(home).into_iter().map(|left| left.new).collect()
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
        let new = owned(home.path()).config;
        assert_eq!(left.to_string(), format!("{} sits in a folder pgbg no longer reads: move it to {}", old.display(), new.display()));
    }

    #[test]
    fn the_history_is_found_in_application_support() {
        let home = tempfile::tempdir().unwrap();
        make(home.path(), "Library/Application Support/pgbg/history/shop.jsonl");
        assert_eq!(news(home.path()), [owned(home.path()).history]);
    }

    #[test]
    fn saved_queries_in_application_support_or_the_config_folder_go_under_a_connection() {
        let home = tempfile::tempdir().unwrap();
        make(home.path(), "Library/Application Support/pgbg/queries/daily.sql");
        make(home.path(), ".config/pgbg/queries/daily.sql");
        let under_a_connection = owned(home.path()).queries.join("<connection>");
        assert_eq!(news(home.path()), [under_a_connection.clone(), under_a_connection]);
    }

    #[test]
    fn usage_counts_in_the_macos_or_the_xdg_cache_folder_go_to_the_data_folder() {
        let home = tempfile::tempdir().unwrap();
        make(home.path(), "Library/Caches/pgbg/usage.jsonl");
        make(home.path(), ".cache/pgbg/usage.jsonl");
        let usage = owned(home.path()).usage;
        assert_eq!(news(home.path()), [usage.clone(), usage]);
    }

    #[test]
    fn an_old_place_pgbg_still_reads_is_not_left_behind() {
        let home = tempfile::tempdir().unwrap();
        let config = make(home.path(), "Library/Application Support/pgbg/config.toml");
        let o = owned(home.path());
        let places = Places {
            config: &config,
            config_dir: &o.config_dir,
            cache_dir: &o.cache_dir,
            history: &o.history,
            queries: &o.queries,
            usage: &o.usage,
        };
        assert_eq!(left_behind(home.path(), &places), []);
    }
}
