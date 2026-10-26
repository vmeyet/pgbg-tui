//! The command line `pgbg` parses, with the help text each flag shows.
use clap::{Parser, Subcommand};

/// The command line: global flags, then one subcommand.
#[derive(Parser, Debug)]
#[command(
    name = "pgbg",
    version,
    about = "Read your PostgreSQL databases from the terminal: local, staging, and production behind a tunnel.",
    propagate_version = true
)]
pub struct Cli {
    /// Print machine-readable JSON instead of the plain output.
    #[arg(long, global = true)]
    pub json: bool,
    /// What to run; the TUI on the first connection when there is none.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Every subcommand `pgbg` knows.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Opens the TUI on a connection: the one named, else the first in `config.toml`.
    Tui {
        /// The connection's name in `config.toml`.
        connection: Option<String>,
    },
    /// The connections in `config.toml`: name, tier, URL, and how pgbg reaches each one.
    Connections,
    /// Runs one statement on a connection and prints its result; read only unless the connection allows writes.
    Run {
        /// The connection's name in `config.toml`.
        connection: String,
        /// One SQL statement.
        sql: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("pgbg").chain(args.iter().copied()))
    }

    #[test]
    fn connections_takes_json_before_or_after_it() {
        assert!(parse(&["connections", "--json"]).unwrap().json);
        assert!(parse(&["--json", "connections"]).unwrap().json);
        assert!(!parse(&["connections"]).unwrap().json);
    }

    #[test]
    fn no_subcommand_leaves_the_command_to_the_tui() {
        assert!(parse(&[]).unwrap().command.is_none());
    }

    #[test]
    fn tui_takes_an_optional_connection() {
        assert!(matches!(parse(&["tui", "prod"]).unwrap().command, Some(Command::Tui { connection: Some(name) }) if name == "prod"));
        assert!(matches!(parse(&["tui"]).unwrap().command, Some(Command::Tui { connection: None })));
    }
}
