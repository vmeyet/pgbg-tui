use crate::config;
use crate::connection::ConnectionName;
use anyhow::Result;

/// Opens the TUI on the connection named `connection`, or on the first one of the config; `ctrl-o` switches to the others.
pub async fn run(connection: Option<&ConnectionName>) -> Result<()> {
    let path = config::path();
    let connections = super::load(&path)?.connections;
    let first = super::pick(&connections, connection, &path)?;
    crate::tui::run(connections, first, path).await
}
