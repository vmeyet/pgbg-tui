//! The joins completion offers after `JOIN`: a table that a foreign key links to a table the statement names, either
//! way, with an alias of its own and the `ON` condition from the key's columns.
use super::context::{Ask, TableRef, Want, without_schema};
use super::index::{Index, Named, qualified_name};
use super::keyword::is_keyword;
use super::rank::{Candidate, Kind};
use crate::db::model::{ForeignKey, needs_quotes, sql_name};
use std::collections::HashMap;

/// A table that a foreign key links to another one, as seen from that other one: the `ON` condition makes each column
/// of `named` equal to the column at the same place in `linked`.
#[derive(Debug)]
pub(super) struct Join {
    /// The linked table's completion name.
    table: String,
    /// Columns of the table already named.
    named: Vec<String>,
    /// Columns of the linked table.
    linked: Vec<String>,
}

/// A foreign key of the table at `from` to the table at `to`.
struct Link<'a> {
    from: usize,
    to: usize,
    key: &'a ForeignKey,
}

/// The joins of each table, by its place in `tables`: first to the tables its keys point to, then from those whose keys
/// point to it; a key to a table the read does not hold is left out.
pub(super) fn joins_by_table(tables: &[Named<'_>], by_name: &HashMap<String, usize>) -> Vec<Vec<Join>> {
    let links: Vec<Link<'_>> = tables.iter().enumerate().flat_map(|(from, table)| links_of(from, table, by_name)).collect();
    let mut joins: Vec<Vec<Join>> = tables.iter().map(|_| vec![]).collect();
    for Link { from, to, key } in &links {
        joins[*from].push(Join { table: tables[*to].name.clone(), named: key.columns.clone(), linked: key.target_columns.clone() });
    }
    for Link { from, to, key } in &links {
        joins[*to].push(Join { table: tables[*from].name.clone(), named: key.target_columns.clone(), linked: key.columns.clone() });
    }
    joins
}

fn links_of<'a>(from: usize, table: &Named<'a>, by_name: &HashMap<String, usize>) -> impl Iterator<Item = Link<'a>> {
    let to = |key: &ForeignKey| by_name.get(&qualified_name(&key.target_schema, &key.target_table)).copied();
    table.table.foreign_keys.iter().filter_map(move |key| Some(Link { from, to: to(key)?, key }))
}

impl Index {
    /// The joins for `ask` after `JOIN` whose table starts with what was typed, whatever its case: for each table the
    /// statement names before it, in order, each table a foreign key links to it, as `customers c ON c.id = o.customer_id`.
    /// Built one at a time, so the ghost formats only the first.
    pub(super) fn joins<'a>(&'a self, ask: &'a Ask) -> impl Iterator<Item = Candidate> + 'a {
        let tables = match &ask.want {
            Want::Join { tables } => tables.as_slice(),
            _ => &[],
        };
        let taken: Vec<&str> = tables.iter().map(TableRef::qualifier).collect();
        let offered = tables.iter().flat_map(|named| self.joins_of(&named.name).iter().map(move |join| (named, join)));
        let typed = offered.filter(|(_, join)| starts_with(&join.table, &ask.prefix));
        typed.map(move |(named, join)| self.join_candidate(named, join, &taken))
    }

    fn join_candidate(&self, named: &TableRef, join: &Join, taken: &[&str]) -> Candidate {
        let alias = alias_of(&join.table, taken);
        let qualifier = sql_name(named.qualifier());
        let equal = |(named, linked): (&String, &String)| format!("{alias}.{} = {qualifier}.{}", sql_name(linked), sql_name(named));
        let condition = join.named.iter().zip(&join.linked).map(equal).collect::<Vec<_>>().join(" AND ");
        let rest = format!(" {alias} ON {condition}");
        Candidate { name: format!("{}{rest}", join.table), insert: format!("{}{rest}", self.insert(&join.table)), kind: Kind::Join }
    }
}

/// The initials of the words of `table`'s name, `oi` for `order_items`, as people write an alias; with a number after
/// them when the statement already calls a table so, or when they would read as a keyword.
pub(crate) fn alias_of(table: &str, taken: &[&str]) -> String {
    let words = without_schema(table).split(|c: char| !c.is_ascii_alphanumeric()).filter_map(|word| word.chars().next());
    let initials: String = words.filter(char::is_ascii_alphabetic).map(|c| c.to_ascii_lowercase()).collect();
    let initials = if initials.is_empty() { "t".to_owned() } else { initials };
    let is_free = |alias: &String| !taken.contains(&alias.as_str()) && !is_keyword(alias) && !needs_quotes(alias);
    let numbered = (2..).map(|n| format!("{initials}{n}"));
    std::iter::once(initials.clone()).chain(numbered).find(is_free).unwrap_or(initials)
}

