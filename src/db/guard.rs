//! Classifies a statement before it is sent: a read, a write, or more than one statement.
//! A best effort that spares a round trip; the read-only transaction and role are what hold.
use super::model::Statement;
use crate::connection::WritePolicy;
use std::fmt;

const READS: [&str; 6] = ["SELECT", "WITH", "VALUES", "TABLE", "EXPLAIN", "SHOW"];
/// Words that change data, create a table (`SELECT ... INTO`) or run the statement (`EXPLAIN ANALYZE`).
const WRITES: [&str; 7] = ["INSERT", "UPDATE", "DELETE", "MERGE", "INTO", "ANALYZE", "ANALYSE"];
/// Row locks a `SELECT` takes; `FOR UPDATE` is caught by `UPDATE` already.
const LOCKS: [[&str; 2]; 2] = [["FOR", "SHARE"], ["KEY", "SHARE"]];

/// A statement cleared to run, and how to run it; `admit` makes one.
#[derive(Debug)]
pub(crate) struct Admitted {
    pub(super) statement: Statement,
    pub(super) mode: Mode,
}

/// How a session runs an admitted statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// Inside `BEGIN READ ONLY`, rolled back.
    ReadOnly,
    /// As is, on a connection that allows writes.
    Writes,
}

/// Why a statement is not sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The statement may change something; holds the word that says so.
    Write(String),
    Multiple,
}

/// `statement` cleared for a connection of policy `writes`, or why it is not.
pub(crate) fn admit(statement: Statement, writes: WritePolicy) -> Result<Admitted, Refusal> {
    if writes == WritePolicy::Allowed {
        return Ok(Admitted { statement, mode: Mode::Writes });
    }
    check_read(&tokens(statement.as_str()))?;
    Ok(Admitted { statement, mode: Mode::ReadOnly })
}

fn check_read(tokens: &[Token]) -> Result<(), Refusal> {
    if holds_more_than_one_statement(tokens) {
        return Err(Refusal::Multiple);
    }
    let words: Vec<&str> = tokens.iter().filter_map(Token::word).collect();
    let Some(&first) = words.first() else { return Ok(()) };
    if !READS.contains(&first) {
        return Err(Refusal::Write(first.to_owned()));
    }
    match first_write_word(&words) {
        Some(word) => Err(Refusal::Write(word.to_owned())),
        None => Ok(()),
    }
}

fn holds_more_than_one_statement(tokens: &[Token]) -> bool {
    let mut after_first_end = tokens.iter().skip_while(|token| **token != Token::End).skip(1);
    after_first_end.any(|token| *token != Token::End)
}

fn first_write_word<'a>(words: &[&'a str]) -> Option<&'a str> {
    let locks = words.windows(2).find(|pair| LOCKS.contains(&[pair[0], pair[1]])).map(|pair| pair[1]);
    words.iter().copied().find(|word| WRITES.contains(word)).or(locks)
}

/// The words of `sql` in upper case, its `;`, and one `Other` for anything else: comments and
/// whitespace drop out, and string literals and quoted identifiers hide what they hold.
fn tokens(sql: &str) -> Vec<Token> {
    let mut tokens = vec![];
    let mut rest = sql;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        rest = match c {
            '-' if after.starts_with('-') => after.find(['\n', '\r']).map_or("", |end| &after[end..]),
            '/' if after.starts_with('*') => after_block_comment(&after[1..]),
            c if c.is_whitespace() => after,
            ';' => {
                tokens.push(Token::End);
                after
            }
            c if c.is_alphabetic() || c == '_' => {
                let end = rest.find(|c: char| !is_word_char(c)).unwrap_or(rest.len());
                let word = rest[..end].to_uppercase();
                let next = &rest[end..];
                let is_escape_string = word == "E" && next.starts_with('\'');
                tokens.push(Token::Word(word));
                if is_escape_string { after_escape_string(&next[1..]) } else { next }
            }
            '\'' | '"' => {
                tokens.push(Token::Other);
                after_quoted(after, c)
            }
            '$' => {
                tokens.push(Token::Other);
                after_dollar(rest)
            }
            _ => {
                tokens.push(Token::Other);
                after
            }
        };
    }
    tokens
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn after_block_comment(body: &str) -> &str {
    let mut depth = 1;
    let mut rest = body;
    while depth > 0 {
        let Some(at) = rest.find(['*', '/']) else { return "" };
        let mark = &rest[at..];
        rest = if let Some(after) = mark.strip_prefix("*/") {
            depth -= 1;
            after
        } else if let Some(after) = mark.strip_prefix("/*") {
            depth += 1;
            after
        } else {
            &mark[1..]
        };
    }
    rest
}

