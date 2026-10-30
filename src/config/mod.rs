//! `config.toml`: the connections, how results show, the command `ctrl-g` asks, and whether usage is counted.
//! It is read once, with a warning for each key pgbg ignores.
use crate::connection::{Connection, Entry};
use crate::format::Mode;
pub(crate) use ai::AiSettings;
use ai::AiTable;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod ai;
mod ignored;
mod keychain;
mod left_behind;
mod write;

pub(crate) use keychain::{keychain_item_to_delete, replaced_keychain_item};
pub(crate) use left_behind::{Places, left_behind};
pub(crate) use write::{ConnectionChange, add_connection, edit_connection, read_kept, remove_connection, set_password};

const PATH_VARIABLE: &str = "PGBG_CONFIG";
const DATA_DIR_VARIABLE: &str = "PGBG_DATA_DIR";

#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    connections: BTreeMap<String, Entry>,
    #[serde(default)]
    display: Display,
    #[serde(default)]
    ai: AiTable,
    #[serde(default)]
    usage: UsageTable,
}

/// The `[usage]` table.
#[derive(Default, Deserialize)]
struct UsageTable {
    #[serde(default)]
    enabled: bool,
}

/// The `[display]` table.
#[derive(Default, Deserialize)]
struct Display {
    #[serde(default)]
    format: Mode,
}

/// The config, checked.
#[derive(Debug, Default)]
pub(crate) struct Config {
    /// Sorted by name.
    pub(crate) connections: Vec<Connection>,
    /// How the TUI's results open: raw, or each column readable.
    pub(crate) format: Mode,
    /// How `ctrl-g` asks for SQL; none when `[ai]` names no command.
    pub(crate) ai: Option<AiSettings>,
    /// `[usage] enabled`: the TUI counts the acts used and the time on each pane, for `pgbg usage`.
    pub(crate) counts_usage: bool,
}

/// A config as read from its file.
#[derive(Debug, Default)]
pub(crate) struct Loaded {
    pub(crate) config: Config,
    /// One line for each key the file holds that this pgbg ignores.
    pub(crate) warnings: Vec<String>,
}

/// The file `$PGBG_CONFIG` names, else `config.toml` in pgbg's config folder.
pub(crate) fn path() -> PathBuf {
    path_from(std::env::var_os(PATH_VARIABLE), &dir())
}

/// `$XDG_CONFIG_HOME/pgbg`, else `~/.config/pgbg`: where terminal tools keep their config on every system, macOS
/// included, so it sits next to gh, helix and a dotfiles repo.
pub(crate) fn dir() -> PathBuf {
    dir_from(std::env::var_os("XDG_CONFIG_HOME"), dirs::home_dir())
}

/// The path an environment variable holds; an empty one holds none, as `PGBG_CONFIG=` unsets it in a shell.
pub(crate) fn named_path(variable: Option<OsString>) -> Option<PathBuf> {
    variable.filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn path_from(variable: Option<OsString>, dir: &Path) -> PathBuf {
    named_path(variable).unwrap_or_else(|| dir.join("config.toml"))
}

fn dir_from(xdg: Option<OsString>, home: Option<PathBuf>) -> PathBuf {
    xdg_folder(xdg, home, ".config").unwrap_or_else(|| PathBuf::from(".")).join("pgbg")
}

/// `$PGBG_DATA_DIR`, else `$XDG_DATA_HOME/pgbg`, else `~/.local/share/pgbg`, on every system, macOS included: where
/// pgbg keeps what it writes for itself, the history, the versions of saved queries and the usage counts.
pub(crate) fn data_dir() -> Option<PathBuf> {
    data_dir_from(std::env::var_os(DATA_DIR_VARIABLE), std::env::var_os("XDG_DATA_HOME"), dirs::home_dir())
}

fn data_dir_from(variable: Option<OsString>, xdg: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    named_path(variable).or_else(|| xdg_folder(xdg, home, ".local/share").map(|dir| dir.join("pgbg")))
}

/// `$XDG_CACHE_HOME/pgbg`, else `~/.cache/pgbg`, on every system, macOS included: where pgbg keeps its log.
pub(crate) fn cache_dir() -> Option<PathBuf> {
    cache_dir_from(std::env::var_os("XDG_CACHE_HOME"), dirs::home_dir())
}

fn cache_dir_from(xdg: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg_folder(xdg, home, ".cache").map(|dir| dir.join("pgbg"))
}

/// The folder an `$XDG_*_HOME` variable holds when it is absolute, as the spec asks, else `under_home` in `home`.
fn xdg_folder(xdg: Option<OsString>, home: Option<PathBuf>, under_home: &str) -> Option<PathBuf> {
    xdg.map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home.map(|h| h.join(under_home)))
}

