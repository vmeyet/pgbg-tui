//! What the cursor wants, read from the tokens of the statement under it alone.
use super::keyword::is_keyword;
use crate::db::guard;
use crate::db::lex::{self, Lexed, Placed, Span, Token};
use std::ops::Range;

/// The words a table name follows.
const TABLE_AFTER: [&str; 3] = ["FROM", "JOIN", "TABLE"];

/// A completion asked at the cursor: the text a candidate replaces, and what kind of name it wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    /// The bytes of the editor text a taken candidate replaces: what was typed of the name, up to the cursor.
    pub(crate) typed: Range<usize>,
    /// The text of `typed`.
    pub(crate) prefix: String,
    pub(crate) want: Want,
}

/// The kind of name the cursor wants; a table is named as PostgreSQL resolves it: a bare name lowercased, a quoted one as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Want {
    /// A keyword, at a statement's start.
    Keyword,
    /// A table, after `FROM` or a `,` of a `FROM` list; what was typed may hold its schema.
    Table,
    /// A table after `JOIN`, where those a foreign key links to `tables`, the tables named before it, come first.
    Join { tables: Vec<TableRef> },
    /// A column of one table, after `orders.` or an alias of it.
    ColumnOf { table: String },
    /// A column of the tables the statement names, a column of any other table, or a keyword.
    Column { tables: Vec<String> },
}

/// A table the statement reads from, and the alias it gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableRef {
    pub(super) name: String,
    pub(super) alias: Option<String>,
}

impl TableRef {
    /// The name the rest of the statement calls it by: its alias, else its name without the schema.
    pub(super) fn qualifier(&self) -> &str {
        self.alias.as_deref().unwrap_or_else(|| without_schema(&self.name))
    }
}

/// `name` past its last `.`: `invoices` for `billing.invoices`.
pub(super) fn without_schema(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(_, table)| table)
}

impl Ask {
    /// What the cursor at byte `cursor` of `text`, read as `lexed`, wants; `None` inside a string, a comment, a quoted
    /// name or a `{{name}}`.
    pub(crate) fn at(text: &str, lexed: &Lexed, cursor: usize) -> Option<Self> {
        let statement = guard::statement_at(text, lexed, cursor).unwrap_or(cursor..cursor);
        if lexed.span_at(cursor) != Span::Code {
            return None;
        }
        let start = statement.start.min(cursor);
        let before = &text[start..cursor];
        let refs = table_refs(&text[start..statement.end.max(cursor)]);
        let (typed, want) = wanted(before, &refs);
        Some(Self { typed: start + typed..cursor, prefix: before[typed..].to_owned(), want })
    }
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
        (Some(previous), _) if is_table_place(before, previous, last_clause(before, ahead)) => {
            (name_start, table_want(before, previous, name_start))
        }
        (_, Some(qualifier)) => (word_start, Want::ColumnOf { table: table_of(&qualifier.name, refs) }),
        (None, None) => (word_start, Want::Keyword),
        (Some(_), None) => (word_start, Want::Column { tables: refs.iter().map(|table| table.name.clone()).collect() }),
    }
}

/// What a table name after `previous` wants: after `JOIN`, a join to the tables named before `name_start`.
fn table_want(before: &str, previous: &Placed, name_start: usize) -> Want {
    let is_join = previous.word(before).is_some_and(|word| word.eq_ignore_ascii_case("JOIN"));
    if is_join { Want::Join { tables: table_refs(&before[..name_start]) } } else { Want::Table }
}

/// The name right before a `.` that ends where the typed word starts, as in `o.` or `billing.`.
struct Qualifier {
    start: usize,
    name: String,
}

fn qualifier(before: &str, head: &[Placed], word_start: usize) -> Option<Qualifier> {
    let [.., name, dot] = head else { return None };
    let is_attached = is_dot(before, dot) && dot.range.end == word_start && name.range.end == dot.range.start;
    if !is_attached {
        return None;
    }
    Some(Qualifier { start: name.range.start, name: resolved(before, name)? })
}

/// The table a qualifier stands for: the one it aliases or names last, else the qualifier itself.
fn table_of(qualifier: &str, refs: &[TableRef]) -> String {
    let is_named = |table: &&TableRef| table.alias.as_deref() == Some(qualifier) || without_schema(&table.name) == qualifier;
    refs.iter().find(is_named).map_or_else(|| qualifier.to_owned(), |table| table.name.clone())
}

/// The tables the statement reads from, each named as it names it.
fn table_refs(statement: &str) -> Vec<TableRef> {
    let tokens = lex::tokens(statement);
    let places = (1..tokens.len()).filter(|&at| is_table_place(statement, &tokens[at - 1], last_clause(statement, &tokens[..at])));
    places.filter_map(|at| table_ref(statement, &tokens[at..])).collect()
}

