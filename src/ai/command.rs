//! Runs the `[ai] command` on a prompt: from an empty temp folder, with only the variables `[ai]` passes, in a process
//! group of its own, its stdout cut at 64 KiB and its time at `[ai] timeout`.
use crate::config::AiSettings;
use crate::groups::{self, Exchanged, with_last_words};
use crate::program::Program;
use std::fmt;
use std::io;
use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::{Instant, error::Elapsed, timeout};

/// The most a reply holds: past it, the command is stopped.
const MAX_REPLY_BYTES: usize = 64 * 1024;

/// An ask running in a task of its own: dropped, the command's whole process group is killed.
#[derive(Debug)]
pub(crate) struct Asking {
    task: JoinHandle<Result<String, CommandError>>,
}

/// Why an ask brought back no reply.
#[derive(Debug)]
pub(crate) enum CommandError {
    /// The command could not start, or its pipes or its temp folder failed.
    SpawnFailed(io::Error),
    TimedOut(Duration),
    /// Its stdout passed 64 KiB.
    TooLong,
    /// It exited with another status than 0.
    Failed {
        status: ExitStatus,
        last_words: Vec<String>,
    },
    /// The ask was cancelled.
    Killed,
}

impl Asking {
    /// Asks the command of `settings` with `prompt` on its stdin.
    pub(crate) fn start(settings: &AiSettings, prompt: &str) -> Self {
        let (settings, prompt) = (settings.clone(), prompt.to_owned());
        Self { task: tokio::spawn(async move { ask(&settings, &prompt).await }) }
    }

    /// What the command printed, once it exited with 0.
    pub(crate) async fn reply(mut self) -> Result<String, CommandError> {
        match (&mut self.task).await {
            Ok(reply) => reply,
            Err(err) if err.is_cancelled() => Err(CommandError::Killed),
            Err(err) => Err(CommandError::SpawnFailed(io::Error::other(err))),
        }
    }
}

