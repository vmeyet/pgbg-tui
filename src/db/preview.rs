//! What a write would touch, read before its confirmation: an `UPDATE` or `DELETE` rewritten as a read that counts the
//! rows its `WHERE` picks, so the count runs read only like any read and never runs the write itself.
use super::lex::{self, Placed, Token};
use super::model::{ResultSet, Statement, Value};
use crate::connection::StatementTimeout;
use crate::format;
use std::fmt;
use std::ops::Range;

/// How long a count may take: the write waits behind it once confirmed.
pub(crate) const COUNT_TIMEOUT: StatementTimeout = StatementTimeout::from_millis(2_000);

/// The rows a count reads at most, and one more, so a write over a big table costs no more to count than a small one.
const COUNTED_ROWS: u64 = 10_000;

/// How many rows a write would touch, as its count read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Touched {
    Rows(u64),
    /// More than this many: the count stopped there.
    MoreThan(u64),
}

/// A word of a statement outside any parentheses, upper case, and the bytes it spans.
struct TopWord {
    word: String,
    range: Range<usize>,
}

/// The parts of an `UPDATE` or `DELETE` a count reads, as bytes of its text: the table it writes, with its alias, the
/// tables it joins (`USING`, `FROM`), and its `WHERE` condition.
struct Parts {
    target: Range<usize>,
    joined: Option<Range<usize>>,
    condition: Option<Range<usize>>,
}

/// The read that counts the rows `write` would touch, its `{{name}}` kept as they are; none for a write it cannot read
/// that way.
pub(crate) fn count_of(write: &Statement) -> Option<Statement> {
    let sql = &lex::blank_comments(write.as_str());
    let tokens = lex::tokens(sql);
    let end = tokens.iter().find(|placed| placed.token == Token::End).map_or(sql.len(), |placed| placed.range.start);
    let parts = parts(&top_words(sql, &tokens), end)?;
    Some(Statement::new(count_text(sql, &tokens, &parts)))
}

fn top_words(sql: &str, tokens: &[Placed]) -> Vec<TopWord> {
    let mut depth = 0_usize;
    let mut words = vec![];
    for placed in tokens {
        match &sql[placed.range.clone()] {
            "(" => depth += 1,
            ")" => depth = depth.saturating_sub(1),
            word if depth == 0 && placed.token == Token::Word => {
                words.push(TopWord { word: word.to_ascii_uppercase(), range: placed.range.clone() });
            }
            _ => {}
        }
    }
    words
}

/// Where the table a write names stands, alias included, and the word that opens the tables it joins.
struct Target {
    range: Range<usize>,
    joiner: &'static str,
}

/// The table of the `DELETE FROM` or `UPDATE ... SET` that `words` start, a statement ending at byte `end`.
fn target(words: &[TopWord], end: usize) -> Option<Target> {
    let start_of = |wanted: &[&str]| words.iter().find(|top| wanted.contains(&top.word.as_str())).map(|top| top.range.start);
    match words {
        [delete, from, ..] if delete.word == "DELETE" && from.word == "FROM" => {
            let target_end = start_of(&["USING", "WHERE", "RETURNING"]).unwrap_or(end);
            Some(Target { range: from.range.end..target_end, joiner: "USING" })
        }
        [update, ..] if update.word == "UPDATE" => Some(Target { range: update.range.end..start_of(&["SET"])?, joiner: "FROM" }),
        _ => None,
    }
}

/// The parts of the write `words` start, a statement ending at byte `end`; none for a write `target` cannot read, or
/// one whose `WHERE` names a cursor's row (`CURRENT OF`).
fn parts(words: &[TopWord], end: usize) -> Option<Parts> {
    let target = target(words, end)?;
    let rest = &words[words.partition_point(|top| top.range.start < target.range.end)..];
    let condition_at = rest.iter().position(|top| top.word == "WHERE");
    if condition_at.is_some_and(|at| opens_current_of(&rest[at + 1..])) {
        return None;
    }
    let returning_start = rest.iter().find(|top| top.word == "RETURNING").map_or(end, |top| top.range.start);
    let joined_end = condition_at.map_or(returning_start, |at| rest[at].range.start);
    let joiner_at = (0..rest.len()).find(|&at| rest[at].range.start < joined_end && is_joiner(rest, at, target.joiner));
    Some(Parts {
        target: target.range,
        joined: joiner_at.map(|at| rest[at].range.end..joined_end),
        condition: condition_at.map(|at| rest[at].range.end..returning_start),
    })
}

/// Whether the word at `at` of `words` is `joiner`, and not the `FROM` of `IS DISTINCT FROM`.
fn is_joiner(words: &[TopWord], at: usize, joiner: &str) -> bool {
    let is_distinct_from = at > 0 && words[at - 1].word == "DISTINCT";
    words[at].word == joiner && !is_distinct_from
}

