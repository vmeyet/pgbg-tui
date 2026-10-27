//! The process groups pgbg owns (a tunnel's, a `before`, password or `[ai]` command's), so nothing they start outlives
//! them, and a panic kills them where no `Drop` runs: release builds abort.
use rustix::io::Errno;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use std::collections::{BTreeSet, VecDeque};
use std::io::ErrorKind;
use std::process::{ExitStatus, Output, Stdio};
use std::sync::{Mutex, Once, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio::time::timeout;

/// A child given this as its group id leads a group of its own.
pub(crate) const OWN_GROUP: i32 = 0;

/// Shared by every owned group and the panic hook, which runs on whichever thread panics.
static LIVE: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());
static HOOK: Once = Once::new();

/// How long a command's pipes are read once it exited, in case a process it left, maybe outside its group, still holds them.
pub(crate) const LAST_WORDS_TIMEOUT: Duration = Duration::from_secs(1);
const LAST_WORDS_LINES: usize = 5;
const LAST_WORDS_LINE_CHARS: usize = 200;

/// Keeps `group` until `kill` takes it, and has a panic kill it.
pub(crate) fn register(group: Pid) {
    HOOK.call_once(kill_live_groups_on_panic);
    LIVE.lock().unwrap_or_else(PoisonError::into_inner).insert(group.as_raw_nonzero().get());
}

/// Kills `group` and forgets it, so neither a second kill nor a panic signals an id the system may reuse.
/// A group the panic hook already killed is not signalled again, and a group with nothing left to signal is no failure.
pub(crate) fn kill(group: Pid) -> rustix::io::Result<()> {
    let was_live = LIVE.lock().unwrap_or_else(PoisonError::into_inner).remove(&group.as_raw_nonzero().get());
    if !was_live {
        return Ok(());
    }
    match kill_process_group(group, Signal::KILL) {
        Err(Errno::SRCH) => Ok(()),
        // macOS refuses to signal a group whose only member left is its zombie leader.
        Err(Errno::PERM) if cfg!(target_os = "macos") => Ok(()),
        killed => killed,
    }
}

/// A command in a group of its own, owned until its leader is reaped: dropped before, the whole group is killed.
struct Owned {
    child: Child,
    /// Taken once the group is killed, so no later kill signals an id the system may reuse.
    group: Option<Pid>,
}

impl Owned {
    fn spawn(command: &mut Command) -> std::io::Result<Self> {
        let child = command.process_group(OWN_GROUP).kill_on_drop(true).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let group = child.id().and_then(|id| Pid::from_raw(id as i32));
        if let Some(group) = group {
            register(group);
        }
        Ok(Self { child, group })
    }

    /// What `streams` read to their end; once the leader exits, waited for [`LAST_WORDS_TIMEOUT`] at most, then the group
    /// is killed, so a child it left holding a pipe stops holding it, and waited for that long again: a process that left
    /// the group still holding a pipe times the read out.
    async fn drained<T>(&mut self, streams: impl Future<Output = T>) -> std::io::Result<T> {
        let exited = leader_exited(self.child.id());
        tokio::pin!(streams, exited);
        tokio::select! {
            read = &mut streams => Ok(read),
            () = &mut exited => match timeout(LAST_WORDS_TIMEOUT, &mut streams).await {
                Ok(read) => Ok(read),
                Err(_lingering) => {
                    self.kill();
                    timeout(LAST_WORDS_TIMEOUT, streams).await.map_err(|_held| {
                        std::io::Error::new(ErrorKind::TimedOut, "a process that left the command's group still holds its output")
                    })
                }
            },
        }
    }

    /// Waits for the leader to exit, kills what it left in its group, then reaps it: a zombie leader keeps the group id
    /// from being reused until then.
    async fn ended(mut self) -> std::io::Result<ExitStatus> {
        leader_exited(self.child.id()).await;
        self.kill();
        self.child.wait().await
    }