fn starts_with(name: &str, typed: &str) -> bool {
    name.get(..typed.len()).is_some_and(|head| head.eq_ignore_ascii_case(typed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::lex::Lexed;
    use crate::db::model::{Catalog, Schema};
    use crate::fixtures::{shop_catalog, shop_schema};

    fn joins_on(schema: Schema, marked: &str) -> Vec<String> {
        let index = Index::new(&Catalog { schema, search_path: vec!["public".to_owned()], is_cut: false });
        let cursor = marked.find('|').unwrap();
        let text = marked.replace('|', "");
        let ask = Ask::at(&text, &Lexed::of(&text), cursor).unwrap();
        index.joins(&ask).map(|candidate| candidate.insert).collect()
    }

    fn joins(marked: &str) -> Vec<String> {
        joins_on(shop_catalog().schema, marked)
    }

    #[test]
    fn a_join_goes_to_the_tables_a_key_points_to_then_to_those_pointing_back() {
        let offered = joins("SELECT * FROM orders o JOIN |");
        assert_eq!(offered, ["customers c ON c.id = o.customer_id", "billing.invoices i ON i.order_id = o.id"]);
    }

    #[test]
    fn a_table_without_an_alias_is_called_by_its_name() {
        assert_eq!(joins("SELECT * FROM public.orders JOIN cu|"), ["customers c ON c.id = orders.customer_id"]);
    }

    #[test]
    fn only_the_joins_whose_table_starts_with_what_was_typed_come() {
        assert_eq!(joins("SELECT * FROM orders o JOIN BILL|"), ["billing.invoices i ON i.order_id = o.id"]);
    }

    #[test]
    fn every_table_named_before_the_join_offers_its_own() {
        let offered = joins("SELECT * FROM billing.invoices i JOIN orders o ON o.id = i.order_id JOIN cu|");
        assert_eq!(offered, ["customers c ON c.id = o.customer_id"]);
    }

    #[test]
    fn no_join_comes_after_from() {
        assert_eq!(joins("SELECT * FROM |"), Vec::<String>::new());
    }

    #[test]
    fn an_alias_the_statement_already_uses_gets_a_number() {
        assert_eq!(joins("SELECT * FROM orders c JOIN cu|"), ["customers c2 ON c2.id = c.customer_id"]);
    }

    #[test]
    fn an_alias_is_the_initials_of_the_words_of_the_name() {
        assert_eq!(alias_of("public.order_items", &[]), "oi");
        assert_eq!(alias_of("Late Invoices", &[]), "li");
    }

    #[test]
    fn an_alias_that_reads_as_a_keyword_gets_a_number() {
        assert_eq!(alias_of("order_notes", &[]), "on2");
    }

    #[test]
    fn a_key_of_several_columns_joins_on_each_pair() {
        let mut schema = shop_schema();
        let key = &mut schema.schemas[1].tables[0].foreign_keys[0];
        key.columns.push("customer_region".to_owned());
        key.target_columns.push("region".to_owned());
        let offered = joins_on(schema, "SELECT * FROM orders o JOIN cu|");
        assert_eq!(offered, ["customers c ON c.id = o.customer_id AND c.region = o.customer_region"]);
    }

    #[test]
    fn a_name_that_needs_quotes_goes_in_quoted() {
        let mut schema = shop_schema();
        schema.schemas[1].tables[1].name = "Customers".to_owned();
        schema.schemas[1].tables[0].foreign_keys[0].target_table = "Customers".to_owned();
        assert_eq!(joins_on(schema, "SELECT * FROM orders \"O\" JOIN cu|"), ["\"Customers\" c ON c.id = \"O\".customer_id"]);
    }

    #[test]
    fn a_key_to_a_table_the_read_does_not_hold_offers_no_join() {
        let mut schema = shop_schema();
        schema.schemas[1].tables[0].foreign_keys[0].target_table = "carts".to_owned();
        assert_eq!(joins_on(schema, "SELECT * FROM orders o JOIN cu|"), Vec::<String>::new());
    }
}
