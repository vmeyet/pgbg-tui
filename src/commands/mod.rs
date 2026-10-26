//! One module per subcommand, each with a `run` the binary calls.
/// `pgbg connection add|edit|remove`.
pub mod connection;
/// `pgbg connections`.
pub mod connections;
/// `pgbg password set`.
pub mod password;
/// `pgbg run`.
pub mod run;
/// `pgbg`, or `pgbg tui`.
pub mod tui;

use crate::config::{self, Config};
use crate::connection::{Connection, ConnectionName};
use crate::render::printable;
use anyhow::{Result, anyhow};
use std::path::Path;

/// `err` and each of its causes on stderr, escaped: a server error can quote a value from the database.
pub fn print_error(err: &anyhow::Error) {
    eprintln!("\x1b[31m✗\x1b[0m {}", printable(&err.to_string()));
    for cause in err.chain().skip(1) {
        eprintln!("  \x1b[2m{}\x1b[0m", printable(&cause.to_string()));
    }
}

/// Each key of the config this pgbg ignores, on stderr.
fn warn_of_ignored_keys(warnings: &[String], path: &Path) {
    if warnings.is_empty() {
        return;
    }
    for warning in warnings {
        eprintln!("! {}", printable(warning));
    }
    eprintln!("  in {}", path.display());
}

/// The config at `path`, once each key it ignores is warned of on stderr.
fn load(path: &Path) -> Result<Config> {
    let loaded = Config::read(path)?;
    warn_of_ignored_keys(&loaded.warnings, path);
    Ok(loaded.config)
}

fn no_connection(name: &ConnectionName, path: &Path) -> anyhow::Error {
    anyhow!("no connection `{name}` in {}: `pgbg connections` lists them", path.display())
}

/// The connection named `name` in the config, or its first one when `name` is `None`.
fn connection(name: Option<&ConnectionName>) -> Result<Connection> {
    let path = config::path();
    pick(&load(&path)?.connections, name, &path)
}

/// The connection named `name` among `connections` of the config at `path`, or the first one when `name` is `None`.
fn pick(connections: &[Connection], name: Option<&ConnectionName>, path: &Path) -> Result<Connection> {
    let picked = match name {
        Some(name) => connections.iter().find(|c| c.name() == name).ok_or_else(|| no_connection(name, path)),
        None => connections.first().ok_or_else(|| anyhow!("no connections in {}: add one under `[connections.<name>]`", path.display())),
    };
    picked.cloned()
}
