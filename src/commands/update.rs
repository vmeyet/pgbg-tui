use super::warn;
use crate::release::{self, PGBG};
use anyhow::Result;
use vtui::update::{Decision, Reinstall, Step};

/// Installs the latest pgbg, with brew when brew installed it, else built with cargo from the repo;
/// `force` installs even when the running one is the latest.
pub fn run(force: bool) -> Result<()> {
    let reinstall = if force { Reinstall::Always } else { Reinstall::WhenBehind };
    match PGBG.update(reinstall, say)? {
        Decision::UpToDate => println!("already the latest pgbg ({})", release::label()),
        Decision::Install => println!("updated: `pgbg --version` shows the one installed"),
    }
    Ok(())
}

fn say(step: Step<'_>) {
    match step {
        Step::CheckFailed(err) => warn(&format!("could not check the latest commit, installing anyway: {err}")),
        Step::Installing(install) => println!("installing the latest pgbg: {install}"),
    }
}
