//! Opening a session along its route: through the connection's tunnel when it has one.
use super::Session;
use super::model::{LocalPort, Route};
use crate::connection::Connection;
use crate::tunnel::Tunnel;

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

/// Opens a session on `connection`, through `tunnel` while it runs, else through a tunnel started again, telling
/// `on_tunnel` as that one starts and once it is up.
pub(crate) async fn open_along(
    connection: &Connection,
    tunnel: Option<Tunnel>,
    on_tunnel: impl FnMut(TunnelStep),
) -> Result<Opened, NotOpened> {
    let tunnel = running_tunnel(connection, tunnel, on_tunnel).await.map_err(NotOpened::Tunnel)?;
    let route = tunnel.as_ref().map_or(Route::Direct, Tunnel::route);
    match Session::open(connection, route).await {
        Ok(session) => Ok(Opened { session, tunnel }),
        Err(error) => Err(NotOpened::Login { error, tunnel: tunnel.map(Box::new) }),
    }
}

async fn running_tunnel(
    connection: &Connection,
    tunnel: Option<Tunnel>,
    mut on_tunnel: impl FnMut(TunnelStep),
) -> anyhow::Result<Option<Tunnel>> {
    let Some(spec) = connection.tunnel() else { return Ok(None) };
    if let Some(mut tunnel) = tunnel
        && tunnel.is_running()
    {
        return Ok(Some(tunnel));
    }
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
        let listening = connection(&format!("tls = \"off\"\ntunnel = {{ command = [\"python3\", \"-c\", {HANG_UP:?}, \"{{port}}\"] }}"));
        let mut steps = vec![];
        let refused = open_along(&listening, None, |step| steps.push(step)).await;
        let Err(NotOpened::Login { tunnel: Some(tunnel), .. }) = refused else { panic!("{refused:?}") };
        assert_eq!(steps, [TunnelStep::Starting, TunnelStep::Up(tunnel.port())]);
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_failed_login_gives_back_the_tunnel_it_started() {
        let listening = connection(&format!("tls = \"off\"\ntunnel = {{ command = [\"python3\", \"-c\", {HANG_UP:?}, \"{{port}}\"] }}"));
        let refused = open_along(&listening, None, |_| {}).await;
        assert!(matches!(refused, Err(NotOpened::Login { tunnel: Some(_), .. })), "{refused:?}");
    }
}
