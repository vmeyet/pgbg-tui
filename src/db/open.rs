//! Opening a session along its route: through the connection's tunnel when it has one.
use super::Session;
use super::model::Route;
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

/// Opens a session on `connection`, through `tunnel` while it runs, else through a tunnel started again.
pub(crate) async fn open_along(connection: &Connection, tunnel: Option<Tunnel>) -> Result<Opened, NotOpened> {
    let tunnel = running_tunnel(connection, tunnel).await.map_err(NotOpened::Tunnel)?;
    let route = tunnel.as_ref().map_or(Route::Direct, Tunnel::route);
    match Session::open(connection, route).await {
        Ok(session) => Ok(Opened { session, tunnel }),
        Err(error) => Err(NotOpened::Login { error, tunnel: tunnel.map(Box::new) }),
    }
}

async fn running_tunnel(connection: &Connection, tunnel: Option<Tunnel>) -> anyhow::Result<Option<Tunnel>> {
    let Some(spec) = connection.tunnel() else { return Ok(None) };
    if let Some(mut tunnel) = tunnel
        && tunnel.is_running()
    {
        return Ok(Some(tunnel));
    }
    Tunnel::start(spec).await.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Entry;

    const HANG_UP: &str =
        "import socket, sys\nserver = socket.create_server(('127.0.0.1', int(sys.argv[1])))\nwhile True: server.accept()[0].close()";

    fn connection(extra: &str) -> Connection {
        let entry: Entry = toml::from_str(&format!("tier = \"local\"\nurl = \"postgres://nina@localhost:1/shop\"\n{extra}")).unwrap();
        Connection::new("shop", entry).unwrap()
    }

    #[tokio::test]
    async fn a_tunnel_that_does_not_start_says_so_before_any_login() {
        let dead = connection("tls = \"off\"\ntunnel = { command = [\"sh\", \"-c\", \"exit 255\", \"{port}\"] }");
        assert!(matches!(open_along(&dead, None).await, Err(NotOpened::Tunnel(_))));
    }

    #[tokio::test]
    async fn a_failed_login_without_a_tunnel_has_none_to_give_back() {
        let refused = open_along(&connection(""), None).await;
        assert!(matches!(refused, Err(NotOpened::Login { tunnel: None, .. })), "{refused:?}");
    }

    #[tokio::test]
    async fn a_failed_login_gives_back_the_tunnel_it_started() {
        let listening = connection(&format!("tls = \"off\"\ntunnel = {{ command = [\"python3\", \"-c\", {HANG_UP:?}, \"{{port}}\"] }}"));
        let refused = open_along(&listening, None).await;
        assert!(matches!(refused, Err(NotOpened::Login { tunnel: Some(_), .. })), "{refused:?}");
    }
}
