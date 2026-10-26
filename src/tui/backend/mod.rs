//! What the runner calls: the session of the open connection, served by a task that owns it.
use super::app::Incoming;
use crate::connection::{Connection, WritePolicy};
use crate::db::model::{CancelToken, Statement};
use crate::db::{Session, guard};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;
use tokio::time::Instant;

/// The way to the session task: statements in, answers out on the loop's channel.
#[derive(Debug)]
pub(super) struct Backend {
    statements: UnboundedSender<Statement>,
    /// The open session's token, set by its task: a cancel cannot wait behind the statement it cancels.
    cancel_token: watch::Receiver<Option<CancelToken>>,
}

impl Backend {
    /// Starts the task that opens a session on `connection` and runs each statement in turn.
    pub(super) fn start(connection: Connection, answers: UnboundedSender<Incoming>) -> Self {
        let (statements, queue) = mpsc::unbounded_channel();
        let (cancel_tokens, cancel_token) = watch::channel(None);
        tokio::spawn(serve(Served { connection, queue, answers, cancel_tokens }));
        Self { statements, cancel_token }
    }

    /// Queues `statement` behind the one running; its answer comes back as `Incoming`.
    pub(super) fn run(&self, statement: Statement) {
        if self.statements.send(statement).is_err() {
            tracing::warn!("the session task is gone: the statement is dropped");
        }
    }

    /// Asks the server to cancel the running statement; that statement's answer says how it ended.
    pub(super) fn cancel(&self) {
        let Some(token) = self.cancel_token.borrow().clone() else { return };
        tokio::spawn(async move {
            if let Err(err) = token.cancel().await {
                tracing::warn!("the cancel request failed: {err:#}");
            }
        });
    }
}

/// What the session task owns.
struct Served {
    connection: Connection,
    queue: UnboundedReceiver<Statement>,
    answers: UnboundedSender<Incoming>,
    cancel_tokens: watch::Sender<Option<CancelToken>>,
}

/// Opens the session at once, and again before a statement when the last try failed.
async fn serve(served: Served) {
    let Served { connection, mut queue, answers, cancel_tokens } = served;
    let send = |incoming: Incoming| {
        if answers.send(incoming).is_err() {
            tracing::warn!("the loop is gone: an answer is dropped");
        }
    };
    let mut session = open(&connection, send).await;
    cancel_tokens.send_replace(session.as_ref().map(Session::cancel_token));
    while let Some(statement) = queue.recv().await {
        if session.is_none() {
            session = open(&connection, send).await;
            cancel_tokens.send_replace(session.as_ref().map(Session::cancel_token));
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
