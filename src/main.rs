//! The `pgbg` binary: parses the command line and runs the matching command.
use anyhow::Result;
use clap::Parser;
use pgbg::cli::{Cli, Command};
use pgbg::commands;

fn main() {
    let cli = Cli::parse();
    pgbg::log::start();
    if let Err(err) = run(&cli) {
        eprintln!("\x1b[31m✗\x1b[0m {err}");
        for cause in err.chain().skip(1) {
            eprintln!("  \x1b[2m{cause}\x1b[0m");
        }
        std::process::exit(1);
    }
}

fn run(cli: &Cli) -> Result<()> {
    match cli.command {
        Command::Connections => commands::connections::run(cli.json),
    }
}
