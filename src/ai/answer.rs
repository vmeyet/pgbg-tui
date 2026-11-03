//! A command's answer read back: the SQL out of its Markdown fence, its markers put back, split into statements.
use super::mask::{Literals, UnknownMarker};
use crate::db::guard;
use crate::db::lex::{self, Token};
use crate::db::model::Statement;
use std::fmt;

const FENCE: &str = "```";
/// What the prompt asks a line to open on when the command cannot answer without a guess.
const CANNOT: &str = "cannot:";

/// Why an answer gives no statement to show.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AnswerError {
    /// The answer holds a marker that stands for no literal of the statement asked about; holds the marker.
    UnknownMarker(String),
    /// The command said why it wrote no statement, in its own words, which may quote what the user typed: for the screen,
    /// never for the log.
    Cannot(String),
    NoStatement,
}

/// The statements `reply` holds, with the literals `literals` masked put back.
pub(super) fn read(reply: &str, literals: &Literals) -> Result<Vec<Statement>, AnswerError> {
    let sql = literals.put_back(fenced(reply))?;
    let statements = split(&sql);
    let is_prose = !reply.contains(FENCE) && !guard::opens_a_statement(&sql);
    if statements.is_empty() || is_prose { Err(no_statement(reply)) } else { Ok(statements) }
}

/// The words of `reply`, one that holds no statement, on one line, without its fences and its `--` and `cannot:` marks.
fn no_statement(reply: &str) -> AnswerError {
    let lines = reply.lines().map(str::trim).filter(|line| !line.starts_with(FENCE));
    let words = lines.map(|line| {
        let line = line.strip_prefix("--").unwrap_or(line).trim_start();
        line.strip_prefix(CANNOT).unwrap_or(line).trim_start()
    });
    let reason = words.filter(|words| !words.is_empty()).collect::<Vec<_>>().join(" ");
    if reason.is_empty() { AnswerError::NoStatement } else { AnswerError::Cannot(reason) }
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
            Self::Cannot(why) => write!(f, "no statement: {why}"),
            Self::NoStatement => f.write_str("the answer holds no statement"),
        }
    }
}

impl std::error::Error for AnswerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::mask::{LiteralPolicy, masked};
    use crate::db::model::Catalog;

    fn statements(sql: &str, reply: &str) -> Result<Vec<String>, AnswerError> {
        let statements = read(reply, &masked(sql, LiteralPolicy::Masked, &Catalog::default()).literals)?;
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
    fn a_chart_line_stays_above_each_statement_it_leads() {
        let reply = "```sql\n-- chart: line x=week y=total\nSELECT 1;\n-- chart: stacked\nSELECT 2\n```";
        assert_eq!(statements("", reply).unwrap(), ["-- chart: line x=week y=total\nSELECT 1", "-- chart: stacked\nSELECT 2"]);
    }

    #[test]
    fn an_answer_of_comments_alone_gives_their_words() {
        assert_eq!(statements("", "```sql\n-- nothing to do\n```"), Err(AnswerError::Cannot("nothing to do".into())));
    }

    #[test]
    fn the_line_the_prompt_asks_for_gives_its_reason_inside_a_fence_or_not() {
        let cannot = Err(AnswerError::Cannot("no key links orders and invoices".into()));
        assert_eq!(statements("", "-- cannot: no key links orders and invoices\n"), cannot);
        assert_eq!(statements("", "```sql\n-- cannot: no key links orders and invoices\n```"), cannot);
    }

    #[test]
    fn prose_without_a_fence_is_the_reason_and_no_statement() {
        let reply = "I can't write this:\nthe columns of invoices are not listed.";
        assert_eq!(statements("", reply), Err(AnswerError::Cannot("I can't write this: the columns of invoices are not listed.".into())));
    }

    #[test]
    fn prose_around_an_empty_fence_is_the_reason() {
        assert_eq!(statements("", "No key is listed.\n```sql\n```"), Err(AnswerError::Cannot("No key is listed.".into())));
    }

    #[test]
    fn a_reply_with_no_words_gives_no_statement() {
        assert_eq!(statements("", "```sql\n```"), Err(AnswerError::NoStatement));
        assert_eq!(statements("", ""), Err(AnswerError::NoStatement));
    }

    #[test]
    fn a_statement_under_a_comment_or_in_brackets_without_a_fence_stays_a_statement() {
        assert_eq!(statements("", "-- the ids\nSELECT id FROM orders").unwrap(), ["-- the ids\nSELECT id FROM orders"]);
        assert_eq!(statements("", "(SELECT 1)").unwrap(), ["(SELECT 1)"]);
    }

    #[test]
    fn the_reason_is_shown_after_saying_there_is_no_statement() {
        assert_eq!(AnswerError::Cannot("no key".into()).to_string(), "no statement: no key");
    }
}
