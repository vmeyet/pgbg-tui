//! Classifies a statement before it is sent: a read, a write, or more than one statement.
//! A best effort that spares a round trip; the read-only transaction and role are what hold.
use super::lex::{self, Lexed, Placed, Token};
use super::model::words::{LOCKS, WRITES};
use super::model::{Params, Statement};
use crate::connection::{StatementTimeout, WritePolicy};
use std::fmt;
use std::ops::Range;

const READS: [&str; 6] = ["SELECT", "WITH", "VALUES", "TABLE", "EXPLAIN", "SHOW"];
/// The words `EXPLAIN` takes before the statement it explains: its options and their values.
const EXPLAIN_OPTIONS: [&str; 23] = [
    "ANALYZE",
    "ANALYSE",
    "VERBOSE",
    "COSTS",
    "SETTINGS",
    "GENERIC_PLAN",
    "BUFFERS",
    "SERIALIZE",
    "WAL",
    "TIMING",
    "SUMMARY",
    "MEMORY",
    "FORMAT",
    "TEXT",
    "XML",
    "JSON",
    "YAML",
    "TRUE",
    "FALSE",
    "ON",
    "OFF",
    "NONE",
    "BINARY",
];
/// The reads that are queries: what `EXPLAIN` may explain, since `ANALYZE` runs it, and what a cursor may hold.
const QUERIES: [&str; 4] = ["SELECT", "WITH", "VALUES", "TABLE"];
/// The words that open a row lock before its `UPDATE` or `SHARE`, which make it a lock rather than a statement of its own.
const LOCK_OPENINGS: [&[&str]; 3] = [&["FOR"], &["FOR", "KEY"], &["FOR", "NO", "KEY"]];
/// The writes a confirmation names with the table or object they write to.
const TARGETED: [&str; 12] =
    ["DELETE", "INSERT", "UPDATE", "MERGE", "INTO", "TRUNCATE", "DROP", "ALTER", "CREATE", "COPY", "VACUUM", "ANALYZE"];
/// The statements that end, open or step through a transaction or a cursor: on a connection that allows writes they
/// change no data by themselves, so they run without asking; one followed by `PREPARED` ends a prepared transaction,
/// maybe another client's, so it asks.
const UNASKED: [&str; 11] = ["BEGIN", "START", "COMMIT", "END", "ROLLBACK", "ABORT", "SAVEPOINT", "RELEASE", "FETCH", "CLOSE", "MOVE"];
/// The words between such a write and its table: `DELETE FROM ONLY`, `TRUNCATE TABLE`, `SELECT INTO TEMP`.
const BEFORE_TARGET: &str = "FROM INTO ONLY TABLE TEMP TEMPORARY UNLOGGED";
/// The writes that change the schema: a confirmation names them with the kind of object they change and that object.
const SCHEMA_CHANGES: [&str; 3] = ["CREATE", "DROP", "ALTER"];
/// The options a change of the schema may take before its kind, left out of its name: `CREATE OR REPLACE`, `CREATE TEMP`.
const BEFORE_KIND: [&str; 9] = ["OR", "REPLACE", "GLOBAL", "LOCAL", "TEMP", "TEMPORARY", "UNLOGGED", "UNIQUE", "RECURSIVE"];
/// The words that open a kind of two words: `MATERIALIZED VIEW`, `FOREIGN TABLE`, `EVENT TRIGGER`.
const KIND_OPENERS: [&str; 3] = ["MATERIALIZED", "FOREIGN", "EVENT"];
/// The kinds of object a change of the schema names, its last word when it has two.
const KINDS: [&str; 29] = [
    "TABLE",
    "VIEW",
    "INDEX",
    "SEQUENCE",
    "SCHEMA",
    "FUNCTION",
    "PROCEDURE",
    "ROUTINE",
    "AGGREGATE",
    "OPERATOR",
    "TYPE",
    "DOMAIN",
    "CAST",
    "COLLATION",
    "CONVERSION",
    "DATABASE",
    "ROLE",
    "USER",
    "GROUP",
    "EXTENSION",
    "LANGUAGE",
    "TRIGGER",
    "POLICY",
    "RULE",
    "STATISTICS",
    "TABLESPACE",
    "SERVER",
    "PUBLICATION",
    "SUBSCRIPTION",
];

/// A statement cleared to run, the values bound to its `$n`, and how to run it; `admit` makes one.
#[derive(Debug)]
pub(crate) struct Admitted {
    pub(super) statement: Statement,
    pub(super) params: Params,
    pub(super) mode: Mode,
    pub(super) transaction_step: TransactionStep,
    /// A query the guard reads as changing nothing, so a cursor can hold it.
    pub(super) is_query: bool,
    /// A timeout of its own for a read-only statement, which runs under it or the session's, whichever is stricter.
    pub(super) own_timeout: Option<StatementTimeout>,
    write: Option<Write>,
}

