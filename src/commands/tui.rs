use crate::config;
use crate::connection::ConnectionName;
use crate::usage::UsageFile;
use anyhow::Result;

/// Opens the TUI on the connection named `connection`, or on the first one of the config; `ctrl-o` switches to the others.
/// A file left where pgbg no longer reads it is warned of on the status line too, since the TUI hides stderr.
pub async fn run(connection: Option<&ConnectionName>) -> Result<()> {
    let path = config::path();
    let config = super::load(&path)?;
    let first = super::pick(&config.connections, connection, &path)?;
    let usage = config.counts_usage.then(UsageFile::from_env).flatten();
    let left_behind = super::left_behind(&path);
    let warning = (!left_behind.is_empty()).then(|| left_behind.join("; "));
    crate::tui::run(config.connections, first, path, config.format, config.ai, usage, warning).await
}
