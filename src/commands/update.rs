use super::warn;
use crate::release::{self, Commit};
use crate::update::{self, Call, Decision, Installer, Reinstall};
use anyhow::{Context, Result, bail};
use std::process::Command;

/// Installs the latest pgbg, with brew when brew installed it, else built with cargo from the repo;
/// `force` installs even when the running one is the latest.
pub fn run(force: bool) -> Result<()> {
    let reinstall = if force { Reinstall::Always } else { Reinstall::WhenBehind };
    match update(&mut System, running_installer(), Commit::running().as_ref(), reinstall)? {
        Decision::UpToDate => println!("already the latest pgbg ({})", release::label()),
        Decision::Install => println!("updated: `pgbg --version` shows the one installed"),
    }
    Ok(())
}

/// Installs the latest pgbg through `runner`, unless `running` is already it, and says which it did.
fn update(runner: &mut impl Runner, installer: Installer, running: Option<&Commit>, reinstall: Reinstall) -> Result<Decision> {
    let decision = if installer.checks_the_repo(reinstall) { update::decide(running, latest(runner).as_ref()) } else { Decision::Install };
    if decision == Decision::Install {
        let install = installer.install(reinstall);
        println!("installing the latest pgbg: {install}");
        runner.run(&install)?;
    }
    Ok(decision)
}

/// The repo's latest commit; `None` with a warning when it cannot be asked, and the update installs anyway.
fn latest(runner: &mut impl Runner) -> Option<Commit> {
    match runner.read(&update::latest_check()) {
        Ok(listing) => update::latest_in(&listing),
        Err(err) => {
            warn(&format!("could not check the latest commit, installing anyway: {err:#}"));
            None
        }
    }
}

/// Who installed the running binary; cargo when its path cannot be read, since cargo can always build pgbg again.
fn running_installer() -> Installer {
    std::env::current_exe().and_then(std::fs::canonicalize).map_or(Installer::Cargo, |exe| Installer::of(&exe))
}

/// Runs the programs an update needs; tests answer in their place, so no test runs brew, cargo or git.
trait Runner {
    /// Runs `call` on the terminal, and fails when it does.
    fn run(&mut self, call: &Call) -> Result<()>;
    /// Runs `call` and returns what it printed.
    fn read(&mut self, call: &Call) -> Result<String>;
}

/// The programs themselves.
#[derive(Debug)]
struct System;

impl Runner for System {
    fn run(&mut self, call: &Call) -> Result<()> {
        let status = command(call).status().with_context(|| format!("could not run {}", call.program))?;
        if !status.success() {
            bail!("`{call}` failed");
        }
        Ok(())
    }

    fn read(&mut self, call: &Call) -> Result<String> {
        let output = command(call).output().with_context(|| format!("could not run {}", call.program))?;
        if !output.status.success() {
            bail!("`{call}` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8(output.stdout)?)
    }
}

fn command(call: &Call) -> Command {
    let mut command = Command::new(&call.program);
    // git, and cargo through it, would otherwise wait on a credentials prompt.
    command.args(call.args).env("GIT_TERMINAL_PROMPT", "0");
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    use crate::fixtures::{NEWER, RUNNING, commit};

    /// Answers the repo check with `listing`, or fails it when `None`, and keeps every call in `ran`.
    #[derive(Debug, Default)]
    struct Fake {
        listing: Option<String>,
        install_fails: bool,
        ran: Vec<String>,
    }

    impl Fake {
        fn answering(listing: &str) -> Self {
            Self { listing: Some(format!("{listing}\tHEAD\n")), ..Self::default() }
        }
    }

    impl Runner for Fake {
        fn run(&mut self, call: &Call) -> Result<()> {
            self.ran.push(call.to_string());
            if self.install_fails { Err(anyhow!("`{call}` failed")) } else { Ok(()) }
        }

        fn read(&mut self, call: &Call) -> Result<String> {
            self.ran.push(call.to_string());
            self.listing.clone().ok_or_else(|| anyhow!("offline"))
        }
    }

    const CHECK: &str = "git ls-remote https://github.com/vmeyet/pgbg-tui HEAD";
    const CARGO_INSTALL: &str = "cargo install --git https://github.com/vmeyet/pgbg-tui --locked --force";

    #[test]
    fn a_cargo_install_on_the_latest_commit_installs_nothing() {
        let mut fake = Fake::answering(RUNNING);
        let decision = update(&mut fake, Installer::Cargo, Some(&commit(RUNNING)), Reinstall::WhenBehind).unwrap();
        assert_eq!((decision, fake.ran), (Decision::UpToDate, vec![CHECK.to_owned()]));
    }

    #[test]
    fn a_cargo_install_behind_the_repo_builds_it() {
        let mut fake = Fake::answering(NEWER);
        let decision = update(&mut fake, Installer::Cargo, Some(&commit(RUNNING)), Reinstall::WhenBehind).unwrap();
        assert_eq!((decision, fake.ran), (Decision::Install, vec![CHECK.to_owned(), CARGO_INSTALL.to_owned()]));
    }

    #[test]
    fn a_failed_check_installs_anyway() {
        let mut fake = Fake::default();
        let decision = update(&mut fake, Installer::Cargo, Some(&commit(RUNNING)), Reinstall::WhenBehind).unwrap();
        assert_eq!((decision, fake.ran), (Decision::Install, vec![CHECK.to_owned(), CARGO_INSTALL.to_owned()]));
    }

    #[test]
    fn force_builds_without_asking_the_repo() {
        let mut fake = Fake::answering(RUNNING);
        update(&mut fake, Installer::Cargo, Some(&commit(RUNNING)), Reinstall::Always).unwrap();
        assert_eq!(fake.ran, [CARGO_INSTALL]);
    }

    #[test]
    fn a_brew_install_upgrades_without_asking_the_repo() {
        let mut fake = Fake::answering(RUNNING);
        update(&mut fake, Installer::Brew, Some(&commit(RUNNING)), Reinstall::WhenBehind).unwrap();
        assert_eq!(fake.ran, ["brew upgrade vmeyet/tap/pgbg"]);
    }

    #[test]
    fn a_failed_install_fails_the_update() {
        let mut fake = Fake { install_fails: true, ..Fake::answering(NEWER) };
        assert!(update(&mut fake, Installer::Cargo, Some(&commit(RUNNING)), Reinstall::WhenBehind).is_err());
    }
}
