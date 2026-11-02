//! The `EXPLAIN` that asks the server for a statement's plan, pure.
use super::guard;
use super::model::Statement;

/// How much of a plan to ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Explain {
    /// The plan alone: the statement does not run.
    Plan,
    /// The plan and what each node did: the statement runs, read only and rolled back.
    Analyze,
}

impl Explain {
    /// What goes before the statement.
    pub(crate) const fn lead(self) -> &'static str {
        match self {
            Self::Plan => "EXPLAIN (FORMAT JSON) ",
            Self::Analyze => "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) ",
        }
    }

    /// `statement` under this `EXPLAIN`; none unless the guard reads it as a query that changes nothing.
    pub(crate) fn of(self, statement: &Statement) -> Option<Statement> {
        guard::is_read_query(statement.as_str()).then(|| Statement::new(format!("{}{}", self.lead(), statement.as_str())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn explained(explain: Explain, sql: &str) -> Option<String> {
        explain.of(&Statement::new(sql)).map(|statement| statement.as_str().to_owned())
    }

    #[test]
    fn a_read_goes_under_the_explain_as_it_is() {
        assert_eq!(explained(Explain::Plan, "SELECT * FROM orders").as_deref(), Some("EXPLAIN (FORMAT JSON) SELECT * FROM orders"));
    }

    #[test]
    fn analyze_asks_for_the_buffers_too() {
        assert_eq!(explained(Explain::Analyze, "TABLE orders").as_deref(), Some("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) TABLE orders"));
    }

    #[test]
    fn a_comment_before_the_statement_stays_inside() {
        assert_eq!(explained(Explain::Plan, "-- paid\nSELECT 1").as_deref(), Some("EXPLAIN (FORMAT JSON) -- paid\nSELECT 1"),);
    }

    #[test]
    fn what_is_no_query_or_writes_or_locks_cannot_be_explained() {
        let refused = [
            "DELETE FROM orders",
            "SHOW search_path",
            "EXPLAIN SELECT 1",
            "BEGIN",
            "SELECT 1; SELECT 2",
            "WITH gone AS (DELETE FROM orders RETURNING id) SELECT * FROM gone",
            "SELECT * FROM orders FOR UPDATE",
            "SELECT 1 INTO kept",
        ];
        for sql in refused {
            assert_eq!(explained(Explain::Analyze, sql), None, "{sql}");
        }
    }
}