impl Config {
    /// The config at `path`, empty when there is no file.
    pub(crate) fn read(path: &Path) -> Result<Loaded> {
        if !path.exists() {
            return Ok(Loaded::default());
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        parse(&text).with_context(|| format!("in {}", path.display()))
    }
}

fn parse(text: &str) -> Result<Loaded> {
    let ignored::Parsed { value: file, warnings } = ignored::parse::<ConfigFile>(text)?;
    let connections: Vec<Connection> = file
        .connections
        .into_iter()
        .map(|(name, entry)| Connection::new(&name, entry).with_context(|| format!("connection `{name}`")))
        .collect::<Result<_>>()?;
    check_queries_named(&connections)?;
    let ai = ai::read(file.ai, &connections)?;
    let warnings = warnings.into_iter().chain(ai.warnings).collect();
    let config = Config { connections, format: file.display.format, ai: ai.settings, counts_usage: file.usage.enabled };
    Ok(Loaded { config, warnings })
}

/// Refuses a `queries` key that names no connection of the config.
fn check_queries_named(connections: &[Connection]) -> Result<()> {
    let is_known = |name| connections.iter().any(|other| other.name() == name);
    if let Some(connection) = connections.iter().find(|connection| !is_known(connection.queries())) {
        bail!("connection `{}`: `queries = \"{}\"` names no connection in the config", connection.name(), connection.queries());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = r#"
[connections.staging]
tier = "staging"
url = "postgres://nina@db.example.com/shop?sslmode=require"

[connections.local]
tier = "local"
url = "postgres://nina@localhost:5432/shop"
"#;

    fn names(loaded: &Loaded) -> Vec<String> {
        loaded.config.connections.iter().map(|c| c.name().to_string()).collect()
    }

    #[test]
    fn connections_load_sorted_by_name() {
        assert_eq!(names(&parse(SHOP).unwrap()), ["local", "staging"]);
    }

    #[test]
    fn results_open_raw_unless_the_display_table_says_readable() {
        assert_eq!(parse(SHOP).unwrap().config.format, Mode::Raw);
        assert_eq!(parse(&format!("[display]\nformat = \"readable\"\n{SHOP}")).unwrap().config.format, Mode::Readable);
    }

    #[test]
    fn usage_is_counted_only_when_the_usage_table_says_so() {
        assert!(!parse(SHOP).unwrap().config.counts_usage);
        assert!(parse(&format!("[usage]\nenabled = true\n{SHOP}")).unwrap().config.counts_usage);
    }

    #[test]
    fn the_ai_table_is_read_beside_the_connections() {
        let loaded = parse(&format!("[ai]\ncommand = [\"llm\"]\n{SHOP}")).unwrap();
        assert_eq!(loaded.config.ai.map(|ai| ai.command), Some(vec!["llm".to_owned()]));
    }

    #[test]
    fn a_refused_ai_table_fails_the_config() {
        assert!(format!("{:#}", parse("[ai]\ncommand = []\n").unwrap_err()).contains("[ai] command"));
    }

    #[test]
    fn an_empty_config_has_no_connection() {
        assert_eq!(parse("").unwrap().config.connections, Vec::<Connection>::new());
    }

    #[test]
    fn an_unknown_key_in_a_connection_warns_and_the_rest_loads() {
        let loaded = parse(&format!("{SHOP}colour = \"green\"\n")).unwrap();
        assert_eq!(names(&loaded), ["local", "staging"]);
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].starts_with("`connections.local.colour`"), "{:?}", loaded.warnings);
    }

    #[test]
    fn a_refused_connection_is_named_in_the_error() {
        let err = format!("{:#}", parse(&format!("{SHOP}password = \"hunter2\"\n")).unwrap_err());
        assert!(err.contains("connection `local`") && !err.contains("hunter2"), "{err}");
    }

