use super::TypeName;
use std::fmt;

/// An error the server reported, in its own words: `ERROR: relation "orders" does not exist`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerError {
    /// `ERROR`, `FATAL`, or the same word in the server's language.
    pub(crate) severity: String,
    pub(crate) message: String,
    pub(crate) detail: Option<String>,
    pub(crate) hint: Option<String>,
    /// Where in the statement the server stopped: a character count from 1, as PostgreSQL counts.
    pub(crate) position: Option<usize>,
}

/// The severity and message, then the detail and the hint on lines of their own.
impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.severity, self.message)?;
        for line in [&self.detail, &self.hint].into_iter().flatten() {
            write!(f, "\n{line}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ServerError {}

/// A type a statement's header names that the server does not know: the run stops before the statement is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnknownType(pub(crate) TypeName);

impl fmt::Display for UnknownType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the server knows no type `{}`", self.0.as_str())
    }
}

impl std::error::Error for UnknownType {}

/// pgbg refused to open its own read-only transaction, since the user's `BEGIN` may have left one open on the
/// session: its `ROLLBACK` would end the user's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InsideUserTransaction;

impl fmt::Display for InsideUserTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a transaction you began may be open on this session: end it with COMMIT or ROLLBACK first")
    }
}

impl std::error::Error for InsideUserTransaction {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_prints_its_detail_and_hint_under_its_message() {
        let error = ServerError {
            severity: "ERROR".into(),
            message: "column \"totl\" does not exist".into(),
            detail: None,
            hint: Some("Perhaps you meant to reference the column \"orders.total\".".into()),
            position: Some(8),
        };
        assert_eq!(error.to_string(), "ERROR: column \"totl\" does not exist\nPerhaps you meant to reference the column \"orders.total\".");
    }
}
