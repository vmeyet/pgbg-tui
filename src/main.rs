//! The `pgbg` binary: parses the command line and runs the matching command.
use anyhow::Result;
use clap::Parser;
use pgbg::cli::{Cli, Command, ConnectionCommand, PasswordCommand};
use pgbg::commands;
use pgbg::signal::{Stopped, until_stopped};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    pgbg::log::start();
    match run(&cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => exit_code_of(&err),
    }
}

/// A signal's code, quietly, since the shell says it; any other error is printed.
fn exit_code_of(err: &anyhow::Error) -> ExitCode {
    if let Some(stopped) = err.downcast_ref::<Stopped>() {
        return stopped.exit_code();
    }
    commands::print_error(err);
    ExitCode::FAILURE
}

async fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        None => commands::tui::run(None).await,
        Some(Command::Tui { connection }) => commands::tui::run(connection.as_ref()).await,
        Some(Command::Connections) => commands::connections::run(cli.json),
        Some(Command::Run { connection, sql }) => until_stopped(commands::run::run(connection, sql, cli.json)).await,
        Some(Command::Doctor { connection }) => until_stopped(commands::doctor::run(connection, cli.json)).await,
        Some(Command::History { connection, limit }) => commands::history::run(connection, *limit, cli.json),
        Some(Command::Schema { connection, filter }) => until_stopped(commands::schema::run(connection, filter.as_deref(), cli.json)).await,
        Some(Command::Connection { command }) => connection(command),
        Some(Command::Password { command: PasswordCommand::Set { connection } }) => commands::password::set(connection).await,
    }
}

fn connection(command: &ConnectionCommand) -> Result<()> {
    match command {
        ConnectionCommand::Add { connection, tier, url, keys } => commands::connection::add(connection, *tier, url, keys),
        ConnectionCommand::Edit { connection, tier, url, keys } => commands::connection::edit(connection, *tier, url.as_deref(), keys),
        ConnectionCommand::Remove { connection, keep_password } => commands::connection::remove(connection, *keep_password),
    }
}