impl Admitted {
    /// What the statement writes, when it is not a read: only a connection that allows writes admits one.
    pub(crate) const fn write(&self) -> Option<&Write> {
        self.write.as_ref()
    }

    /// The statement once the user said yes to it: a read that tried to write runs as a write; a read-only
    /// connection stays read only.
    pub(crate) fn confirmed(self) -> Self {
        let mode = if self.mode == Mode::ReadThenAsk { Mode::Writes } else { self.mode };
        Self { mode, ..self }
    }

    /// The statement run under `timeout` too, when it gives one and the statement is read only.
    pub(crate) fn with_own_timeout(self, timeout: Option<StatementTimeout>) -> Self {
        Self { own_timeout: timeout, ..self }
    }
}

/// A statement that is not a read, as a confirmation names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Write {
    /// Each write the statement holds, in the order it says them, and whether it says `CASCADE`.
    Statement {
        changes: Vec<Change>,
        cascades: bool,
    },
    Several,
    /// A statement the guard reads as a read that the server refused as a write, as a function that writes does.
    Hidden,
}

/// One write of a statement: the word that makes it one, and the names it writes to when the guard can read them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    verb: String,
    targets: Vec<String>,
}

/// Where a statement stops being a read: at one of its words, counted among its words only, or at a second statement.
enum NotRead {
    Word(usize),
    Several,
}

/// What the statement of `tokens`, read out of `sql`, writes; `None` for a read and for a statement in `UNASKED`.
pub(crate) fn write_in(sql: &str, tokens: &[Placed]) -> Option<Write> {
    let upper = upper_case_words(sql, tokens);
    let words: Vec<&str> = upper.iter().map(String::as_str).collect();
    not_read(tokens, &words).and_then(|found| named(sql, tokens, &words, &found))
}

/// What a statement does to a transaction the user began, read from its first words; the server's answer to it
/// decides whether it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TransactionStep {
    Keeps,
    Opens,
    Ends,
    /// It ends one in a way the guard cannot read, or runs code that may end one inside (a procedure, a `DO` block).
    Unsure,
}

/// How a session runs an admitted statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// Inside `BEGIN READ ONLY`, always rolled back.
    ReadOnly,
    /// A read on a connection that allows writes: run with writes refused, so a write hidden in it, as in a function,
    /// fails before anything is kept, and runs only once confirmed.
    ReadThenAsk,
    /// As is, on a connection that allows writes.
    Writes,
}

/// Why a statement is not sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The statement may change something; holds the word that says so.
    Write(String),
    Multiple,
    /// The statement is a write, and no one said yes to it.
    Unconfirmed(Write),
    /// The statement numbers `expected` parameters, and `given` values came with it.
    Params {
        expected: usize,
        given: usize,
    },
}

/// Clears `statement`, with `params` bound to its `$n`, for a connection of policy `writes`, or says why not.
/// More than one statement is always refused, since the extended protocol runs one at a time.
pub(crate) fn admit(statement: Statement, params: Params, writes: WritePolicy) -> Result<Admitted, Refusal> {
    let tokens = lex::tokens(statement.as_str());
    let upper = upper_case_words(statement.as_str(), &tokens);
    let words: Vec<&str> = upper.iter().map(String::as_str).collect();
    let found = not_read(&tokens, &words);
    let (mode, transaction_step) = match (writes, &found) {
        (_, Some(NotRead::Several)) => return Err(Refusal::Multiple),
        (WritePolicy::Allowed, None) => (Mode::ReadThenAsk, TransactionStep::Keeps),
        (WritePolicy::Allowed, Some(NotRead::Word(_))) => (Mode::Writes, transaction_step(&words)),
        (WritePolicy::ReadOnly, Some(NotRead::Word(at))) => return Err(Refusal::Write(words[*at].to_owned())),
        (WritePolicy::ReadOnly, None) => (Mode::ReadOnly, TransactionStep::Keeps),
    };
    check_params(&tokens, &params)?;
    let is_query = words.first().is_some_and(|first| QUERIES.contains(first)) && found.is_none();
    let write = found.and_then(|found| named(statement.as_str(), &tokens, &words, &found));
    Ok(Admitted { statement, params, mode, transaction_step, is_query, own_timeout: None, write })
}

/// The words of `tokens` out of `sql`, in upper case as the guard's lists write them; PostgreSQL folds ASCII only.
fn upper_case_words(sql: &str, tokens: &[Placed]) -> Vec<String> {
    tokens.iter().filter_map(|placed| placed.word(sql)).map(str::to_ascii_uppercase).collect()
}

