use crate::config;
use crate::connection::{Connection, ConnectionName};
use crate::db::{NotOpened, Opened, Session, open_along};
use crate::tunnel::Tunnel;
use anyhow::{Result, bail};
use report::{Check, Report};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod report;
mod verdicts;

/// Checks the connection named `name`, one line per check, or JSON with `json`; fails when a check does.
pub async fn run(name: &ConnectionName, json: bool) -> Result<()> {
    let report = Report { connection: name, checks: checks(name).await };
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

async fn checks(name: &ConnectionName) -> Vec<Check> {
    let connection = match super::connection(Some(name)) {
        Ok(connection) => connection,
        Err(err) => return vec![verdicts::config_refused(&err)],
    };
    let local = [verdicts::config(&connection), file_mode(&config::path()), verdicts::tls(&connection)];
    local.into_iter().chain(reached(&connection).await).collect()
}

fn file_mode(path: &Path) -> Check {
    match std::fs::metadata(path) {
        Ok(metadata) => verdicts::file_mode(path, metadata.permissions().mode()),
        Err(err) => verdicts::not_read("file mode", &err.into()),
    }
}

/// The checks that need the server: the tunnel when there is one, the login, then what the session reads.
async fn reached(connection: &Connection) -> Vec<Check> {
    let (opened, tunnel) = match open_along(connection, None).await {
        Ok(Opened { session, tunnel }) => (Ok(session), tunnel),
        Err(NotOpened::Tunnel(err)) => return vec![verdicts::tunnel_failed(&err)],
        Err(NotOpened::Login { error, tunnel }) => (Err(error), tunnel.map(|tunnel| *tunnel)),
    };
    let started = connection.tunnel().zip(tunnel.as_ref()).map(|(spec, tunnel)| verdicts::tunnel_started(spec, tunnel.port()));
    let session = match opened {
        Ok(session) => read(connection, session).await,
        Err(err) => vec![verdicts::login_failed(&err)],
    };
    let stopped = match tunnel {
        Some(tunnel) => Some(stop(tunnel).await),
        None => None,
    };
    started.into_iter().chain(session).chain(stopped).collect()
}

/// What the session says of itself; it is dropped once read, before its tunnel stops.
async fn read(connection: &Connection, mut session: Session) -> Vec<Check> {
    let facts = match session.server_facts().await {
        Ok(facts) => facts,
        Err(err) => return vec![verdicts::not_read("server", &err)],
    };
    let [login, server] = verdicts::server(&facts);
    vec![login, server, verdicts::timeout(connection, &facts)]
}

async fn stop(mut tunnel: Tunnel) -> Check {
    let is_port_free = tunnel.stop().await.is_ok();
    verdicts::tunnel_stopped(tunnel.port(), is_port_free)
}
