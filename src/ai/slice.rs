//! The part of a catalog a prompt shows, the tables most likely wanted first: each as the body of a `CREATE TABLE`, its
//! columns with their types, enum labels, foreign keys and comments; then the name alone of every other table, so the
//! model can name a table nothing pointed to.
use crate::db::lex::{self, Token};
use crate::db::model::{Catalog, ForeignKey, SchemaColumn, SchemaNode, Table, qualified, sql_name, sql_string};
use crate::secret;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

/// The shortest instruction word that may name a table or a column, so `a` or `of` pulls in nothing.
const MIN_WORD: usize = 3;
/// Instruction words too common to point to anything: `for` would name a table `forms`.
const STOP_WORDS: [&str; 44] = [
    "about", "after", "all", "and", "any", "are", "before", "between", "but", "can", "each", "every", "find", "for", "from", "get", "give",
    "has", "have", "how", "into", "join", "list", "many", "not", "only", "per", "select", "show", "than", "that", "the", "their", "them",
    "then", "this", "those", "was", "what", "when", "where", "which", "who", "with",
];
/// Columns so many tables hold that naming one says nothing of the table wanted.
const GENERIC_COLUMNS: [&str; 9] = ["uuid", "name", "title", "description", "type", "status", "created_at", "updated_at", "deleted_at"];
/// How much of a comment a line keeps, so one long comment never takes the room of other tables.
const MAX_COMMENT_BYTES: usize = 300;
/// What ends a comment or a list of enum labels cut short.
const CUT_MARK: &str = "…";

/// One line of a slice: a table in detail, or its name alone, which matters less and goes first past a cap.
pub(super) enum Line {
    Detailed(String),
    NameAlone(String),
}

impl Line {
    pub(super) fn text(&self) -> &str {
        match self {
            Self::Detailed(text) | Self::NameAlone(text) => text,
        }
    }
}

/// Why a table is in the slice, the most likely wanted first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reason {
    NamedByStatement,
    NamedByInstruction,
    ColumnNamedByInstruction,
}

/// A table of the catalog and the schema holding it.
type Entry<'a> = (&'a SchemaNode, &'a Table);

/// How many statements of a connection's history name each table of a catalog, by its lowercase name: it orders a slice
/// and goes no further, so no count or statement reaches a prompt.
/// The default, of no history, leaves the tables in catalog order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TableUse(HashMap<String, usize>);

impl TableUse {
    pub(crate) fn counted<'a>(catalog: &Catalog, statements: impl Iterator<Item = &'a str>) -> Self {
        let mut counts: HashMap<String, usize> = tables(catalog).map(|(_, table)| (table.name.to_lowercase(), 0)).collect();
        let named_once = statements.flat_map(|statement| statement_names(statement).into_iter().collect::<HashSet<_>>());
        for name in named_once {
            if let Some(count) = counts.get_mut(&name) {
                *count += 1;
            }
        }
        counts.retain(|_, count| *count > 0);
        Self(counts)
    }

    fn of(&self, table: &Table) -> usize {
        self.0.get(&table.name.to_lowercase()).copied().unwrap_or_default()
    }
}

/// One line per table of `catalog` the `statement` names or the `instruction`'s words point to, as
/// `schema.table(column type references schema.table(column), ...)`: first the tables the statement names, then those
/// the instruction names, then those holding a column it names, then those a foreign key links to one of them, the
/// links of the first kept first; among those with the same reason, the tables `table_use` counts most come first.
/// Then one line per other table naming it alone, in the order of the search path, then the other schemas.
/// Each line is written only once asked for, so a caller that stops at a cap writes no more.
pub(super) fn slice<'a>(
    catalog: &'a Catalog,
    statement: &str,
    instruction: &str,
    table_use: &TableUse,
) -> impl Iterator<Item = Line> + use<'a> {
    let kept = kept(catalog, &statement_names(statement), &instruction_words(instruction), table_use);
    let linked = linked(catalog, &kept);
    let detailed: Vec<Entry<'a>> = kept.into_iter().chain(linked).collect();
    let others = names_other_than(catalog, &detailed);
    detailed.into_iter().map(|(node, table)| Line::Detailed(line(node, table))).chain(others.map(Line::NameAlone))
}