fn transaction_step(words: &[&str]) -> TransactionStep {
    match words {
        ["BEGIN" | "START", ..] => TransactionStep::Opens,
        ["CALL" | "DO", ..] => TransactionStep::Unsure,
        ["PREPARE", "TRANSACTION", ..] => TransactionStep::Ends,
        ["COMMIT" | "END" | "ROLLBACK" | "ABORT", "WORK" | "TRANSACTION", rest @ ..]
        | ["COMMIT" | "END" | "ROLLBACK" | "ABORT", rest @ ..] => step_after_an_ending(rest),
        _ => TransactionStep::Keeps,
    }
}

/// What a `COMMIT` or `ROLLBACK` does, given `rest`, the words after it: a chain opens the next transaction at once,
/// and a rollback to a savepoint or the end of a prepared transaction keeps the open one.
fn step_after_an_ending(rest: &[&str]) -> TransactionStep {
    match rest {
        [] | ["AND", "NO", "CHAIN"] => TransactionStep::Ends,
        ["AND", "CHAIN"] => TransactionStep::Opens,
        ["TO" | "PREPARED", ..] => TransactionStep::Keeps,
        _ => TransactionStep::Unsure,
    }
}

/// Checks only when values came: a statement without any may hold `$n` of its own (`CREATE FUNCTION ... RETURN $1 + 1`),
/// which the server checks.
fn check_params(tokens: &[Placed], params: &Params) -> Result<(), Refusal> {
    if params.is_empty() {
        return Ok(());
    }
    let numbered = tokens.iter().filter_map(|placed| if let Token::Numbered(n) = placed.token { Some(n) } else { None });
    let expected = numbered.max().map_or(0, |highest| usize::try_from(highest).unwrap_or(usize::MAX));
    if expected == params.len() { Ok(()) } else { Err(Refusal::Params { expected, given: params.len() }) }
}

fn not_read(tokens: &[Placed], words: &[&str]) -> Option<NotRead> {
    if holds_more_than_one_statement(tokens) {
        return Some(NotRead::Several);
    }
    let &first = words.first()?;
    if !READS.contains(&first) {
        return Some(NotRead::Word(0));
    }
    let start = if first == "EXPLAIN" {
        match explained(&words[1..]) {
            Ok(at) => 1 + at,
            Err(at) => return Some(NotRead::Word(1 + at)),
        }
    } else {
        0
    };
    first_write_word(&words[start..]).map(|at| NotRead::Word(start + at))
}

/// `found` as a confirmation names it: each write of the statement from `found` on, and what it writes to; `None` for a
/// statement in `UNASKED`.
fn named(sql: &str, tokens: &[Placed], words: &[&str], found: &NotRead) -> Option<Write> {
    let &NotRead::Word(first) = found else { return Some(Write::Several) };
    if first == 0 && is_unasked(words) {
        return None;
    }
    let changes = change_starts(words, first).into_iter().map(|at| change(sql, tokens, words, at)).collect();
    Some(Write::Statement { changes, cascades: words.contains(&"CASCADE") })
}

fn is_unasked(words: &[&str]) -> bool {
    UNASKED.contains(&words[0]) && words.get(1) != Some(&"PREPARED")
}

/// Where each write in `words` starts, from `first` on. A write word that belongs to another write does not count:
/// the `INTO` of `INSERT INTO`, the `UPDATE` of `ON CONFLICT DO UPDATE` or of a `MERGE`'s `THEN UPDATE`. Only a read
/// holds several writes, in its `WITH`: in a statement led by its own verb, a later write word is part of it, as the
/// `INSERT` of `CREATE TRIGGER ... AFTER INSERT`.
fn change_starts(words: &[&str], first: usize) -> Vec<usize> {
    if !READS.contains(&words[0]) {
        return vec![first];
    }
    let is_part_of_another = |at: usize| matches!((words[at - 1], words[at]), ("INSERT" | "MERGE", "INTO") | ("THEN" | "DO", _));
    let later = (first + 1..words.len()).filter(|&at| is_write_word(words, at) && !is_part_of_another(at));
    std::iter::once(first).chain(later).collect()
}

/// The write that starts at the word counted `at`: a row lock alone, from its `FOR`, the end of a prepared transaction
/// alone, else its verb and the names it writes to.
fn change(sql: &str, tokens: &[Placed], words: &[&str], at: usize) -> Change {
    let verb = words[at];
    if let Some(opening) = LOCK_OPENINGS.iter().find(|opening| words[..at].ends_with(opening)) {
        return Change { verb: words[at - opening.len()..=at].join(" "), targets: vec![] };
    }
    if words.get(at + 1) == Some(&"PREPARED") {
        return Change { verb: format!("{verb} PREPARED"), targets: vec![] };
    }
    let Some(index) = TARGETED.contains(&verb).then(|| word_token(tokens, at)).flatten() else {
        return Change { verb: verb.to_owned(), targets: vec![] };
    };
    if SCHEMA_CHANGES.contains(&verb) {
        return schema_change(sql, verb, &tokens[index + 1..]);
    }
    Change { verb: verb.to_owned(), targets: targets_in(sql, &tokens[index + 1..]) }
}

