//! Where a password comes from, and the password once read: never written anywhere, never printed.
use anyhow::{Context, Result, anyhow, bail};
pub(crate) use keychain::{Keychain, KeychainItem};
use serde::{Deserialize, Serialize};
use std::env::VarError;
use std::fmt;
use std::process::Stdio;

mod keychain;
mod typed;

/// A password, read when a session opens and dropped once connected, or typed in the TUI's connection form.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// Adds `c` to a password being typed.
    pub(crate) fn push(&mut self, c: char) {
        self.0.push(c);
    }

    /// Erases the last character typed.
    pub(crate) fn pop(&mut self) {
        self.0.pop();
    }

    /// How many characters the form shows as `•`.
    pub(crate) fn typed_count(&self) -> usize {
        self.0.chars().count()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A pointer to a password: a keychain item, an environment variable, or a command that prints it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SecretSource {
    Keychain(KeychainItem),
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

    /// The password this source holds now; an error never quotes what a command printed.
    pub(crate) async fn read(&self) -> Result<Secret> {
        match self {
            Self::Keychain(account) => Keychain::PGBG.read(account).await,
            Self::Env(variable) => from_variable(variable, std::env::var(variable)),
            Self::Command(argv) => from_command(argv).await,
        }
    }

    /// Why this source cannot name a password, when it cannot.
    pub(crate) fn problem(&self) -> Option<&'static str> {
        match self {
            Self::Env(variable) if variable.is_empty() => Some("`env` names no variable"),
            Self::Command(argv) if argv.is_empty() => Some("`command` is empty"),
            _ => None,
        }
    }
}

fn from_variable(variable: &str, value: Result<String, VarError>) -> Result<Secret> {
    match value {
        Ok(password) if !password.is_empty() => Ok(Secret(password)),
        Ok(_) | Err(VarError::NotPresent) => bail!("the environment variable `{variable}` is not set"),
        Err(VarError::NotUnicode(_)) => bail!("the environment variable `{variable}` is not UTF-8"),
    }
}

async fn from_command(argv: &[String]) -> Result<Secret> {
    let (program, args) = argv.split_first().ok_or_else(|| anyhow!("`command` is empty"))?;
    let output = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("running `{program}` for the password"))?;
    if !output.status.success() {
        bail!("`{program}` failed ({}) instead of printing the password", output.status);
    }
    let printed =
        String::from_utf8(output.stdout).map_err(|_password_bytes| anyhow!("`{program}` printed a password that is not UTF-8"))?;
    Ok(Secret(without_line_end(&printed).to_owned()))
}

/// `text` without the newlines a command or a terminal ends it with.
fn without_line_end(text: &str) -> &str {
    text.trim_end_matches(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> SecretSource {
        toml::from_str::<std::collections::BTreeMap<String, SecretSource>>(text).unwrap().remove("password").unwrap()
    }

    #[test]
    fn each_source_is_spelled_by_its_key() {
        assert_eq!(source("password = { keychain = \"pgbg/prod\" }"), SecretSource::Keychain("pgbg/prod".parse().unwrap()));
        assert_eq!(source("password = { env = \"SHOP_PGPASSWORD\" }").kind(), "env");
        assert_eq!(source("password = { command = [\"pass\", \"shop\"] }").kind(), "command");
    }

    #[tokio::test]
    async fn a_command_source_reads_what_the_command_prints_without_its_newline() {
        let secret = SecretSource::Command(vec!["printf".into(), "hunter2\\n".into()]).read().await.unwrap();
        assert_eq!(secret.expose(), "hunter2");
    }

    #[tokio::test]
    async fn a_command_that_fails_is_an_error() {
        assert!(SecretSource::Command(vec!["false".into()]).read().await.unwrap_err().to_string().contains("`false` failed"));
    }

    #[test]
    fn a_variable_source_reads_its_value() {
        assert_eq!(from_variable("SHOP_PGPASSWORD", Ok("hunter2".into())).unwrap().expose(), "hunter2");
    }

    #[test]
    fn an_unset_or_empty_variable_is_an_error_that_names_it() {
        assert!(from_variable("SHOP_PGPASSWORD", Err(VarError::NotPresent)).unwrap_err().to_string().contains("SHOP_PGPASSWORD"));
        assert!(from_variable("SHOP_PGPASSWORD", Ok(String::new())).is_err());
    }

    #[test]
    fn a_secret_never_shows_in_debug_output() {
        assert_eq!(format!("{:?}", Secret("hunter2".into())), "<redacted>");
    }

    #[test]
    fn a_keychain_item_that_is_not_one_word_is_refused_as_the_config_is_read() {
        for item in ["\"\"", "\"pgbg/prod -s other\""] {
            let text = format!("password = {{ keychain = {item} }}");
            assert!(toml::from_str::<std::collections::BTreeMap<String, SecretSource>>(&text).is_err(), "{item}");
        }
    }

    #[test]
    fn a_source_that_names_nothing_has_a_problem() {
        assert!(source("password = { command = [] }").problem().is_some());
        assert!(source("password = { env = \"\" }").problem().is_some());
        assert_eq!(source("password = { env = \"SHOP_PGPASSWORD\" }").problem(), None);
    }
}
