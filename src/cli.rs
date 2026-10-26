//! The command line `pgbg` parses, with the help text each flag shows.
use crate::connection::ConnectionName;
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
        connection: Option<ConnectionName>,
    },
    /// The connections in `config.toml`: name, tier, URL, and how pgbg reaches each one.
    Connections,
    /// Runs one statement on a connection and prints its result; read only unless the connection allows writes.
    Run {
        /// The connection's name in `config.toml`.
        connection: ConnectionName,
        /// One SQL statement.
        sql: String,
    },
    /// The passwords pgbg keeps in the macOS keychain; there is no flag for a password, which `ps` would show.
    Password {
        /// What to do with it.
        #[command(subcommand)]
        command: PasswordCommand,
    },
}

/// What `pgbg password` does.
#[derive(Subcommand, Debug)]
pub enum PasswordCommand {
    /// Saves a connection's password in the keychain, typed hidden or piped in; a connection with no password yet gets `{ keychain = "pgbg/<name>" }`.
    Set {
        /// The connection's name in `config.toml`.
        connection: ConnectionName,
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
    fn a_connection_name_the_config_cannot_hold_is_refused_while_parsing() {
        assert!(parse(&["run", "my shop", "SELECT 1"]).is_err());
        assert!(parse(&["tui", "shop/eu"]).is_err());
    }

    #[test]
    fn a_password_is_never_taken_from_the_command_line() {
        assert!(matches!(parse(&["password", "set", "prod"]).unwrap().command, Some(Command::Password { .. })));
        assert!(parse(&["password", "set", "prod", "--password", "hunter2"]).is_err());
        assert!(parse(&["password", "set", "prod", "hunter2"]).is_err());
    }

    #[test]
    fn tui_takes_an_optional_connection() {
        assert!(
            matches!(parse(&["tui", "prod"]).unwrap().command, Some(Command::Tui { connection: Some(name) }) if name.to_string() == "prod")
        );
        assert!(matches!(parse(&["tui"]).unwrap().command, Some(Command::Tui { connection: None })));
    }
}