/// A change of the schema, `verb` then `tokens`, named by place: its kind, then the names past its options, whatever
/// they spell: `DROP TABLE IF EXISTS role`, `CREATE TRIGGER audit`.
fn schema_change(sql: &str, verb: &str, tokens: &[Placed]) -> Change {
    let upper: Vec<String> = tokens.iter().map_while(|placed| placed.word(sql)).map(str::to_ascii_uppercase).collect();
    let words: Vec<&str> = upper.iter().map(String::as_str).collect();
    let kind_start = words.iter().take_while(|word| BEFORE_KIND.contains(word)).count();
    let kind_end = kind_start + kind_length(&words[kind_start..]);
    let start = kind_end + options_after_kind(&words[kind_end..]);
    let named = std::iter::once(verb).chain(words[kind_start..kind_end].iter().copied());
    Change { verb: named.collect::<Vec<_>>().join(" "), targets: names_at(sql, &tokens[start..]) }
}

/// How many of `words` the kind they start with spans: an opener or none, then one of `KINDS`.
fn kind_length(words: &[&str]) -> usize {
    let openers = words.iter().take_while(|word| KIND_OPENERS.contains(word)).count();
    openers + usize::from(words.get(openers).is_some_and(|word| KINDS.contains(word)))
}

/// How many of `words` are options between a kind and its object: `CONCURRENTLY`, `ONLY`, `ON`, `IF [NOT] EXISTS`.
fn options_after_kind(words: &[&str]) -> usize {
    match words {
        ["CONCURRENTLY" | "ONLY" | "ON", rest @ ..] => 1 + options_after_kind(rest),
        ["IF", "EXISTS", rest @ ..] => 2 + options_after_kind(rest),
        ["IF", "NOT", "EXISTS", rest @ ..] => 3 + options_after_kind(rest),
        _ => 0,
    }
}

/// The place in `tokens` of the word at place `at` among the words alone.
fn word_token(tokens: &[Placed], at: usize) -> Option<usize> {
    tokens.iter().enumerate().filter(|(_, placed)| placed.token == Token::Word).nth(at).map(|(index, _)| index)
}

/// The names `tokens` start with once past `BEFORE_TARGET`: `DELETE FROM ONLY orders`.
fn targets_in(sql: &str, tokens: &[Placed]) -> Vec<String> {
    let is_before_target = |placed: &Placed| {
        placed.word(sql).is_some_and(|word| BEFORE_TARGET.split_whitespace().any(|known| known.eq_ignore_ascii_case(word)))
    };
    let start = tokens.iter().position(|placed| !is_before_target(placed)).unwrap_or(tokens.len());
    names_at(sql, &tokens[start..])
}

/// The names `tokens` start with, a `,` between each: `orders, customers`.
fn names_at(sql: &str, tokens: &[Placed]) -> Vec<String> {
    let mut targets = vec![];
    let mut rest = tokens;
    while let Some(length) = name_length(sql, rest) {
        targets.push(rest[..length].iter().map(|placed| &sql[placed.range.clone()]).collect());
        match rest.get(length) {
            Some(comma) if &sql[comma.range.clone()] == "," => rest = &rest[length + 1..],
            _ => break,
        }
    }
    targets
}

/// How many tokens the name at the start of `tokens` spans: words, quoted names and the `.` between them, glued or
/// with space around the `.`, as PostgreSQL reads `shop . orders`.
fn name_length(sql: &str, tokens: &[Placed]) -> Option<usize> {
    let is_dot = |placed: &Placed| &sql[placed.range.clone()] == ".";
    let is_part = |placed: &Placed| placed.token == Token::Word || is_dot(placed) || sql[placed.range.clone()].starts_with('"');
    if !is_part(tokens.first()?) {
        return None;
    }
    let is_joined =
        |pair: &[Placed]| is_part(&pair[1]) && (pair[0].range.end == pair[1].range.start || is_dot(&pair[0]) || is_dot(&pair[1]));
    Some(1 + tokens.windows(2).take_while(|pair| is_joined(pair)).count())
}

