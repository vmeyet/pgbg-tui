//! One module per subcommand, each with a `run` the binary calls.
/// `pgbg connections`.
pub mod connections;
/// `pgbg run`.
pub mod run;
/// `pgbg`, or `pgbg tui`.
pub mod tui;

use crate::config::{self, Config};
use crate::connection::{Connection, ConnectionName};
use anyhow::{Result, anyhow};
use std::path::Path;

/// Each key of the config this pgbg ignores, on stderr.
fn warn_of_ignored_keys(warnings: &[String], path: &Path) {
    if warnings.is_empty() {
        return;
    }
    for warning in warnings {
        eprintln!("! {warning}");
    }
    eprintln!("  in {}", path.display());
}

/// The config at `path`, once each key it ignores is warned of on stderr.
fn load(path: &Path) -> Result<Config> {
    let loaded = Config::read(path)?;
    warn_of_ignored_keys(&loaded.warnings, path);
    Ok(loaded.config)
}

/// The connection named `name` in the config, or its first one when `name` is `None`.
fn connection(name: Option<&ConnectionName>) -> Result<Connection> {
    let path = config::path();
    let mut connections = load(&path)?.connections.into_iter();
    match name {
        Some(name) => connections
            .find(|c| c.name() == name)
            .ok_or_else(|| anyhow!("no connection `{name}` in {}: `pgbg connections` lists them", path.display())),
        None => connections.next().ok_or_else(|| anyhow!("no connections in {}: add one under `[connections.<name>]`", path.display())),
    }
}
