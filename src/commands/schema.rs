use crate::connection::ConnectionName;
use crate::db::model::{Index, Schema, SchemaColumn, SchemaNode, Table, TableKind};
use crate::render::{self, printable};
use anyhow::Result;

const INDENT: &str = "  ";
const NO_TABLE: &str = "(no table the role can read)\n";
const CUT: &str = "more than 20 000 tables: only the first 20 000 by name are shown";

/// Prints every table the role can read on the connection named `connection`, with its columns and indexes; only
/// those whose name holds `filter` when there is one. JSON with `json`.
pub async fn run(connection: &ConnectionName, filter: Option<&str>, json: bool) -> Result<()> {
    let connection = super::connection(Some(connection))?;
    let catalog = super::on_session(&connection, async |session| session.read_schema_matching(filter.unwrap_or_default()).await).await?;
    if catalog.is_cut {
        super::warn(CUT);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&catalog.schema)?);
    } else {
        print!("{}", text(&catalog.schema));
    }
    Ok(())
}

fn text(schema: &Schema) -> String {
    let tables: Vec<String> = schema.schemas.iter().flat_map(|node| node.tables.iter().map(move |table| table_text(node, table))).collect();
    if tables.is_empty() { NO_TABLE.to_owned() } else { tables.join("\n") }
}

fn table_text(node: &SchemaNode, table: &Table) -> String {
    let header =
        format!("{}.{}  {}{}\n", printable(&node.name), printable(&table.name), kind_name(table.kind), rows_text(table.estimated_rows));
    let columns = render::table(&table.columns.iter().map(column_cells).collect::<Vec<_>>());
    let columns: String = columns.lines().map(|line| format!("{INDENT}{line}\n")).collect();
    let indexes: String = table.indexes.iter().map(index_text).collect();
    header + &columns + &indexes
}

const fn kind_name(kind: TableKind) -> &'static str {
    match kind {
        TableKind::Table => "table",
        TableKind::View => "view",
        TableKind::MaterializedView => "materialized view",
        TableKind::Foreign => "foreign table",
        TableKind::Partitioned => "partitioned table",
    }
}

fn rows_text(estimated_rows: Option<u64>) -> String {
    estimated_rows.map_or_else(String::new, |rows| format!("  ~{rows} rows"))
}

fn column_cells(column: &SchemaColumn) -> Vec<String> {
    let nullable = if column.is_nullable { "" } else { "not null" };
    let default = column.default.as_ref().map_or_else(String::new, |default| format!("default {default}"));
    vec![column.name.clone(), column.type_name.as_str().to_owned(), nullable.to_owned(), default]
}

fn index_text(index: &Index) -> String {
    let role = if index.is_primary {
        " primary"
    } else if index.is_unique {
        " unique"
    } else {
        ""
    };
    format!("{INDENT}index {} ({}){role}\n", printable(&index.name), printable(&index.columns.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orders() -> Schema {
        let public = crate::fixtures::shop_schema().schemas.into_iter().find(|node| node.name == "public").unwrap();
        Schema { schemas: vec![SchemaNode { tables: public.tables.into_iter().take(1).collect(), ..public }] }
    }

    #[test]
    fn a_table_prints_its_kind_rows_columns_and_indexes() {
        assert_eq!(
            text(&orders()),
            "public.orders  table  ~1200 rows\n  id           integer        not null\n  customer_id  integer        not null\n  total        numeric(10,2)            default 0\n  index orders_pkey (id) primary\n"
        );
    }

    #[test]
    fn a_schema_without_tables_says_so() {
        assert_eq!(text(&Schema::default()), NO_TABLE);
    }
}