/// The table named at the start of `tokens`, `schema.table` or bare, with its alias, `AS` or not; `None` when no name
/// starts there.
fn table_ref(statement: &str, tokens: &[Placed]) -> Option<TableRef> {
    let resolve = |placed: &Placed| resolved(statement, placed);
    let (name, rest) = match tokens {
        [schema, dot, table, rest @ ..] if is_dot(statement, dot) && resolve(table).is_some() => {
            (format!("{}.{}", resolve(schema)?, resolve(table)?), rest)
        }
        [table, rest @ ..] => (resolve(table)?, rest),
        [] => return None,
    };
    let is_as = rest.first().and_then(|placed| placed.word(statement)).is_some_and(|word| word.eq_ignore_ascii_case("AS"));
    let rest = if is_as { &rest[1..] } else { rest };
    let is_clause = |placed: &&Placed| placed.word(statement).is_some_and(is_keyword);
    let alias = rest.first().filter(|placed| !is_clause(placed)).and_then(resolve);
    Some(TableRef { name, alias })
}

/// The name a word or a quoted name stands for, as PostgreSQL resolves it: a word lowercased, a quoted name as written.
fn resolved(text: &str, placed: &Placed) -> Option<String> {
    let written = &text[placed.range.clone()];
    if placed.token == Token::Word {
        return Some(written.to_ascii_lowercase());
    }
    let quoted = written.strip_prefix('"')?.strip_suffix('"')?;
    Some(quoted.replace("\"\"", "\""))
}

/// Whether a table name goes after `previous`: a `FROM` or `JOIN`, or a `,` in a `FROM` list.
fn is_table_place(text: &str, previous: &Placed, clause: Option<&str>) -> bool {
    let is_after_word = previous.word(text).is_some_and(|word| lex::is_one_of(word, &TABLE_AFTER));
    let is_in_list = &text[previous.range.clone()] == "," && clause.is_some_and(|clause| lex::is_one_of(clause, &TABLE_AFTER));
    is_after_word || is_in_list
}

/// The last keyword of `tokens`, read out of `text`, that opens a clause, `AS` aside.
fn last_clause<'a>(text: &'a str, tokens: &[Placed]) -> Option<&'a str> {
    let is_clause = |word: &&str| is_keyword(word) && !word.eq_ignore_ascii_case("AS");
    tokens.iter().rev().filter_map(|placed| placed.word(text)).find(is_clause)
}

fn is_word(placed: &Placed) -> bool {
    placed.token == Token::Word
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
        Ask::at(&text, &Lexed::of(&text), cursor).map(|ask| (text[ask.typed].to_owned(), ask.want))
    }

    fn columns_of(tables: &[&str]) -> Want {
        Want::Column { tables: tables.iter().map(|table| (*table).to_owned()).collect() }
    }

    fn joined_to(tables: &[(&str, Option<&str>)]) -> Want {
        let table = |(name, alias): &(&str, Option<&str>)| TableRef { name: (*name).to_owned(), alias: alias.map(str::to_owned) };
        Want::Join { tables: tables.iter().map(table).collect() }
    }

    #[test]
    fn a_name_after_from_is_a_table() {
        assert_eq!(asked("SELECT * FROM or|"), Some(("or".to_owned(), Want::Table)));
    }

    #[test]
    fn a_name_after_join_is_a_join_to_the_tables_named_before_it() {
        let want = joined_to(&[("orders", Some("o")), ("billing.invoices", None)]);
        assert_eq!(asked("SELECT * FROM orders o, billing.invoices LEFT JOIN cu| WHERE true"), Some(("cu".to_owned(), want)));
    }

    #[test]
    fn a_table_is_called_by_its_alias_else_by_its_name_without_the_schema() {
        let Some((_, Want::Join { tables })) = asked("SELECT * FROM orders o JOIN billing.invoices ON true JOIN |") else { panic!() };
        assert_eq!(tables.iter().map(TableRef::qualifier).collect::<Vec<_>>(), ["o", "invoices"]);
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

    #[test]
    fn a_bare_table_is_named_lowercased_and_a_quoted_one_as_written() {
        let want = columns_of(&["Orders", "orders"]);
        assert_eq!(asked("SELECT t| FROM \"Orders\" JOIN ORDERS o ON true"), Some(("t".to_owned(), want)));
    }

    #[test]
    fn a_quoted_table_named_with_its_schema_keeps_its_case() {
        assert_eq!(asked("SELECT t| FROM public.\"Orders\""), Some(("t".to_owned(), columns_of(&["public.Orders"]))));
    }

    #[test]
    fn a_quoted_qualifier_or_alias_stands_for_its_table_as_written() {
        let want = Want::ColumnOf { table: "Orders".to_owned() };
        assert_eq!(asked("SELECT \"Orders\".| FROM \"Orders\""), Some((String::new(), want.clone())));
        assert_eq!(asked("SELECT \"O\".| FROM \"Orders\" AS \"O\""), Some((String::new(), want)));
        assert_eq!(asked("SELECT O.| FROM orders o"), Some((String::new(), Want::ColumnOf { table: "orders".to_owned() })));
    }
}