/// The bytes of the statement that holds the cursor at byte `at` of `sql`, without its `;` and the whitespace around
/// it; `None` when `sql` holds no statement. A cursor right after a `;`, or on blank text after it, belongs to the
/// statement before it.
pub(crate) fn statement_at(sql: &str, lexed: &Lexed, at: usize) -> Option<Range<usize>> {
    let pieces = pieces(lexed.tokens(), sql.len());
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

fn holds_more_than_one_statement(tokens: &[Placed]) -> bool {
    let mut after_first_end = tokens.iter().skip_while(|placed| placed.token != Token::End).skip(1);
    after_first_end.any(|placed| placed.token != Token::End)
}

/// Where the statement `EXPLAIN` explains starts among `words`, past its options. It must be a read whatever the
/// options: a quoted `"analyze"` is not a word, yet still runs it. Any other statement is refused at its first word.
fn explained(words: &[&str]) -> Result<usize, usize> {
    let options = words.iter().take_while(|word| EXPLAIN_OPTIONS.contains(word)).count();
    match words.get(options) {
        Some(first) if !QUERIES.contains(first) => Err(options),
        _ => Ok(options),
    }
}

fn first_write_word(words: &[&str]) -> Option<usize> {
    (0..words.len()).find(|&at| is_write_word(words, at))
}

/// Whether the word counted `at` makes its statement a write: one of `WRITES`, or the end of a row lock.
fn is_write_word(words: &[&str], at: usize) -> bool {
    WRITES.contains(&words[at]) || at.checked_sub(1).is_some_and(|before| LOCKS.contains(&[words[before], words[at]]))
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write(word) => {
                write!(f, "`{word}` is not a read, and this connection is read only: set `writes = \"allowed\"` on it to run writes")
            }
            Self::Multiple => f.write_str("one statement at a time: this text holds more than one"),
            Self::Unconfirmed(Write::Hidden) => {
                f.write_str("this statement tried to write, and no one said yes to it: answer `y`, or pass `--yes` where no terminal can")
            }
            Self::Unconfirmed(write) => {
                write!(f, "`{write}` is a write, and no one said yes to it: answer `y`, or pass `--yes` where no terminal can")
            }
            Self::Params { expected, given } => {
                write!(f, "the statement takes {}, and {} came with it", counted(*expected, "parameter"), counted(*given, "value"))
            }
        }
    }
}

