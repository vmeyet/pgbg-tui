//! The command line `pgbg` parses, with the help text each flag shows.
use crate::connection::{ConnectionName, Tier, WritePolicy};
use clap::{Args, Parser, Subcommand};

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
    /// Adds, changes or removes a connection in `config.toml`, keeping the rest of the file as written.
    Connection {
        /// What to do with it.
        #[command(subcommand)]
        command: ConnectionCommand,
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

/// What `pgbg connection` does.
#[derive(Subcommand, Debug)]
pub enum ConnectionCommand {
    /// Adds a connection; read only unless `--writes allowed`.
    Add {
        /// The new connection's name.
        connection: ConnectionName,
        /// Sets the colour and the timeout.
        #[arg(long)]
        tier: Tier,
        /// `postgres://user@host:port/database`, without a password.
        #[arg(long)]
        url: String,
        /// The other keys.
        #[command(flatten)]
        keys: ConnectionKeys,
    },
    /// Changes the keys given and leaves the others as they are.
    #[command(group(clap::ArgGroup::new("change").required(true).multiple(true)
        .args(["tier", "url", "tunnel", "password_env", "password_command", "keychain", "writes"])))]
    Edit {
        /// The connection's name in `config.toml`.
        connection: ConnectionName,
        /// Sets the colour and the timeout.
        #[arg(long)]
        tier: Option<Tier>,
        /// `postgres://user@host:port/database`, without a password.
        #[arg(long)]
        url: Option<String>,
        /// The other keys.
        #[command(flatten)]
        keys: ConnectionKeys,
    },
    /// Removes a connection, and its keychain item unless another connection names it too.
    Remove {
        /// The connection's name in `config.toml`.
        connection: ConnectionName,
        /// Leaves the keychain item in place.
        #[arg(long)]
        keep_password: bool,
    },
}

/// The keys `add` and `edit` share.
#[derive(Args, Debug)]
pub struct ConnectionKeys {
    /// The command that forwards a local port, `{port}` where the port goes, split as a shell would:
    /// "ssh -N -o 'ProxyCommand ssh -W %h:%p jump' -L 127.0.0.1:{port}:db.example.com:5432 bastion".
    #[arg(long)]
    pub tunnel: Option<String>,
    /// Where the password comes from.
    #[command(flatten)]
    pub password: PasswordFlags,
    /// `allowed` lets statements other than reads run.
    #[arg(long)]
    pub writes: Option<WritePolicy>,
}

/// One place the password comes from; no flag takes the password itself, which `ps` would show.
#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct PasswordFlags {
    /// The environment variable that holds it.
    #[arg(long)]
    pub password_env: Option<String>,
    /// The command that prints it, split as a shell would: "pass show 'shop main'".
    #[arg(long)]
    pub password_command: Option<String>,
    /// The keychain item that holds it; `pgbg password set` saves it there.
    #[arg(long)]
    pub keychain: Option<String>,
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
    fn connection_add_needs_a_tier_and_a_url_and_takes_no_password() {
        let add = ["connection", "add", "prod", "--tier", "production", "--url", "postgres://nina@db.example.com/shop"];
        assert!(parse(&add).is_ok());
        assert!(parse(&add[..5]).is_err());
        assert!(parse(&[&add[..], &["--tier", "qa"]].concat()).is_err());
        assert!(parse(&[&add[..], &["--password", "hunter2"]].concat()).is_err());
    }

    #[test]
    fn a_connection_takes_one_password_source_at_most() {
        let edit = ["connection", "edit", "prod", "--password-env", "PROD_PGPASSWORD"];
        assert!(parse(&edit).is_ok());
        assert!(parse(&[&edit[..], &["--keychain", "pgbg/prod"]].concat()).is_err());
    }

    #[test]
    fn connection_edit_needs_something_to_change() {
        assert!(parse(&["connection", "edit", "prod"]).is_err());
        assert!(parse(&["connection", "edit", "prod", "--writes", "read-only"]).is_ok());
    }

    #[test]
    fn tui_takes_an_optional_connection() {
        assert!(
            matches!(parse(&["tui", "prod"]).unwrap().command, Some(Command::Tui { connection: Some(name) }) if name.to_string() == "prod")
        );
        assert!(matches!(parse(&["tui"]).unwrap().command, Some(Command::Tui { connection: None })));
    }
}
