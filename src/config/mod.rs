//! `config.toml`: the connections, how results show, and the command `ctrl-g` asks.
//! It is read once, with a warning for each key pgbg ignores.
use crate::connection::{Connection, Entry};
use crate::format::Mode;
pub(crate) use ai::AiSettings;
use ai::AiTable;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod ai;
mod ignored;
mod keychain;
mod write;

pub(crate) use keychain::{keychain_item_to_delete, replaced_keychain_item};
pub(crate) use write::{ConnectionChange, add_connection, edit_connection, remove_connection, set_password};

const PATH_VARIABLE: &str = "PGBG_CONFIG";

#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    connections: BTreeMap<String, Entry>,
    #[serde(default)]
    display: Display,
    #[serde(default)]
    ai: AiTable,
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
    #[cfg_attr(not(test), expect(dead_code, reason = "`ctrl-g` reads it, the next step"))]
    pub(crate) ai: Option<AiSettings>,
}

/// A config as read from its file.
#[derive(Debug, Default)]
pub(crate) struct Loaded {
    pub(crate) config: Config,
    /// One line for each key the file holds that this pgbg ignores.
    pub(crate) warnings: Vec<String>,
}

/// The file `$PGBG_CONFIG` names, else `pgbg/config.toml` in the user's config folder.
pub(crate) fn path() -> PathBuf {
    path_from(std::env::var_os(PATH_VARIABLE), dirs::config_dir())
}

/// The path an environment variable holds; an empty one holds none, as `PGBG_CONFIG=` unsets it in a shell.
pub(crate) fn named_path(variable: Option<OsString>) -> Option<PathBuf> {
    variable.filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn path_from(variable: Option<OsString>, config_dir: Option<PathBuf>) -> PathBuf {
    named_path(variable).unwrap_or_else(|| config_dir.unwrap_or_else(|| PathBuf::from(".")).join("pgbg").join("config.toml"))
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
    let ai = ai::read(file.ai, &connections)?;
    let warnings = warnings.into_iter().chain(ai.warnings).collect();
    Ok(Loaded { config: Config { connections, format: file.display.format, ai: ai.settings }, warnings })
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
        let folder = Some(PathBuf::from("/Users/nina/Library/Application Support"));
        assert_eq!(path_from(Some("/tmp/shop.toml".into()), folder.clone()), PathBuf::from("/tmp/shop.toml"));
        assert_eq!(path_from(None, folder.clone()), PathBuf::from("/Users/nina/Library/Application Support/pgbg/config.toml"));
        assert_eq!(path_from(Some("".into()), folder), PathBuf::from("/Users/nina/Library/Application Support/pgbg/config.toml"));
    }
}