fn opens_current_of(words: &[TopWord]) -> bool {
    matches!(words, [current, of, ..] if current.word == "CURRENT" && of.word == "OF")
}

/// `SELECT count(*)` of the rows of the target that the condition picks, each row once, as a write touches it once.
/// Joined to other tables, the rows come out by their place (`tableoid`, `ctid`) from the same `FROM` and `WHERE` the
/// write reads, so each name binds as it does in the write, and fails as it does.
fn count_text(sql: &str, tokens: &[Placed], parts: &Parts) -> String {
    let text = |range: &Range<usize>| sql[range.clone()].trim();
    let target = text(&parts.target);
    let rows = match &parts.joined {
        Some(joined) => {
            let named = row_name(sql, tokens, &parts.target);
            format!("DISTINCT {named}.tableoid, {named}.ctid FROM {target}, {}", text(joined))
        }
        None => format!("FROM {target}"),
    };
    let filter = parts.condition.as_ref().map_or_else(String::new, |condition| format!(" WHERE {}", text(condition)));
    format!("SELECT count(*) FROM (SELECT {rows}{filter} LIMIT {}) AS touched", COUNTED_ROWS + 1)
}

/// The name a column of the target is qualified with: its alias, else its table's name without its schema; the last
/// name among its tokens either way, past an inheritance `*`.
fn row_name<'a>(sql: &'a str, tokens: &[Placed], target: &Range<usize>) -> &'a str {
    let is_name = |placed: &Placed| placed.token == Token::Word || sql[placed.range.clone()].starts_with('"');
    let last = tokens.iter().rfind(|placed| target.contains(&placed.range.start) && is_name(placed));
    last.map_or("", |placed| &sql[placed.range.clone()])
}

impl Touched {
    /// What the read `count_of` gave says.
    pub(crate) fn read(counted: &ResultSet) -> Option<Self> {
        let Value::Int(rows) = counted.rows().first()?.first()? else { return None };
        let rows = u64::try_from(*rows).ok()?;
        Some(if rows > COUNTED_ROWS { Self::MoreThan(COUNTED_ROWS) } else { Self::Rows(rows) })
    }
}

