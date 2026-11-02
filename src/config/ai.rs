//! The `[ai]` table: the command `ctrl-g` asks for SQL, the one the idle ghost asks, and what reaches them.
use crate::ai::{LiteralPolicy, ServerErrors};
use crate::connection::{Connection, TunnelSpec};
use crate::secret::SecretSource;
use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;
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
    ghost_command: Option<Vec<String>>,
    #[serde(default)]
    env: Vec<String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    literals: LiteralPolicy,
    #[serde(default)]
    errors: ServerErrors,
    #[serde(default)]
    ghost: bool,
}

/// How to ask: the program and its arguments, never run through a shell, the variables it gets, how long it has,
/// whether the statement's literals reach it, and whether the editor asks it on its own once idle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AiSettings {
    pub(crate) command: Vec<String>,
    /// The names of the variables it gets: `PATH`, `HOME`, `LANG`, `TERM` and `[ai] env`, none a password names.
    pub(crate) env: Vec<String>,
    pub(crate) timeout: Duration,
    pub(crate) literals: LiteralPolicy,
    /// Whether an ask for a fix may tell of the server's error: off unless `errors = "code"`.
    pub(crate) errors: ServerErrors,
    /// Whether a second without a key asks for the rest of the statement under the cursor: off unless `ghost = true`.
    pub(crate) ghost: bool,
    /// What the idle ghost runs in place of `command`, when set; `ghost` alone decides whether it runs.
    pub(crate) ghost_command: Option<Vec<String>>,
}

/// The `[ai]` table checked, with a warning for each variable it lists that a password names.
pub(super) struct ReadAi {
    /// None when it names no command: asking is off.
    pub(super) settings: Option<AiSettings>,
    pub(super) warnings: Vec<String>,
}

/// The settings `table` gives, given `connections`, whose password variables never reach the command.
pub(super) fn read(table: AiTable, connections: &[Connection]) -> Result<ReadAi> {
    let AiTable { command, ghost_command, env, timeout, literals, errors, ghost } = table;
    let ghost_command = checked_command(ghost_command, "ghost_command")?;
    let Some(command) = checked_command(command, "command")? else { return Ok(ReadAi { settings: None, warnings: vec![] }) };
    let timeout = match timeout {
        Some(0) => bail!("`[ai] timeout` is a number of seconds above 0"),
        seconds => Duration::from_secs(seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS)),
    };
    let passwords = password_variables(connections);
    let arguments = command_arguments(connections);
    let warnings = env.iter().filter_map(|name| warning(name, &passwords, &arguments)).collect();
    let env = passed_variables(env, &passwords);
    Ok(ReadAi { settings: Some(AiSettings { command, env, timeout, literals, errors, ghost, ghost_command }), warnings })
}

/// `command` as the `[ai]` table's `key` gives it, refused when it names no program.
fn checked_command(command: Option<Vec<String>>, key: &str) -> Result<Option<Vec<String>>> {
    match command {
        Some(argv) if argv.first().is_none_or(String::is_empty) => {
            bail!("`[ai] {key}` names no program: give it as a list, such as [\"llm\", \"-m\", \"local\"]")
        }
        checked => Ok(checked),
    }
}

impl AiSettings {
    /// These settings without the variables a password of `connections` names: a connection added or changed since the
    /// config loaded may name one.
    pub(crate) fn without_passwords_of(&self, connections: &[Connection]) -> Self {
        let passwords = password_variables(connections);
        let env = self.env.iter().filter(|name| !passwords.contains(name.as_str())).cloned().collect();
        Self { env, ..self.clone() }
    }

    /// These settings with the command the idle ghost asks in place of `command`.
    pub(crate) fn for_idle_ghost(&self) -> Self {
        Self { command: self.idle_ghost_command().to_vec(), ..self.clone() }
    }

    /// The program's file name, `claude` for `/opt/bin/claude`, as the footer names it while it runs.
    pub(crate) fn program_name(&self) -> &str {
        file_name_of(&self.command)
    }

    /// As [`Self::program_name`], for the program the idle ghost asks.
    pub(crate) fn idle_ghost_program_name(&self) -> &str {
        file_name_of(self.idle_ghost_command())
    }

    fn idle_ghost_command(&self) -> &[String] {
        self.ghost_command.as_deref().unwrap_or(&self.command)
    }
}

