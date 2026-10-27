//! Why a session did not open, was lost, or gave a run no result: one kind per reason, its words drawn or logged only.
use crate::db::guard::Refusal;
use crate::db::model::{ServerError, TriedToWrite, UnknownType};
use crate::query::params::ParamError;
use std::fmt;

const CANCELLED_BEFORE_SENT: &str = "cancelled before it was sent";
const STOPPED_WITH_ITS_SESSION: &str = "cancelled: its session stopped";

/// Why a session can run no more statements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Lost {
    ServerClosed,
    /// The link failed, in the driver's or the server's words.
    LinkFailed(String),
    /// The task that drove the link is gone without saying why.
    LinkTaskGone,
    /// A `ROLLBACK` failed, so the server may still hold the transaction open.
    RollbackFailed,
    TunnelDied,
}

impl fmt::Display for Lost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServerClosed => f.write_str("the server closed the link"),
            Self::LinkFailed(why) => f.write_str(why),
            Self::LinkTaskGone => f.write_str("the link's task is gone"),
            Self::RollbackFailed => f.write_str("a rollback failed, so the server may still hold its transaction"),
            Self::TunnelDied => f.write_str("its tunnel died"),
        }
    }
}

/// Why a session did not open, or a run, a page or a schema read gave no result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The connection's tunnel did not start, and how it ended.
    TunnelNotStarted(String),
    /// The login failed, in the driver's or the server's words.
    Login(String),
    /// The link failed before the server answered, in the driver's words.
    Link(String),
    /// pgbg kept the statement from the server: the guard, its parameters, a type the server does not know, or a write
    /// not confirmed.
    Refused(String),
    /// The server refused a write in a statement the guard read as a read, so nothing of it ran.
    TriedToWrite,
    CancelledBeforeSent,
    StoppedWithItsSession,
    /// The server failed the statement, in its words; it may say where.
    Server {
        message: String,
        /// A character position in the statement sent, counted from 1 as PostgreSQL counts.
        position: Option<usize>,
    },
}

impl Failure {
    /// Where in the statement the server stopped, when it said.
    pub(crate) fn position(&self) -> Option<usize> {
        match self {
            Self::Server { position, .. } => *position,
            _ => None,
        }
    }
}

/// The kind of an error a run, a page or a schema read returned.
impl From<&anyhow::Error> for Failure {
    fn from(err: &anyhow::Error) -> Self {
        let words = format!("{err:#}");
        if err.is::<TriedToWrite>() {
            return Self::TriedToWrite;
        }
        if err.is::<Refusal>() || err.is::<ParamError>() || err.is::<UnknownType>() {
            return Self::Refused(words);
        }
        match err.downcast_ref::<ServerError>() {
            Some(server) => Self::Server { position: server.position, message: words },
            None => Self::Link(words),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TunnelNotStarted(how) => write!(f, "its tunnel did not start: {how}"),
            Self::Login(words) | Self::Link(words) | Self::Server { message: words, .. } => f.write_str(words),
            Self::Refused(why) => write!(f, "not sent: {why}"),
            Self::TriedToWrite => write!(f, "{TriedToWrite}"),
            Self::CancelledBeforeSent => f.write_str(CANCELLED_BEFORE_SENT),
            Self::StoppedWithItsSession => f.write_str(STOPPED_WITH_ITS_SESSION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::TypeName;

    fn server_error(position: Option<usize>) -> ServerError {
        ServerError {
            severity: "ERROR".into(),
            message: "column \"totl\" does not exist".into(),
            detail: None,
            hint: None,
            position,
            is_refused_write: false,
        }
    }

    #[test]
    fn a_tunnel_that_did_not_start_says_how_it_ended() {
        let failure = Failure::TunnelNotStarted("the tunnel `ssh` died, exit status: 255".into());
        assert_eq!(failure.to_string(), "its tunnel did not start: the tunnel `ssh` died, exit status: 255");
    }

    #[test]
    fn a_server_error_keeps_where_the_server_stopped() {
        let failure = Failure::from(&anyhow::Error::new(server_error(Some(8))));
        assert_eq!(failure.position(), Some(8));
    }

    #[test]
    fn a_server_error_says_the_servers_words() {
        let failure = Failure::from(&anyhow::Error::new(server_error(None)).context("running it"));
        assert_eq!(failure.to_string(), "running it: ERROR: column \"totl\" does not exist");
    }

    #[test]
    fn a_type_the_server_does_not_know_is_refused() {
        let failure = Failure::from(&anyhow::Error::new(UnknownType(TypeName::new("dat"))));
        assert_eq!(failure.to_string(), "not sent: the server knows no type `dat`");
    }
}
