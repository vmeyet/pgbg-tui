//! The release pgbg runs: its number and the commit it was built from.
use std::fmt;
use std::sync::LazyLock;

/// A git commit, as its full hash of 40 hex characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Commit(String);

impl Commit {
    /// The commit this binary was built from; `None` when it was built without git history.
    pub(crate) fn running() -> Option<Self> {
        Self::parse(env!("GIT_HASH"))
    }

    /// `text` as a commit, when it is a full hash.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        (text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit())).then(|| Self(text.to_owned()))
    }

    /// The first seven characters, as git shows a commit.
    fn short(&self) -> &str {
        &self.0[..7]
    }
}

impl fmt::Display for Commit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `0.1.0 (a1b2c3d)`, as `pgbg --version` shows it; the number alone when built without git history.
pub(crate) fn label() -> &'static str {
    static LABEL: LazyLock<String> = LazyLock::new(|| labelled(env!("CARGO_PKG_VERSION"), Commit::running().as_ref()));
    &LABEL
}

fn labelled(number: &str, commit: Option<&Commit>) -> String {
    match commit {
        Some(commit) => format!("{number} ({})", commit.short()),
        None => number.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::RUNNING;

    #[test]
    fn a_full_hash_is_a_commit() {
        assert_eq!(Commit::parse(RUNNING).map(|c| c.to_string()), Some(RUNNING.to_owned()));
    }

    #[test]
    fn an_empty_or_short_hash_is_not_a_commit() {
        assert_eq!(Commit::parse(""), None);
        assert_eq!(Commit::parse("9731436"), None);
    }

    #[test]
    fn a_hash_that_is_not_hex_is_not_a_commit() {
        assert_eq!(Commit::parse(&"z".repeat(40)), None);
    }

    #[test]
    fn the_label_shows_the_short_commit_after_the_number() {
        assert_eq!(labelled("0.1.0", Commit::parse(RUNNING).as_ref()), "0.1.0 (9731436)");
    }

    #[test]
    fn the_label_is_the_number_alone_without_a_commit() {
        assert_eq!(labelled("0.1.0", None), "0.1.0");
    }
}
