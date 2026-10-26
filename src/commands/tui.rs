use anyhow::Result;

/// Opens the TUI on the connection named `connection`, or on the first one of the config.
pub async fn run(connection: Option<&str>) -> Result<()> {
    crate::tui::run(super::connection(connection)?).await
}