    fn kill(&mut self) {
        let Some(group) = self.group.take() else { return };
        if let Err(err) = kill(group) {
            tracing::warn!("a command's process group could not be killed: {err}");
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Resolves once the process `leader` exited, left unreaped; a thread that cannot start leaves it to
/// [`LAST_WORDS_TIMEOUT`] and the group kill.
async fn leader_exited(leader: Option<u32>) {
    let Some(leader) = leader.and_then(|id| Pid::from_raw(id as i32)) else { return };
    match ExitWatch::start(leader) {
        Ok(mut exit) => exit.exited().await,
        Err(err) => tracing::warn!("a command's exit cannot be watched: {err}"),
    }
}

/// Watches a group's leader exit without reaping it, so its group can be killed while its id cannot be reused yet.
/// A thread of its own waits, so neither the runtime nor a paused test clock is held.
#[derive(Debug)]
pub(crate) struct ExitWatch {
    /// Closed once the leader exited; nothing is ever sent.
    exited: watch::Receiver<()>,
}

impl ExitWatch {
    pub(crate) fn start(leader: Pid) -> std::io::Result<Self> {
        let (exiting, exited) = watch::channel(());
        std::thread::Builder::new().name("pgbg-exit".into()).spawn(move || {
            wait_unreaped(leader);
            drop(exiting);
        })?;
        Ok(Self { exited })
    }

    pub(crate) fn has_exited(&self) -> bool {
        self.exited.has_changed().is_err()
    }

    /// Resolves once the leader exited; safe to drop halfway and to await again.
    pub(crate) async fn exited(&mut self) {
        while self.exited.changed().await.is_ok() {}
    }
}

fn wait_unreaped(leader: Pid) {
    while matches!(waitid(WaitId::Pid(leader), WaitIdOptions::EXITED | WaitIdOptions::NOWAIT), Err(Errno::INTR)) {}
}

/// Runs `command` to its end as `Command::output` does, in a group of its own: dropped halfway (a timeout, a stop), it
/// kills the whole group, and once it ended, it kills what it left running, so nothing the command started outlives it.
pub(crate) async fn output(command: &mut Command) -> std::io::Result<Output> {
    let mut owned = Owned::spawn(command)?;
    let (Some(stdout), Some(stderr)) = (owned.child.stdout.take(), owned.child.stderr.take()) else {
        return Err(std::io::Error::other("a piped stream of the command is missing"));
    };
    let (stdout, stderr) = owned.drained(async { tokio::try_join!(read_to_end(stdout), read_to_end(stderr)) }).await??;
    let status = owned.ended().await?;
    Ok(Output { status, stdout, stderr })
}

async fn read_to_end(mut stream: impl AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut read = vec![];
    stream.read_to_end(&mut read).await?;
    Ok(read)
}

/// How a command given its stdin ended.
#[derive(Debug)]
pub(crate) enum Exchanged {
    Ended {
        status: ExitStatus,
        stdout: Vec<u8>,
        /// The last lines of its stderr, as [`keep_last_words`] keeps them.
        last_words: Vec<String>,
    },
    /// Its stdout passed the cap, so its group was killed then.
    PastCap,
}

/// Why a read or a write of [`exchange`] stopped early.
enum Stop {
    PastCap,
    Failed(std::io::Error),
}

/// Runs `command` in a group of its own, writing `input` to its stdin while it reads its stdout, so a big prompt never
/// deadlocks on a full pipe, and stops it once its stdout passes `cap` bytes; dropped halfway, it kills the whole group,
/// and once it ended, it kills what it left running.
pub(crate) async fn exchange(command: &mut Command, input: &[u8], cap: usize) -> std::io::Result<Exchanged> {
    let mut owned = Owned::spawn(command.stdin(Stdio::piped()))?;
    let (Some(stdin), Some(stdout), Some(stderr)) = (owned.child.stdin.take(), owned.child.stdout.take(), owned.child.stderr.take()) else {
        return Err(std::io::Error::other("a piped stream of the command is missing"));
    };
    let streams =
        async { tokio::try_join!(write_then_close(stdin, input), read_up_to(stdout, cap), async { Ok(keep_last_words(stderr).await) }) };
    let (stdout, last_words) = match owned.drained(streams).await? {
        Ok(((), stdout, last_words)) => (stdout, last_words),
        Err(Stop::PastCap) => return Ok(Exchanged::PastCap),
        Err(Stop::Failed(err)) => return Err(err),
    };
    let status = owned.ended().await?;
    Ok(Exchanged::Ended { status, stdout, last_words })
}

/// Writes `input` then closes the pipe; a command that exits without reading it all is no failure.
async fn write_then_close(mut stdin: impl AsyncWrite + Unpin, input: &[u8]) -> Result<(), Stop> {
    match stdin.write_all(input).await.and(stdin.shutdown().await) {
        Err(err) if err.kind() != ErrorKind::BrokenPipe => Err(Stop::Failed(err)),
        _ => Ok(()),
    }
}

async fn read_up_to(stdout: impl AsyncRead + Unpin, cap: usize) -> Result<Vec<u8>, Stop> {
    let mut read = vec![];
    stdout.take(cap as u64 + 1).read_to_end(&mut read).await.map_err(Stop::Failed)?;
    if read.len() > cap {
        return Err(Stop::PastCap);
    }
    Ok(read)
}

/// Reads `stderr` to its end, so a command never blocks on a full pipe, and keeps its last lines, each cut short;
/// a byte that is not UTF-8 shows as `�`.
pub(crate) async fn keep_last_words(stderr: impl AsyncRead + Unpin) -> Vec<String> {
    let mut lines = BufReader::new(stderr).split(b'\n');
    let mut kept = VecDeque::with_capacity(LAST_WORDS_LINES);
    while let Ok(Some(line)) = lines.next_segment().await {
        if kept.len() == LAST_WORDS_LINES {
            kept.pop_front();
        }
        kept.push_back(shortened(&line));
    }
    kept.into()
}

fn shortened(line: &[u8]) -> String {
    String::from_utf8_lossy(line).trim_end_matches('\r').chars().take(LAST_WORDS_LINE_CHARS).collect()
}

/// `how` a command ended, then its last words when it left some.
pub(crate) fn with_last_words(how: &str, last_words: &[String]) -> String {
    match last_words.join(" / ") {
        words if words.is_empty() => how.to_owned(),
        words => format!("{how}: {words}"),
    }
}

/// How the build treats a panic: an abort ends the process, an unwind may be caught.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanicStrategy {
    Abort,
    Unwind,
}

const BUILD_PANIC_STRATEGY: PanicStrategy = if cfg!(panic = "abort") { PanicStrategy::Abort } else { PanicStrategy::Unwind };
const MAIN_THREAD: &str = "main";

/// Whether a panic on the thread named `thread` ends the process: always when the build aborts; when it unwinds,
/// only on the main thread, since a tokio task, a blocking thread or a test catches its own panic and goes on.
fn is_fatal(strategy: PanicStrategy, thread: Option<&str>) -> bool {
    strategy == PanicStrategy::Abort || thread == Some(MAIN_THREAD)
}

fn kill_live_groups_on_panic() {
    let earlier = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !is_fatal(BUILD_PANIC_STRATEGY, std::thread::current().name()) {
            earlier(info);
            return;
        }
        let live = std::mem::take(&mut *LIVE.lock().unwrap_or_else(PoisonError::into_inner));
        for group in live.into_iter().filter_map(Pid::from_raw) {
            if let Err(err) = kill_process_group(group, Signal::KILL) {
                tracing::warn!("a tunnel could not be killed on panic: {err}");
            }
        }
        earlier(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{is_gone, leaving_a_sleep, sleep_goes_when_dropped, written_pid};
    use std::os::unix::process::CommandExt;

    async fn exchanged(argv: &[&str], input: &[u8], cap: usize) -> Exchanged {
        let mut command = Command::new(argv[0]);
        exchange(command.args(&argv[1..]), input, cap).await.unwrap()
    }

    #[tokio::test]
    async fn last_words_keep_the_last_lines_even_past_utf8() {
        let stderr = b"one\ntwo\nthree\nfour\nfive\n\xffsix\r\nseven";
        assert_eq!(keep_last_words(&stderr[..]).await, ["three", "four", "five", "\u{fffd}six", "seven"]);
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn an_input_bigger_than_a_pipe_comes_back_whole_without_a_deadlock() {
        let input = b"SELECT 1;\n".repeat(100_000);
        let Exchanged::Ended { status, stdout, .. } = exchanged(&["cat"], &input, input.len()).await else { panic!("cut") };
        assert!(status.success());
        assert_eq!(stdout, input);
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn an_output_past_the_cap_stops_the_command() {
        assert!(matches!(exchanged(&["yes"], b"", 1024).await, Exchanged::PastCap));
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn a_command_that_reads_no_input_still_ends_with_its_status_and_last_words() {
        let input = b"SELECT 1;\n".repeat(100_000);
        let argv = ["sh", "-c", "echo 'no model named shop' >&2; exit 3"];
        let Exchanged::Ended { status, stdout, last_words } = exchanged(&argv, &input, 1024).await else { panic!("cut") };
        assert_eq!((status.code(), stdout, last_words), (Some(3), vec![], vec!["no model named shop".to_owned()]));
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn an_exchange_dropped_halfway_kills_what_its_command_started() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("sleep.pid");
        let argv = leaving_a_sleep(&pid_file);
        let mut command = Command::new(&argv[0]);
        let exchanging = exchange(command.args(&argv[1..]), b"", 1024);
        assert!(sleep_goes_when_dropped(exchanging, &pid_file).await);
    }

    fn leaving(line: &str, pid_file: &std::path::Path) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", line, "sh"]).arg(pid_file);
        command
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn an_exchange_ends_with_its_command_though_a_child_holds_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("sleep.pid");
        let mut command = leaving("(sleep 60; echo late) & echo $! > \"$1\"; echo 'SELECT 1'", &pid_file);
        let Exchanged::Ended { stdout, .. } = exchange(&mut command, b"", 1024).await.unwrap() else { panic!("cut") };
        assert_eq!(stdout, b"SELECT 1\n");
        assert!(is_gone(written_pid(&pid_file).await).await);
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn an_exchange_kills_what_its_command_left_running() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("sleep.pid");
        let mut command = leaving("nohup sleep 60 >/dev/null 2>&1 & echo $! > \"$1\"", &pid_file);
        exchange(&mut command, b"", 1024).await.unwrap();
        assert!(is_gone(written_pid(&pid_file).await).await);
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn an_output_ends_with_its_command_though_a_child_holds_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("sleep.pid");
        let output = output(&mut leaving("(sleep 60; echo late) & echo $! > \"$1\"; echo hunter2", &pid_file)).await.unwrap();
        assert_eq!(output.stdout, b"hunter2\n");
        assert!(is_gone(written_pid(&pid_file).await).await);
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn an_output_kills_what_its_command_left_running() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("sleep.pid");
        output(&mut leaving("nohup sleep 60 >/dev/null 2>&1 & echo $! > \"$1\"", &pid_file)).await.unwrap();
        assert!(is_gone(written_pid(&pid_file).await).await);
    }

    #[tokio::test(start_paused = true)]
    #[ignore = "starts a process"]
    async fn an_output_held_by_a_process_outside_its_group_is_given_up() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("python.pid");
        let python = r#"import os, sys, time; os.setsid(); open(sys.argv[1], "w").write(str(os.getpid())); time.sleep(60)"#;
        let line = format!("python3 -c '{python}' \"$1\" & while [ ! -s \"$1\" ]; do sleep 0.01; done");
        let given_up = output(&mut leaving(&line, &pid_file)).await;
        rustix::process::kill_process(written_pid(&pid_file).await, Signal::KILL).unwrap();
        assert_eq!(given_up.map_err(|err| err.kind()).err(), Some(ErrorKind::TimedOut));
    }

    #[test]
    #[ignore = "starts a process"]
    fn killing_a_group_left_with_only_its_zombie_leader_succeeds() {
        let mut child = std::process::Command::new("true").process_group(0).spawn().unwrap();
        let group = Pid::from_raw(child.id() as i32).unwrap();
        wait_unreaped(group);
        register(group);
        let killed = kill(group);
        child.wait().unwrap();
        assert_eq!(killed, Ok(()));
    }

    #[test]
    fn a_panic_ends_the_process_when_the_build_aborts_or_on_the_main_thread() {
        assert!(is_fatal(PanicStrategy::Abort, Some("tokio-runtime-worker")));
        assert!(is_fatal(PanicStrategy::Unwind, Some("main")));
    }

    #[test]
    fn an_unwinding_panic_off_the_main_thread_is_caught_and_kills_nothing() {
        assert!(!is_fatal(PanicStrategy::Unwind, Some("tokio-runtime-worker")));
        assert!(!is_fatal(PanicStrategy::Unwind, None));
    }

    #[test]
    #[ignore = "starts a process"]
    fn killing_a_group_no_longer_held_signals_nothing() {
        let mut child = std::process::Command::new("sleep").arg("30").process_group(0).spawn().unwrap();
        let group = Pid::from_raw(child.id() as i32).unwrap();
        kill(group).unwrap();
        let ended = (0..20).find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            child.try_wait().unwrap()
        });
        assert_eq!(ended, None);
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
