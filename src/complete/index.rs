//! The names of one catalog, lowercased and sorted once, so a prefix is a binary search.
use super::keyword::{is_keyword, keywords};
use crate::db::model::{Catalog, SchemaNode, Table};
use std::collections::{HashMap, HashSet};

/// The tables, columns and keywords completion offers, built once per catalog and never changed.
#[derive(Debug)]
pub(crate) struct Index {
    /// Bare in a schema of the search path that is the first to hold that name, else `schema.table`.
    pub(super) tables: Vec<Name>,
    /// The columns of each table, in the order of `tables`.
    pub(super) table_columns: Vec<Vec<Name>>,
    /// The columns of every table, each name once.
    pub(super) columns: Vec<Name>,
    pub(super) keywords: Vec<Name>,
    /// Each table by its name in `tables` and by `schema.table`, lowercased.
    by_name: HashMap<String, usize>,
}

/// A name as the catalog writes it, its lowercase key for the search, and its text quoted when it needs quotes.
#[derive(Debug)]
pub(super) struct Name {
    pub(super) key: String,
    pub(super) text: String,
    pub(super) quoted: Option<Box<str>>,
}

/// A table as completion names it, with its `schema.table`.
struct Named<'a> {
    name: Name,
    qualified: String,
    table: &'a Table,
}

impl Index {
    pub(crate) fn new(catalog: &Catalog) -> Self {
        let mut named = named_tables(catalog);
        named.sort_unstable_by(|left, right| left.name.key.cmp(&right.name.key));
        let by_name = named.iter().enumerate().flat_map(|(at, table)| [(table.name.key.clone(), at), (table.qualified.clone(), at)]);
        let by_name = by_name.collect();
        let table_columns: Vec<Vec<Name>> =
            named.iter().map(|table| sorted(table.table.columns.iter().map(|column| Name::new(&column.name)))).collect();
        let mut columns = sorted(named.iter().flat_map(|table| table.table.columns.iter().map(|column| Name::new(&column.name))));
        columns.dedup_by(|left, right| left.key == right.key);
        let keywords = sorted(keywords().map(Name::new));
        Self { tables: named.into_iter().map(|table| table.name).collect(), table_columns, columns, keywords, by_name }
    }

    /// The columns of the table a statement names `name`, lowercased, bare or with its schema.
    pub(super) fn columns_of(&self, name: &str) -> Option<&[Name]> {
        self.by_name.get(name).map(|&at| self.table_columns[at].as_slice())
    }
}

impl Default for Index {
    fn default() -> Self {
        Self::new(&Catalog::default())
    }
}

impl Name {
    fn new(text: &str) -> Self {
        Self { key: text.to_lowercase(), text: text.to_owned(), quoted: needs_quotes(text).then(|| quoted(text).into()) }
    }

    fn qualified(schema: &str, table: &str) -> Self {
        let is_quoted = needs_quotes(schema) || needs_quotes(table);
        let text = format!("{schema}.{table}");
        let quoted = is_quoted.then(|| format!("{}.{}", quoted_if_needed(schema), quoted_if_needed(table)).into());
        Self { key: text.to_lowercase(), text, quoted }
    }
}

/// The names that start with `prefix`, already lowercased.
pub(super) fn starting<'a>(names: &'a [Name], prefix: &str) -> &'a [Name] {
    let from = names.partition_point(|name| name.key.as_str() < prefix);
    let count = names[from..].partition_point(|name| name.key.starts_with(prefix));
    &names[from..from + count]
}

fn named_tables(catalog: &Catalog) -> Vec<Named<'_>> {
    let bare = bare_tables(catalog);
    let schemas = catalog.schema.schemas.iter();
    schemas.flat_map(|node| node.tables.iter().map(|table| named_table(node, table, &bare))).collect()
}

fn named_table<'a>(node: &SchemaNode, table: &'a Table, bare: &HashSet<(&str, &str)>) -> Named<'a> {
    let is_bare = bare.contains(&(node.name.as_str(), table.name.as_str()));
    let name = if is_bare { Name::new(&table.name) } else { Name::qualified(&node.name, &table.name) };
    Named { name, qualified: format!("{}.{}", node.name, table.name).to_lowercase(), table }
}

