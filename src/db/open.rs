//! Opening a session along its route: through the connection's tunnel when it has one.
use super::Session;
use super::model::{LocalPort, Route};
use crate::connection::Connection;
use crate::tunnel::Tunnel;
use std::error::Error;
use std::io;
use tokio::time::error::Elapsed;

/// An open session, and the tunnel it goes through when its connection has one; the session drops first.
#[derive(Debug)]
pub(crate) struct Opened {
    pub(crate) session: Session,
    pub(crate) tunnel: Option<Tunnel>,
}

/// Why a session did not open: its tunnel did not start, or the login failed, with the tunnel kept for the next try.
#[derive(Debug)]
pub(crate) enum NotOpened {
    Tunnel(anyhow::Error),
    Login { error: anyhow::Error, tunnel: Option<Box<Tunnel>> },
}

/// Where a tunnel being started stands, for whoever shows it while the session opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TunnelStep {
    Starting,
    Up(LocalPort),
}

/// One open of a session that is not retried: the session, or why its login failed, and the tunnel it went through.
#[derive(Debug)]
pub(crate) struct Attempt {
    pub(crate) session: anyhow::Result<Session>,
    pub(crate) tunnel: Option<Tunnel>,
}

/// Opens a session on `connection` once, starting its tunnel when it has one; fails only when that tunnel does not start.
pub(crate) async fn open_once(connection: &Connection) -> anyhow::Result<Attempt> {
    match open_along(connection, None, |_| {}).await {
        Ok(Opened { session, tunnel }) => Ok(Attempt { session: Ok(session), tunnel }),
        Err(NotOpened::Tunnel(err)) => Err(err),
        Err(NotOpened::Login { error, tunnel }) => Ok(Attempt { session: Err(error), tunnel: tunnel.map(|tunnel| *tunnel) }),
    }
}

/// Opens a session on `connection` through `tunnel` while it runs, else through a new tunnel; `on_tunnel` hears when
/// that new tunnel starts and when it is up.
/// A running tunnel may no longer carry anything, as an ssh whose far side is gone: when the link fails through it, it is
/// stopped and a new one gets the same try. A login the server answered leaves it be.
pub(crate) async fn open_along(
    connection: &Connection,
    tunnel: Option<Tunnel>,
    on_tunnel: impl FnMut(TunnelStep),
) -> Result<Opened, NotOpened> {
    if let Some(kept) = still_running(tunnel) {
        match open_through(connection, Some(kept)).await {
            Err(NotOpened::Login { error, tunnel: Some(kept) }) if is_link_failure(&error) => stop_for_a_new_one(*kept, &error).await,
            settled => return settled,
        }
    }
    let tunnel = started_tunnel(connection, on_tunnel).await.map_err(NotOpened::Tunnel)?;
    open_through(connection, tunnel).await
}

async fn open_through(connection: &Connection, tunnel: Option<Tunnel>) -> Result<Opened, NotOpened> {
    let route = tunnel.as_ref().map_or(Route::Direct, Tunnel::route);
    match Session::open(connection, route).await {
        Ok(session) => Ok(Opened { session, tunnel }),
        Err(error) => Err(NotOpened::Login { error, tunnel: tunnel.map(Box::new) }),
    }
}

fn still_running(tunnel: Option<Tunnel>) -> Option<Tunnel> {
    let mut tunnel = tunnel?;
    tunnel.is_running().then_some(tunnel)
}

/// Whether a login failed on the way to the server, not by its answer: no answer in time, or the driver's link ending.
fn is_link_failure(error: &anyhow::Error) -> bool {
    error.is::<Elapsed>() || error.downcast_ref::<tokio_postgres::Error>().is_some_and(ended_the_link)
}

/// TLS reports a certificate it refuses as `InvalidData`, and a new tunnel would meet the same certificate.
fn ended_the_link(error: &tokio_postgres::Error) -> bool {
    let io = error.source().and_then(|source| source.downcast_ref::<io::Error>());
    error.is_closed() || io.is_some_and(|io| io.kind() != io::ErrorKind::InvalidData)
}

