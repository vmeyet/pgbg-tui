//! Every live tunnel's process group, so a panic kills them where no `Drop` runs: release builds abort.
use rustix::process::{Pid, Signal, kill_process_group};
use std::collections::BTreeSet;
use std::sync::{Mutex, Once, PoisonError};

/// Shared by every tunnel and the panic hook, which runs on whichever thread panics.
static LIVE: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());
static HOOK: Once = Once::new();

/// Keeps `group` until `kill` takes it, and has a panic kill it.
pub(super) fn register(group: Pid) {
    HOOK.call_once(kill_live_groups_on_panic);
    LIVE.lock().unwrap_or_else(PoisonError::into_inner).insert(group.as_raw_nonzero().get());
}

/// Kills `group` and forgets it, so neither a second kill nor a panic signals an id the system may reuse; a group
/// the panic hook already killed is not signalled again.
pub(super) fn kill(group: Pid) -> rustix::io::Result<()> {
    let was_live = LIVE.lock().unwrap_or_else(PoisonError::into_inner).remove(&group.as_raw_nonzero().get());
    if !was_live {
        return Ok(());
    }
    kill_process_group(group, Signal::KILL)
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
