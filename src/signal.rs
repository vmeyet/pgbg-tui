//! The signals that ask pgbg to stop: ctrl-c outside the TUI, a `kill`, a closed terminal.
use anyhow::Result;
use std::fmt;
use std::process::ExitCode;
use tokio::signal::unix::{SignalKind, signal};

/// The exit code of a process a signal stopped is this plus the signal's number, as shells report it.
const SIGNALLED_EXIT_BASE: i32 = 128;

/// The signal that stopped pgbg; `main` exits with its code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stopped(SignalKind);

impl Stopped {
    /// `128 + n` for signal `n`, as a shell reports a process that signal stopped.
    pub fn exit_code(self) -> ExitCode {
        let code = SIGNALLED_EXIT_BASE + self.0.as_raw_value();
        ExitCode::from(u8::try_from(code).unwrap_or(u8::MAX))
    }
}

impl fmt::Display for Stopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stopped by signal {}", self.0.as_raw_value())
    }
}

impl std::error::Error for Stopped {}

/// Runs `work` to its end, or until a signal asks pgbg to stop: `work` is then dropped, and with it any tunnel it holds.
pub async fn until_stopped<T>(work: impl Future<Output = Result<T>>) -> Result<T> {
    let mut signals = Signals::listen()?;
    tokio::select! {
        outcome = work => outcome,
        stopped = signals.next() => Err(stopped.into()),
    }
}

/// The signals pgbg stops on, listened to from the moment it is built so none is missed.
pub(crate) struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
}

impl Signals {
    pub(crate) fn listen() -> Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
        })
    }

    /// Waits for the next of them.
    pub(crate) async fn next(&mut self) -> Stopped {
        let kind = tokio::select! {
            _ = self.interrupt.recv() => SignalKind::interrupt(),
            _ = self.terminate.recv() => SignalKind::terminate(),
            _ = self.hangup.recv() => SignalKind::hangup(),
        };
        Stopped(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_exits_with_128_plus_the_signal_number() {
        assert_eq!(Stopped(SignalKind::terminate()).exit_code(), ExitCode::from(143));
    }
}
