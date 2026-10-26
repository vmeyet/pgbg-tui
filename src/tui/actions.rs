//! The action runner: each action the app asks for goes to the backend, which answers through `Incoming`.
use super::app::Action;
use super::backend::Backend;

/// Hands `action` over without waiting on it; the loop never awaits the database.
pub(super) fn spawn(action: Action, backend: &mut Backend) {
    match action {
        Action::Open(connection) => backend.open(connection),
        Action::Run { connection, run, statement } => backend.run(&connection, run, statement),
        Action::Cancel { connection, run } => backend.cancel(&connection, run),
        Action::ListQueries => backend.queries.list(),
        Action::OpenQuery(query) => backend.queries.open(query),
    }
}
