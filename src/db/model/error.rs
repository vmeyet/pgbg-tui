use std::fmt;

/// An error the server reported, in its own words: `ERROR: relation "orders" does not exist`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerError {
    /// `ERROR`, `FATAL`, or the server's word in its own language.
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
