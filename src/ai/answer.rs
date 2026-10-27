//! A command's answer read back: the SQL out of its Markdown fence, its markers put back, split into statements.
use super::mask::{Literals, UnknownMarker};
use crate::db::lex::{self, Token};
use crate::db::model::Statement;
use std::fmt;

const FENCE: &str = "```";

/// Why an answer gives no statement to show.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AnswerError {
    /// The answer holds a marker that stands for no literal of the statement asked about; holds the marker.
    UnknownMarker(String),
    NoStatement,
}

/// The statements `reply` holds, with the literals `literals` masked put back.
pub(super) fn read(reply: &str, literals: &Literals) -> Result<Vec<Statement>, AnswerError> {
    let sql = literals.put_back(fenced(reply))?;
    let statements = split(&sql);
    if statements.is_empty() { Err(AnswerError::NoStatement) } else { Ok(statements) }
}

/// What the first Markdown fence of `reply` holds, without its language; the whole reply when it holds no fence.
fn fenced(reply: &str) -> &str {
    let Some((_, opened)) = reply.split_once(FENCE) else { return reply };
    let body = opened.split_once('\n').map_or("", |(_, body)| body);
    body.split_once(FENCE).map_or(body, |(inside, _)| inside)
}

/// Each statement of `sql`, cut at its `;`, without the blanks around it; a piece of comments alone is none.
fn split(sql: &str) -> Vec<Statement> {
    let mut statements = vec![];
    let mut start = 0;
    let mut has_tokens = false;
    for placed in lex::tokens(sql) {
        if placed.token == Token::End {
            statements.extend(has_tokens.then(|| statement(&sql[start..placed.range.start])));
            (start, has_tokens) = (placed.range.end, false);
        } else {
            has_tokens = true;
        }
    }
    statements.extend(has_tokens.then(|| statement(&sql[start..])));
    statements
}

fn statement(text: &str) -> Statement {
    Statement::new(text.trim_matches(lex::is_blank))
}

impl From<UnknownMarker> for AnswerError {
    fn from(marker: UnknownMarker) -> Self {
        Self::UnknownMarker(marker.0)
    }
}

impl fmt::Display for AnswerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMarker(marker) => write!(f, "the answer holds `{marker}`, which stands for no value of the statement"),
            Self::NoStatement => f.write_str("the answer holds no statement"),
        }
    }
}

impl std::error::Error for AnswerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::mask::masked;

    fn statements(sql: &str, reply: &str) -> Result<Vec<String>, AnswerError> {
        let statements = read(reply, &masked(sql).literals)?;
        Ok(statements.iter().map(|statement| statement.as_str().to_owned()).collect())
    }

    #[test]
    fn the_prose_around_a_fence_is_dropped() {
        let reply = "Here it is:\n```sql\nSELECT id FROM orders\n```\nIt reads the ids.";
        assert_eq!(statements("", reply).unwrap(), ["SELECT id FROM orders"]);
    }

    #[test]
    fn a_reply_without_a_fence_is_read_whole() {
        assert_eq!(statements("", "SELECT id FROM orders;\n").unwrap(), ["SELECT id FROM orders"]);
    }

    #[test]
    fn each_statement_of_the_answer_is_one() {
        let reply = "SELECT 1; -- note\nSELECT ';'; ;";
        assert_eq!(statements("", reply).unwrap(), ["SELECT 1", "-- note\nSELECT ';'"]);
    }

    #[test]
    fn the_literals_of_the_statement_asked_about_are_put_back() {
        let reply = "SELECT id FROM customers WHERE name = '<literal 1>'";
        assert_eq!(
            statements("SELECT * FROM customers WHERE name = 'nina'", reply).unwrap(),
            ["SELECT id FROM customers WHERE name = 'nina'"]
        );
    }

    #[test]
    fn an_invented_marker_gives_no_statement() {
        assert_eq!(statements("SELECT 1", "SELECT <number 2>"), Err(AnswerError::UnknownMarker("<number 2>".into())));
    }

    #[test]
    fn an_answer_of_comments_alone_gives_no_statement() {
        assert_eq!(statements("", "```sql\n-- nothing to do\n```"), Err(AnswerError::NoStatement));
    }
}