/// The tables a statement names bare: in a schema of the search path, the first one there to hold that name.
fn bare_tables(catalog: &Catalog) -> HashSet<(&str, &str)> {
    let mut claimed = HashSet::new();
    let in_path = catalog.search_path.iter().filter_map(|schema| catalog.schema.schemas.iter().find(|node| &node.name == schema));
    let tables = in_path.flat_map(|node| node.tables.iter().map(move |table| (node.name.as_str(), table.name.as_str())));
    tables.filter(|(_, table)| claimed.insert(*table)).collect()
}

fn sorted(names: impl Iterator<Item = Name>) -> Vec<Name> {
    let mut names: Vec<Name> = names.collect();
    names.sort_unstable_by(|left, right| left.key.cmp(&right.key));
    names
}

/// Whether PostgreSQL reads `name` as written only between double quotes: an upper-case letter, a sign or a keyword.
fn needs_quotes(name: &str) -> bool {
    let is_plain = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$';
    let starts_plain = name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_');
    !starts_plain || !name.chars().all(is_plain) || is_keyword(&name.to_uppercase())
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn quoted_if_needed(name: &str) -> String {
    if needs_quotes(name) { quoted(name) } else { name.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::shop_schema;

    fn shop(search_path: &[&str]) -> Index {
        let search_path = search_path.iter().map(|schema| (*schema).to_owned()).collect();
        Index::new(&Catalog { schema: shop_schema(), search_path, is_cut: false })
    }

    fn texts(names: &[Name]) -> Vec<&str> {
        names.iter().map(|name| name.text.as_str()).collect()
    }

    #[test]
    fn tables_in_the_search_path_are_bare_and_the_others_carry_their_schema() {
        assert_eq!(texts(&shop(&["public"]).tables), ["billing.invoices", "customers", "orders"]);
        assert_eq!(texts(&shop(&["billing"]).tables), ["invoices", "public.customers", "public.orders"]);
    }

    #[test]
    fn a_name_two_schemas_of_the_path_hold_is_bare_in_the_first_only() {
        let mut schema = shop_schema();
        schema.schemas[0].tables[0].name = "orders".to_owned();
        let index = Index::new(&Catalog { schema, search_path: vec!["billing".to_owned(), "public".to_owned()], is_cut: false });
        assert_eq!(texts(&index.tables), ["customers", "orders", "public.orders"]);
    }

    #[test]
    fn a_prefix_finds_the_names_it_starts() {
        assert_eq!(texts(starting(&shop(&["public"]).tables, "c")), ["customers"]);
        assert_eq!(texts(starting(&shop(&["public"]).tables, "billing.")), ["billing.invoices"]);
        assert_eq!(texts(starting(&shop(&["public"]).tables, "z")), Vec::<&str>::new());
    }

    #[test]
    fn keywords_are_found_by_a_prefix_in_any_case() {
        assert_eq!(texts(starting(&shop(&["public"]).keywords, "sel")), ["SELECT"]);
    }

    #[test]
    fn every_column_comes_once_whatever_the_tables_holding_it() {
        assert_eq!(texts(&shop(&["public"]).columns), ["customer_id", "id", "name", "order_id", "total"]);
    }

    #[test]
    fn a_table_is_found_bare_or_with_its_schema() {
        let index = shop(&["public"]);
        assert_eq!(index.columns_of("orders").map(texts), Some(vec!["customer_id", "id", "total"]));
        assert_eq!(index.columns_of("public.orders").map(texts), Some(vec!["customer_id", "id", "total"]));
        assert_eq!(index.columns_of("invoices").map(texts), None);
    }

    #[test]
    fn a_name_with_an_upper_case_letter_a_sign_or_a_keyword_is_quoted() {
        assert_eq!(Name::new("Order Items").quoted.as_deref(), Some("\"Order Items\""));
        assert_eq!(Name::new("order").quoted.as_deref(), Some("\"order\""));
        assert_eq!(Name::new("orders").quoted, None);
        assert_eq!(Name::qualified("Billing", "invoices").quoted.as_deref(), Some("\"Billing\".invoices"));
    }
}
