use serde::Serialize;

/// Every schema the role can see, with its tables, their columns and their indexes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Schema {
    pub(crate) schemas: Vec<SchemaNode>,
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
}

/// A column of a table, as the catalog declares it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SchemaColumn {
    pub(crate) name: String,
    /// The type as PostgreSQL writes it: `integer`, `character varying(80)`, `timestamp with time zone`.
    #[serde(rename = "type")]
    pub(crate) type_name: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_materialized_view_says_matview_in_json() {
        assert_eq!(serde_json::to_value(TableKind::MaterializedView).unwrap(), "matview");
    }
}
