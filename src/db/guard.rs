//! Classifies a statement before it is sent: a read, a write, or more than one statement.
//! A best effort that spares a round trip; the read-only transaction and role are what hold.
use super::lex::{self, Placed, Token};
use super::model::{Params, Statement};
use crate::connection::WritePolicy;
use std::fmt;
use std::ops::Range;

const READS: [&str; 6] = ["SELECT", "WITH", "VALUES", "TABLE", "EXPLAIN", "SHOW"];
/// Words that change data, create a table (`SELECT ... INTO`) or run the statement (`EXPLAIN ANALYZE`).
const WRITES: [&str; 7] = ["INSERT", "UPDATE", "DELETE", "MERGE", "INTO", "ANALYZE", "ANALYSE"];
/// Row locks a `SELECT` takes; `FOR UPDATE` is caught by `UPDATE` already.
const LOCKS: [[&str; 2]; 2] = [["FOR", "SHARE"], ["KEY", "SHARE"]];

/// A statement cleared to run, the values bound to its `$n`, and how to run it; `admit` makes one.
#[derive(Debug)]
pub(crate) struct Admitted {
    pub(super) statement: Statement,
    pub(super) params: Params,
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
    /// The statement numbers `expected` parameters, and `given` values came with it.
    Params {
        expected: usize,
        given: usize,
    },
}

/// `statement` with `params` bound to its `$n`, cleared for a connection of policy `writes`, or why it is not.
pub(crate) fn admit(statement: Statement, params: Params, writes: WritePolicy) -> Result<Admitted, Refusal> {
    let tokens = tokens(statement.as_str());
    let mode = match writes {
        WritePolicy::Allowed => Mode::Writes,
        WritePolicy::ReadOnly => check_read(&tokens).map(|()| Mode::ReadOnly)?,
    };
    check_params(&tokens, &params)?;
    Ok(Admitted { statement, params, mode })
}

/// Only when values came: a statement without any may number `$n` of its own (`CREATE FUNCTION ... RETURN $1 + 1`),
/// and the server judges those.
fn check_params(tokens: &[Token], params: &Params) -> Result<(), Refusal> {
    if params.is_empty() {
        return Ok(());
    }
    let numbered = tokens.iter().filter_map(|token| if let Token::Numbered(n) = token { Some(*n) } else { None });
    let expected = numbered.max().map_or(0, |highest| usize::try_from(highest).unwrap_or(usize::MAX));
    if expected == params.len() { Ok(()) } else { Err(Refusal::Params { expected, given: params.len() }) }
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

/// Where the statement the cursor at byte `at` of `sql` stands in begins and ends, its `;` and the
/// whitespace around it left out; `None` when `sql` holds no statement. A cursor right after a `;`, or on blank text after
/// it, stands in the statement before it.
pub(crate) fn statement_at(sql: &str, at: usize) -> Option<Range<usize>> {
    let pieces = pieces(&lex::tokens(sql), sql.len());
    let holding = pieces.iter().position(|piece| at <= piece.range.end + 1).unwrap_or(pieces.len() - 1);
    let statement = pieces[..=holding].iter().rev().find(|piece| piece.has_tokens)?;
    Some(trimmed(sql, statement.range.clone()))
}

/// The text between two `;`, and whether it holds anything but comments and whitespace.
struct Piece {
    range: Range<usize>,
    has_tokens: bool,
}

fn pieces(tokens: &[Placed], len: usize) -> Vec<Piece> {
    let mut pieces = vec![];
    let mut start = 0;
    let mut has_tokens = false;
    for placed in tokens {
        if placed.token == Token::End {
            pieces.push(Piece { range: start..placed.range.start, has_tokens });
            (start, has_tokens) = (placed.range.end, false);
        } else {
            has_tokens = true;
        }
    }
    pieces.push(Piece { range: start..len, has_tokens });
    pieces
}

fn trimmed(sql: &str, range: Range<usize>) -> Range<usize> {
    let text = &sql[range.clone()];
    let start = range.start + text.len() - text.trim_start_matches(lex::is_blank).len();
    let end = range.end - (text.len() - text.trim_end_matches(lex::is_blank).len());
    start..end
}

fn holds_more_than_one_statement(tokens: &[Token]) -> bool {
    let mut after_first_end = tokens.iter().skip_while(|token| **token != Token::End).skip(1);
    after_first_end.any(|token| *token != Token::End)
}

fn first_write_word<'a>(words: &[&'a str]) -> Option<&'a str> {
    let locks = words.windows(2).find(|pair| LOCKS.contains(&[pair[0], pair[1]])).map(|pair| pair[1]);
    words.iter().copied().find(|word| WRITES.contains(word)).or(locks)
}

