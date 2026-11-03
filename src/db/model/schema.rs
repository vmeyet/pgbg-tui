use super::{ColumnNumber, TableOid, TypeName};
use serde::Serialize;
use std::collections::HashSet;

/// Every schema the role can see, with its tables, their columns, their indexes and their foreign keys.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Schema {
    pub(crate) schemas: Vec<SchemaNode>,
}

/// The schema as one read gave it, the search path of the session that read it, whose tables a statement may name
/// without their schema, and whether the read kept only the first tables by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub(crate) schema: Schema,
    /// `current_schemas(false)` on the session that read it: the schemas of its search path that exist, in its order.
    /// It logs in as the statement session does, so the two share a search path unless a statement changed it.
    pub(crate) search_path: Vec<String>,
    pub(crate) is_cut: bool,
}

impl Catalog {
    /// Those of `names` that a schema, a table, a column or an index bears exactly, as a quoted name must; one walk of the
    /// catalog however many names, since a statement may quote hundreds.
    pub(crate) fn held_among(&self, names: &HashSet<String>) -> HashSet<String> {
        if names.is_empty() {
            return HashSet::new();
        }
        let tables = || self.schema.schemas.iter().flat_map(|node| &node.tables);
        let schema_names = self.schema.schemas.iter().map(|node| &node.name);
        let table_names = tables().map(|table| &table.name);
        let column_names = tables().flat_map(|table| &table.columns).map(|column| &column.name);
        let index_names = tables().flat_map(|table| &table.indexes).map(|index| &index.name);
        schema_names.chain(table_names).chain(column_names).chain(index_names).filter(|name| names.contains(*name)).cloned().collect()
    }
}

/// One schema and the tables in it the role can read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SchemaNode {
    pub(crate) name: String,
    pub(crate) tables: Vec<Table>,
}

/// What kind of relation a table is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TableKind {
    Table,
    View,
    #[serde(rename = "matview")]
    MaterializedView,
    Foreign,
    Partitioned,
}

/// A table, a view or any other relation a statement can read from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Table {
    #[serde(skip)]
    pub(crate) oid: TableOid,
    pub(crate) name: String,
    pub(crate) kind: TableKind,
    /// The planner's guess from the last `ANALYZE`; none for a view or a table never analyzed.
    pub(crate) estimated_rows: Option<u64>,
    pub(crate) columns: Vec<SchemaColumn>,
    pub(crate) indexes: Vec<Index>,
    pub(crate) foreign_keys: Vec<ForeignKey>,
    /// What `COMMENT ON TABLE` says of it, when anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) comment: Option<String>,
}

/// A column of a table, as the catalog declares it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SchemaColumn {
    #[serde(skip)]
    pub(crate) number: ColumnNumber,
    pub(crate) name: String,
    /// The type as PostgreSQL writes it: `integer`, `character varying(80)`, `timestamp with time zone`.
    #[serde(rename = "type")]
    pub(crate) type_name: TypeName,
    pub(crate) is_nullable: bool,
    pub(crate) default: Option<String>,
    /// The labels of its enum type in their sort order, for a column of an enum, an array of one or a domain over one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) enum_labels: Vec<String>,
    /// What `COMMENT ON COLUMN` says of it, when anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) comment: Option<String>,
}

/// An index of a table: its key columns, or the expression a key is built from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Index {
    pub(crate) name: String,
    pub(crate) columns: Vec<String>,
    pub(crate) is_unique: bool,
    pub(crate) is_primary: bool,
}

/// A foreign key of a table: its columns, each pointing to the column at the same place in `target_columns` of the
/// table `target_schema.target_table`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ForeignKey {
    pub(crate) columns: Vec<String>,
    pub(crate) target_schema: String,
    pub(crate) target_table: String,
    pub(crate) target_columns: Vec<String>,
}

impl Table {
    /// The number of its column `name`.
    pub(crate) fn column_number(&self, name: &str) -> Option<ColumnNumber> {
        self.columns.iter().find(|column| column.name == name).map(|column| column.number)
    }
}

impl SchemaNode {
    /// Whether `schema.table` holds `filter`, which is lowercase, whatever the name's case.
    pub(crate) fn holds(&self, table: &Table, filter: &str) -> bool {
        self.qualified(table).to_lowercase().contains(filter)
    }

    fn qualified(&self, table: &Table) -> String {
        format!("{}.{}", self.name, table.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::shop_schema as shop;

    #[test]
    fn the_catalog_holds_each_kind_of_name_exactly_as_written() {
        let names: HashSet<String> = ["public", "orders", "total", "orders_pkey", "Total", "nina"].map(str::to_owned).into();
        let expected: HashSet<String> = ["public", "orders", "total", "orders_pkey"].map(str::to_owned).into();
        assert_eq!(crate::fixtures::shop_catalog().held_among(&names), expected);
    }

    #[test]
    fn a_table_holds_a_filter_on_its_schema_and_name_together() {
        let billing = &shop().schemas[0];
        assert!(billing.holds(&billing.tables[0], "billing.inv") && !billing.holds(&billing.tables[0], "public"));
    }

    #[test]
    fn a_materialized_view_says_matview_in_json() {
        assert_eq!(serde_json::to_value(TableKind::MaterializedView).unwrap(), "matview");
    }
}
