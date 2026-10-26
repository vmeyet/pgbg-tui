//! `config.toml`: the connections, read once, with a warning for each key pgbg ignores.
use crate::connection::{Connection, Entry};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod ignored;

const PATH_VARIABLE: &str = "PGBG_CONFIG";

#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    connections: BTreeMap<String, Entry>,
}

/// The config, checked.
#[derive(Debug, Default)]
pub(crate) struct Config {
    /// Sorted by name.
    pub(crate) connections: Vec<Connection>,
}

/// A config as read from its file.
#[derive(Debug, Default)]
pub(crate) struct Loaded {
    pub(crate) config: Config,
    /// One line for each key the file holds that this pgbg ignores.
    pub(crate) warnings: Vec<String>,
}

/// The file `$PGBG_CONFIG` names, else `config.toml` in pgbg's folder of the user config folder.
pub(crate) fn path() -> PathBuf {
    path_from(std::env::var_os(PATH_VARIABLE), dirs::config_dir())
}

fn path_from(variable: Option<OsString>, config_dir: Option<PathBuf>) -> PathBuf {
    let named = variable.filter(|v| !v.is_empty()).map(PathBuf::from);
    named.unwrap_or_else(|| config_dir.unwrap_or_else(|| PathBuf::from(".")).join("pgbg").join("config.toml"))
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
    let (file, warnings) = ignored::parse::<ConfigFile>(text)?;
    let connections = file
        .connections
        .into_iter()
        .map(|(name, entry)| Connection::new(&name, entry).with_context(|| format!("connection `{name}`")))
        .collect::<Result<_>>()?;
    Ok(Loaded { config: Config { connections }, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = r#"
[connections.staging]
tier = "staging"
url = "postgres://nina@db.example.com/shop"

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
