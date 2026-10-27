use crate::db::model::LocalPort;
use serde::{Deserialize, Serialize};

const PORT: &str = "{port}";

#[derive(Deserialize)]
struct TunnelEntry {
    command: Vec<String>,
    #[serde(default)]
    before: Option<Vec<String>>,
}

/// How to reach a database out of reach: `command` forwards a local port, written `{port}` in it,
/// and `before`, when set, runs once first.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "TunnelEntry")]
pub(crate) struct TunnelSpec {
    command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<Vec<String>>,
}

impl TunnelSpec {
    /// A tunnel that runs `command`, once `before` ran when there is one.
    pub(crate) fn new(command: Vec<String>, before: Option<Vec<String>>) -> anyhow::Result<Self> {
        Self::try_from(TunnelEntry { command, before }).map_err(anyhow::Error::msg)
    }

    pub(crate) fn command(&self) -> &[String] {
        &self.command
    }

    /// The tunnel's command with `port` in place of every `{port}`.
    pub(crate) fn command_on(&self, port: LocalPort) -> Vec<String> {
        self.command.iter().map(|arg| arg.replace(PORT, &port.to_string())).collect()
    }

    pub(crate) fn before(&self) -> Option<&[String]> {
        self.before.as_deref()
    }
}

impl TryFrom<TunnelEntry> for TunnelSpec {
    type Error = String;

    fn try_from(entry: TunnelEntry) -> Result<Self, Self::Error> {
        if !entry.command.iter().any(|arg| arg.contains(PORT)) {
            return Err(format!("`tunnel.command` never says `{PORT}`: pgbg picks the local port and puts it there"));
        }
        if entry.before.as_ref().is_some_and(Vec::is_empty) {
            return Err("`tunnel.before` is empty: leave it out instead".to_owned());
        }
        Ok(Self { command: entry.command, before: entry.before })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tunnel(text: &str) -> Result<TunnelSpec, toml::de::Error> {
        toml::from_str::<std::collections::BTreeMap<String, TunnelSpec>>(text).map(|mut table| table.remove("tunnel").unwrap())
    }

    #[test]
    fn a_tunnel_keeps_its_command_and_its_before_command() {
        let spec = tunnel("tunnel = { command = [\"ssh\", \"-N\", \"-L\", \"{port}:localhost:5432\", \"db.example.com\"], before = [\"wake\", \"db.example.com\"] }").unwrap();
        assert_eq!(spec.command[3], "{port}:localhost:5432");
        assert_eq!(spec.before, Some(vec!["wake".to_owned(), "db.example.com".to_owned()]));
    }

    #[test]
    fn the_port_placeholder_may_sit_inside_a_flag() {
        assert!(tunnel("tunnel = { command = [\"proxy\", \"--local-host-port=localhost:{port}\"] }").is_ok());
    }

    #[test]
    fn the_command_on_a_port_says_that_port_wherever_it_said_the_placeholder() {
        let spec = tunnel("tunnel = { command = [\"proxy\", \"--port\", \"{port}\", \"--local-host-port=localhost:{port}\"] }").unwrap();
        assert_eq!(spec.command_on(LocalPort::new(49213)), ["proxy", "--port", "49213", "--local-host-port=localhost:49213"]);
    }

    #[test]
    fn a_command_without_the_port_placeholder_is_refused() {
        let err = tunnel("tunnel = { command = [\"ssh\", \"-N\", \"-L\", \"5433:localhost:5432\", \"db.example.com\"] }").unwrap_err();
        assert!(err.to_string().contains("{port}"), "{err}");
    }

    #[test]
    fn a_tunnel_built_from_a_command_without_the_port_placeholder_is_refused() {
        assert!(TunnelSpec::new(vec!["ssh".into(), "db.example.com".into()], None).unwrap_err().to_string().contains("{port}"));
    }

    #[test]
    fn an_empty_before_command_is_refused() {
        assert!(tunnel("tunnel = { command = [\"proxy\", \"{port}\"], before = [] }").is_err());
    }
}
