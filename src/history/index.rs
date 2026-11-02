//! The history as the ghost reads it, pure: each successful read once under a form that ignores case and blanks,
//! sorted so the statements starting with what is typed sit side by side.
use super::{Entry, History, Outcome};
use crate::connection::WritePolicy;
use crate::db::guard;
use crate::db::lex::{self, Placed, Token};
use crate::db::model::{Params, Statement};
use crate::secret;
use chrono::{DateTime, Utc};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Bound;

/// How many characters, blanks aside, must be typed before the history offers anything: fewer match too much to help.
const MIN_TYPED_CHARS: usize = 3;

/// A connection's successful reads, by their [`plain_form`].
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct HistoryIndex {
    by_form: BTreeMap<String, Past>,
}

/// A statement as last written, from its first token to its last, and how often and when it ran.
#[derive(Debug, PartialEq, Eq)]
struct Past {
    statement: Statement,
    runs: usize,
    last_run: DateTime<Utc>,
}

impl HistoryIndex {
    /// The successful runs of `history`.
    pub(crate) fn of(history: &History) -> Self {
        let mut index = Self::default();
        history.entries.iter().filter(|entry| is_success(entry)).for_each(|entry| index.add(&entry.statement, entry.started_at));
        index
    }

    /// Counts a successful run of `statement` at `at`; a statement the guard does not read as a read is left out, since
    /// one key should not bring back a `DELETE`, and so is one holding a secret, as the history file leaves it out.
    pub(crate) fn add(&mut self, statement: &Statement, at: DateTime<Utc>) {
        let sql = statement.as_str();
        if !is_read(statement) || secret::holds_secret(sql) {
            return;
        }
        let tokens = lex::tokens(sql);
        let Some(last) = tokens.iter().rposition(|placed| placed.token != Token::End) else { return };
        let tokens = &tokens[..=last];
        let written = Statement::new(&sql[tokens[0].range.start..tokens[last].range.end]);
        self.merge(plain_form(sql, tokens), Past { statement: written, runs: 1, last_run: at });
    }

    /// Counts the runs of `other` too.
    pub(crate) fn add_runs_of(&mut self, other: Self) {
        other.by_form.into_iter().for_each(|(form, past)| self.merge(form, past));
    }

    /// The statement that continues `typed` and ran most, then last; none when `typed` is too short or nothing continues it.
    /// A statement that is just `typed` adds nothing, so the next one shows.
    pub(crate) fn best_after(&self, typed: &str) -> Option<&Statement> {
        let tokens = lex::tokens(typed);
        let ends_on_token = tokens.last().is_some_and(|last| last.range.end == typed.len());
        let form = plain_form(typed, &tokens) + if ends_on_token { "" } else { " " };
        if form.chars().filter(|c| *c != ' ').count() < MIN_TYPED_CHARS {
            return None;
        }
        let after_typed = (Bound::Excluded(form.as_str()), Bound::Unbounded);
        let continuing = self.by_form.range::<str, _>(after_typed).take_while(|(known, _)| known.starts_with(&form));
        continuing.map(|(_, past)| past).max_by_key(|past| (past.runs, past.last_run)).map(|past| &past.statement)
    }

    fn merge(&mut self, form: String, past: Past) {
        let Some(known) = self.by_form.get_mut(&form) else {
            self.by_form.insert(form, past);
            return;
        };
        known.runs += past.runs;
        if past.last_run >= known.last_run {
            known.statement = past.statement;
            known.last_run = past.last_run;
        }
    }
}

/// Whether a read-only connection would admit `statement`.
fn is_read(statement: &Statement) -> bool {
    guard::admit(statement.clone(), Params::default(), WritePolicy::ReadOnly).is_ok()
}

fn is_success(entry: &Entry) -> bool {
    matches!(entry.outcome, Outcome::Rows { .. })
}

