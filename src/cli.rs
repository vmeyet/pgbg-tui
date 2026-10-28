//! The command line `pgbg` parses, with the help text each flag shows.
use crate::connection::{ConnectionName, StatementTimeout, Target, Tier, Tls, WritePolicy};
use crate::query::params::Assignment;
use clap::builder::TypedValueParser;
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use std::ffi::OsStr;
use std::path::PathBuf;
use vtui::usage::Window;

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
        /// What to run.
        #[command(flatten)]
        statement: StatementArgs,
        /// The value of `{{name}}`, as `name=value`; may be given once per name.
        #[arg(long = "param", value_name = "NAME=VALUE")]
        params: Vec<Assignment>,
        /// A file of `name=value` lines, or `-` for stdin, so a value need not sit in the shell's history.
        #[arg(long, value_name = "FILE")]
        params_file: Option<PathBuf>,
        /// Sends a write without asking; without it a write asks `y/N` at a terminal, and is refused where none can answer.
        #[arg(long)]
        yes: bool,
    },
    /// Checks a connection: its config, TLS, tunnel, login, timeout, and whether its role can write; exits 1 when a check fails.
    Doctor {
        /// The connection's name in `config.toml`.
        connection: ConnectionName,
    },
    /// The statements run on a connection, newest first: when, how long, what came of it; never a row.
    History {
        /// The connection's name in `config.toml`.
        connection: ConnectionName,
        /// Lists this many at most.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// What you use the TUI for, from the `[usage]` counts: the acts never or rarely used, the most used, the time per pane.
    Usage {
        /// How far back to look: `30d`, `4w`, or `all`.
        #[arg(long, default_value = "30d")]
        since: Window,
    },
    /// The tables the connection's role can read, with their columns and indexes.
    Schema {
        /// The connection's name in `config.toml`.
        connection: ConnectionName,
        /// Only the tables whose name, or `schema.table`, holds this text, whatever its case.
        filter: Option<String>,
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
        #[arg(long, value_parser = UrlParser)]
        url: Target,
        /// The other keys.
        #[command(flatten)]
        keys: ConnectionKeys,
    },
    /// Changes the keys given and leaves the others as they are.
    #[command(group(clap::ArgGroup::new("change").required(true).multiple(true)
        .args(["tier", "url", "tunnel", "password_env", "password_command", "keychain", "writes", "tls", "statement_timeout", "queries"])))]
    Edit {
        /// The connection's name in `config.toml`.
        connection: ConnectionName,
        /// Sets the colour and the timeout.
        #[arg(long)]
        tier: Option<Tier>,
        /// `postgres://user@host:port/database`, without a password.
        #[arg(long, value_parser = UrlParser)]
        url: Option<Target>,
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

/// Where `run` takes its statement from: one of the three.
#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct StatementArgs {
    /// One SQL statement; each `{{name}}` in it is bound to the value given for it, never spliced into the text.
    pub sql: Option<String>,
    /// A saved query, by its path under `.pgbg/queries/` or the connection's own queries folder: `orders/daily`.
    #[arg(long, value_name = "PATH")]
    pub query: Option<String>,
    /// A file holding the statement, or `-` for stdin.
    #[arg(long, value_name = "FILE")]
    pub file: Option<PathBuf>,
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
    /// `off` talks to a host that is not this machine in clear, on purpose; `on` asks for TLS whatever `url` says.
    #[arg(long)]
    pub tls: Option<Tls>,
    /// How long the server lets a statement run, such as "2min"; "0s" for no limit. Else the tier's.
    #[arg(long)]
    pub statement_timeout: Option<StatementTimeout>,
    /// Another connection whose saved queries this one uses, as reports shared by several databases.
    #[arg(long, value_name = "CONNECTION")]
    pub queries: Option<ConnectionName>,
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

/// Reads `--url` into a `Target`; unlike clap's own refusal, its refusal never repeats the value, which may hold a password.
#[derive(Clone)]
struct UrlParser;

impl TypedValueParser for UrlParser {
    type Value = Target;

    fn parse_ref(&self, cmd: &clap::Command, _arg: Option<&clap::Arg>, value: &OsStr) -> Result<Target, clap::Error> {
        let refusal =
            |why: &dyn std::fmt::Display| clap::Error::raw(ErrorKind::ValueValidation, format!("in `--url`: {why}\n")).with_cmd(cmd);
        let text = value.to_str().ok_or_else(|| refusal(&"not UTF-8"))?;
        Target::parse(text).map_err(|err| refusal(&format!("{err:#}")))
    }
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
    fn a_url_is_checked_while_parsing() {
        assert!(parse(&["connection", "edit", "prod", "--url", "postgres://nina@db.example.com/shop"]).is_ok());
        assert!(parse(&["connection", "edit", "prod", "--url", "mysql://nina@db.example.com/shop"]).is_err());
    }

    #[test]
    fn a_refused_url_is_never_repeated() {
        let err = parse(&["connection", "edit", "prod", "--url", "postgres://nina:hunter2@db.example.com/shop"]).unwrap_err().to_string();
        assert!(err.contains("`--url`") && err.contains("password") && !err.contains("hunter2"), "{err}");
    }

    #[test]
    fn a_connection_takes_one_password_source_at_most() {
        let edit = ["connection", "edit", "prod", "--password-env", "PROD_PGPASSWORD"];
        assert!(parse(&edit).is_ok());
        assert!(parse(&[&edit[..], &["--keychain", "pgbg/prod"]].concat()).is_err());
    }

    #[test]
    fn a_statement_timeout_is_checked_while_parsing() {
        assert!(parse(&["connection", "edit", "prod", "--statement-timeout", "2min"]).is_ok());
        assert!(parse(&["connection", "edit", "prod", "--statement-timeout", "soon"]).is_err());
    }

    #[test]
    fn connection_edit_needs_something_to_change() {
        assert!(parse(&["connection", "edit", "prod"]).is_err());
        assert!(parse(&["connection", "edit", "prod", "--writes", "read-only"]).is_ok());
    }

    #[test]
    fn run_takes_each_param_as_a_name_and_a_value() {
        let run = parse(&["run", "shop", "SELECT {{day}}", "--param", "day=2024-01-01", "--params-file", "-"]).unwrap();
        assert!(matches!(run.command, Some(Command::Run { params, params_file: Some(_), .. }) if params.len() == 1));
        assert!(parse(&["run", "shop", "SELECT {{day}}", "--param", "day"]).is_err());
    }

    #[test]
    fn run_takes_its_statement_from_one_place_only() {
        assert!(parse(&["run", "shop", "--query", "orders/daily"]).is_ok());
        assert!(parse(&["run", "shop", "--file", "-"]).is_ok());
        assert!(parse(&["run", "shop"]).is_err());
        assert!(parse(&["run", "shop", "SELECT 1", "--query", "orders/daily"]).is_err());
    }

    #[test]
    fn history_lists_twenty_unless_told_otherwise() {
        assert!(matches!(parse(&["history", "shop"]).unwrap().command, Some(Command::History { limit: 20, .. })));
        assert!(matches!(parse(&["history", "shop", "--limit", "3"]).unwrap().command, Some(Command::History { limit: 3, .. })));
    }

    #[test]
    fn usage_looks_back_30_days_unless_told_otherwise() {
        assert!(matches!(parse(&["usage"]).unwrap().command, Some(Command::Usage { since: Window::Days(30) })));
        assert!(matches!(parse(&["usage", "--since", "all"]).unwrap().command, Some(Command::Usage { since: Window::All })));
        assert!(parse(&["usage", "--since", "30"]).is_err());
    }

    #[test]
    fn tui_takes_an_optional_connection() {
        assert!(
            matches!(parse(&["tui", "prod"]).unwrap().command, Some(Command::Tui { connection: Some(name) }) if name.to_string() == "prod")
        );
        assert!(matches!(parse(&["tui"]).unwrap().command, Some(Command::Tui { connection: None })));
    }
}
