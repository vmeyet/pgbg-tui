use crate::config;
use crate::connection::{Connection, ConnectionName};
use crate::db::{Attempt, Session, open_once};
use crate::signal::Signals;
use crate::tunnel::Tunnel;
use anyhow::{Result, bail};
use report::{Check, Report};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod report;
mod verdicts;

/// Checks the connection named `name`, one line per check, or JSON with `json`; fails when a check does.
pub async fn run(name: &ConnectionName, json: bool) -> Result<()> {
    let report = Report { connection: name, checks: checks(name).await? };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.text());
    }
    match report.failures() {
        0 => Ok(()),
        1 => bail!("1 check failed"),
        failures => bail!("{failures} checks failed"),
    }
}

/// Every check, unless a signal stops pgbg first.
async fn checks(name: &ConnectionName) -> Result<Vec<Check>> {
    let connection = match super::connection(Some(name)) {
        Ok(connection) => connection,
        Err(err) => return Ok(vec![verdicts::config_refused(&err)]),
    };
    let local = [verdicts::config(&connection), file_mode(&config::path()), verdicts::tls(&connection)];
    Ok(local.into_iter().chain(reached(&connection).await?).collect())
}

fn file_mode(path: &Path) -> Check {
    match std::fs::metadata(path) {
        Ok(metadata) => verdicts::file_mode(path, metadata.permissions().mode()),
        Err(err) => verdicts::not_read("file mode", &err.into()),
    }
}

/// The checks that need the server: the tunnel when there is one, the login, then what the session reads; a signal
/// stops them as it stops `pgbg run`, the tunnel stopped still.
async fn reached(connection: &Connection) -> Result<Vec<Check>> {
    let mut signals = Signals::listen()?;
    let Attempt { session, tunnel } = match signals.or_stopped(open_once(connection)).await? {
        Ok(attempt) => attempt,
        Err(err) => return Ok(vec![verdicts::tunnel_failed(&err)]),
    };
    let started = connection.tunnel().zip(tunnel.as_ref()).map(|(spec, tunnel)| verdicts::tunnel_started(spec, tunnel.port()));
    let session = match session {
        Ok(mut session) => {
            super::cancelled_on_signal(&mut signals, &mut session, async |session| Ok(read(connection, session).await)).await
        }
        Err(err) => Ok(vec![verdicts::login_failed(&err)]),
    };
    let stopped = match tunnel {
        Some(tunnel) => Some(stop(tunnel).await),
        None => None,
    };
    Ok(started.into_iter().chain(session?).chain(stopped).collect())
}

/// What the session says of itself and of its role; the session is dropped once read, before its tunnel stops.
async fn read(connection: &Connection, session: &mut Session) -> Vec<Check> {
    let facts = match session.server_facts().await {
        Ok(facts) => facts,
        Err(err) => return vec![verdicts::not_read("server", &err)],
    };
    let role = match session.role_rights().await {
        Ok(rights) => verdicts::role(connection.writes(), &rights),
        Err(err) => verdicts::not_read("role", &err),
    };
    let beyond = match session.read_only_gaps().await {
        Ok(gaps) => verdicts::beyond_read_only(connection.writes(), &gaps),
        Err(err) => verdicts::not_read("beyond read only", &err),
    };
    let [login, server] = verdicts::server(&facts);
    vec![login, server, verdicts::timeout(connection, &facts), role, beyond]
}

async fn stop(mut tunnel: Tunnel) -> Check {
    let is_port_free = tunnel.stop().await.is_ok();
    verdicts::tunnel_stopped(tunnel.port(), is_port_free)
}