fn tokens(sql: &str) -> Vec<Token> {
    lex::tokens(sql).into_iter().map(|placed| placed.token).collect()
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write(word) => {
                write!(f, "`{word}` is not a read, and this connection is read only: set `writes = \"allowed\"` on it to run writes")
            }
            Self::Multiple => f.write_str("one statement at a time: this text holds more than one"),
            Self::Params { expected, given } => {
                write!(f, "the statement takes {}, and {} came with it", counted(*expected, "parameter"), counted(*given, "value"))
            }
        }
    }
}

impl std::error::Error for Refusal {}

fn counted(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::ParamValue;

    fn verdict(sql: &str) -> Result<Mode, Refusal> {
        admit(Statement::new(sql), Params::default(), WritePolicy::ReadOnly).map(|admitted| admitted.mode)
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

    fn statement_under(marked: &str) -> Option<String> {
        let (sql, at) = (marked.replace('|', ""), marked.find('|').unwrap());
        statement_at(&sql, at).map(|range| sql[range].to_owned())
    }

    #[test]
    fn the_cursor_picks_the_statement_it_stands_in() {
        assert_eq!(statement_under("SELECT 1;\nSELECT |2;\nSELECT 3"), Some("SELECT 2".into()));
        assert_eq!(statement_under("|SELECT 1; SELECT 2"), Some("SELECT 1".into()));
    }

    #[test]
    fn a_cursor_right_after_a_semicolon_or_on_blank_text_after_it_picks_the_statement_before() {
        assert_eq!(statement_under("SELECT 1;| SELECT 2"), Some("SELECT 1".into()));
        assert_eq!(statement_under("SELECT 1;\n  -- done\n|"), Some("SELECT 1".into()));
    }

    #[test]
    fn a_semicolon_in_a_string_or_a_comment_splits_nothing() {
        assert_eq!(
            statement_under("SELECT ';' AS mark, $$;$$ /* ; */ -- ;\nFROM orders|"),
            Some("SELECT ';' AS mark, $$;$$ /* ; */ -- ;\nFROM orders".into())
        );
    }

    #[test]
    fn a_no_break_space_at_the_end_stays_in_the_statement() {
        assert_eq!(statement_under("SELECT a\u{a0}|"), Some("SELECT a\u{a0}".into()));
    }

    #[test]
    fn blank_text_holds_no_statement() {
        assert_eq!(statement_under("  -- nothing\n|"), None);
    }

    #[test]
    fn a_connection_that_allows_writes_runs_anything_as_is() {
        let admitted = admit(Statement::new("DELETE FROM orders; DELETE FROM customers"), Params::default(), WritePolicy::Allowed);
        assert_eq!(admitted.map(|admitted| admitted.mode), Ok(Mode::Writes));
    }

    fn admitted_with(sql: &str, values: usize, writes: WritePolicy) -> Result<Mode, Refusal> {
        let params = Params::new(vec![Some(ParamValue::new("7")); values], vec![]);
        admit(Statement::new(sql), params, writes).map(|admitted| admitted.mode)
    }

    #[test]
    fn a_statement_runs_with_one_value_for_each_of_its_parameters() {
        assert_eq!(admitted_with("SELECT * FROM orders WHERE id = $1 OR id = $2 OR id = $1", 2, WritePolicy::ReadOnly), Ok(Mode::ReadOnly));
    }

    #[test]
    fn a_count_of_values_that_is_not_the_count_of_parameters_is_refused() {
        assert_eq!(admitted_with("SELECT $1, $2", 1, WritePolicy::ReadOnly), Err(Refusal::Params { expected: 2, given: 1 }));
        assert_eq!(admitted_with("SELECT 1", 1, WritePolicy::Allowed), Err(Refusal::Params { expected: 0, given: 1 }));
    }

    #[test]
    fn a_statement_given_no_values_leaves_its_own_numbers_to_the_server() {
        let function = "CREATE FUNCTION add1(int) RETURNS int RETURN $1 + 1";
        assert_eq!(admitted_with(function, 0, WritePolicy::Allowed), Ok(Mode::Writes));
        assert_eq!(admitted_with("PREPARE p(int) AS SELECT $1", 0, WritePolicy::Allowed), Ok(Mode::Writes));
    }

    #[test]
    fn a_refused_count_of_one_says_parameter() {
        assert_eq!(Refusal::Params { expected: 1, given: 2 }.to_string(), "the statement takes 1 parameter, and 2 values came with it");
        assert_eq!(Refusal::Params { expected: 2, given: 1 }.to_string(), "the statement takes 2 parameters, and 1 value came with it");
    }

    #[test]
    fn a_parameter_in_a_string_needs_no_value() {
        assert_eq!(admitted_with("SELECT '$1'", 0, WritePolicy::ReadOnly), Ok(Mode::ReadOnly));
    }
}
