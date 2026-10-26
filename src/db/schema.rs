//! The schema the role can see, read from the catalogs in one read-only transaction.
use super::model::{Index, Schema, SchemaColumn, SchemaNode, Table, TableKind};
use super::{Session, server_error};
use anyhow::{Result, bail};
use std::collections::HashMap;
use tokio_postgres::{Row, Transaction};

/// The relations a statement reads from, outside the catalogs, that the role may read at least one column of; a partition
/// is left to its parent, which reads them all.
const VISIBLE: &str = "WITH visible AS (
    SELECT c.oid, n.nspname, c.relname, c.relkind, c.reltuples
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE c.relkind IN ('r', 'v', 'm', 'f', 'p') AND NOT c.relispartition
      AND n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema'
      AND has_schema_privilege(n.oid, 'USAGE') AND has_any_column_privilege(c.oid, 'SELECT')
)";
const TABLES: &str = "SELECT oid, nspname, relname, relkind::text, reltuples::float8 FROM visible ORDER BY nspname, relname";
const COLUMNS: &str =
    "SELECT a.attrelid, a.attname, format_type(a.atttypid, a.atttypmod), NOT a.attnotnull, pg_get_expr(d.adbin, d.adrelid)
    FROM pg_attribute a JOIN visible v ON v.oid = a.attrelid
    LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
    WHERE a.attnum > 0 AND NOT a.attisdropped AND has_column_privilege(a.attrelid, a.attnum, 'SELECT')
    ORDER BY a.attrelid, a.attnum";
const INDEXES: &str = "SELECT i.indrelid, c.relname, i.indisunique, i.indisprimary,
        ARRAY(SELECT pg_get_indexdef(i.indexrelid, key, true) FROM generate_series(1, i.indnkeyatts) AS key)
    FROM pg_index i JOIN visible v ON v.oid = i.indrelid JOIN pg_class c ON c.oid = i.indexrelid
    ORDER BY i.indrelid, c.relname";

/// A table as the catalog query gives it, before its columns and indexes join it.
struct Listed {
    oid: u32,
    schema: String,
    table: Table,
}

impl Session {
    /// Every schema, table, column and index the role can read, in one read-only transaction.
    pub(crate) async fn read_schema(&mut self) -> Result<Schema> {
        self.read_only(async |transaction| {
            let tables = rows(transaction, TABLES).await?.iter().map(listed).collect::<Result<Vec<_>>>()?;
            let columns = rows(transaction, COLUMNS).await?.iter().map(column).collect::<Result<Vec<_>>>()?;
            let indexes = rows(transaction, INDEXES).await?.iter().map(index).collect::<Result<Vec<_>>>()?;
            Ok(assembled(tables, columns, indexes))
        })
        .await
    }
}

async fn rows(transaction: &Transaction<'_>, sql: &str) -> Result<Vec<Row>> {
    transaction.query(&format!("{VISIBLE} {sql}"), &[]).await.map_err(server_error)
}

fn listed(row: &Row) -> Result<Listed> {
    let kind = table_kind(&row.try_get::<_, String>(3)?)?;
    let estimated_rows = estimated_rows(kind, row.try_get(4)?);
    let table = Table { name: row.try_get(2)?, kind, estimated_rows, columns: vec![], indexes: vec![] };
    Ok(Listed { oid: row.try_get(0)?, schema: row.try_get(1)?, table })
}

fn table_kind(relkind: &str) -> Result<TableKind> {
    Ok(match relkind {
        "r" => TableKind::Table,
        "v" => TableKind::View,
        "m" => TableKind::MaterializedView,
        "f" => TableKind::Foreign,
        "p" => TableKind::Partitioned,
        other => bail!("the catalog lists a relation of kind `{other}`"),
    })
}

/// None for a view, which keeps no rows, and for a table never analyzed, which PostgreSQL marks `-1`.
fn estimated_rows(kind: TableKind, reltuples: f64) -> Option<u64> {
    let is_counted = kind != TableKind::View && reltuples >= 0.0;
    is_counted.then_some(reltuples as u64)
}

fn column(row: &Row) -> Result<(u32, SchemaColumn)> {
    let column = SchemaColumn { name: row.try_get(1)?, type_name: row.try_get(2)?, is_nullable: row.try_get(3)?, default: row.try_get(4)? };
    Ok((row.try_get(0)?, column))
}

fn index(row: &Row) -> Result<(u32, Index)> {
    let index = Index { name: row.try_get(1)?, is_unique: row.try_get(2)?, is_primary: row.try_get(3)?, columns: row.try_get(4)? };
    Ok((row.try_get(0)?, index))
}

/// The tables, in order, each with its columns and indexes, grouped by schema.
fn assembled(tables: Vec<Listed>, columns: Vec<(u32, SchemaColumn)>, indexes: Vec<(u32, Index)>) -> Schema {
    let mut columns_by_table = grouped(columns);
    let mut indexes_by_table = grouped(indexes);
    let mut schemas: Vec<SchemaNode> = vec![];
    for Listed { oid, schema, table } in tables {
        let table = Table {
            columns: columns_by_table.remove(&oid).unwrap_or_default(),
            indexes: indexes_by_table.remove(&oid).unwrap_or_default(),
            ..table
        };
        match schemas.last_mut() {
            Some(last) if last.name == schema => last.tables.push(table),
            _ => schemas.push(SchemaNode { name: schema, tables: vec![table] }),
        }
    }
    Schema { schemas }
}

