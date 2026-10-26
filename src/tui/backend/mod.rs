//! What the runner calls: the session of the open connection, served by a task that owns it.
use super::app::Incoming;
use crate::connection::{Connection, WritePolicy};
use crate::db::model::Statement;
use crate::db::{Session, guard};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

/// The way to the session task: statements in, answers out on the loop's channel.
#[derive(Debug)]
pub(super) struct Backend {
    statements: UnboundedSender<Statement>,
}

impl Backend {
    /// Starts the task that opens a session on `connection` and runs each statement in turn.
    pub(super) fn start(connection: Connection, answers: UnboundedSender<Incoming>) -> Self {
        let (statements, queue) = mpsc::unbounded_channel();
        tokio::spawn(serve(connection, queue, answers));
        Self { statements }
    }

    /// Queues `statement` behind the one running; its answer comes back as `Incoming`.
    pub(super) fn run(&self, statement: Statement) {
        if self.statements.send(statement).is_err() {
            tracing::warn!("the session task is gone: the statement is dropped");
        }
    }
}

/// Opens the session at once, and again before a statement when the last try failed.
async fn serve(connection: Connection, mut queue: UnboundedReceiver<Statement>, answers: UnboundedSender<Incoming>) {
    let send = |incoming: Incoming| {
        if answers.send(incoming).is_err() {
            tracing::warn!("the loop is gone: an answer is dropped");
        }
    };
    let mut session = open(&connection, send).await;
    while let Some(statement) = queue.recv().await {
        if session.is_none() {
            session = open(&connection, send).await;
        }
        if let Some(open) = session.as_mut() {
            send(run(open, connection.writes(), statement).await);
        }
    }
}

async fn open(connection: &Connection, send: impl Fn(Incoming)) -> Option<Session> {
    match Session::open(connection).await {
        Ok(session) => {
            send(Incoming::Opened);
            Some(session)
        }
        Err(err) => {
            send(Incoming::NotOpened(format!("{err:#}")));
            None
        }
    }
}

async fn run(session: &mut Session, writes: WritePolicy, statement: Statement) -> Incoming {
    let admitted = match guard::admit(statement, writes) {
        Ok(admitted) => admitted,
        Err(refusal) => return Incoming::Refused(refusal.to_string()),
    };
    let started = Instant::now();
    match session.run(&admitted).await {
        Ok(result) => Incoming::Result { result, took: started.elapsed() },
        Err(err) => Incoming::Failed(format!("{err:#}")),
    }
}
