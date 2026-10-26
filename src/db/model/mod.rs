//! The neutral model of statements and their results: code outside `db/` never sees a driver type.

/// One SQL statement, as typed: the unit pgbg runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Statement(String);

impl Statement {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
