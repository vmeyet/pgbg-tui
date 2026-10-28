use super::TypeName;
use serde::Serialize;

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
    pub(crate) name: String,
    pub(crate) kind: TableKind,
    /// The planner's guess from the last `ANALYZE`; none for a view or a table never analyzed.
    pub(crate) estimated_rows: Option<u64>,
    pub(crate) columns: Vec<SchemaColumn>,
    pub(crate) indexes: Vec<Index>,
    pub(crate) foreign_keys: Vec<ForeignKey>,
}

/// A column of a table, as the catalog declares it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SchemaColumn {
    pub(crate) name: String,
    /// The type as PostgreSQL writes it: `integer`, `character varying(80)`, `timestamp with time zone`.
    #[serde(rename = "type")]
    pub(crate) type_name: TypeName,
    pub(crate) is_nullable: bool,
    pub(crate) default: Option<String>,
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
    fn a_table_holds_a_filter_on_its_schema_and_name_together() {
        let billing = &shop().schemas[0];
        assert!(billing.holds(&billing.tables[0], "billing.inv") && !billing.holds(&billing.tables[0], "public"));
    }

    #[test]
    fn a_materialized_view_says_matview_in_json() {
        assert_eq!(serde_json::to_value(TableKind::MaterializedView).unwrap(), "matview");
    }
}