fn kept<'a>(catalog: &'a Catalog, named: &[String], asked: &[String], table_use: &TableUse) -> Vec<Entry<'a>> {
    let pointing: Vec<&str> = asked.iter().map(String::as_str).filter(|word| points_past_its_spelling(word)).collect();
    let column_words: Vec<&str> = pointing.iter().copied().filter(|word| !GENERIC_COLUMNS.contains(word)).collect();
    let ranked = |entry: Entry<'a>| Some((reason(entry.1, named, asked, &pointing, &column_words)?, Reverse(table_use.of(entry.1)), entry));
    let mut kept: Vec<_> = tables(catalog).filter_map(ranked).collect();
    kept.sort_by_key(|&(reason, used, _)| (reason, used));
    kept.into_iter().map(|(_, _, entry)| entry).collect()
}

fn tables(catalog: &Catalog) -> impl Iterator<Item = Entry<'_>> {
    catalog.schema.schemas.iter().flat_map(|node| node.tables.iter().map(move |table| (node, table)))
}

fn reason(table: &Table, named: &[String], asked: &[String], pointing: &[&str], column_words: &[&str]) -> Option<Reason> {
    let name = table.name.to_lowercase();
    let is_long_name = name.chars().count() >= MIN_WORD;
    let is_named_by_start = |word: &&str| name.starts_with(*word) || (is_long_name && word.starts_with(name.as_str()));
    let is_asked_column = |column: &SchemaColumn| column_words.contains(&column.name.to_lowercase().as_str());
    let has_asked_column = !column_words.is_empty() && table.columns.iter().any(is_asked_column);
    match () {
        () if named.contains(&name) => Some(Reason::NamedByStatement),
        () if asked.contains(&name) || pointing.iter().any(is_named_by_start) => Some(Reason::NamedByInstruction),
        () if has_asked_column => Some(Reason::ColumnNamedByInstruction),
        () => None,
    }
}

/// The tables one foreign key away from those `kept`, none kept already: for each kept table in order, those it points
/// to, then those pointing to it.
fn linked<'a>(catalog: &'a Catalog, kept: &[Entry<'a>]) -> Vec<Entry<'a>> {
    let by_name: HashMap<(&str, &str), Entry<'a>> = tables(catalog).map(|entry| (name_of(entry), entry)).collect();
    let pointing_to = pointing_to(catalog);
    let targets = |(_, table): Entry<'a>| table.foreign_keys.iter().filter_map(|key| by_name.get(&target_name(key)).copied());
    let pointing = |entry: Entry<'a>| pointing_to.get(&name_of(entry)).into_iter().flatten().copied();
    let mut seen: HashSet<(&str, &str)> = kept.iter().copied().map(name_of).collect();
    let neighbours = kept.iter().flat_map(|&entry| targets(entry).chain(pointing(entry)));
    neighbours.filter(|&neighbour| seen.insert(name_of(neighbour))).collect()
}

/// The tables whose foreign keys point to each table, by its name.
fn pointing_to(catalog: &Catalog) -> HashMap<(&str, &str), Vec<Entry<'_>>> {
    let mut pointing_to: HashMap<(&str, &str), Vec<Entry<'_>>> = HashMap::new();
    for entry in tables(catalog) {
        for key in &entry.1.foreign_keys {
            pointing_to.entry(target_name(key)).or_default().push(entry);
        }
    }
    pointing_to
}

fn name_of((node, table): Entry<'_>) -> (&str, &str) {
    (node.name.as_str(), table.name.as_str())
}

fn target_name(key: &ForeignKey) -> (&str, &str) {
    (key.target_schema.as_str(), key.target_table.as_str())
}

