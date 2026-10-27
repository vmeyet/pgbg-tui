//! The names of one catalog, each list a vtui completion index, and how each name goes in the editor.
use super::keyword::keywords;
use crate::db::model::{Catalog, SchemaNode, Table, needs_quotes, qualified, sql_name};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use vtui::complete;
use vtui::toward::Toward;

/// The tables, columns and keywords completion offers, built once per catalog and never changed.
#[derive(Debug)]
pub(crate) struct Index {
    /// Each table bare when its schema is the first of the search path to hold that name, else `schema.table`.
    pub(super) tables: Names,
    /// The columns of each table, by its place in `by_name`.
    table_columns: Vec<Names>,
    /// The columns of every table, each name once.
    pub(super) columns: Names,
    pub(super) keywords: Names,
    /// Each table by its completion name and by `schema.table`, spelled exactly as PostgreSQL resolves a name.
    by_name: HashMap<String, usize>,
    /// The names that need quotes, as spelled: every other name goes in as it is.
    quoted: HashMap<String, Box<str>>,
}

/// One list of names: vtui's index of it, and how to spell back a name vtui found.
#[derive(Debug)]
pub(super) struct Names {
    index: complete::Index,
    /// The first name of the list holding each lowercase text, when it is not that text: vtui ranks it before the others.
    spellings: HashMap<String, Box<str>>,
}

/// The name the ghost found: what shows after what was typed, and the name as the catalog spells it.
pub(super) struct Found {
    pub(super) rest: String,
    pub(super) name: String,
}

/// An index is never compared by content: two are equal only when they are the same one, as the `Arc` a sheet keeps.
impl PartialEq for Index {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Index {}

/// A table as completion names it, with its `schema.table` and its name quoted when it needs quotes.
struct Named<'a> {
    name: String,
    quoted: Option<String>,
    qualified: String,
    table: &'a Table,
}

impl Index {
    pub(crate) fn new(catalog: &Catalog) -> Self {
        let named = named_tables(catalog);
        let by_name = named.iter().enumerate().flat_map(|(at, table)| [(table.name.clone(), at), (table.qualified.clone(), at)]);
        let table_columns = named.iter().map(|table| Names::new(column_names(table.table).collect())).collect();
        let columns = distinct(named.iter().flat_map(|table| column_names(table.table)));
        let quoted_tables = named.iter().filter_map(|table| Some((table.name.clone(), table.quoted.clone()?.into())));
        let quoted_columns = named.iter().flat_map(|table| column_names(table.table)).filter(|name| needs_quotes(name));
        let quoted = quoted_tables.chain(quoted_columns.map(|name| (name.to_owned(), sql_name(name).into()))).collect();
        Self {
            tables: Names::new(named.iter().map(|table| table.name.as_str()).collect()),
            table_columns,
            columns: Names::new(columns),
            keywords: Names::new(keywords().collect()),
            by_name: by_name.collect(),
            quoted,
        }
    }

    /// The columns of the table a statement names `name`, as PostgreSQL resolves it, bare or with its schema.
    pub(super) fn columns_of(&self, name: &str) -> Option<&Names> {
        self.by_name.get(name).map(|&at| &self.table_columns[at])
    }

    /// What goes in the editor for the table or column `name`, spelled as the catalog does: quoted when it needs quotes.
    pub(super) fn insert(&self, name: &str) -> String {
        self.quoted.get(name).map_or_else(|| name.to_owned(), ToString::to_string)
    }

    /// The index of no catalog, the keywords alone, for a sheet whose schema is not read.
    pub(crate) fn of_keywords() -> &'static Self {
        static KEYWORDS_ONLY: LazyLock<Index> = LazyLock::new(|| Index::new(&Catalog::default()));
        &KEYWORDS_ONLY
    }
}

impl Names {
    fn new(names: Vec<&str>) -> Self {
        Self { spellings: spellings(&names), index: complete::Index::new(names) }
    }

    /// The best name `typed` starts, or the first name when nothing was typed.
    pub(super) fn ghost(&self, typed: &str) -> Option<Found> {
        if typed.is_empty() {
            let name = self.index.cycle("")?.current().to_owned();
            return Some(Found { rest: name.clone(), name });
        }
        let rest = self.index.ghost(typed)?;
        let key = format!("{typed}{rest}").to_lowercase();
        let name = self.spellings.get(&key).map_or(key, ToString::to_string);
        Some(Found { rest, name })
    }

    /// The names vtui ranks for `token`, best first, at most 50.
    pub(super) fn ranked(&self, token: &str) -> Vec<String> {
        let Some(first) = self.index.cycle(token) else { return vec![] };
        let mut cycle = first.clone();
        let mut ranked = vec![];
        loop {
            ranked.push(cycle.current().to_owned());
            cycle.advance(Toward::Next);
            if cycle == first {
                return ranked;
            }
        }
    }
}

/// The first of `names` to hold each lowercase text, when it is not that text.
fn spellings(names: &[&str]) -> HashMap<String, Box<str>> {
    let mut first = HashMap::new();
    for name in names {
        first.entry(name.to_lowercase()).or_insert(*name);
    }
    first.into_iter().filter(|(key, name)| key != name).map(|(key, name)| (key, name.into())).collect()
}

fn column_names(table: &Table) -> impl Iterator<Item = &str> {
    table.columns.iter().map(|column| column.name.as_str())
}

/// `names` in their order, each once.
fn distinct<'a>(names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut seen = HashSet::new();
    names.filter(|name| seen.insert(*name)).collect()
}

fn named_tables(catalog: &Catalog) -> Vec<Named<'_>> {
    let bare = bare_tables(catalog);
    let schemas = catalog.schema.schemas.iter();
    schemas.flat_map(|node| node.tables.iter().map(|table| named_table(node, table, &bare))).collect()
}

