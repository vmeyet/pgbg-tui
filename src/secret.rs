//! Where a password comes from; the password itself is never read here yet, nor ever written anywhere.
use serde::{Deserialize, Serialize};

/// A pointer to a password: a keychain item, an environment variable, or a command that prints it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SecretSource {
    Keychain(String),
    Env(String),
    Command(Vec<String>),
}

impl SecretSource {
    /// `keychain`, `env` or `command`, as the config spells it.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Keychain(_) => "keychain",
            Self::Env(_) => "env",
            Self::Command(_) => "command",
        }
    }

    /// Why this source cannot name a password, when it cannot.
    pub(crate) fn problem(&self) -> Option<&'static str> {
        match self {
            Self::Keychain(item) if item.is_empty() => Some("`keychain` names no item"),
            Self::Env(variable) if variable.is_empty() => Some("`env` names no variable"),
            Self::Command(argv) if argv.is_empty() => Some("`command` is empty"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> SecretSource {
        toml::from_str::<std::collections::BTreeMap<String, SecretSource>>(text).unwrap().remove("password").unwrap()
    }

    #[test]
    fn each_source_is_spelled_by_its_key() {
        assert_eq!(source("password = { keychain = \"pgbg/prod\" }"), SecretSource::Keychain("pgbg/prod".into()));
        assert_eq!(source("password = { env = \"SHOP_PGPASSWORD\" }").kind(), "env");
        assert_eq!(source("password = { command = [\"pass\", \"shop\"] }").kind(), "command");
    }

    #[test]
    fn a_source_that_names_nothing_has_a_problem() {
        assert!(source("password = { command = [] }").problem().is_some());
        assert!(source("password = { keychain = \"\" }").problem().is_some());
        assert!(source("password = { env = \"\" }").problem().is_some());
        assert_eq!(source("password = { env = \"SHOP_PGPASSWORD\" }").problem(), None);
    }
}
