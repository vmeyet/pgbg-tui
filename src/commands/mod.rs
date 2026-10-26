//! One module per subcommand, each with a `run` the binary calls.
/// `pgbg connections`.
pub mod connections;
/// `pgbg run`.
pub mod run;
/// `pgbg`, or `pgbg tui`.
pub mod tui;

use crate::config::{self, Config};
use crate::connection::Connection;
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

/// The connection named `name` in the config, or its first one when `name` is `None`.
fn connection(name: Option<&str>) -> Result<Connection> {
    let path = config::path();
    let loaded = Config::read(&path)?;
    warn_of_ignored_keys(&loaded.warnings, &path);
    let mut connections = loaded.config.connections.into_iter();
    match name {
        Some(name) => connections
            .find(|c| c.name().to_string() == name)
            .ok_or_else(|| anyhow!("no connection `{name}` in {}: `pgbg connections` lists them", path.display())),
        None => connections.next().ok_or_else(|| anyhow!("no connections in {}: add one under `[connections.<name>]`", path.display())),
    }
}
