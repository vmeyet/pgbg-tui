//! The `pgbg` binary: parses the command line and runs the matching command.
use anyhow::Result;
use clap::Parser;
use pgbg::cli::{Cli, Command, ConnectionCommand, PasswordCommand};
use pgbg::commands;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    pgbg::log::start();
    if let Err(err) = run(&cli).await {
        eprintln!("\x1b[31m✗\x1b[0m {err}");
        for cause in err.chain().skip(1) {
            eprintln!("  \x1b[2m{cause}\x1b[0m");
        }
        std::process::exit(1);
    }
}

async fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        None => commands::tui::run(None).await,
        Some(Command::Tui { connection }) => commands::tui::run(connection.as_ref()).await,
        Some(Command::Connections) => commands::connections::run(cli.json),
        Some(Command::Run { connection, sql }) => commands::run::run(connection, sql, cli.json).await,
        Some(Command::Connection { command }) => connection(command),
        Some(Command::Password { command: PasswordCommand::Set { connection } }) => commands::password::set(connection),
    }
}

fn connection(command: &ConnectionCommand) -> Result<()> {
    match command {
        ConnectionCommand::Add { connection, tier, url, keys } => commands::connection::add(connection, tier, url, keys),
        ConnectionCommand::Edit { connection, tier, url, keys } => {
            commands::connection::edit(connection, tier.as_deref(), url.as_deref(), keys)
        }
        ConnectionCommand::Remove { connection, keep_password } => commands::connection::remove(connection, *keep_password),
    }
}
