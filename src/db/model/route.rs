use std::fmt;

/// How a session reaches its server: straight to the host of its URL, or through a tunnel's local port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Direct,
    Tunnel(LocalPort),
}

/// A port on `127.0.0.1` that a tunnel forwards to the database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalPort(u16);

impl LocalPort {
    pub(crate) fn new(port: u16) -> Self {
        Self(port)
    }

    pub(crate) fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for LocalPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