fn named_table<'a>(node: &SchemaNode, table: &'a Table, bare: &HashSet<(&str, &str)>) -> Named<'a> {
    let is_bare = bare.contains(&(node.name.as_str(), table.name.as_str()));
    let qualified_name = format!("{}.{}", node.name, table.name);
    let name = if is_bare { table.name.clone() } else { qualified_name.clone() };
    let quoted = if is_bare { sql_name(&table.name).into_owned() } else { qualified(&node.name, &table.name) };
    Named { quoted: (quoted != name).then_some(quoted), name, qualified: qualified_name, table }
}

/// The tables a statement names bare: in a schema of the search path, the first one there to hold that name.
fn bare_tables(catalog: &Catalog) -> HashSet<(&str, &str)> {
    let mut claimed = HashSet::new();
    let in_path = catalog.search_path.iter().filter_map(|schema| catalog.schema.schemas.iter().find(|node| &node.name == schema));
    let tables = in_path.flat_map(|node| node.tables.iter().map(move |table| (node.name.as_str(), table.name.as_str())));
    tables.filter(|(_, table)| claimed.insert(*table)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::shop_schema;

    fn shop(search_path: &[&str]) -> Index {
        let search_path = search_path.iter().map(|schema| (*schema).to_owned()).collect();
        Index::new(&Catalog { schema: shop_schema(), search_path, is_cut: false })
    }

    fn all(names: &Names) -> Vec<String> {
        names.ranked("")
    }

    #[test]
    fn tables_in_the_search_path_are_bare_and_the_others_carry_their_schema() {
        assert_eq!(all(&shop(&["public"]).tables), ["billing.invoices", "orders", "customers"]);
        assert_eq!(all(&shop(&["billing"]).tables), ["invoices", "public.orders", "public.customers"]);
    }

    #[test]
    fn a_name_two_schemas_of_the_path_hold_is_bare_in_the_first_only() {
        let mut schema = shop_schema();
        schema.schemas[0].tables[0].name = "orders".to_owned();
        let index = Index::new(&Catalog { schema, search_path: vec!["billing".to_owned(), "public".to_owned()], is_cut: false });
        assert_eq!(all(&index.tables), ["orders", "public.orders", "customers"]);
    }

    #[test]
    fn every_column_comes_once_whatever_the_tables_holding_it() {
        assert_eq!(all(&shop(&["public"]).columns), ["id", "order_id", "customer_id", "total", "name"]);
    }

    #[test]
    fn a_table_is_found_bare_or_with_its_schema() {
        let index = shop(&["public"]);
        assert_eq!(index.columns_of("orders").map(all), Some(vec!["id".to_owned(), "customer_id".to_owned(), "total".to_owned()]));
        assert_eq!(index.columns_of("public.orders").map(all), Some(vec!["id".to_owned(), "customer_id".to_owned(), "total".to_owned()]));
        assert!(index.columns_of("invoices").is_none());
    }

    #[test]
    fn a_name_with_an_upper_case_letter_a_sign_or_a_keyword_goes_in_quoted() {
        let mut schema = shop_schema();
        schema.schemas[0].name = "Billing".to_owned();
        schema.schemas[1].tables[1].name = "Order Items".to_owned();
        schema.schemas[1].tables[0].columns[2].name = "order".to_owned();
        let index = Index::new(&Catalog { schema, search_path: vec!["public".to_owned()], is_cut: false });
        assert_eq!(index.insert("Order Items"), "\"Order Items\"");
        assert_eq!(index.insert("order"), "\"order\"");
        assert_eq!(index.insert("Billing.invoices"), "\"Billing\".invoices");
        assert_eq!(index.insert("orders"), "orders");
    }

    #[test]
    fn a_table_goes_in_as_spelled_beside_one_that_differs_only_in_case() {
        let mut schema = shop_schema();
        schema.schemas[1].tables[1].name = "Orders".to_owned();
        let index = Index::new(&Catalog { schema, search_path: vec!["public".to_owned()], is_cut: false });
        assert_eq!(index.insert("orders"), "orders");
        assert_eq!(index.insert("Orders"), "\"Orders\"");
    }

    #[test]
    fn a_column_goes_in_as_spelled_beside_one_that_differs_only_in_case() {
        let mut schema = shop_schema();
        schema.schemas[1].tables[1].columns[1].name = "ID".to_owned();
        let index = Index::new(&Catalog { schema, search_path: vec!["public".to_owned()], is_cut: false });
        assert_eq!(index.insert("id"), "id");
        assert_eq!(index.insert("ID"), "\"ID\"");
    }

    #[test]
    fn a_table_is_found_by_its_exact_name_beside_one_that_differs_only_in_case() {
        let mut schema = shop_schema();
        schema.schemas[1].tables[1].name = "Orders".to_owned();
        let index = Index::new(&Catalog { schema, search_path: vec!["public".to_owned()], is_cut: false });
        assert_eq!(index.columns_of("orders").map(all), Some(vec!["id".to_owned(), "customer_id".to_owned(), "total".to_owned()]));
        assert_eq!(index.columns_of("Orders").map(all), Some(vec!["id".to_owned(), "name".to_owned()]));
        assert_eq!(index.columns_of("public.Orders").map(all), Some(vec!["id".to_owned(), "name".to_owned()]));
    }

    #[test]
    fn two_columns_that_differ_only_in_case_both_come() {
        let mut schema = shop_schema();
        schema.schemas[1].tables[1].columns[1].name = "ID".to_owned();
        let index = Index::new(&Catalog { schema, search_path: vec!["public".to_owned()], is_cut: false });
        assert_eq!(all(&index.columns), ["id", "order_id", "customer_id", "total", "ID"]);
    }
}
