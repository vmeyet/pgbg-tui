//! What the cursor wants, read from the tokens of the statement under it alone.
use super::keyword::is_keyword;
use crate::db::guard;
use crate::db::lex::{self, Placed, Token};
use std::ops::Range;

/// The words a table name follows.
const TABLE_AFTER: [&str; 3] = ["FROM", "JOIN", "TABLE"];
/// A word character typed after the text, to learn whether a word could start there or a string or comment holds it.
const PROBE: &str = "x";

/// A completion asked at the cursor: the text a candidate replaces, and what kind of name it wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    /// The bytes of the editor text a taken candidate replaces: what was typed of the name, up to the cursor.
    pub(crate) typed: Range<usize>,
    /// The text of `typed`.
    pub(crate) prefix: String,
    pub(crate) want: Want,
}

/// The kind of name the cursor wants; table names are lowercased, as the statement names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Want {
    /// A keyword, at a statement's start.
    Keyword,
    /// A table, after `FROM` or `JOIN`; what was typed may hold its schema.
    Table,
    /// A column of one table, after `orders.` or an alias of it.
    ColumnOf { table: String },
    /// A column of the tables the statement names, a column of any other table, or a keyword.
    Column { tables: Vec<String> },
}

/// A table the statement reads from, and the alias it gives it.
#[derive(Debug, PartialEq, Eq)]
struct TableRef {
    name: String,
    alias: Option<String>,
}

impl Ask {
    /// What the cursor at byte `cursor` of `text` wants; `None` inside a string, a comment or a quoted name.
    pub(crate) fn at(text: &str, cursor: usize) -> Option<Self> {
        let statement = guard::statement_at(text, cursor).unwrap_or(cursor..cursor);
        let start = statement.start.min(cursor);
        let before = &text[start..cursor];
        if !is_in_code(before) {
            return None;
        }
        let refs = table_refs(&text[start..statement.end.max(cursor)]);
        let (typed, want) = wanted(before, &refs);
        Some(Self { typed: start + typed..cursor, prefix: before[typed..].to_owned(), want })
    }
}

fn is_in_code(before: &str) -> bool {
    let probed = format!("{before}{PROBE}");
    lex::tokens(&probed).last().is_some_and(|last| last.range.end == probed.len() && is_word(last))
}

/// Where the name typed before the cursor starts in `before`, and what it wants.
fn wanted(before: &str, refs: &[TableRef]) -> (usize, Want) {
    let tokens = lex::tokens(before);
    let is_typing = tokens.last().is_some_and(|last| last.range.end == before.len() && is_word(last));
    let head = &tokens[..tokens.len() - usize::from(is_typing)];
    let word_start = if is_typing { tokens[head.len()].range.start } else { before.len() };
    let qualifier = qualifier(before, head, word_start);
    let name_start = qualifier.as_ref().map_or(word_start, |qualifier| qualifier.start);
    let ahead = &head[..head.iter().take_while(|placed| placed.range.end <= name_start).count()];
    match (ahead.last(), qualifier) {
        (Some(previous), _) if is_table_place(before, previous, last_clause(ahead)) => (name_start, Want::Table),
        (_, Some(qualifier)) => (word_start, Want::ColumnOf { table: table_of(&qualifier.name, refs) }),
        (None, None) => (word_start, Want::Keyword),
        (Some(_), None) => (word_start, Want::Column { tables: refs.iter().map(|table| table.name.clone()).collect() }),
    }
}

/// The name right before a `.` that ends where the typed word starts, as in `o.` or `billing.`.
struct Qualifier {
    start: usize,
    name: String,
}

fn qualifier(before: &str, head: &[Placed], word_start: usize) -> Option<Qualifier> {
    let [.., name, dot] = head else { return None };
    let is_attached = is_dot(before, dot) && dot.range.end == word_start && name.range.end == dot.range.start;
    (is_attached && is_word(name)).then(|| Qualifier { start: name.range.start, name: before[name.range.clone()].to_lowercase() })
}

/// The table a qualifier stands for: the one it aliases or names last, else the qualifier itself.
fn table_of(qualifier: &str, refs: &[TableRef]) -> String {
    let is_named = |table: &&TableRef| {
        let last_part = table.name.rsplit('.').next();
        table.alias.as_deref() == Some(qualifier) || last_part == Some(qualifier)
    };
    refs.iter().find(is_named).map_or_else(|| qualifier.to_owned(), |table| table.name.clone())
}

/// The tables the statement reads from, each named as it names it.
fn table_refs(statement: &str) -> Vec<TableRef> {
    let tokens = lex::tokens(statement);
    let places = (1..tokens.len()).filter(|&at| is_table_place(statement, &tokens[at - 1], last_clause(&tokens[..at])));
    places.filter(|&at| is_word(&tokens[at])).map(|at| table_ref(statement, &tokens[at..])).collect()
}

