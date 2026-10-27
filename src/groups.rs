//! The process groups pgbg owns (a tunnel's, a `before`, password or `[ai]` command's), so nothing they start outlives
//! them, and a panic kills them where no `Drop` runs: release builds abort.
use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process_group};
use std::collections::{BTreeSet, VecDeque};
use std::io::ErrorKind;
use std::process::{ExitStatus, Output, Stdio};
use std::sync::{Mutex, Once, PoisonError};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// A child given this as its group id leads a group of its own.
pub(crate) const OWN_GROUP: i32 = 0;

/// Shared by every owned group and the panic hook, which runs on whichever thread panics.
static LIVE: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());
static HOOK: Once = Once::new();

const LAST_WORDS_LINES: usize = 5;
const LAST_WORDS_LINE_CHARS: usize = 200;

/// Keeps `group` until `kill` takes it, and has a panic kill it.
pub(crate) fn register(group: Pid) {
    HOOK.call_once(kill_live_groups_on_panic);
    LIVE.lock().unwrap_or_else(PoisonError::into_inner).insert(group.as_raw_nonzero().get());
}

/// Kills `group` and forgets it, so neither a second kill nor a panic signals an id the system may reuse.
/// A group the panic hook already killed is not signalled again.
pub(crate) fn kill(group: Pid) -> rustix::io::Result<()> {
    let was_live = LIVE.lock().unwrap_or_else(PoisonError::into_inner).remove(&group.as_raw_nonzero().get());
    if !was_live {
        return Ok(());
    }
    kill_process_group(group, Signal::KILL)
}

/// Forgets `group` without a signal: its leader was reaped, so its id may already be another's.
fn forget(group: Pid) {
    LIVE.lock().unwrap_or_else(PoisonError::into_inner).remove(&group.as_raw_nonzero().get());
}

/// A group owned until its leader is reaped: dropped before, the whole group is killed.
struct Owned(Option<Pid>);

impl Owned {
    fn new(leader: Option<u32>) -> Self {
        let group = leader.and_then(|id| Pid::from_raw(id as i32));
        if let Some(group) = group {
            register(group);
        }
        Self(group)
    }

    fn reaped(mut self) {
        if let Some(group) = self.0.take() {
            forget(group);
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        let Some(group) = self.0.take() else { return };
        match kill(group) {
            Ok(()) | Err(Errno::SRCH) => {}
            Err(err) => tracing::warn!("a command's process group could not be killed: {err}"),
        }
    }
}

/// Runs `command` to its end as `Command::output` does, in a group of its own: dropped halfway (a timeout, a stop), it
/// kills the whole group, so nothing the command started outlives it.
pub(crate) async fn output(command: &mut Command) -> std::io::Result<Output> {
    let child = command.process_group(OWN_GROUP).kill_on_drop(true).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let group = Owned::new(child.id());
    let output = child.wait_with_output().await;
    group.reaped();
    output
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
/// deadlocks on a full pipe, and stops it once its stdout passes `cap` bytes; dropped halfway, it kills the whole group.
pub(crate) async fn exchange(command: &mut Command, input: &[u8], cap: usize) -> std::io::Result<Exchanged> {
    let mut child =
        command.process_group(OWN_GROUP).kill_on_drop(true).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let group = Owned::new(child.id());
    let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        return Err(std::io::Error::other("a piped stream of the command is missing"));
    };
    let streams = tokio::try_join!(write_then_close(stdin, input), read_up_to(stdout, cap), async { Ok(keep_last_words(stderr).await) });
    let (stdout, last_words) = match streams {
        Ok(((), stdout, last_words)) => (stdout, last_words),
        Err(Stop::PastCap) => return Ok(Exchanged::PastCap),
        Err(Stop::Failed(err)) => return Err(err),
    };
    let status = child.wait().await?;
    group.reaped();
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
    use crate::fixtures::{leaving_a_sleep, sleep_goes_when_dropped};
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
            std::thread::sleep(std::time::Duration::from_millis(10));
            child.try_wait().unwrap()
        });
        assert_eq!(ended, None);
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