async fn stop_for_a_new_one(mut tunnel: Tunnel, why: &anyhow::Error) {
    tracing::warn!("the link failed through a running tunnel, so a new one starts: {why:#}");
    if let Err(err) = tunnel.stop().await {
        tracing::warn!("{err:#}");
    }
}

async fn started_tunnel(connection: &Connection, mut on_tunnel: impl FnMut(TunnelStep)) -> anyhow::Result<Option<Tunnel>> {
    let Some(spec) = connection.tunnel() else { return Ok(None) };
    on_tunnel(TunnelStep::Starting);
    let started = Tunnel::start(spec).await?;
    on_tunnel(TunnelStep::Up(started.port()));
    Ok(Some(started))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    const HANG_UP: &str =
        "import socket, sys\nserver = socket.create_server(('127.0.0.1', int(sys.argv[1])))\nwhile True: server.accept()[0].close()";

    fn connection(extra: &str) -> Connection {
        fixtures::connection(&format!("tier = \"local\"\nurl = \"postgres://nina@localhost:1/shop\"\n{extra}"))
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_tunnel_that_does_not_start_says_so_before_any_login() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let dead = connection("tls = \"off\"\ntunnel = { command = [\"sh\", \"-c\", \"exit 255\", \"{port}\"] }");
        assert!(matches!(open_along(&dead, None, |_| {}).await, Err(NotOpened::Tunnel(_))));
    }

    #[tokio::test]
    #[ignore = "opens a socket"]
    async fn a_failed_login_without_a_tunnel_has_none_to_give_back() {
        let refused = open_along(&connection(""), None, |_| {}).await;
        assert!(matches!(refused, Err(NotOpened::Login { tunnel: None, .. })), "{refused:?}");
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_tunnel_started_says_it_starts_then_on_which_port_it_is_up() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let listening = connection(&format!("tls = \"off\"\ntunnel = {{ command = [\"python3\", \"-c\", {HANG_UP:?}, \"{{port}}\"] }}"));
        let mut steps = vec![];
        let refused = open_along(&listening, None, |step| steps.push(step)).await;
        let Err(NotOpened::Login { tunnel: Some(tunnel), .. }) = refused else { panic!("{refused:?}") };
        assert_eq!(steps, [TunnelStep::Starting, TunnelStep::Up(tunnel.port())]);
    }

    #[tokio::test(start_paused = true)]
    async fn no_answer_in_time_is_a_link_failure() {
        let elapsed = tokio::time::timeout(std::time::Duration::from_secs(1), std::future::pending::<()>()).await.unwrap_err();
        assert!(is_link_failure(&anyhow::Error::new(elapsed).context("connecting to shop")));
    }

    #[test]
    fn a_password_that_cannot_be_read_is_no_link_failure() {
        assert!(!is_link_failure(&anyhow::anyhow!("the password command failed")));
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_running_tunnel_that_carries_nothing_gives_way_to_a_new_one() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let hanging_up = connection(&format!("tls = \"off\"\ntunnel = {{ command = [\"python3\", \"-c\", {HANG_UP:?}, \"{{port}}\"] }}"));
        let first = open_along(&hanging_up, None, |_| {}).await;
        let Err(NotOpened::Login { tunnel: Some(first), .. }) = first else { panic!("{first:?}") };
        let first_port = first.port();
        let mut steps = vec![];
        let again = open_along(&hanging_up, Some(*first), |step| steps.push(step)).await;
        let Err(NotOpened::Login { tunnel: Some(second), .. }) = again else { panic!("{again:?}") };
        assert_eq!(steps, [TunnelStep::Starting, TunnelStep::Up(second.port())]);
        assert_ne!(second.port(), first_port);
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_failed_login_gives_back_the_tunnel_it_started() {
        let _ports = fixtures::TUNNEL_PORTS.lock().await;
        let listening = connection(&format!("tls = \"off\"\ntunnel = {{ command = [\"python3\", \"-c\", {HANG_UP:?}, \"{{port}}\"] }}"));
        let refused = open_along(&listening, None, |_| {}).await;
        assert!(matches!(refused, Err(NotOpened::Login { tunnel: Some(_), .. })), "{refused:?}");
    }
}
