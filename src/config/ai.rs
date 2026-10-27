//! The `[ai]` table: the command `ctrl-g` asks for SQL, and what reaches it.
use crate::ai::LiteralPolicy;
use crate::connection::Connection;
use crate::secret::SecretSource;
use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::time::Duration;

/// The variables every command gets, when pgbg has them.
const BASE_VARIABLES: [&str; 4] = ["PATH", "HOME", "LANG", "TERM"];
const DEFAULT_TIMEOUT_SECONDS: u64 = 60;

/// The `[ai]` table, before it is checked.
#[derive(Default, Deserialize)]
pub(super) struct AiTable {
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default)]
    env: Vec<String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    literals: LiteralPolicy,
}

/// How to ask: the program and its arguments, never run through a shell, the variables it gets, how long it has, and
/// whether the statement's literals reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AiSettings {
    pub(crate) command: Vec<String>,
    /// The names of the variables it gets: `PATH`, `HOME`, `LANG`, `TERM` and `[ai] env`, none a password names.
    pub(crate) env: Vec<String>,
    pub(crate) timeout: Duration,
    pub(crate) literals: LiteralPolicy,
}

/// The `[ai]` table checked, with a warning for each variable it lists that a password names.
pub(super) struct ReadAi {
    /// None when it names no command: asking is off.
    pub(super) settings: Option<AiSettings>,
    pub(super) warnings: Vec<String>,
}

/// The settings `table` gives, given `connections`, whose password variables never reach the command.
pub(super) fn read(table: AiTable, connections: &[Connection]) -> Result<ReadAi> {
    let AiTable { command, env, timeout, literals } = table;
    let Some(command) = command else { return Ok(ReadAi { settings: None, warnings: vec![] }) };
    if command.first().is_none_or(String::is_empty) {
        bail!("`[ai] command` names no program: give it as a list, such as [\"llm\", \"-m\", \"local\"]");
    }
    let timeout = match timeout {
        Some(0) => bail!("`[ai] timeout` is a number of seconds above 0"),
        seconds => Duration::from_secs(seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS)),
    };
    let passwords = password_variables(connections);
    let warnings = env.iter().filter(|name| passwords.contains(name.as_str())).map(|name| password_warning(name)).collect();
    let env = passed_variables(env, &passwords);
    Ok(ReadAi { settings: Some(AiSettings { command, env, timeout, literals }), warnings })
}

fn password_variables(connections: &[Connection]) -> BTreeSet<&str> {
    connections
        .iter()
        .filter_map(|connection| match connection.password() {
            Some(SecretSource::Env(variable)) => Some(variable.as_str()),
            _ => None,
        })
        .collect()
}

/// The base variables then those `listed`, each once, without those `passwords` holds.
fn passed_variables(listed: Vec<String>, passwords: &BTreeSet<&str>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    BASE_VARIABLES
        .iter()
        .map(|&name| name.to_owned())
        .chain(listed)
        .filter(|name| !passwords.contains(name.as_str()) && seen.insert(name.clone()))
        .collect()
}

fn password_warning(name: &str) -> String {
    format!("`[ai] env` lists `{name}`, which a connection's password names: the command never gets it")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::named;

    fn settings(table: &str, connections: &[Connection]) -> Result<ReadAi> {
        read(toml::from_str(table)?, connections)
    }

    fn shop_with_password_in(variable: &str) -> Connection {
        named("shop", &format!("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\npassword = {{ env = \"{variable}\" }}\n"))
    }

    #[test]
    fn no_command_turns_asking_off() {
        assert_eq!(settings("literals = \"sent\"", &[]).unwrap().settings, None);
    }

    #[test]
    fn a_command_gets_the_base_variables_and_those_listed_60_s_and_masked_literals() {
        let read = settings("command = [\"llm\", \"-m\", \"local\"]\nenv = [\"OPENAI_API_KEY\"]", &[]).unwrap();
        let expected = AiSettings {
            command: vec!["llm".into(), "-m".into(), "local".into()],
            env: ["PATH", "HOME", "LANG", "TERM", "OPENAI_API_KEY"].map(str::to_owned).to_vec(),
            timeout: Duration::from_secs(60),
            literals: LiteralPolicy::Masked,
        };
        assert_eq!((read.settings, read.warnings.len()), (Some(expected), 0));
    }

    #[test]
    fn the_timeout_and_the_literals_may_be_set() {
        let read = settings("command = [\"llm\"]\ntimeout = 20\nliterals = \"sent\"", &[]).unwrap().settings.unwrap();
        assert_eq!((read.timeout, read.literals), (Duration::from_secs(20), LiteralPolicy::Sent));
    }

    #[test]
    fn a_variable_a_password_names_never_reaches_the_command_even_when_listed() {
        let read =
            settings("command = [\"llm\"]\nenv = [\"SHOP_PGPASSWORD\", \"OPENAI_API_KEY\"]", &[shop_with_password_in("SHOP_PGPASSWORD")]);
        let read = read.unwrap();
        assert!(!read.settings.unwrap().env.contains(&"SHOP_PGPASSWORD".to_owned()));
        assert!(read.warnings[0].contains("SHOP_PGPASSWORD"), "{:?}", read.warnings);
    }

    #[test]
    fn a_variable_listed_twice_is_passed_once() {
        let read = settings("command = [\"llm\"]\nenv = [\"PATH\", \"OPENAI_API_KEY\", \"OPENAI_API_KEY\"]", &[]).unwrap();
        assert_eq!(read.settings.unwrap().env, ["PATH", "HOME", "LANG", "TERM", "OPENAI_API_KEY"]);
    }

    #[test]
    fn a_base_variable_a_password_names_is_dropped_too() {
        let read = settings("command = [\"llm\"]", &[shop_with_password_in("HOME")]).unwrap().settings.unwrap();
        assert_eq!(read.env, ["PATH", "LANG", "TERM"]);
    }

    #[test]
    fn a_command_with_no_program_is_refused() {
        assert!(settings("command = []", &[]).is_err());
        assert!(settings("command = [\"\"]", &[]).is_err());
    }

    #[test]
    fn a_command_as_one_string_is_refused() {
        assert!(settings("command = \"llm -m local\"", &[]).is_err());
    }

    #[test]
    fn a_zero_timeout_is_refused() {
        assert!(settings("command = [\"llm\"]\ntimeout = 0", &[]).is_err());
    }

    #[test]
    fn an_unknown_literal_policy_is_refused() {
        assert!(settings("command = [\"llm\"]\nliterals = \"some\"", &[]).is_err());
    }
}