impl fmt::Display for Write {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (changes, cascades) = match self {
            Self::Statement { changes, cascades } => (changes, cascades),
            Self::Several => return f.write_str("more than one statement"),
            Self::Hidden => return f.write_str("this statement tried to write, and may run twice: run it"),
        };
        let changes: Vec<String> = changes.iter().map(ToString::to_string).collect();
        f.write_str(&changes.join(" and "))?;
        if *cascades { f.write_str(" CASCADE") } else { Ok(()) }
    }
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { verb, targets } = self;
        if targets.is_empty() {
            return f.write_str(verb);
        }
        let targets = targets.join(", ");
        match verb.as_str() {
            "DELETE" => write!(f, "DELETE from {targets}"),
            "INSERT" | "MERGE" => write!(f, "{verb} into {targets}"),
            "INTO" => write!(f, "SELECT into {targets}"),
            _ => write!(f, "{verb} {targets}"),
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
    fn an_explain_analyze_of_a_read_is_a_read() {
        assert_eq!(verdict("EXPLAIN ANALYZE SELECT * FROM orders"), Ok(Mode::ReadOnly));
        assert_eq!(verdict("explain (analyse, buffers, format json) WITH paid AS (SELECT 1) TABLE paid"), Ok(Mode::ReadOnly));
        assert_eq!(verdict("EXPLAIN ANALYZE VERBOSE VALUES (1)"), Ok(Mode::ReadOnly));
    }

    #[test]
    fn an_explain_analyze_of_a_write_is_a_write() {
        assert_eq!(refused_word("EXPLAIN ANALYZE DELETE FROM orders"), "DELETE");
        assert_eq!(refused_word("EXPLAIN (ANALYZE true) WITH gone AS (DELETE FROM orders RETURNING id) SELECT * FROM gone"), "DELETE");
        assert_eq!(refused_word("EXPLAIN ANALYZE SELECT * INTO orders_copy FROM orders"), "INTO");
    }

    #[test]
    fn an_explain_analyze_of_what_is_not_a_read_is_refused() {
        assert_eq!(refused_word("EXPLAIN ANALYZE EXECUTE close_orders"), "EXECUTE");
        assert_eq!(refused_word("EXPLAIN (ANALYZE) CREATE TABLE orders_copy AS SELECT 1"), "CREATE");
    }

    #[test]
    fn an_explain_analyze_with_its_option_quoted_or_spelt_otherwise_explains_a_read_only() {
        assert_eq!(refused_word("EXPLAIN (\"analyze\") CREATE TABLE orders_copy AS SELECT 1"), "CREATE");
        assert_eq!(refused_word("EXPLAIN (\"analyze\" true, \"format\" 'json') CREATE TABLE orders_copy AS SELECT 1"), "CREATE");
        assert_eq!(refused_word("EXPLAIN (ANALYSE 1) CREATE TABLE orders_copy AS SELECT 1"), "CREATE");
        assert_eq!(refused_word("EXPLAIN (analyze 'on') EXECUTE close_orders"), "EXECUTE");
        assert_eq!(verdict("EXPLAIN (\"analyze\", \"format\" 'json') SELECT * FROM orders"), Ok(Mode::ReadOnly));
    }

    #[test]
    fn an_explain_without_analyze_explains_a_read_only() {
        assert_eq!(refused_word("EXPLAIN CREATE TABLE orders_copy AS SELECT 1"), "CREATE");
        assert_eq!(refused_word("EXPLAIN (FORMAT JSON) EXECUTE close_orders"), "EXECUTE");
    }

    #[test]
    fn an_analyze_of_a_table_is_a_write() {
        assert_eq!(refused_word("ANALYZE orders"), "ANALYZE");
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
        statement_at(&sql, &Lexed::of(&sql), at).map(|range| sql[range].to_owned())
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
    fn a_connection_that_allows_writes_runs_a_write_as_is() {
        let admitted = admit(Statement::new("DELETE FROM orders"), Params::default(), WritePolicy::Allowed);
        assert_eq!(admitted.map(|admitted| admitted.mode), Ok(Mode::Writes));
    }

    #[test]
    fn a_read_where_writes_are_allowed_runs_with_writes_refused_until_confirmed() {
        let admitted = admit(Statement::new("SELECT archive_orders()"), Params::default(), WritePolicy::Allowed).unwrap();
        assert_eq!(admitted.mode, Mode::ReadThenAsk);
        assert_eq!(admitted.confirmed().mode, Mode::Writes);
    }

    #[test]
    fn a_confirmed_read_on_a_read_only_connection_stays_read_only() {
        let admitted = admit(Statement::new("SELECT archive_orders()"), Params::default(), WritePolicy::ReadOnly).unwrap();
        assert_eq!(admitted.confirmed().mode, Mode::ReadOnly);
    }

    #[test]
    fn a_hidden_write_asks_whether_to_run_the_statement_that_may_run_twice() {
        assert_eq!(Write::Hidden.to_string(), "this statement tried to write, and may run twice: run it");
    }

    #[test]
    fn an_unconfirmed_hidden_write_says_the_statement_tried_to_write() {
        let refusal = Refusal::Unconfirmed(Write::Hidden).to_string();
        assert!(refusal.starts_with("this statement tried to write, and no one said yes to it"), "{refusal}");
    }

    #[test]
    fn more_than_one_statement_is_refused_even_where_writes_are_allowed() {
        let admitted = admit(Statement::new("DELETE FROM orders; DELETE FROM customers"), Params::default(), WritePolicy::Allowed);
        assert_eq!(admitted.map(|admitted| admitted.mode), Err(Refusal::Multiple));
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

    fn write(sql: &str) -> Option<String> {
        write_in(sql, &lex::tokens(sql)).map(|write| write.to_string())
    }

    #[test]
    fn a_write_names_its_verb_and_the_table_it_writes() {
        assert_eq!(write("DELETE FROM orders WHERE id = 1").as_deref(), Some("DELETE from orders"));
        assert_eq!(write("insert into shop.orders(id) VALUES (1)").as_deref(), Some("INSERT into shop.orders"));
        assert_eq!(write(r#"UPDATE ONLY "Orders" SET total = 0"#).as_deref(), Some(r#"UPDATE "Orders""#));
        assert_eq!(write("MERGE INTO orders USING customers ON true WHEN MATCHED THEN DELETE").as_deref(), Some("MERGE into orders"));
        assert_eq!(write("SELECT * INTO archived_orders FROM orders").as_deref(), Some("SELECT into archived_orders"));
    }

    #[test]
    fn a_write_inside_a_read_names_the_table_it_writes() {
        let sql = "WITH gone AS (DELETE FROM orders RETURNING id) SELECT count(*) FROM gone";
        assert_eq!(write(sql).as_deref(), Some("DELETE from orders"));
        assert_eq!(write("EXPLAIN ANALYZE UPDATE customers SET name = 'nina'").as_deref(), Some("UPDATE customers"));
    }

    #[test]
    fn a_change_of_the_schema_names_what_it_changes() {
        assert_eq!(write("DROP TABLE IF EXISTS orders").as_deref(), Some("DROP TABLE orders"));
        assert_eq!(write("TRUNCATE TABLE ONLY orders").as_deref(), Some("TRUNCATE orders"));
        assert_eq!(write("CREATE INDEX ON orders (total)").as_deref(), Some("CREATE INDEX orders"));
    }

    #[test]
    fn every_write_of_a_statement_is_named() {
        let sql = "WITH moved AS (INSERT INTO archived_orders SELECT * FROM orders RETURNING id) DELETE FROM orders WHERE id IN (SELECT id FROM moved)";
        assert_eq!(write(sql).as_deref(), Some("INSERT into archived_orders and DELETE from orders"));
    }

    #[test]
    fn a_statement_led_by_its_own_verb_names_no_later_write() {
        assert_eq!(write("CREATE TRIGGER audit AFTER INSERT ON orders EXECUTE FUNCTION audit()").as_deref(), Some("CREATE TRIGGER audit"));
    }

    #[test]
    fn the_parts_of_an_upsert_or_a_merge_are_no_writes_of_their_own() {
        let upsert = "INSERT INTO orders VALUES (1) ON CONFLICT (id) DO UPDATE SET total = 0";
        assert_eq!(write(upsert).as_deref(), Some("INSERT into orders"));
        let merge =
            "MERGE INTO orders USING customers ON true WHEN MATCHED THEN UPDATE SET total = 0 WHEN NOT MATCHED THEN INSERT VALUES (1)";
        assert_eq!(write(merge).as_deref(), Some("MERGE into orders"));
    }

    #[test]
    fn every_table_a_write_lists_is_named() {
        assert_eq!(write("TRUNCATE orders, shop.customers").as_deref(), Some("TRUNCATE orders, shop.customers"));
        assert_eq!(write("DROP TABLE IF EXISTS orders, customers").as_deref(), Some("DROP TABLE orders, customers"));
    }

    #[test]
    fn a_write_that_cascades_says_so() {
        assert_eq!(write("DROP SCHEMA shop CASCADE").as_deref(), Some("DROP SCHEMA shop CASCADE"));
        assert_eq!(write("truncate orders cascade").as_deref(), Some("TRUNCATE orders CASCADE"));
    }

    #[test]
    fn transaction_control_and_cursor_statements_are_not_asked_for() {
        for sql in
            ["BEGIN", "START TRANSACTION", "COMMIT", "END", "ROLLBACK TO SAVEPOINT before_orders", "ABORT", "SAVEPOINT before_orders"]
        {
            assert_eq!(write(sql), None, "{sql}");
        }
        for sql in ["RELEASE SAVEPOINT before_orders", "FETCH 10 FROM recent", "CLOSE recent", "MOVE NEXT IN recent"] {
            assert_eq!(write(sql), None, "{sql}");
        }
    }

    #[test]
    fn a_write_without_a_table_names_its_verb_alone() {
        assert_eq!(write("DISCARD ALL").as_deref(), Some("DISCARD"));
        assert_eq!(write("SET search_path = shop").as_deref(), Some("SET"));
        assert_eq!(write("SELECT * FROM orders FOR UPDATE NOWAIT").as_deref(), Some("FOR UPDATE"));
        assert_eq!(write("SELECT * FROM orders FOR KEY SHARE").as_deref(), Some("FOR KEY SHARE"));
    }

    #[test]
    fn a_row_lock_is_named_from_its_for() {
        assert_eq!(write("SELECT * FROM orders FOR NO KEY UPDATE").as_deref(), Some("FOR NO KEY UPDATE"));
        assert_eq!(write("SELECT * FROM orders FOR SHARE").as_deref(), Some("FOR SHARE"));
    }

    #[test]
    fn the_end_of_a_prepared_transaction_is_asked_for() {
        assert_eq!(write("COMMIT PREPARED 'nightly'").as_deref(), Some("COMMIT PREPARED"));
        assert_eq!(write("rollback prepared 'nightly'").as_deref(), Some("ROLLBACK PREPARED"));
    }

    #[test]
    fn a_change_of_the_schema_names_its_object_past_its_kind_and_options() {
        assert_eq!(write("CREATE OR REPLACE FUNCTION add1(int) RETURNS int RETURN 1").as_deref(), Some("CREATE FUNCTION add1"));
        assert_eq!(write("CREATE TEMP TABLE scratch (id int)").as_deref(), Some("CREATE TABLE scratch"));
        assert_eq!(write("CREATE UNLOGGED TABLE scratch (id int)").as_deref(), Some("CREATE TABLE scratch"));
        assert_eq!(write("DROP DATABASE shop").as_deref(), Some("DROP DATABASE shop"));
        assert_eq!(write("DROP ROLE IF EXISTS nina").as_deref(), Some("DROP ROLE nina"));
        assert_eq!(write("DROP MATERIALIZED VIEW IF EXISTS shop . totals").as_deref(), Some("DROP MATERIALIZED VIEW shop.totals"));
    }

    #[test]
    fn a_change_of_the_schema_names_its_object_by_place_whatever_it_spells() {
        assert_eq!(write("DROP TABLE role").as_deref(), Some("DROP TABLE role"));
        assert_eq!(write("DROP VIEW view").as_deref(), Some("DROP VIEW view"));
        assert_eq!(write(r#"ALTER TABLE "table" ADD COLUMN note text"#).as_deref(), Some(r#"ALTER TABLE "table""#));
    }

    #[test]
    fn a_select_into_a_temporary_table_names_that_table() {
        assert_eq!(write("SELECT * INTO TEMP scratch FROM orders").as_deref(), Some("SELECT into scratch"));
        assert_eq!(write("SELECT * INTO TEMPORARY TABLE scratch FROM orders").as_deref(), Some("SELECT into scratch"));
    }

    #[test]
    fn a_dotted_name_is_read_across_spaces() {
        assert_eq!(write("DELETE FROM shop . orders").as_deref(), Some("DELETE from shop.orders"));
        assert_eq!(write(r#"UPDATE shop. "Orders" SET total = 0"#).as_deref(), Some(r#"UPDATE shop."Orders""#));
    }

    #[test]
    fn a_table_named_like_a_kind_of_object_is_still_named() {
        assert_eq!(write("DELETE FROM role WHERE id = 1").as_deref(), Some("DELETE from role"));
    }

    #[test]
    fn a_read_is_no_write() {
        assert_eq!(write("SELECT 'DELETE FROM orders' AS note FROM orders"), None);
        assert_eq!(write("EXPLAIN ANALYZE SELECT * FROM orders"), None);
    }

    #[test]
    fn more_than_one_statement_is_named_so() {
        assert_eq!(write("SELECT 1; DELETE FROM orders").as_deref(), Some("more than one statement"));
    }

    #[test]
    fn an_admitted_write_says_what_it_writes_and_an_admitted_read_says_nothing() {
        let admitted = |sql: &str, writes| admit(Statement::new(sql), Params::default(), writes).unwrap().write().map(ToString::to_string);
        assert_eq!(admitted("DELETE FROM orders", WritePolicy::Allowed).as_deref(), Some("DELETE from orders"));
        assert_eq!(admitted("SELECT * FROM orders", WritePolicy::Allowed), None);
        assert_eq!(admitted("SELECT * FROM orders", WritePolicy::ReadOnly), None);
    }

    fn step(sql: &str) -> TransactionStep {
        admit(Statement::new(sql), Params::default(), WritePolicy::Allowed).unwrap().transaction_step
    }

    #[test]
    fn a_begin_opens_a_transaction_of_the_user() {
        assert_eq!(step("begin isolation level serializable"), TransactionStep::Opens);
        assert_eq!(step("START TRANSACTION READ WRITE"), TransactionStep::Opens);
    }

    #[test]
    fn a_commit_or_a_rollback_ends_it() {
        for sql in ["COMMIT", "END", "ROLLBACK", "ABORT", "COMMIT WORK", "ROLLBACK TRANSACTION AND NO CHAIN;", "PREPARE TRANSACTION 'shop'"]
        {
            assert_eq!(step(sql), TransactionStep::Ends, "{sql}");
        }
    }

    #[test]
    fn a_commit_and_chain_opens_the_next_one() {
        assert_eq!(step("COMMIT AND CHAIN"), TransactionStep::Opens);
        assert_eq!(step("rollback work and chain"), TransactionStep::Opens);
    }

    #[test]
    fn a_rollback_to_a_savepoint_or_of_a_prepared_transaction_keeps_it() {
        assert_eq!(step("ROLLBACK TO SAVEPOINT before_orders"), TransactionStep::Keeps);
        assert_eq!(step("COMMIT PREPARED 'shop'"), TransactionStep::Keeps);
    }

    #[test]
    fn any_other_statement_keeps_it() {
        assert_eq!(step("INSERT INTO orders VALUES (1) RETURNING id"), TransactionStep::Keeps);
        assert_eq!(step("-- COMMIT\nSELECT 1"), TransactionStep::Keeps);
    }

    #[test]
    fn a_procedure_or_a_do_block_is_unsure_since_it_may_commit_inside() {
        assert_eq!(step("CALL close_orders()"), TransactionStep::Unsure);
        assert_eq!(step("do $$ BEGIN COMMIT; END $$"), TransactionStep::Unsure);
    }

    #[test]
    fn an_ending_the_guard_cannot_read_is_unsure() {
        assert_eq!(step("COMMIT AND MAYBE"), TransactionStep::Unsure);
    }

    #[test]
    fn a_read_only_statement_keeps_it() {
        let admitted = admit(Statement::new("SELECT 1"), Params::default(), WritePolicy::ReadOnly).unwrap();
        assert_eq!(admitted.transaction_step, TransactionStep::Keeps);
    }
}