/// `tokens` of `sql` one space apart, words in lower case: what a literal or a quoted name holds keeps its case, since
/// it changes what the statement means.
fn plain_form(sql: &str, tokens: &[Placed]) -> String {
    let plain = |placed: &Placed| match placed.word(sql) {
        Some(word) => Cow::Owned(word.to_ascii_lowercase()),
        None => Cow::Borrowed(&sql[placed.range.clone()]),
    };
    tokens.iter().map(plain).collect::<Vec<Cow<'_, str>>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::run_at;

    fn index_of(entries: Vec<Entry>) -> HistoryIndex {
        HistoryIndex::of(&History { entries })
    }

    fn failed_run(sql: &str, outcome: Outcome) -> Entry {
        Entry { outcome, ..run_at(sql, 0) }
    }

    fn best<'a>(index: &'a HistoryIndex, typed: &str) -> Option<&'a str> {
        index.best_after(typed).map(Statement::as_str)
    }

    #[test]
    fn the_statement_run_most_comes_first_whatever_ran_last() {
        let index = index_of(vec![
            run_at("SELECT id FROM orders", 3),
            run_at("SELECT total FROM orders", 2),
            run_at("SELECT total FROM orders", 1),
        ]);
        assert_eq!(best(&index, "SELECT "), Some("SELECT total FROM orders"));
    }

    #[test]
    fn among_statements_run_as_often_the_last_run_comes_first() {
        let index = index_of(vec![run_at("SELECT id FROM orders", 1), run_at("SELECT total FROM orders", 2)]);
        assert_eq!(best(&index, "SELECT "), Some("SELECT total FROM orders"));
    }

    #[test]
    fn a_past_write_is_left_out() {
        assert_eq!(index_of(vec![run_at("DELETE FROM orders", 1)]), HistoryIndex::default());
    }

    #[test]
    fn a_read_holding_a_write_is_left_out() {
        assert_eq!(index_of(vec![run_at("WITH gone AS (DELETE FROM orders RETURNING id) SELECT * FROM gone", 1)]), HistoryIndex::default());
    }

    #[test]
    fn a_read_that_locks_its_rows_is_left_out() {
        assert_eq!(index_of(vec![run_at("SELECT * FROM orders FOR UPDATE", 1)]), HistoryIndex::default());
    }

    #[test]
    fn an_explain_of_a_read_counts() {
        assert_eq!(best(&index_of(vec![run_at("EXPLAIN SELECT * FROM orders", 1)]), "EXP"), Some("EXPLAIN SELECT * FROM orders"));
    }

    #[test]
    fn a_read_with_a_parameter_counts() {
        let index = index_of(vec![run_at("SELECT * FROM orders WHERE id = {{id}}", 1)]);
        assert_eq!(best(&index, "SELECT * "), Some("SELECT * FROM orders WHERE id = {{id}}"));
    }

    #[test]
    fn a_failed_run_does_not_count() {
        let index = index_of(vec![
            failed_run("SELECT nope FROM orders", Outcome::ServerError),
            failed_run("SELECT id FROM orders", Outcome::Refused),
            failed_run("SELECT id FROM customers", Outcome::LinkError),
        ]);
        assert_eq!(best(&index, "SELECT "), None);
    }

    #[test]
    fn case_and_blanks_make_the_same_statement() {
        let index = index_of(vec![
            run_at("SELECT id FROM orders", 1),
            run_at("select  total\nfrom ORDERS", 2),
            run_at("SELECT total FROM orders", 3),
        ]);
        let counted = index.by_form.get("select total from orders").map(|past| past.runs);
        assert_eq!(counted, Some(2));
    }

    #[test]
    fn the_case_of_a_literal_makes_another_statement() {
        let index = index_of(vec![run_at("SELECT 'Paid' FROM orders", 1), run_at("SELECT 'paid' FROM orders", 2)]);
        assert_eq!(best(&index, "SELECT 'P"), Some("SELECT 'Paid' FROM orders"));
    }

    #[test]
    fn what_is_typed_matches_whatever_its_case_and_blanks() {
        assert_eq!(best(&index_of(vec![run_at("SELECT * FROM orders", 1)]), "select  *\nfrom ord"), Some("SELECT * FROM orders"));
    }

    #[test]
    fn a_comment_above_is_not_kept() {
        let index = index_of(vec![run_at("-- the big ones\nSELECT * FROM orders", 1)]);
        assert_eq!(best(&index, "SELECT "), Some("SELECT * FROM orders"));
    }

    #[test]
    fn a_comment_above_is_not_matched() {
        assert_eq!(best(&index_of(vec![run_at("-- the big ones\nSELECT * FROM orders", 1)]), "the big"), None);
    }

    #[test]
    fn a_closing_semicolon_is_not_part_of_the_statement() {
        let index = index_of(vec![run_at("SELECT * FROM orders;", 1), run_at("SELECT * FROM orders", 2)]);
        assert_eq!(index.by_form.get("select * from orders").map(|past| past.runs), Some(2));
    }

    #[test]
    fn fewer_than_three_typed_characters_offer_nothing() {
        assert_eq!(best(&index_of(vec![run_at("SELECT * FROM orders", 1)]), "se"), None);
    }

    #[test]
    fn blanks_do_not_count_as_typed_characters() {
        assert_eq!(best(&index_of(vec![run_at("(SELECT 1)", 1)]), "( s"), None);
    }

    #[test]
    fn three_typed_characters_offer_a_statement() {
        assert_eq!(best(&index_of(vec![run_at("SELECT * FROM orders", 1)]), "sel"), Some("SELECT * FROM orders"));
    }

    #[test]
    fn a_statement_that_is_just_what_is_typed_gives_way_to_a_longer_one() {
        let index = index_of(vec![run_at("SELECT 1", 1), run_at("SELECT 1", 2), run_at("SELECT 1 + 1", 3)]);
        assert_eq!(best(&index, "SELECT 1"), Some("SELECT 1 + 1"));
    }

    #[test]
    fn a_word_followed_by_a_blank_must_be_whole() {
        assert_eq!(best(&index_of(vec![run_at("SELECT * FROM orders", 1)]), "SELECT * FROM ord "), None);
    }

    #[test]
    fn the_note_of_a_statement_not_kept_offers_nothing() {
        assert_eq!(index_of(vec![run_at(secret::NOT_KEPT, 1)]), HistoryIndex::default());
    }

    #[test]
    fn a_run_holding_a_secret_is_left_out() {
        let mut index = HistoryIndex::default();
        index.add(
            &Statement::new("SELECT * FROM dblink('postgresql://nina:hunter2@db.example.com/shop', 'SELECT 1')"),
            DateTime::UNIX_EPOCH,
        );
        assert_eq!(index, HistoryIndex::default());
    }

    #[test]
    fn an_added_run_counts_with_those_read() {
        let mut index = index_of(vec![run_at("SELECT id FROM orders", 2), run_at("SELECT total FROM orders", 1)]);
        index.add(&Statement::new("SELECT total FROM orders"), DateTime::UNIX_EPOCH);
        assert_eq!(best(&index, "SELECT "), Some("SELECT total FROM orders"));
    }

    #[test]
    fn runs_kept_before_the_read_count_with_it() {
        let mut kept = index_of(vec![run_at("SELECT id FROM orders", 0)]);
        kept.add_runs_of(index_of(vec![run_at("SELECT id FROM orders", 1), run_at("SELECT total FROM orders", 1)]));
        assert_eq!(best(&kept, "SELECT "), Some("SELECT id FROM orders"));
    }

    #[test]
    fn the_last_way_a_statement_was_written_is_the_one_kept() {
        let index = index_of(vec![run_at("select  *  from orders", 2), run_at("SELECT * FROM orders", 1)]);
        assert_eq!(best(&index, "sel"), Some("select  *  from orders"));
    }

    /// The wall clock, read in a release build only, by CI's own step: `cargo test --release --lib -- --ignored budget::`.
    #[cfg(not(debug_assertions))]
    mod budget {
        use super::*;
        use crate::fixtures::best_of;
        use std::time::Duration;

        /// The worst key takes 0.7 ms on a laptop (`sel`, every line to weigh): ten times slower fails.
        const BUDGET: Duration = Duration::from_micros(7_000);

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn one_key_on_a_hundred_thousand_lines_finds_its_statement_within_the_budget() {
            let index = index_of((0..100_000).map(|n| run_at(&format!("SELECT * FROM orders WHERE id = {n} ORDER BY total"), 1)).collect());
            for typed in ["sel", "SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE id = 99999", "SELECT * FROM customers"] {
                let took = best_of(|| best(&index, typed).map(str::len));
                assert!(took < BUDGET, "{typed} took {took:?}");
            }
        }
    }
}
