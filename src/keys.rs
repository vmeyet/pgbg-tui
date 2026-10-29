//! The keys of a result's columns: which are the primary key, and where a foreign key points, read from the catalog.
//! Pure, and only sure when the statement reads one table: a result column is that table's column of the same name.
use crate::complete;
use crate::db::model::{Catalog, Column, ForeignKey, Statement, Table};

/// The row a foreign key points to: its table, and the column there that each column of the key equals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) schema: String,
    pub(crate) table: String,
    /// One for each column of the key, most keys having one.
    pub(crate) pairs: Vec<KeyPair>,
}

/// A column of a foreign key: its place in the result, and the column of the target it equals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyPair {
    pub(crate) at: usize,
    pub(crate) column: String,
}

/// What a result's column is in the table its statement reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ColumnKey {
    pub(crate) is_primary: bool,
    /// Where it points, when it is a column of a foreign key whose every column the result holds once.
    pub(crate) points_to: Option<Target>,
}

/// The key of each of `columns`, the result of `statement` read with `catalog`; none when the statement reads more than
/// one table, or one the catalog does not hold.
pub(crate) fn column_keys(catalog: &Catalog, statement: &Statement, columns: &[Column]) -> Vec<ColumnKey> {
    let Some(table) = one_table_read(catalog, statement) else { return vec![ColumnKey::default(); columns.len()] };
    columns.iter().map(|column| key_of(table, column.name.as_str(), columns)).collect()
}

fn key_of(table: &Table, name: &str, columns: &[Column]) -> ColumnKey {
    let is_primary = table.indexes.iter().any(|index| index.is_primary && index.columns.iter().any(|key| key == name));
    let keys = table.foreign_keys.iter().filter(|key| key.columns.iter().any(|column| column == name));
    ColumnKey { is_primary, points_to: keys.into_iter().find_map(|key| target_of(key, columns)) }
}

/// Where `key` points from `columns`, when they hold each of its columns once.
fn target_of(key: &ForeignKey, columns: &[Column]) -> Option<Target> {
    let pair = |(from, to): (&String, &String)| Some(KeyPair { at: only_column(columns, from)?, column: to.clone() });
    let pairs: Option<Vec<KeyPair>> = key.columns.iter().zip(&key.target_columns).map(pair).collect();
    Some(Target { schema: key.target_schema.clone(), table: key.target_table.clone(), pairs: pairs? })
}

/// The place of the one column named `name`; none when `columns` hold no such column, or several.
fn only_column(columns: &[Column], name: &str) -> Option<usize> {
    let mut named = columns.iter().enumerate().filter(|(_, column)| column.name.as_str() == name).map(|(at, _)| at);
    let first = named.next()?;
    named.next().is_none().then_some(first)
}

/// The table `statement` reads, when it names exactly one, found as PostgreSQL resolves its name: with its schema, or
/// bare in the first schema of the search path that holds it.
fn one_table_read<'a>(catalog: &'a Catalog, statement: &Statement) -> Option<&'a Table> {
    let [name] = complete::tables_named(statement.as_str()).try_into().ok()?;
    match name.split_once('.') {
        Some((schema, table)) => table_in(catalog, schema, table),
        None => catalog.search_path.iter().find_map(|schema| table_in(catalog, schema, &name)),
    }
}

fn table_in<'a>(catalog: &'a Catalog, schema: &str, name: &str) -> Option<&'a Table> {
    let schema = catalog.schema.schemas.iter().find(|node| node.name == schema)?;
    schema.tables.iter().find(|table| table.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::ColumnKind;
    use crate::fixtures::{column, shop_catalog};

    fn keys(statement: &str, names: &[&str]) -> Vec<ColumnKey> {
        let columns: Vec<Column> = names.iter().map(|name| column(name, ColumnKind::Int)).collect();
        column_keys(&shop_catalog(), &Statement::new(statement), &columns)
    }

    /// A key of the one column at `at` pointing to `id` of `schema.table`.
    fn to_id(schema: &str, table: &str, at: usize) -> Target {
        Target { schema: schema.to_owned(), table: table.to_owned(), pairs: vec![KeyPair { at, column: "id".to_owned() }] }
    }

    #[test]
    fn a_column_of_the_primary_key_of_the_one_table_read_is_primary() {
        assert!(keys("SELECT * FROM orders", &["id"])[0].is_primary);
    }

    #[test]
    fn a_column_with_a_foreign_key_points_to_its_target() {
        let keys = keys("SELECT id, customer_id FROM orders", &["id", "customer_id"]);
        assert_eq!(keys[1].points_to, Some(to_id("public", "customers", 1)));
    }

    #[test]
    fn a_table_named_with_its_schema_is_found_in_that_schema() {
        assert_eq!(keys("SELECT * FROM billing.invoices", &["order_id"])[0].points_to, Some(to_id("public", "orders", 0)));
    }

    #[test]
    fn a_key_column_the_result_holds_twice_points_nowhere() {
        assert_eq!(keys("SELECT customer_id, customer_id FROM orders", &["customer_id", "customer_id"])[0].points_to, None);
    }

    #[test]
    fn each_column_of_a_key_of_several_points_to_the_whole_key() {
        let mut catalog = shop_catalog();
        let key = &mut catalog.schema.schemas[1].tables[0].foreign_keys[0];
        key.columns.push("total".to_owned());
        key.target_columns.push("region".to_owned());
        let columns = ["id", "customer_id", "total"].map(|name| column(name, ColumnKind::Int));
        let keys = column_keys(&catalog, &Statement::new("SELECT * FROM orders"), &columns);
        let region = KeyPair { at: 2, column: "region".to_owned() };
        let both = Target { pairs: vec![KeyPair { at: 1, column: "id".to_owned() }, region], ..to_id("public", "customers", 1) };
        assert_eq!([&keys[1].points_to, &keys[2].points_to], [&Some(both.clone()), &Some(both)]);
    }

    #[test]
    fn a_bare_table_outside_the_search_path_is_not_found() {
        assert_eq!(keys("SELECT * FROM invoices", &["order_id"]), [ColumnKey::default()]);
    }

    #[test]
    fn a_statement_reading_two_tables_has_no_keys() {
        assert_eq!(keys("SELECT * FROM orders JOIN customers ON true", &["id"]), [ColumnKey::default()]);
    }

    #[test]
    fn a_column_the_table_does_not_hold_has_no_key() {
        assert_eq!(keys("SELECT count(*) AS total FROM orders", &["total"]), [ColumnKey::default()]);
    }
}
