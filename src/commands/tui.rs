use crate::config;
use crate::connection::ConnectionName;
use crate::usage::UsageFile;
use anyhow::Result;

/// Opens the TUI on the connection named `connection`, or on the first one of the config; `ctrl-o` switches to the others.
pub async fn run(connection: Option<&ConnectionName>) -> Result<()> {
    let path = config::path();
    let config = super::load(&path)?;
    let first = super::pick(&config.connections, connection, &path)?;
    let usage = config.counts_usage.then(UsageFile::from_env).flatten();
    crate::tui::run(config.connections, first, path, config.format, config.ai, usage).await
}
