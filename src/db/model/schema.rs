use serde::Serialize;

const DEFAULT_SCHEMA: &str = "public";

/// Every schema the role can see, with its tables, their columns and their indexes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Schema {
    pub(crate) schemas: Vec<SchemaNode>,
}

/// The schema as one read gave it: the schemas the session's search path names, whose tables a statement names bare,
/// and whether the read kept only the first tables by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub(crate) schema: Schema,
    /// `current_schemas(false)`: the schemas of the search path that exist, in its order.
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

/// What the editor completes at the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompletionContext<'a> {
    /// A table, after `FROM` or `JOIN`.
    Table,
    /// A column of `tables`, named as the statement names them; of every table when it names none.
    Column { tables: &'a [&'a str] },
}

/// A table with the name a statement uses for it: bare in `public`, else `schema.table`.
struct Named<'a> {
    name: String,
    table: &'a Table,
}

impl Schema {
    /// Only the tables whose name, or `schema.table`, holds `filter`, whatever its case.
    pub(crate) fn matching(self, filter: &str) -> Self {
        let filter = filter.to_lowercase();
        let schemas = self.schemas.into_iter().map(|schema| schema.matching(&filter)).filter(|schema| !schema.tables.is_empty());
        Self { schemas: schemas.collect() }
    }

    /// The names that start with `prefix`, whatever its case, sorted and each once.
    pub(crate) fn complete(&self, prefix: &str, context: CompletionContext<'_>) -> Vec<String> {
        let candidates = match context {
            CompletionContext::Table => self.named().map(|named| named.name).collect(),
            CompletionContext::Column { tables } => self.columns_of(tables),
        };
        starting_with(candidates, prefix)
    }

    fn named(&self) -> impl Iterator<Item = Named<'_>> {
        self.schemas.iter().flat_map(|schema| schema.tables.iter().map(|table| Named { name: schema.name_of(table), table }))
    }

    fn columns_of(&self, tables: &[&str]) -> Vec<String> {
        let is_named = |named: &Named<'_>| tables.is_empty() || tables.iter().any(|table| table.eq_ignore_ascii_case(&named.name));
        self.named().filter(is_named).flat_map(|named| named.table.columns.iter().map(|column| column.name.clone())).collect()
    }
}

impl SchemaNode {
    fn matching(self, filter: &str) -> Self {
        let tables = self.tables.iter().filter(|table| self.holds(table, filter)).cloned().collect();
        Self { name: self.name, tables }
    }

    /// Whether `schema.table` holds `filter`, which is lowercase, whatever the name's case.
    pub(crate) fn holds(&self, table: &Table, filter: &str) -> bool {
        self.qualified(table).to_lowercase().contains(filter)
    }

    fn qualified(&self, table: &Table) -> String {
        format!("{}.{}", self.name, table.name)
    }

    fn name_of(&self, table: &Table) -> String {
        if self.name == DEFAULT_SCHEMA { table.name.clone() } else { self.qualified(table) }
    }
}

fn starting_with(mut candidates: Vec<String>, prefix: &str) -> Vec<String> {
    let prefix = prefix.to_lowercase();
    candidates.retain(|candidate| candidate.to_lowercase().starts_with(&prefix));
    candidates.sort();
    candidates.dedup();
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::shop_schema as shop;

    fn tables_of(schema: &Schema) -> Vec<String> {
        schema.schemas.iter().flat_map(|node| node.tables.iter().map(|table| node.qualified(table))).collect()
    }

    #[test]
    fn a_filter_keeps_the_tables_whose_name_holds_it_whatever_its_case() {
        assert_eq!(tables_of(&shop().matching("ORD")), ["public.orders"]);
    }

    #[test]
    fn a_filter_matches_the_schema_and_table_together() {
        assert_eq!(tables_of(&shop().matching("billing.inv")), ["billing.invoices"]);
    }

    #[test]
    fn a_filter_drops_the_schemas_it_empties() {
        assert_eq!(shop().matching("customers").schemas.len(), 1);
    }

    #[test]
    fn tables_complete_bare_in_public_and_with_their_schema_elsewhere() {
        assert_eq!(shop().complete("", CompletionContext::Table), ["billing.invoices", "customers", "orders"]);
    }

    #[test]
    fn a_prefix_narrows_the_tables_whatever_its_case() {
        assert_eq!(shop().complete("Cu", CompletionContext::Table), ["customers"]);
        assert_eq!(shop().complete("billing.", CompletionContext::Table), ["billing.invoices"]);
    }

    #[test]
    fn columns_complete_from_the_tables_the_statement_names() {
        let tables = ["orders"];
        assert_eq!(shop().complete("", CompletionContext::Column { tables: &tables }), ["customer_id", "id", "total"]);
    }

    #[test]
    fn columns_of_every_table_come_once_each_when_the_statement_names_none() {
        assert_eq!(shop().complete("i", CompletionContext::Column { tables: &[] }), ["id"]);
    }

    #[test]
    fn a_table_in_another_schema_is_named_with_it_for_its_columns() {
        let tables = ["BILLING.invoices"];
        assert_eq!(shop().complete("", CompletionContext::Column { tables: &tables }), ["id", "order_id"]);
    }

    #[test]
    fn a_materialized_view_says_matview_in_json() {
        assert_eq!(serde_json::to_value(TableKind::MaterializedView).unwrap(), "matview");
    }
}