fn grouped<T>(items: Vec<(u32, T)>) -> HashMap<u32, Vec<T>> {
    let mut by_table: HashMap<u32, Vec<T>> = HashMap::new();
    for (oid, item) in items {
        by_table.entry(oid).or_default().push(item);
    }
    by_table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::guard;
    use crate::db::model::{Route, Statement};
    use crate::fixtures;

    async fn session() -> Session {
        Session::open(&fixtures::test_database("local", "writes = \"allowed\"\n"), Route::Direct).await.unwrap()
    }

    async fn write(session: &mut Session, sql: &str) {
        session.run(&guard::admit(Statement::new(sql), WritePolicy::Allowed).unwrap()).await.unwrap();
    }

    /// A session on a fresh schema named `schema`, made by `sql`.
    async fn session_on(schema: &str, sql: &[&str]) -> Session {
        let mut session = session().await;
        write(&mut session, &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
        write(&mut session, &format!("CREATE SCHEMA {schema}")).await;
        for statement in sql {
            write(&mut session, statement).await;
        }
        session
    }

    fn tables_of<'a>(schema: &'a Schema, name: &str) -> &'a [Table] {
        schema.schemas.iter().find(|node| node.name == name).map_or(&[], |node| node.tables.as_slice())
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_gives_each_table_its_columns_and_indexes() {
        let mut session = session_on(
            "shop_read",
            &[
                "CREATE TABLE shop_read.orders (id serial PRIMARY KEY, customer text NOT NULL, total numeric(10, 2) DEFAULT 0)",
                "CREATE UNIQUE INDEX orders_customer ON shop_read.orders (lower(customer), id)",
            ],
        )
        .await;
        let schema = session.read_schema().await.unwrap();
        let [orders] = tables_of(&schema, "shop_read") else { panic!("{schema:?}") };
        let described: Vec<_> = orders.columns.iter().map(|c| (c.name.as_str(), c.type_name.as_str(), c.is_nullable)).collect();
        assert_eq!(described, [("id", "integer", false), ("customer", "text", false), ("total", "numeric(10,2)", true)]);
        assert_eq!(orders.columns[2].default.as_deref(), Some("0"));
        let indexes: Vec<_> = orders.indexes.iter().map(|i| (i.name.as_str(), i.columns.clone(), i.is_unique, i.is_primary)).collect();
        assert_eq!(
            indexes,
            [
                ("orders_customer", vec!["lower(customer)".to_owned(), "id".to_owned()], true, false),
                ("orders_pkey", vec!["id".to_owned()], true, true),
            ]
        );
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_names_each_kind_of_relation() {
        let mut session = session_on(
            "shop_kinds",
            &[
                "CREATE TABLE shop_kinds.orders (id int)",
                "CREATE VIEW shop_kinds.recent AS SELECT id FROM shop_kinds.orders",
                "CREATE MATERIALIZED VIEW shop_kinds.totals AS SELECT count(*) FROM shop_kinds.orders",
                "CREATE TABLE shop_kinds.days (day date) PARTITION BY RANGE (day)",
            ],
        )
        .await;
        let schema = session.read_schema().await.unwrap();
        let kinds: Vec<_> = tables_of(&schema, "shop_kinds").iter().map(|table| (table.name.as_str(), table.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("days", TableKind::Partitioned),
                ("orders", TableKind::Table),
                ("recent", TableKind::View),
                ("totals", TableKind::MaterializedView)
            ]
        );
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_lists_a_partitioned_table_without_its_partitions() {
        let mut session = session_on(
            "shop_parts",
            &[
                "CREATE TABLE shop_parts.orders (day date) PARTITION BY RANGE (day)",
                "CREATE TABLE shop_parts.orders_2026 PARTITION OF shop_parts.orders FOR VALUES FROM ('2026-01-01') TO ('2027-01-01')",
                "CREATE TABLE shop_parts.orders_2027 PARTITION OF shop_parts.orders FOR VALUES FROM ('2027-01-01') TO ('2028-01-01')",
            ],
        )
        .await;
        let schema = session.read_schema().await.unwrap();
        let kinds: Vec<_> = tables_of(&schema, "shop_parts").iter().map(|table| (table.name.as_str(), table.kind)).collect();
        assert_eq!(kinds, [("orders", TableKind::Partitioned)]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_leaves_the_catalogs_out() {
        let schema = session().await.read_schema().await.unwrap();
        let names: Vec<&str> = schema.schemas.iter().map(|node| node.name.as_str()).collect();
        assert!(!names.iter().any(|name| name.starts_with("pg_") || *name == "information_schema"), "{names:?}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_schema_shows_only_what_the_role_can_read() {
        let mut session = session_on(
            "shop_private",
            &[
                "CREATE TABLE shop_private.orders (id int, total int)",
                "CREATE TABLE shop_private.customers (id int, card text)",
                "DO $$ BEGIN CREATE ROLE pgbg_test_reader; EXCEPTION WHEN duplicate_object THEN NULL; END $$",
                "GRANT USAGE ON SCHEMA shop_private TO pgbg_test_reader",
                "GRANT SELECT (id) ON shop_private.customers TO pgbg_test_reader",
                "SET ROLE pgbg_test_reader",
            ],
        )
        .await;
        let schema = session.read_schema().await.unwrap();
        let [customers] = tables_of(&schema, "shop_private") else { panic!("{schema:?}") };
        assert_eq!(customers.name, "customers");
        assert_eq!(customers.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id"]);
    }
}