impl Drop for Asking {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn ask(settings: &AiSettings, prompt: &str) -> Result<String, CommandError> {
    let started = Instant::now();
    let workdir = Workdir::new().await.map_err(CommandError::SpawnFailed)?;
    let mut command = cleared_command(settings, workdir.path());
    let exchanged = timeout(settings.timeout, groups::exchange(&mut command, prompt.as_bytes(), MAX_REPLY_BYTES)).await;
    let reply = reply(exchanged, settings.timeout);
    workdir.remove().await;
    log(settings, started, prompt.len(), &reply);
    reply
}

/// The command run from `workdir` with only the variables `settings` passes, those pgbg has.
fn cleared_command(settings: &AiSettings, workdir: &Path) -> Command {
    let args = settings.command.get(1..).unwrap_or_default();
    let mut command = Command::new(program(settings));
    command.args(args).current_dir(workdir).env_clear();
    for name in &settings.env {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

fn reply(exchanged: Result<io::Result<Exchanged>, Elapsed>, limit: Duration) -> Result<String, CommandError> {
    match exchanged {
        Err(_elapsed) => Err(CommandError::TimedOut(limit)),
        Ok(Err(err)) => Err(CommandError::SpawnFailed(err)),
        Ok(Ok(Exchanged::PastCap)) => Err(CommandError::TooLong),
        Ok(Ok(Exchanged::Ended { status, last_words, .. })) if !status.success() => Err(CommandError::Failed { status, last_words }),
        Ok(Ok(Exchanged::Ended { stdout, .. })) => Ok(String::from_utf8_lossy(&stdout).into_owned()),
    }
}

/// Which program ran, how long, how it ended and the bytes each way; never the prompt nor the reply.
fn log(settings: &AiSettings, started: Instant, sent: usize, reply: &Result<String, CommandError>) {
    let program = program(settings);
    let (ended, received) = match reply {
        Ok(reply) => ("exit status: 0".to_owned(), reply.len()),
        Err(CommandError::Failed { status, .. }) => (status.to_string(), 0),
        Err(err) => (err.to_string(), 0),
    };
    tracing::info!("[ai] `{program}` ran {} ms, {ended}, {sent} bytes in, {received} bytes out", started.elapsed().as_millis());
}

fn program(settings: &AiSettings) -> Program {
    Program::new(settings.command.first().map_or("", String::as_str))
}

/// The empty folder a command runs from, removed after on a blocking thread.
struct Workdir(Option<TempDir>);

impl Workdir {
    async fn new() -> io::Result<Self> {
        let made = tokio::task::spawn_blocking(tempfile::tempdir).await.map_err(io::Error::other)?;
        Ok(Self(Some(made?)))
    }

    fn path(&self) -> &Path {
        self.0.as_ref().map_or(Path::new("."), TempDir::path)
    }

    async fn remove(mut self) {
        let Some(dir) = self.0.take() else { return };
        let removed = tokio::task::spawn_blocking(|| dir.close()).await.map_err(io::Error::other).flatten();
        if let Err(err) = removed {
            tracing::warn!("the [ai] command's temp folder could not be removed: {err}");
        }
    }
}

/// An ask cancelled halfway removes its folder too, on a blocking thread when a runtime runs.
impl Drop for Workdir {
    fn drop(&mut self) {
        let Some(dir) = self.0.take() else { return };
        let remove = move || {
            if let Err(err) = dir.close() {
                tracing::warn!("the [ai] command's temp folder could not be removed: {err}");
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(remove)),
            Err(_no_runtime) => remove(),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpawnFailed(err) => write!(f, "the [ai] command could not run: {err}"),
            Self::TimedOut(limit) => write!(f, "the [ai] command gave no answer within {} s", limit.as_secs()),
            Self::TooLong => write!(f, "the [ai] command printed more than {} KiB", MAX_REPLY_BYTES / 1024),
            Self::Failed { status, last_words } => {
                write!(f, "the [ai] command failed, {}", with_last_words(&status.to_string(), last_words))
            }
            Self::Killed => f.write_str("the ask was cancelled"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Two failures to spawn are equal when their kind is, as `io::Error` has no equality of its own.
impl PartialEq for CommandError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::SpawnFailed(left), Self::SpawnFailed(right)) => left.kind() == right.kind(),
            (Self::TimedOut(left), Self::TimedOut(right)) => left == right,
            (Self::Failed { status, last_words }, Self::Failed { status: other_status, last_words: other_words }) => {
                status == other_status && last_words == other_words
            }
            (Self::TooLong, Self::TooLong) | (Self::Killed, Self::Killed) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::LiteralPolicy;
    use crate::fixtures::{is_gone, leaving_a_sleep, written_pid};

    fn settings(command: &[&str]) -> AiSettings {
        AiSettings {
            command: command.iter().map(|&arg| arg.to_owned()).collect(),
            env: vec!["PATH".to_owned(), "HOME".to_owned()],
            timeout: Duration::from_secs(60),
            literals: LiteralPolicy::Masked,
            ghost: false,
        }
    }

    async fn asked(command: &[&str], prompt: &str) -> Result<String, CommandError> {
        Asking::start(&settings(command), prompt).reply().await
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn the_reply_is_what_the_command_prints_from_the_prompt() {
        let reply = asked(&["sh", "-c", "read -r line; echo \"SELECT id FROM orders; -- $line\""], "only the ids\n").await.unwrap();
        assert_eq!(reply, "SELECT id FROM orders; -- only the ids\n");
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn the_command_runs_from_an_empty_folder_removed_after() {
        let reply = asked(&["sh", "-c", "pwd -P; ls -A"], "").await.unwrap();
        let folder = Path::new(reply.trim_end());
        assert!(folder.is_absolute() && reply.lines().count() == 1, "{reply}");
        assert!(!folder.exists());
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn the_command_gets_only_the_variables_passed() {
        let reply = asked(&["/usr/bin/env"], "").await.unwrap();
        let names: Vec<&str> = reply.lines().filter_map(|line| line.split_once('=')).map(|(name, _)| name).collect();
        assert!(names.contains(&"PATH") && names.iter().all(|name| ["PATH", "HOME"].contains(name)), "{names:?}");
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn another_exit_than_0_says_its_status_and_last_words() {
        let failed = asked(&["sh", "-c", "echo 'no model named shop' >&2; exit 3"], "").await.unwrap_err();
        assert!(matches!(&failed, CommandError::Failed { status, .. } if status.code() == Some(3)));
        assert!(failed.to_string().contains("no model named shop"), "{failed}");
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn a_reply_past_64_kib_is_too_long() {
        assert!(matches!(asked(&["yes"], "").await, Err(CommandError::TooLong)));
    }

    #[tokio::test(start_paused = true)]
    #[ignore = "starts a process"]
    async fn a_command_past_its_timeout_is_stopped() {
        assert!(matches!(asked(&["sleep", "120"], "").await, Err(CommandError::TimedOut(limit)) if limit == Duration::from_secs(60)));
    }

    #[tokio::test]
    async fn a_program_that_cannot_run_says_so() {
        assert!(matches!(asked(&["pgbg-no-such-program"], "").await, Err(CommandError::SpawnFailed(_))));
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn a_dropped_ask_kills_the_command_and_what_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("sleep.pid");
        let argv = leaving_a_sleep(&pid_file);
        let asking = Asking::start(&settings(&argv.iter().map(String::as_str).collect::<Vec<_>>()), "");
        let sleep = written_pid(&pid_file).await;
        drop(asking);
        assert!(is_gone(sleep).await);
    }
}
