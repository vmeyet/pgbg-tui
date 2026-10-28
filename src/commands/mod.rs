//! One module per subcommand, each with a `run` the binary calls.
/// `pgbg connection add|edit|remove`.
pub mod connection;
/// `pgbg connections`.
pub mod connections;
/// `pgbg doctor`.
pub mod doctor;
/// `pgbg history`.
pub mod history;
/// `pgbg password set`.
pub mod password;
/// `pgbg run`.
pub mod run;
/// `pgbg schema`.
pub mod schema;
/// `pgbg`, or `pgbg tui`.
pub mod tui;
/// `pgbg usage`.
pub mod usage;

use crate::config::{self, Config};
use crate::connection::{Connection, ConnectionName};
use crate::db::{Attempt, Interrupted, Session, open_once};
use crate::render::printable;
use crate::signal::Signals;
use crate::tunnel::Tunnel;
use anyhow::{Result, anyhow};
use std::path::Path;

/// Prints `err` and each of its causes on stderr, escaped: a server error can quote a value from the database.
pub fn print_error(err: &anyhow::Error) {
    eprintln!("\x1b[31m✗\x1b[0m {}", printable(&err.to_string()));
    for cause in err.chain().skip(1) {
        eprintln!("  \x1b[2m{}\x1b[0m", printable(&cause.to_string()));
    }
}

/// Prints `text` on stderr as a warning, escaped.
fn warn(text: &str) {
    eprintln!("! {}", printable(text));
}

/// Runs `work` on a session opened on `connection` along its route; its tunnel, when it has one, stops once `work` is done.
/// A signal stops the opening, or cancels the statement `work` runs on the server; see `cancelled_on_signal`.
async fn on_session<T>(connection: &Connection, work: impl AsyncFnOnce(&mut Session) -> Result<T>) -> Result<T> {
    let mut signals = Signals::listen()?;
    let Attempt { session, tunnel } = signals.or_stopped(open_once(connection)).await??;
    let result = match session {
        Ok(mut session) => cancelled_on_signal(&mut signals, &mut session, work).await,
        Err(err) => Err(err),
    };
    stop(tunnel).await;
    result
}

/// Runs `work` on `session`; a signal stops it as `CancelToken::unless_stopped` says, while its link and tunnel are
/// still up. Otherwise a server that cannot tell pgbg is gone (before PostgreSQL 14, off Linux, behind a pooler) would
/// run it to its end.
async fn cancelled_on_signal<T>(
    signals: &mut Signals,
    session: &mut Session,
    work: impl AsyncFnOnce(&mut Session) -> Result<T>,
) -> Result<T> {
    let token = session.cancel_token();
    let Interrupted { by, cancel } = match token.unless_stopped(work(session), signals.next()).await {
        Ok(outcome) => return outcome,
        Err(interrupted) => interrupted,
    };
    if let Err(err) = cancel {
        warn(&format!("the statement may still run on the server: {err:#}"));
    }
    Err(by.into())
}

/// Stops `tunnel` and warns when its forward outlives it.
async fn stop(tunnel: Option<Tunnel>) {
    let Some(mut tunnel) = tunnel else { return };
    if let Err(err) = tunnel.stop().await {
        warn(&format!("{err:#}"));
    }
}

/// Warns on stderr of each config key this version of pgbg ignores.
fn warn_of_ignored_keys(warnings: &[String], path: &Path) {
    if warnings.is_empty() {
        return;
    }
    for warning in warnings {
        warn(warning);
    }
    eprintln!("  in {}", path.display());
}

/// Loads the config at `path`, warning on stderr of each key it ignores.
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
