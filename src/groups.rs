//! The process groups pgbg owns (a tunnel's, a `before` or password command's), so nothing they start outlives them,
//! and a panic kills them where no `Drop` runs: release builds abort.
use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process_group};
use std::collections::BTreeSet;
use std::process::{Output, Stdio};
use std::sync::{Mutex, Once, PoisonError};
use tokio::process::Command;

/// A child given this as its group id leads a group of its own.
pub(crate) const OWN_GROUP: i32 = 0;

/// Shared by every owned group and the panic hook, which runs on whichever thread panics.
static LIVE: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());
static HOOK: Once = Once::new();

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
    use std::os::unix::process::CommandExt;

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