fn file_name_of(command: &[String]) -> &str {
    let program = command.first().map_or("", String::as_str);
    Path::new(program).file_name().and_then(|name| name.to_str()).unwrap_or(program)
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

/// The arguments of the connections' password commands and tunnels, which may read a variable `[ai] env` passes on.
fn command_arguments(connections: &[Connection]) -> Vec<&str> {
    connections.iter().flat_map(commands_of).flatten().map(String::as_str).collect()
}

fn commands_of(connection: &Connection) -> [&[String]; 3] {
    let password_command = match connection.password() {
        Some(SecretSource::Command(argv)) => argv.as_slice(),
        _ => &[],
    };
    let tunnel = connection.tunnel();
    [password_command, tunnel.map(TunnelSpec::command).unwrap_or_default(), tunnel.and_then(TunnelSpec::before).unwrap_or_default()]
}

/// Why `name` in `[ai] env` is worth a warning: a password names it, so it is left out, or a command's `arguments`
/// name it, which pgbg cannot tell is a secret, so it is passed.
fn warning(name: &str, passwords: &BTreeSet<&str>, arguments: &[&str]) -> Option<String> {
    let is_word_of = |argument: &&str| argument.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|word| word == name);
    match () {
        () if passwords.contains(name) => Some(password_warning(name)),
        () if arguments.iter().any(is_word_of) => Some(command_warning(name)),
        () => None,
    }
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

fn command_warning(name: &str) -> String {
    format!("`[ai] env` lists `{name}`, which a connection's password command or tunnel names: the command gets it all the same")
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
            errors: ServerErrors::Off,
            ghost: false,
            ghost_command: None,
        };
        assert_eq!((read.settings, read.warnings.len()), (Some(expected), 0));
    }

    #[test]
    fn errors_code_is_read_and_any_other_word_refused() {
        let read = settings("command = [\"llm\"]\nerrors = \"code\"", &[]).unwrap().settings.unwrap();
        assert_eq!(read.errors, ServerErrors::Code);
        assert!(settings("command = [\"llm\"]\nerrors = \"words\"", &[]).is_err());
    }

    #[test]
    fn the_program_name_is_the_file_name_of_the_program() {
        let read = settings("command = [\"/opt/bin/claude\", \"-p\"]", &[]).unwrap().settings.unwrap();
        assert_eq!(read.program_name(), "claude");
    }

    #[test]
    fn the_timeout_and_the_literals_may_be_set() {
        let read = settings("command = [\"llm\"]\ntimeout = 20\nliterals = \"sent\"", &[]).unwrap().settings.unwrap();
        assert_eq!((read.timeout, read.literals), (Duration::from_secs(20), LiteralPolicy::Sent));
    }

    #[test]
    fn the_idle_ghost_may_be_turned_on() {
        assert!(settings("command = [\"llm\"]\nghost = true", &[]).unwrap().settings.unwrap().ghost);
    }

    #[test]
    fn with_no_ghost_command_the_idle_ghost_asks_the_command() {
        let read = settings("command = [\"/opt/bin/llm\"]", &[]).unwrap().settings.unwrap();
        assert_eq!(read.for_idle_ghost(), read);
        assert_eq!(read.idle_ghost_program_name(), "llm");
    }

    #[test]
    fn a_ghost_command_is_what_the_idle_ghost_asks_and_only_it() {
        let read = settings("command = [\"llm\"]\nghost_command = [\"/opt/bin/fast\", \"-q\"]", &[]).unwrap().settings.unwrap();
        assert_eq!(read.for_idle_ghost().command, ["/opt/bin/fast", "-q"]);
        assert_eq!((read.idle_ghost_program_name(), read.program_name()), ("fast", "llm"));
    }

    #[test]
    fn a_ghost_command_with_no_program_is_refused() {
        assert!(settings("command = [\"llm\"]\nghost_command = []", &[]).is_err());
        assert!(settings("command = [\"llm\"]\nghost_command = [\"\"]", &[]).is_err());
    }

    #[test]
    fn a_ghost_command_with_no_program_is_refused_without_a_command_too() {
        assert!(settings("ghost_command = []", &[]).is_err());
    }

    #[test]
    fn a_ghost_command_as_one_string_is_refused() {
        assert!(settings("command = [\"llm\"]\nghost_command = \"fast -q\"", &[]).is_err());
    }

    #[test]
    fn a_ghost_command_alone_leaves_asking_off() {
        assert_eq!(settings("ghost_command = [\"fast\"]\nghost = true", &[]).unwrap().settings, None);
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
    fn a_variable_a_password_command_names_is_passed_with_a_warning() {
        let shop = named(
            "shop",
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\npassword = { command = [\"sh\", \"-c\", \"echo $SHOP_TOKEN\"] }\n",
        );
        let read = settings("command = [\"llm\"]\nenv = [\"SHOP_TOKEN\"]", &[shop]).unwrap();
        assert!(read.settings.unwrap().env.contains(&"SHOP_TOKEN".to_owned()));
        assert!(read.warnings[0].contains("SHOP_TOKEN"), "{:?}", read.warnings);
    }

    #[test]
    fn a_variable_a_tunnel_names_is_passed_with_a_warning() {
        let tunnel = "tunnel = { command = [\"proxy\", \"{port}\"], before = [\"login\", \"--key=${PROXY_KEY}\"] }\n";
        let shop = named("shop", &format!("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n{tunnel}"));
        let read = settings("command = [\"llm\"]\nenv = [\"PROXY_KEY\"]", &[shop]).unwrap();
        assert!(read.settings.unwrap().env.contains(&"PROXY_KEY".to_owned()));
        assert!(read.warnings[0].contains("PROXY_KEY"), "{:?}", read.warnings);
    }

    #[test]
    fn a_variable_only_part_of_a_word_in_a_command_is_passed_without_a_warning() {
        let shop = named(
            "shop",
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\npassword = { command = [\"pass\", \"SHOP_TOKEN_OLD\"] }\n",
        );
        let read = settings("command = [\"llm\"]\nenv = [\"SHOP_TOKEN\"]", &[shop]).unwrap();
        assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    }

    #[test]
    fn a_variable_a_password_names_since_the_config_loaded_is_left_out() {
        let read = settings("command = [\"llm\"]\nenv = [\"SHOP_PGPASSWORD\", \"OPENAI_API_KEY\"]", &[]).unwrap().settings.unwrap();
        let now = read.without_passwords_of(&[shop_with_password_in("SHOP_PGPASSWORD")]);
        assert_eq!(now.env, ["PATH", "HOME", "LANG", "TERM", "OPENAI_API_KEY"]);
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
