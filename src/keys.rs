//! The keys of a result's columns: which are the primary key, and where a foreign key points, read from the catalog.
//! Pure, and only sure when the statement reads one table: a result column is that table's column of the same name.
use crate::complete;
use crate::db::model::{Catalog, Column, ColumnName, Statement, Table};

/// The column a foreign key points to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) schema: String,
    pub(crate) table: String,
    pub(crate) column: String,
}

/// What a result's column is in the table its statement reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ColumnKey {
    pub(crate) is_primary: bool,
    /// Where it points, when a foreign key of that column alone points elsewhere.
    pub(crate) points_to: Option<Target>,
}

/// The key of each of `columns`, the result of `statement` read with `catalog`; none when the statement reads more than
/// one table, or one the catalog does not hold.
pub(crate) fn column_keys(catalog: &Catalog, statement: &Statement, columns: &[Column]) -> Vec<ColumnKey> {
    let Some(table) = one_table_read(catalog, statement) else { return vec![ColumnKey::default(); columns.len()] };
    columns.iter().map(|column| key_of(table, &column.name)).collect()
}

fn key_of(table: &Table, column: &ColumnName) -> ColumnKey {
    let name = column.as_str();
    let is_primary = table.indexes.iter().any(|index| index.is_primary && index.columns.iter().any(|key| key == name));
    let points_to = table.foreign_keys.iter().find_map(|key| match (key.columns.as_slice(), key.target_columns.as_slice()) {
        ([from], [to]) if from == name => {
            Some(Target { schema: key.target_schema.clone(), table: key.target_table.clone(), column: to.clone() })
        }
        _ => None,
    });
    ColumnKey { is_primary, points_to }
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

    fn customers() -> Target {
        Target { schema: "public".to_owned(), table: "customers".to_owned(), column: "id".to_owned() }
    }

    #[test]
    fn a_column_of_the_primary_key_of_the_one_table_read_is_primary() {
        assert!(keys("SELECT * FROM orders", &["id"])[0].is_primary);
    }

    #[test]
    fn a_column_with_a_foreign_key_points_to_its_target() {
        assert_eq!(keys("SELECT id, customer_id FROM orders", &["id", "customer_id"])[1].points_to, Some(customers()));
    }

    #[test]
    fn a_table_named_with_its_schema_is_found_in_that_schema() {
        let target = Target { schema: "public".to_owned(), table: "orders".to_owned(), column: "id".to_owned() };
        assert_eq!(keys("SELECT * FROM billing.invoices", &["order_id"])[0].points_to, Some(target));
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
