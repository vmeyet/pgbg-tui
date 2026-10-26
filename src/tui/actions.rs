//! The action runner: each action the app asks for goes to the backend, which answers through `Incoming`.
use super::app::Action;
use super::backend::Backend;

/// Hands `action` over without waiting on it; the loop never awaits the database.
pub(super) fn spawn(action: Action, backend: &Backend) {
    match action {
        Action::Run { run, statement } => backend.run(run, statement),
        Action::Cancel(run) => backend.cancel(run),
    }
}