/// After a literal opened by `quote`, where a doubled quote stands for itself.
fn after_quoted(body: &str, quote: char) -> &str {
    let mut rest = body;
    while let Some(at) = rest.find(quote) {
        let after = &rest[at + 1..];
        if !after.starts_with(quote) {
            return after;
        }
        rest = &after[1..];
    }
    ""
}

/// After an `E'...'` literal, where a backslash also escapes the next character.
fn after_escape_string(body: &str) -> &str {
    let mut chars = body.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '\'' if body[at + 1..].starts_with('\'') => {
                chars.next();
            }
            '\'' => return &body[at + 1..],
            _ => {}
        }
    }
    ""
}

/// After a `$tag$...$tag$` literal; a `$` that opens none, such as the parameter `$1`, is skipped alone.
fn after_dollar(rest: &str) -> &str {
    let tag_end = rest[1..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map(|at| at + 1);
    let opens_quote = tag_end.is_some_and(|end| rest[end..].starts_with('$')) && !rest[1..].starts_with(|c: char| c.is_ascii_digit());
    match tag_end {
        Some(end) if opens_quote => {
            let tag = &rest[..=end];
            let body = &rest[tag.len()..];
            body.find(tag).map_or("", |at| &body[at + tag.len()..])
        }
        _ => &rest[1..],
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    End,
    Other,
}

impl Token {
    fn word(&self) -> Option<&str> {
        match self {
            Self::Word(word) => Some(word),
            Self::End | Self::Other => None,
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write(word) => {
                write!(f, "`{word}` is not a read, and this connection is read only: set `writes = \"allowed\"` on it to run writes")
            }
            Self::Multiple => f.write_str("one statement at a time: this text holds more than one"),
        }
    }
}

impl std::error::Error for Refusal {}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(sql: &str) -> Result<Mode, Refusal> {
        admit(Statement::new(sql), WritePolicy::ReadOnly).map(|admitted| admitted.mode)
    }

    fn refused_word(sql: &str) -> String {
        match verdict(sql) {
            Err(Refusal::Write(word)) => word,
            other => panic!("{sql}: {other:?}"),
        }
    }

    #[test]
    fn a_select_is_a_read() {
        assert_eq!(verdict("SELECT id, total FROM orders WHERE total > 10"), Ok(Mode::ReadOnly));
    }

    #[test]
    fn keywords_read_whatever_their_case() {
        assert_eq!(verdict("select * from orders"), Ok(Mode::ReadOnly));
        assert_eq!(refused_word("delete from orders"), "DELETE");
    }

    #[test]
    fn values_table_and_show_are_reads() {
        assert_eq!(verdict("VALUES (1, 'nina')"), Ok(Mode::ReadOnly));
        assert_eq!(verdict("TABLE orders"), Ok(Mode::ReadOnly));
        assert_eq!(verdict("SHOW search_path"), Ok(Mode::ReadOnly));
    }

    #[test]
    fn a_with_that_only_selects_is_a_read() {
        assert_eq!(verdict("WITH recent AS (SELECT * FROM orders) SELECT count(*) FROM recent"), Ok(Mode::ReadOnly));
    }

    #[test]
    fn a_with_that_deletes_is_a_write() {
        assert_eq!(refused_word("WITH gone AS (DELETE FROM orders RETURNING id) SELECT count(*) FROM gone"), "DELETE");
    }

    #[test]
    fn insert_update_delete_and_merge_are_writes() {
        assert_eq!(refused_word("INSERT INTO orders VALUES (1)"), "INSERT");
        assert_eq!(refused_word("UPDATE orders SET total = 0"), "UPDATE");
        assert_eq!(refused_word("DELETE FROM orders"), "DELETE");
        assert_eq!(refused_word("MERGE INTO orders USING customers ON true WHEN MATCHED THEN DELETE"), "MERGE");
    }

    #[test]
    fn an_explain_is_a_read() {
        assert_eq!(verdict("EXPLAIN (FORMAT JSON) SELECT * FROM orders"), Ok(Mode::ReadOnly));
    }

    #[test]
    fn an_explain_analyze_runs_the_statement_so_it_is_a_write() {
        assert_eq!(refused_word("EXPLAIN ANALYZE SELECT * FROM orders"), "ANALYZE");
        assert_eq!(refused_word("explain (analyse, buffers) SELECT * FROM orders"), "ANALYSE");
    }

    #[test]
    fn a_select_that_locks_rows_is_a_write() {
        assert_eq!(refused_word("SELECT * FROM orders FOR UPDATE"), "UPDATE");
        assert_eq!(refused_word("SELECT * FROM orders FOR SHARE"), "SHARE");
        assert_eq!(refused_word("SELECT * FROM orders FOR KEY SHARE SKIP LOCKED"), "SHARE");
    }

    #[test]
    fn a_select_into_creates_a_table_so_it_is_a_write() {
        assert_eq!(refused_word("SELECT * INTO archived_orders FROM orders"), "INTO");
    }

    #[test]
    fn copy_set_and_ddl_are_not_reads() {
        assert_eq!(refused_word("COPY orders TO STDOUT"), "COPY");
        assert_eq!(refused_word("SET search_path = shop"), "SET");
        assert_eq!(refused_word("DROP TABLE orders"), "DROP");
        assert_eq!(refused_word("BEGIN"), "BEGIN");
    }

    #[test]
    fn two_statements_are_refused() {
        assert_eq!(verdict("SELECT 1; SELECT 2"), Err(Refusal::Multiple));
        assert_eq!(verdict("SELECT 1; 2"), Err(Refusal::Multiple));
    }

    #[test]
    fn a_trailing_semicolon_and_comment_leave_one_statement() {
        assert_eq!(verdict("SELECT 1; -- done\n"), Ok(Mode::ReadOnly));
        assert_eq!(verdict("SELECT 1;;"), Ok(Mode::ReadOnly));
    }

    #[test]
    fn keywords_inside_comments_are_ignored() {
        assert_eq!(verdict("-- DELETE FROM orders;\nSELECT 1"), Ok(Mode::ReadOnly));
        assert_eq!(verdict("/* DROP /* nested */ TABLE orders; */ SELECT 1"), Ok(Mode::ReadOnly));
    }

    #[test]
    fn keywords_inside_string_literals_are_ignored() {
        assert_eq!(verdict("SELECT 'it''s; DELETE FROM orders' AS note"), Ok(Mode::ReadOnly));
        assert_eq!(verdict(r"SELECT E'nina\'s; DELETE' AS note"), Ok(Mode::ReadOnly));
        assert_eq!(verdict("SELECT $body$; DELETE FROM orders$body$, $$;UPDATE$$"), Ok(Mode::ReadOnly));
    }

    #[test]
    fn a_quoted_identifier_is_no_keyword() {
        assert_eq!(verdict(r#"SELECT "delete", "a;b" FROM orders"#), Ok(Mode::ReadOnly));
    }

    #[test]
    fn a_carriage_return_ends_a_line_comment() {
        assert_eq!(refused_word("SELECT * FROM orders --\rFOR UPDATE"), "UPDATE");
    }

    #[test]
    fn a_comment_cannot_hide_a_leading_write() {
        assert_eq!(refused_word("/* SELECT */ DELETE FROM orders"), "DELETE");
    }

    #[test]
    fn a_parameter_or_a_lone_dollar_hides_nothing() {
        assert_eq!(refused_word("SELECT * FROM orders WHERE id = $1 FOR UPDATE"), "UPDATE");
        assert_eq!(refused_word("SELECT $tag FROM orders FOR UPDATE"), "UPDATE");
    }

    #[test]
    fn a_connection_that_allows_writes_runs_anything_as_is() {
        let admitted = admit(Statement::new("DELETE FROM orders; DELETE FROM customers"), WritePolicy::Allowed);
        assert_eq!(admitted.map(|admitted| admitted.mode), Ok(Mode::Writes));
    }
}