/// The `schema.table` of every table but those `shown`, in the order of the search path, then the other schemas.
fn names_other_than<'a>(catalog: &'a Catalog, shown: &[Entry<'a>]) -> impl Iterator<Item = String> + use<'a> {
    let shown: HashSet<(&str, &str)> = shown.iter().copied().map(name_of).collect();
    let mut entries: Vec<Entry<'a>> = tables(catalog).filter(|&entry| !shown.contains(&name_of(entry))).collect();
    entries.sort_by_key(|(node, _)| catalog.search_path.iter().position(|schema| *schema == node.name).unwrap_or(usize::MAX));
    entries.into_iter().map(|(node, table)| qualified(&node.name, &table.name))
}

/// The words and quoted names of `statement`, lowercased and unquoted; what its literals hold is no name.
fn statement_names(statement: &str) -> Vec<String> {
    let tokens = lex::tokens(statement);
    let is_name = |text: &str, token: &Token| *token == Token::Word || text.starts_with('"');
    let texts = tokens.iter().map(|placed| (&statement[placed.range.clone()], &placed.token));
    texts.filter(|(text, token)| is_name(text, token)).map(|(text, _)| text.trim_matches('"').to_lowercase()).collect()
}

fn instruction_words(instruction: &str) -> Vec<String> {
    let words = instruction.split(|c: char| !(c.is_alphanumeric() || c == '_'));
    words.map(str::to_lowercase).collect()
}

/// Whether a lowercase instruction `word` may name what it starts or a column: too short or too common a one only names a
/// table spelt just as it is, so a table `io` or `list` stays reachable.
fn points_past_its_spelling(word: &str) -> bool {
    word.chars().count() >= MIN_WORD && !STOP_WORDS.contains(&word)
}

/// `schema.table(...)` with each column, then each foreign key no single column carries, then the table's comment.
fn line(node: &SchemaNode, table: &Table) -> String {
    let columns = table.columns.iter().map(|column| column_part(column, &table.foreign_keys));
    let carried_by_none = table.foreign_keys.iter().filter(|key| !is_carried_by_a_column(table, key));
    let parts: Vec<String> = columns.chain(carried_by_none.map(table_key)).collect();
    format!("{}({}){}", qualified(&node.name, &table.name), parts.join(", "), comment_part(table.comment.as_deref()))
}

/// `name type`, its enum labels, each foreign key it carries alone as `references schema.table(column)`, then its comment.
fn column_part(column: &SchemaColumn, keys: &[ForeignKey]) -> String {
    let references: String =
        keys.iter().filter(|key| key.columns == [column.name.as_str()]).map(|key| format!(" references {}", target(key))).collect();
    let labels = labels_part(&column.enum_labels);
    let comment = comment_part(column.comment.as_deref());
    format!("{} {}{labels}{references}{comment}", sql_name(&column.name), column.type_name.as_str())
}

/// ` one of ('label', ...)` on one line and cut to [`MAX_COMMENT_BYTES`], a label holding a secret left out as a comment
/// holding one is, and [`CUT_MARK`] saying so; nothing when no label is left.
fn labels_part(labels: &[String]) -> String {
    let shown: Vec<String> = labels.iter().filter(|label| !secret::holds_secret(label)).map(|label| label_literal(label)).collect();
    if shown.is_empty() {
        return String::new();
    }
    let left_out = if shown.len() < labels.len() { format!(", {CUT_MARK}") } else { String::new() };
    format!(" one of ({}{left_out})", cut(&shown.join(", ")))
}

/// `label` as a string literal on one line.
fn label_literal(label: &str) -> String {
    sql_string(&on_one_line(label))
}

/// Whether `key` is made of one column of `table`, which then carries it.
fn is_carried_by_a_column(table: &Table, key: &ForeignKey) -> bool {
    matches!(key.columns.as_slice(), [only] if table.columns.iter().any(|column| column.name == *only))
}

fn table_key(key: &ForeignKey) -> String {
    format!("foreign key ({}) references {}", column_list(&key.columns), target(key))
}

fn target(key: &ForeignKey) -> String {
    format!("{}({})", qualified(&key.target_schema, &key.target_table), column_list(&key.target_columns))
}

fn column_list(columns: &[String]) -> String {
    columns.iter().map(|column| sql_name(column)).collect::<Vec<_>>().join(", ")
}

/// ` /* comment */` on one line and cut to [`MAX_COMMENT_BYTES`], or nothing when there is none or it holds a secret, by
/// the rule the history keeps.
fn comment_part(comment: Option<&str>) -> String {
    let Some(comment) = comment.filter(|comment| !secret::holds_secret(comment)) else { return String::new() };
    format!(" /* {} */", cut(&on_one_line(comment)).replace("*/", "* /"))
}

fn on_one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` whole when it fits in [`MAX_COMMENT_BYTES`], else its first characters that do and [`CUT_MARK`].
fn cut(text: &str) -> String {
    if text.len() <= MAX_COMMENT_BYTES {
        return text.to_owned();
    }
    let end = text.floor_char_boundary(MAX_COMMENT_BYTES - CUT_MARK.len());
    format!("{}{CUT_MARK}", text[..end].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{key_to_id, schema_column, schema_table, shop_catalog};

    fn lines_used<'a>(
        catalog: &'a Catalog,
        statement: &str,
        instruction: &str,
        table_use: &TableUse,
    ) -> impl Iterator<Item = String> + use<'a> {
        slice(catalog, statement, instruction, table_use).map(|line| line.text().to_owned())
    }

    fn lines_of<'a>(catalog: &'a Catalog, statement: &str, instruction: &str) -> impl Iterator<Item = String> + use<'a> {
        lines_used(catalog, statement, instruction, &TableUse::default())
    }

    fn names_in(lines: impl Iterator<Item = String>) -> Vec<String> {
        lines.map(|line| line.split('(').next().unwrap_or_default().to_owned()).collect()
    }

    fn tables_in(catalog: &Catalog, statement: &str, instruction: &str) -> Vec<String> {
        names_in(lines_of(catalog, statement, instruction))
    }

    fn line_of(catalog: &Catalog, name: &str) -> String {
        tables(catalog).find(|(_, table)| table.name == name).map(|(node, table)| line(node, table)).unwrap()
    }

    /// `shop` with `public.users`, which `customers.user_id` points to: `orders` reaches `users` through `customers`.
    fn shop_with_users() -> Catalog {
        let mut catalog = shop_catalog();
        let public = &mut catalog.schema.schemas[1];
        public.tables[1].columns.push(schema_column("user_id", "integer", None));
        public.tables[1].foreign_keys.push(key_to_id("user_id", "public", "users"));
        public.tables.push(schema_table("users", vec![schema_column("id", "integer", None)]));
        catalog
    }

    #[test]
    fn a_table_shows_its_columns_types_and_foreign_keys() {
        assert_eq!(
            line_of(&shop_catalog(), "orders"),
            "public.orders(id integer, customer_id integer references public.customers(id), total numeric(10,2))"
        );
    }

    #[test]
    fn a_foreign_key_of_several_columns_follows_the_columns() {
        let mut catalog = shop_catalog();
        let key = &mut catalog.schema.schemas[1].tables[0].foreign_keys[0];
        key.columns = vec!["customer_id".into(), "total".into()];
        key.target_columns = vec!["id".into(), "name".into()];
        assert_eq!(
            line_of(&catalog, "orders"),
            "public.orders(id integer, customer_id integer, total numeric(10,2), \
             foreign key (customer_id, total) references public.customers(id, name))"
        );
    }

    #[test]
    fn the_comments_of_a_table_and_its_columns_show_on_its_line() {
        let mut catalog = shop_catalog();
        let customers = &mut catalog.schema.schemas[1].tables[1];
        customers.comment = Some("one row a buyer".into());
        customers.columns[1].comment = Some("as typed\nat checkout */".into());
        assert_eq!(
            line_of(&catalog, "customers"),
            "public.customers(id integer, name text /* as typed at checkout * / */) /* one row a buyer */"
        );
    }

    #[test]
    fn a_long_comment_is_cut_to_a_few_hundred_bytes() {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables[1].comment = Some("one row a buyer ".repeat(100));
        let line = line_of(&catalog, "customers");
        assert!(line.len() < 400 && line.ends_with("… */"), "{line}");
    }

    #[test]
    fn a_comment_is_cut_between_two_characters() {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables[1].comment = Some("Zoë".repeat(200));
        assert!(line_of(&catalog, "customers").ends_with("… */"));
    }

    #[test]
    fn a_comment_holding_a_secret_is_left_out() {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables[1].columns[1].comment = Some("password of nina".into());
        assert_eq!(line_of(&catalog, "customers"), "public.customers(id integer, name text)");
    }

    fn customers_with_labels(labels: &[&str]) -> String {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables[1].columns[1].enum_labels = labels.iter().map(|label| (*label).to_owned()).collect();
        line_of(&catalog, "customers")
    }

    #[test]
    fn the_labels_of_an_enum_follow_the_type_of_their_column() {
        assert_eq!(customers_with_labels(&["new", "won't pay"]), "public.customers(id integer, name text one of ('new', 'won''t pay'))");
    }

    #[test]
    fn a_label_holding_a_secret_is_left_out_and_the_list_says_it_is_not_whole() {
        assert_eq!(customers_with_labels(&["new", "password"]), "public.customers(id integer, name text one of ('new', …))");
    }

    #[test]
    fn labels_all_holding_a_secret_show_nothing() {
        assert_eq!(customers_with_labels(&["password"]), "public.customers(id integer, name text)");
    }

    #[test]
    fn a_long_list_of_labels_is_cut_to_a_few_hundred_bytes() {
        let line = customers_with_labels(&["some quite long label"; 100]);
        assert!(line.len() < 400 && line.ends_with("…))"), "{line}");
    }

    #[test]
    fn a_label_on_several_lines_shows_on_one() {
        assert_eq!(customers_with_labels(&["on\nhold"]), "public.customers(id integer, name text one of ('on hold'))");
    }

    #[test]
    fn the_tables_the_statement_names_come_first() {
        let kept = tables_in(&shop_catalog(), "SELECT * FROM billing.invoices", "join the customers");
        assert_eq!(kept[..2], ["billing.invoices", "public.customers"]);
    }

    #[test]
    fn an_instruction_word_names_a_table_in_the_singular_too() {
        assert_eq!(tables_in(&shop_catalog(), "", "the last order")[0], "public.orders");
    }

    #[test]
    fn an_instruction_word_naming_a_column_brings_its_tables_after_the_named_ones() {
        assert_eq!(tables_in(&shop_catalog(), "", "customers by order_id")[..2], ["public.customers", "billing.invoices"]);
    }

    #[test]
    fn the_tables_a_foreign_key_links_come_after_the_kept_ones_either_way() {
        assert_eq!(tables_in(&shop_catalog(), "SELECT * FROM orders", ""), ["public.orders", "public.customers", "billing.invoices"]);
    }

    #[test]
    fn the_links_of_the_first_kept_table_come_first() {
        let kept = tables_in(&shop_with_users(), "SELECT * FROM users, invoices", "");
        assert_eq!(kept, ["billing.invoices", "public.users", "public.orders", "public.customers"]);
    }

    #[test]
    fn orders_of_a_user_bring_the_customers_linking_them() {
        let kept = tables_in(&shop_with_users(), "", "Show orders associated to user nina");
        assert!(kept.contains(&"public.customers".to_owned()), "{kept:?}");
    }

    #[test]
    fn orders_of_a_customer_bring_the_users_the_customers_point_to() {
        let kept = tables_in(&shop_with_users(), "", "Show orders associated to customer nina");
        assert!(kept.contains(&"public.users".to_owned()), "{kept:?}");
    }

    fn used(catalog: &Catalog, statements: &[&str]) -> TableUse {
        TableUse::counted(catalog, statements.iter().copied())
    }

    #[test]
    fn with_the_same_reason_the_table_run_most_comes_first() {
        let catalog = shop_catalog();
        let table_use = used(&catalog, &["SELECT * FROM public.customers", "SELECT name FROM CUSTOMERS", "SELECT * FROM orders"]);
        assert_eq!(names_in(lines_used(&catalog, "", "orders of customers", &table_use))[..2], ["public.customers", "public.orders"]);
    }

    #[test]
    fn use_does_not_pass_a_table_of_a_stronger_reason() {
        let catalog = shop_catalog();
        let table_use = used(&catalog, &["SELECT * FROM customers", "SELECT * FROM customers"]);
        let lines = lines_used(&catalog, "SELECT * FROM orders", "the customers", &table_use);
        assert_eq!(names_in(lines)[..2], ["public.orders", "public.customers"]);
    }

    #[test]
    fn with_no_use_tables_of_the_same_reason_stay_in_catalog_order() {
        assert_eq!(tables_in(&shop_catalog(), "", "orders of customers")[..2], ["public.orders", "public.customers"]);
    }

    #[test]
    fn a_table_named_twice_in_a_statement_counts_once() {
        let catalog = shop_catalog();
        let table_use = used(&catalog, &["SELECT * FROM orders a, orders b", "SELECT * FROM customers", "SELECT * FROM customers"]);
        assert_eq!(names_in(lines_used(&catalog, "", "orders of customers", &table_use))[..2], ["public.customers", "public.orders"]);
    }

    #[test]
    fn a_table_named_only_in_a_literal_or_a_comment_is_not_counted() {
        let catalog = shop_catalog();
        let table_use = used(&catalog, &["SELECT 'customers' -- customers"]);
        assert_eq!(table_use, used(&catalog, &[]));
    }

    #[test]
    fn no_statement_or_count_of_the_history_reaches_a_line() {
        let catalog = shop_catalog();
        let table_use = used(&catalog, &["SELECT 'nina' FROM customers"]);
        let lines: Vec<String> = lines_used(&catalog, "", "customers", &table_use).collect();
        assert_eq!(lines, lines_of(&catalog, "", "customers").collect::<Vec<_>>());
    }

    #[test]
    fn a_table_with_a_short_name_is_not_named_by_every_word_it_starts() {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables.push(schema_table("o", vec![]));
        assert_eq!(tables_in(&catalog, "", "the last order")[0], "public.orders");
    }

    #[test]
    fn the_other_tables_follow_by_name_alone_the_search_path_first() {
        let lines: Vec<String> = lines_of(&shop_with_users(), "SELECT * FROM users", "").collect();
        assert_eq!(
            lines,
            [
                "public.users(id integer)",
                "public.customers(id integer, name text, user_id integer references public.users(id))",
                "public.orders",
                "billing.invoices",
            ]
        );
    }

    #[test]
    fn with_no_table_named_every_table_shows_by_name_alone_the_search_path_first() {
        assert_eq!(
            lines_of(&shop_catalog(), "SELECT * FROM ", "").collect::<Vec<_>>(),
            ["public.orders", "public.customers", "billing.invoices"]
        );
    }

    #[test]
    fn a_word_inside_a_literal_names_no_table() {
        assert!(lines_of(&shop_catalog(), "SELECT 'orders'", "").eq(lines_of(&shop_catalog(), "", "")));
    }

    #[test]
    fn a_short_instruction_word_names_nothing() {
        assert!(lines_of(&shop_catalog(), "", "an id of it").eq(lines_of(&shop_catalog(), "", "")));
    }

    #[test]
    fn a_table_spelt_as_a_short_or_common_word_comes_in_detail_when_asked_for() {
        for name in ["io", "list"] {
            let mut catalog = shop_catalog();
            catalog.schema.schemas[1].tables.push(schema_table(name, vec![schema_column("id", "integer", None)]));
            assert_eq!(tables_in(&catalog, "", &format!("rows of {name}"))[0], format!("public.{name}"));
            assert!(lines_of(&catalog, "", &format!("rows of {name}")).any(|line| line == format!("public.{name}(id integer)")));
        }
    }

    #[test]
    fn a_table_shown_by_name_alone_comes_in_detail_once_the_instruction_asks_for_it() {
        let catalog = shop_catalog();
        assert!(lines_of(&catalog, "SELECT * FROM customers", "").any(|line| line == "billing.invoices"));
        assert!(lines_of(&catalog, "SELECT * FROM customers", "join the invoices").any(|line| line.starts_with("billing.invoices(")));
    }

    #[test]
    fn a_common_word_names_no_table_it_starts() {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables.push(schema_table("forms", vec![schema_column("title", "text", None)]));
        assert!(lines_of(&catalog, "", "for the last week").eq(lines_of(&catalog, "", "")));
    }

    #[test]
    fn a_common_word_names_no_column() {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables[0].columns.push(schema_column("from", "date", None));
        assert!(lines_of(&catalog, "", "from last week").eq(lines_of(&catalog, "", "")));
    }

    #[test]
    fn a_generic_column_name_brings_no_table() {
        for generic in GENERIC_COLUMNS {
            let mut catalog = shop_catalog();
            catalog.schema.schemas[1].tables[0].columns.push(schema_column(generic, "text", None));
            assert!(lines_of(&catalog, "", &format!("sorted by {generic}")).eq(lines_of(&catalog, "", "")), "{generic}");
        }
    }

    #[test]
    fn a_generic_word_still_names_a_table_called_so() {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables.push(schema_table("statuses", vec![]));
        assert_eq!(tables_in(&catalog, "", "the status of it")[0], "public.statuses");
    }
}
