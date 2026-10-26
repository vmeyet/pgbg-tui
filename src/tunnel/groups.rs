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

/// Kills `group` and forgets it, so neither a second kill nor a panic signals an id the system may reuse.
pub(super) fn kill(group: Pid) -> rustix::io::Result<()> {
    LIVE.lock().unwrap_or_else(PoisonError::into_inner).remove(&group.as_raw_nonzero().get());
    kill_process_group(group, Signal::KILL)
}

fn kill_live_groups_on_panic() {
    let earlier = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let live = std::mem::take(&mut *LIVE.lock().unwrap_or_else(PoisonError::into_inner));
        for group in live.into_iter().filter_map(Pid::from_raw) {
            if let Err(err) = kill_process_group(group, Signal::KILL) {
                tracing::warn!("a tunnel could not be killed on panic: {err}");
            }
        }
        earlier(info);
    }));
}