    #[test]
    fn a_plain_password_of_any_type_is_refused_without_repeating_it() {
        for password in ["84217733", "842.17733", "true", "[\"hunter2\"]"] {
            let err = format!("{:#}", parse(&format!("{SHOP}password = {password}\n")).unwrap_err());
            assert!(err.contains("plain password") && !err.contains(password), "{err}");
        }
    }

    #[test]
    fn a_syntax_error_names_its_line_without_quoting_it() {
        let err = format!("{:#}", parse(&format!("{SHOP}password = \"hunter2\n")).unwrap_err());
        assert!(err.contains("line 9") && !err.contains("hunter2"), "{err}");
    }

    #[test]
    fn a_connection_may_use_the_queries_of_another_one() {
        let loaded = parse(&format!("{SHOP}queries = \"staging\"\n")).unwrap();
        assert_eq!(loaded.config.connections[0].queries().to_string(), "staging");
    }

    #[test]
    fn a_queries_key_naming_no_connection_is_refused() {
        let err = format!("{:#}", parse(&format!("{SHOP}queries = \"reports\"\n")).unwrap_err());
        assert!(err.contains("`queries = \"reports\"` names no connection"), "{err}");
    }

    #[test]
    fn a_missing_file_is_an_empty_config() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::read(&dir.path().join("config.toml")).unwrap().config.connections, Vec::<Connection>::new());
    }

    #[test]
    fn a_file_that_fails_names_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[connections.local]\ntier = \"qa\"\n").unwrap();
        assert!(format!("{:#}", Config::read(&path).unwrap_err()).contains("config.toml"));
    }

    #[test]
    fn the_path_comes_from_pgbg_config_else_the_config_folder() {
        let folder = Path::new("/Users/nina/.config/pgbg");
        assert_eq!(path_from(Some("/tmp/shop.toml".into()), folder), PathBuf::from("/tmp/shop.toml"));
        assert_eq!(path_from(None, folder), PathBuf::from("/Users/nina/.config/pgbg/config.toml"));
        assert_eq!(path_from(Some("".into()), folder), PathBuf::from("/Users/nina/.config/pgbg/config.toml"));
    }

    #[test]
    fn the_config_folder_is_xdg_config_home_else_dot_config_under_home() {
        let home = Some(PathBuf::from("/Users/nina"));
        assert_eq!(dir_from(None, home.clone()), PathBuf::from("/Users/nina/.config/pgbg"));
        assert_eq!(dir_from(Some("/tmp/xdg".into()), home.clone()), PathBuf::from("/tmp/xdg/pgbg"));
        assert_eq!(
            dir_from(Some("relative".into()), home.clone()),
            PathBuf::from("/Users/nina/.config/pgbg"),
            "a relative XDG path is ignored"
        );
        assert_eq!(dir_from(Some("".into()), home), PathBuf::from("/Users/nina/.config/pgbg"));
    }

    #[test]
    fn the_data_folder_is_the_variable_else_pgbg_in_xdg_data_home_else_under_home() {
        let home = || Some(PathBuf::from("/Users/nina"));
        assert_eq!(data_dir_from(Some("/tmp/pgbg-data".into()), Some("/data".into()), home()), Some(PathBuf::from("/tmp/pgbg-data")));
        assert_eq!(data_dir_from(Some("".into()), Some("/data".into()), home()), Some(PathBuf::from("/data/pgbg")));
        assert_eq!(data_dir_from(None, None, home()), Some(PathBuf::from("/Users/nina/.local/share/pgbg")));
        assert_eq!(data_dir_from(None, Some("relative".into()), home()), Some(PathBuf::from("/Users/nina/.local/share/pgbg")));
        assert_eq!(data_dir_from(None, None, None), None);
    }

    #[test]
    fn the_cache_folder_is_pgbg_in_xdg_cache_home_else_dot_cache_under_home() {
        let home = || Some(PathBuf::from("/Users/nina"));
        assert_eq!(cache_dir_from(Some("/cache".into()), home()), Some(PathBuf::from("/cache/pgbg")));
        assert_eq!(cache_dir_from(None, home()), Some(PathBuf::from("/Users/nina/.cache/pgbg")));
        assert_eq!(cache_dir_from(Some("relative".into()), home()), Some(PathBuf::from("/Users/nina/.cache/pgbg")));
        assert_eq!(cache_dir_from(None, None), None);
    }
}
