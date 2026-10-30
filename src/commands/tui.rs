use crate::config::{self, Loaded};
use crate::connection::ConnectionName;
use crate::usage::UsageFile;
use anyhow::Result;

/// Opens the TUI on the connection named `connection`, or on the first one of the config; `ctrl-o` switches to the others.
/// A config warning, and a file left where pgbg no longer reads it, are warned of on the status line too, since the TUI
/// hides stderr.
pub async fn run(connection: Option<&ConnectionName>) -> Result<()> {
    let path = config::path();
    let Loaded { config, warnings } = super::load_warned(&path)?;
    let first = super::pick(&config.connections, connection, &path)?;
    let usage = config.counts_usage.then(UsageFile::from_env).flatten();
    let warnings: Vec<String> = warnings.into_iter().chain(super::left_behind(&path)).collect();
    let warning = (!warnings.is_empty()).then(|| warnings.join("; "));
    crate::tui::run(config, first, path, usage, warning).await
}
