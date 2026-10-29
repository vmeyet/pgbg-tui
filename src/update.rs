//! Where a newer `pgbg` comes from, and whether the running one is already it; `commands/update.rs` runs what it says.
use crate::program::Program;
use crate::release::Commit;
use std::fmt;
use std::path::Path;

/// The repo `pgbg update` builds from when cargo installed pgbg.
const REPO: &str = env!("CARGO_PKG_REPOSITORY");

/// The Homebrew formula `pgbg update` upgrades when brew installed pgbg.
const FORMULA: &str = "vmeyet/tap/pgbg";

/// Who installed the running binary, and so who updates it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Installer {
    Brew,
    Cargo,
}

impl Installer {
    /// Who installed the binary at `exe`, a path with its links resolved.
    pub(crate) fn of(exe: &Path) -> Self {
        if exe.components().any(|part| part.as_os_str() == "Cellar") { Self::Brew } else { Self::Cargo }
    }

    /// What installs the latest pgbg: `brew upgrade` (`reinstall` when `Always`), or a cargo build from the repo.
    pub(crate) fn install(self, reinstall: Reinstall) -> Call {
        match (self, reinstall) {
            (Self::Brew, Reinstall::WhenBehind) => Call::new("brew", &["upgrade", FORMULA]),
            (Self::Brew, Reinstall::Always) => Call::new("brew", &["reinstall", FORMULA]),
            (Self::Cargo, _) => Call::new("cargo", &["install", "--git", REPO, "--locked", "--force"]),
        }
    }

    /// Whether to compare the running commit to the repo's latest before installing.
    /// brew ships tagged releases only, so the latest commit says nothing about what brew would install.
    pub(crate) fn checks_the_repo(self, reinstall: Reinstall) -> bool {
        self == Self::Cargo && reinstall == Reinstall::WhenBehind
    }
}

/// Whether `pgbg update` installs over the latest version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reinstall {
    WhenBehind,
    Always,
}

/// What `pgbg update` does once it knows the latest commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Install,
    UpToDate,
}

/// Only a running commit known to be the latest spares the install; anything unanswered installs.
pub(crate) fn decide(running: Option<&Commit>, latest: Option<&Commit>) -> Decision {
    match (running, latest) {
        (Some(running), Some(latest)) if running == latest => Decision::UpToDate,
        _ => Decision::Install,
    }
}

/// What asks the repo for its latest commit; its output reads through [`latest_in`].
pub(crate) fn latest_check() -> Call {
    Call::new("git", &["ls-remote", REPO, "HEAD"])
}

/// The commit `git ls-remote <repo> HEAD` printed, as `<hash>\tHEAD`.
pub(crate) fn latest_in(listing: &str) -> Option<Commit> {
    listing.split_whitespace().next().and_then(Commit::parse)
}

/// A program and its arguments, run by `commands/update.rs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Call {
    pub(crate) program: Program,
    pub(crate) args: &'static [&'static str],
}

impl Call {
    fn new(program: &str, args: &'static [&'static str]) -> Self {
        Self { program: Program::new(program), args }
    }
}

/// The command line as typed in a shell: `brew upgrade vmeyet/tap/pgbg`.
impl fmt::Display for Call {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.program, self.args.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{NEWER, RUNNING, commit};

    #[test]
    fn a_binary_in_the_brew_cellar_is_installed_by_brew() {
        assert_eq!(Installer::of(Path::new("/opt/homebrew/Cellar/pgbg/0.1.0/bin/pgbg")), Installer::Brew);
    }

    #[test]
    fn a_binary_built_by_cargo_is_installed_by_cargo() {
        assert_eq!(Installer::of(Path::new("/Users/nina/.cargo/bin/pgbg")), Installer::Cargo);
    }

    #[test]
    fn brew_upgrades_the_formula() {
        assert_eq!(Installer::Brew.install(Reinstall::WhenBehind).to_string(), "brew upgrade vmeyet/tap/pgbg");
    }

    #[test]
    fn brew_reinstalls_the_formula_when_asked_to_always_install() {
        assert_eq!(Installer::Brew.install(Reinstall::Always).to_string(), "brew reinstall vmeyet/tap/pgbg");
    }

    #[test]
    fn cargo_builds_the_repo_with_its_lockfile_over_the_installed_binary() {
        assert_eq!(
            Installer::Cargo.install(Reinstall::WhenBehind).to_string(),
            "cargo install --git https://github.com/vmeyet/pgbg-tui --locked --force"
        );
    }

    #[test]
    fn a_cargo_install_checks_the_repo_first() {
        assert!(Installer::Cargo.checks_the_repo(Reinstall::WhenBehind));
    }

    #[test]
    fn a_brew_install_never_checks_the_repo() {
        assert!(!Installer::Brew.checks_the_repo(Reinstall::WhenBehind));
    }

    #[test]
    fn installing_always_skips_the_check() {
        assert!(!Installer::Cargo.checks_the_repo(Reinstall::Always));
    }

    #[test]
    fn the_same_commit_needs_no_install() {
        assert_eq!(decide(Some(&commit(RUNNING)), Some(&commit(RUNNING))), Decision::UpToDate);
    }

    #[test]
    fn a_newer_commit_installs() {
        assert_eq!(decide(Some(&commit(RUNNING)), Some(&commit(NEWER))), Decision::Install);
    }

    #[test]
    fn an_unanswered_check_installs() {
        assert_eq!(decide(Some(&commit(RUNNING)), None), Decision::Install);
    }

    #[test]
    fn a_binary_built_without_git_history_installs() {
        assert_eq!(decide(None, Some(&commit(NEWER))), Decision::Install);
    }

    #[test]
    fn the_latest_commit_is_the_first_word_of_the_listing() {
        assert_eq!(latest_in(&format!("{NEWER}\tHEAD\n")), Some(commit(NEWER)));
    }

    #[test]
    fn an_empty_listing_has_no_latest_commit() {
        assert_eq!(latest_in(""), None);
    }
}