impl fmt::Display for Touched {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let grouped = |rows: &u64| format::grouped_digits(&rows.to_string());
        match self {
            Self::Rows(0) => f.write_str("no row"),
            Self::Rows(1) => f.write_str("1 row"),
            Self::Rows(rows) => write!(f, "{} rows", grouped(rows)),
            Self::MoreThan(rows) => write!(f, "more than {} rows", grouped(rows)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::guard;
    use crate::db::model::{ColumnKind, Params};
    use crate::fixtures::{changing_the_catalog, column, run, session};

    fn count(write: &str) -> Option<String> {
        count_of(&Statement::new(write)).map(|read| read.as_str().to_owned())
    }

    /// What the count of `write` reads its rows from: the text between `SELECT` and its cap.
    fn picked(write: &str) -> Option<String> {
        let read = count(write)?;
        Some(read.strip_prefix("SELECT count(*) FROM (SELECT ")?.strip_suffix(" LIMIT 10001) AS touched")?.to_owned())
    }

    fn counted(rows: i64) -> ResultSet {
        ResultSet::whole(vec![column("count", ColumnKind::Int)], vec![vec![Value::Int(rows)]])
    }

    #[test]
    fn a_delete_counts_the_rows_its_where_picks_up_to_its_cap() {
        let read = count("DELETE FROM orders WHERE status = 'void'");
        assert_eq!(read.as_deref(), Some("SELECT count(*) FROM (SELECT FROM orders WHERE status = 'void' LIMIT 10001) AS touched"));
    }

    #[test]
    fn a_delete_without_where_counts_every_row() {
        assert_eq!(picked("delete from only shop.orders o").as_deref(), Some("FROM only shop.orders o"));
    }

    #[test]
    fn an_update_leaves_its_set_and_its_returning_out() {
        let write = "UPDATE orders AS o SET total = (SELECT max(total) FROM orders) WHERE id = {{id}} RETURNING *";
        assert_eq!(picked(write).as_deref(), Some("FROM orders AS o WHERE id = {{id}}"));
    }

    #[test]
    fn a_write_joining_other_tables_counts_each_row_of_its_own_once() {
        let write = "UPDATE orders o SET status = 'void' FROM customers c WHERE c.id = o.customer_id";
        assert_eq!(picked(write).as_deref(), Some("DISTINCT o.tableoid, o.ctid FROM orders o, customers c WHERE c.id = o.customer_id"));
    }

    #[test]
    fn a_joining_write_without_alias_names_its_rows_by_its_table() {
        let write = "DELETE FROM only shop.\"Orders\" USING customers;";
        assert_eq!(picked(write).as_deref(), Some("DISTINCT \"Orders\".tableoid, \"Orders\".ctid FROM only shop.\"Orders\", customers"));
    }

    #[test]
    fn a_line_comment_at_the_end_of_a_part_leaves_the_rest_of_the_count_alone() {
        assert_eq!(picked("DELETE FROM orders -- all of them\nWHERE id = 7 -- the bad one").as_deref(), Some("FROM orders WHERE id = 7"));
    }

    #[test]
    fn a_distinct_from_in_a_set_is_no_join() {
        let write = "UPDATE orders SET is_changed = total IS DISTINCT FROM paid WHERE id = 7";
        assert_eq!(picked(write).as_deref(), Some("FROM orders WHERE id = 7"));
    }

    #[test]
    fn a_column_named_current_is_no_cursor() {
        assert_eq!(picked("DELETE FROM orders WHERE current = 1").as_deref(), Some("FROM orders WHERE current = 1"));
    }

    #[test]
    fn a_write_the_count_cannot_read_has_none() {
        for write in [
            "INSERT INTO orders VALUES (1)",
            "TRUNCATE orders",
            "WITH old AS (SELECT 1) DELETE FROM orders",
            "DELETE FROM orders WHERE CURRENT OF pgbg_result_1",
            "UPDATE orders",
            "DELETE orders",
        ] {
            assert_eq!(count(write), None, "{write}");
        }
    }

    #[test]
    fn a_count_reads_as_the_rows_it_found() {
        assert_eq!(Touched::read(&counted(42)), Some(Touched::Rows(42)));
    }

    #[test]
    fn a_count_past_its_cap_reads_as_more_than_the_cap() {
        assert_eq!(Touched::read(&counted(10_001)), Some(Touched::MoreThan(10_000)));
    }

    #[test]
    fn a_count_says_its_rows_in_words() {
        let said: Vec<String> =
            [Touched::Rows(0), Touched::Rows(1), Touched::Rows(1_732), Touched::MoreThan(10_000)].map(|touched| touched.to_string()).into();
        assert_eq!(said, ["no row", "1 row", "1 732 rows", "more than 10 000 rows"]);
    }

    #[test]
    fn every_count_is_a_read_the_guard_admits_read_only() {
        let write = "UPDATE orders o SET status = 'void' FROM customers c WHERE c.id = o.customer_id RETURNING o.id";
        let read = count_of(&Statement::new(write)).unwrap();
        assert!(guard::admit(read, Params::default(), WritePolicy::ReadOnly).is_ok());
    }

    /// The rows `write` would touch, counted read only on a session that allows writes.
    async fn counted_on_the_server(write: &str) -> anyhow::Result<Option<Touched>> {
        let read = guard::admit(count_of(&Statement::new(write)).unwrap(), Params::default(), WritePolicy::ReadOnly).unwrap();
        let _changing = changing_the_catalog().await;
        Ok(Touched::read(&session(WritePolicy::Allowed).await.run(&read).await?))
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn an_update_joining_a_table_counts_each_of_its_rows_once() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS pgbg_preview_items, pgbg_preview_orders").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE pgbg_preview_orders AS SELECT n AS id FROM generate_series(1, 3) AS n").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE pgbg_preview_items AS SELECT 1 AS order_id UNION ALL SELECT 1 UNION ALL SELECT 2")
            .await
            .unwrap();
        let write = "UPDATE pgbg_preview_orders o SET id = id FROM pgbg_preview_items i WHERE i.order_id = o.id";
        assert_eq!(counted_on_the_server(write).await.unwrap(), Some(Touched::Rows(2)));
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_count_fails_on_a_column_the_write_finds_ambiguous() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS pgbg_preview_lines, pgbg_preview_carts").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE pgbg_preview_carts AS SELECT 7 AS id").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE pgbg_preview_lines AS SELECT 7 AS id, 7 AS cart_id").await.unwrap();
        let write = "UPDATE pgbg_preview_carts c SET id = 8 FROM pgbg_preview_lines l WHERE id = 7 AND l.cart_id = c.id";
        let err = counted_on_the_server(write).await.unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_count_never_runs_a_write_hidden_in_its_condition() {
        run(WritePolicy::Allowed, "DROP TABLE IF EXISTS pgbg_preview_log").await.unwrap();
        run(WritePolicy::Allowed, "CREATE TABLE pgbg_preview_log AS SELECT now() AS at").await.unwrap();
        let logs = "CREATE OR REPLACE FUNCTION pgbg_preview_logs() RETURNS bool LANGUAGE sql AS 'INSERT INTO pgbg_preview_log VALUES (now()) RETURNING true'";
        run(WritePolicy::Allowed, logs).await.unwrap();
        assert!(counted_on_the_server("DELETE FROM pgbg_preview_log WHERE pgbg_preview_logs()").await.is_err());
        let kept = run(WritePolicy::ReadOnly, "SELECT count(*) FROM pgbg_preview_log").await.unwrap();
        assert_eq!(Touched::read(&kept), Some(Touched::Rows(1)));
    }
}