/// The table named at the start of `tokens`, `schema.table` or bare, with its alias, `AS` or not.
fn table_ref(statement: &str, tokens: &[Placed]) -> TableRef {
    let text = |placed: &Placed| statement[placed.range.clone()].to_lowercase();
    let (name, rest) = match tokens {
        [schema, dot, table, rest @ ..] if is_dot(statement, dot) && is_word(table) => (format!("{}.{}", text(schema), text(table)), rest),
        [table, rest @ ..] => (text(table), rest),
        [] => (String::new(), tokens),
    };
    let rest = if rest.first().and_then(|placed| placed.token.word()) == Some("AS") { &rest[1..] } else { rest };
    let alias = rest.first().filter(|placed| placed.token.word().is_some_and(|word| !is_keyword(word))).map(text);
    TableRef { name, alias }
}

/// Whether a table name goes after `previous`: a `FROM` or `JOIN`, or a `,` in a `FROM` list.
fn is_table_place(text: &str, previous: &Placed, clause: Option<&str>) -> bool {
    let is_after_word = previous.token.word().is_some_and(|word| TABLE_AFTER.contains(&word));
    let is_in_list = &text[previous.range.clone()] == "," && clause.is_some_and(|clause| TABLE_AFTER.contains(&clause));
    is_after_word || is_in_list
}

/// The last keyword of `tokens` that opens a clause, `AS` aside.
fn last_clause(tokens: &[Placed]) -> Option<&str> {
    tokens.iter().rev().filter_map(|placed| placed.token.word()).find(|word| is_keyword(word) && *word != "AS")
}

fn is_word(placed: &Placed) -> bool {
    matches!(placed.token, Token::Word(_))
}

fn is_dot(text: &str, placed: &Placed) -> bool {
    &text[placed.range.clone()] == "."
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text typed and the want at the `|` of `marked`.
    fn asked(marked: &str) -> Option<(String, Want)> {
        let cursor = marked.find('|').unwrap();
        let text = marked.replace('|', "");
        Ask::at(&text, cursor).map(|ask| (text[ask.typed].to_owned(), ask.want))
    }

    fn columns_of(tables: &[&str]) -> Want {
        Want::Column { tables: tables.iter().map(|table| (*table).to_owned()).collect() }
    }

    #[test]
    fn a_name_after_from_or_join_is_a_table() {
        assert_eq!(asked("SELECT * FROM or|"), Some(("or".to_owned(), Want::Table)));
        assert_eq!(asked("SELECT * FROM orders o JOIN cu|"), Some(("cu".to_owned(), Want::Table)));
    }

    #[test]
    fn a_table_typed_with_its_schema_keeps_the_schema_in_what_it_replaces() {
        assert_eq!(asked("SELECT * FROM billing.in|"), Some(("billing.in".to_owned(), Want::Table)));
    }

    #[test]
    fn a_name_after_a_comma_of_a_from_list_is_a_table() {
        assert_eq!(asked("SELECT * FROM orders AS o, cu|"), Some(("cu".to_owned(), Want::Table)));
        assert_eq!(asked("SELECT id, to|"), Some(("to".to_owned(), columns_of(&[]))));
    }

    #[test]
    fn a_name_after_an_alias_and_a_dot_is_a_column_of_its_table() {
        let want = Want::ColumnOf { table: "orders".to_owned() };
        assert_eq!(asked("SELECT o.| FROM orders o"), Some((String::new(), want.clone())));
        assert_eq!(asked("SELECT o.to| FROM orders AS o"), Some(("to".to_owned(), want)));
    }

    #[test]
    fn a_table_named_with_its_schema_is_found_by_its_bare_name() {
        let want = Want::ColumnOf { table: "billing.invoices".to_owned() };
        assert_eq!(asked("SELECT invoices.| FROM billing.invoices"), Some((String::new(), want)));
    }

    #[test]
    fn a_qualifier_the_statement_does_not_name_stands_for_itself() {
        assert_eq!(asked("SELECT orders.i|"), Some(("i".to_owned(), Want::ColumnOf { table: "orders".to_owned() })));
    }

    #[test]
    fn a_word_at_a_statements_start_is_a_keyword() {
        assert_eq!(asked("se|"), Some(("se".to_owned(), Want::Keyword)));
        assert_eq!(asked("SELECT 1; /* next */ se|"), Some(("se".to_owned(), Want::Keyword)));
    }

    #[test]
    fn any_other_name_is_a_column_of_the_tables_the_statement_names() {
        let want = columns_of(&["orders", "customers"]);
        assert_eq!(asked("SELECT to| FROM orders o JOIN customers c ON true"), Some(("to".to_owned(), want)));
    }

    #[test]
    fn only_the_statement_under_the_cursor_names_tables() {
        assert_eq!(asked("SELECT * FROM customers; SELECT i| FROM orders"), Some(("i".to_owned(), columns_of(&["orders"]))));
    }

    #[test]
    fn nothing_completes_inside_a_string_a_comment_or_a_quoted_name() {
        assert_eq!(asked("SELECT 'fr|"), None);
        assert_eq!(asked("SELECT 1 -- fr|"), None);
        assert_eq!(asked("SELECT /* fr|"), None);
        assert_eq!(asked("SELECT \"Or|"), None);
        assert_eq!(asked("SELECT {{na|"), None);
    }
}
