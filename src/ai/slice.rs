//! The part of a catalog a prompt shows, the tables most likely wanted first: each as the body of a `CREATE TABLE`, its
//! columns with their types, foreign keys and comments; or, when nothing points to a table, every table's name alone.
use crate::db::lex::{self, Token};
use crate::db::model::{Catalog, ForeignKey, SchemaColumn, SchemaNode, Table, qualified, sql_name};
use crate::secret;
use std::collections::{HashMap, HashSet};

/// The shortest instruction word that may name a table or a column, so `a` or `of` pulls in nothing.
const MIN_WORD: usize = 3;
/// How much of a comment a line keeps, so one long comment never takes the room of other tables.
const MAX_COMMENT_BYTES: usize = 300;
/// What ends a comment cut short.
const CUT_MARK: &str = "…";

/// Why a table is in the slice, the most likely wanted first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reason {
    NamedByStatement,
    NamedByInstruction,
    ColumnNamedByInstruction,
}

/// A table of the catalog and the schema holding it.
type Entry<'a> = (&'a SchemaNode, &'a Table);

/// One line per table of `catalog` the `statement` names or the `instruction`'s words point to, as
/// `schema.table(column type references schema.table(column), ...)`: first the tables the statement names, then those
/// the instruction names, then those holding a column it names, then those a foreign key links to one of them, the
/// links of the first kept first.
/// When none is kept, one line per table naming it alone, in the order of the search path, then the other schemas.
/// Each line is written only once asked for, so a caller that stops at a cap writes no more.
pub(super) fn slice<'a>(catalog: &'a Catalog, statement: &str, instruction: &str) -> impl Iterator<Item = String> + 'a {
    let kept = kept(catalog, &statement_names(statement), &instruction_words(instruction));
    let names = kept.is_empty().then(|| names(catalog)).into_iter().flatten();
    let linked = linked(catalog, &kept);
    kept.into_iter().chain(linked).map(|(node, table)| line(node, table)).chain(names)
}

fn kept<'a>(catalog: &'a Catalog, named: &[String], asked: &[String]) -> Vec<Entry<'a>> {
    let mut kept: Vec<(Reason, Entry<'a>)> = tables(catalog).filter_map(|entry| Some((reason(entry.1, named, asked)?, entry))).collect();
    kept.sort_by_key(|(reason, _)| *reason);
    kept.into_iter().map(|(_, entry)| entry).collect()
}

fn tables(catalog: &Catalog) -> impl Iterator<Item = Entry<'_>> {
    catalog.schema.schemas.iter().flat_map(|node| node.tables.iter().map(move |table| (node, table)))
}

fn reason(table: &Table, named: &[String], asked: &[String]) -> Option<Reason> {
    let name = table.name.to_lowercase();
    let is_long_name = name.chars().count() >= MIN_WORD;
    let is_asked = |word: &String| name.starts_with(word.as_str()) || (is_long_name && word.starts_with(name.as_str()));
    let has_asked_column = table.columns.iter().any(|column| asked.contains(&column.name.to_lowercase()));
    match () {
        () if named.contains(&name) => Some(Reason::NamedByStatement),
        () if asked.iter().any(is_asked) => Some(Reason::NamedByInstruction),
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

/// Every table's `schema.table`, in the order of the search path, then the other schemas.
fn names(catalog: &Catalog) -> impl Iterator<Item = String> {
    let mut entries: Vec<Entry<'_>> = tables(catalog).collect();
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
    words.filter(|word| word.chars().count() >= MIN_WORD).map(str::to_lowercase).collect()
}

/// `schema.table(...)` with each column, then each foreign key no single column carries, then the table's comment.
fn line(node: &SchemaNode, table: &Table) -> String {
    let columns = table.columns.iter().map(|column| column_part(column, &table.foreign_keys));
    let carried_by_none = table.foreign_keys.iter().filter(|key| !is_carried_by_a_column(table, key));
    let parts: Vec<String> = columns.chain(carried_by_none.map(table_key)).collect();
    format!("{}({}){}", qualified(&node.name, &table.name), parts.join(", "), comment_part(table.comment.as_deref()))
}

/// `name type`, each foreign key it carries alone as `references schema.table(column)`, then its comment.
fn column_part(column: &SchemaColumn, keys: &[ForeignKey]) -> String {
    let references: String =
        keys.iter().filter(|key| key.columns == [column.name.as_str()]).map(|key| format!(" references {}", target(key))).collect();
    let comment = comment_part(column.comment.as_deref());
    format!("{} {}{references}{comment}", sql_name(&column.name), column.type_name.as_str())
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
    let one_line = comment.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(" /* {} */", cut(&one_line).replace("*/", "* /"))
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

    fn tables_in(catalog: &Catalog, statement: &str, instruction: &str) -> Vec<String> {
        slice(catalog, statement, instruction).map(|line| line.split('(').next().unwrap_or_default().to_owned()).collect()
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

    #[test]
    fn a_table_with_a_short_name_is_not_named_by_every_word_it_starts() {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables.push(schema_table("o", vec![]));
        assert_eq!(tables_in(&catalog, "", "the last order")[0], "public.orders");
    }

    #[test]
    fn with_no_table_named_every_table_shows_by_name_alone_the_search_path_first() {
        assert_eq!(
            slice(&shop_catalog(), "SELECT * FROM ", "").collect::<Vec<_>>(),
            ["public.orders", "public.customers", "billing.invoices"]
        );
    }

    #[test]
    fn a_word_inside_a_literal_names_no_table() {
        assert!(slice(&shop_catalog(), "SELECT 'orders'", "").eq(slice(&shop_catalog(), "", "")));
    }

    #[test]
    fn a_short_instruction_word_names_nothing() {
        assert!(slice(&shop_catalog(), "", "an id of it").eq(slice(&shop_catalog(), "", "")));
    }
}
