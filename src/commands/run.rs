use crate::connection::{Connection, ConnectionName};
use crate::db::guard;
use crate::db::model::{ResultSet, Statement};
use crate::history::{Entry, Outcome, Store};
use crate::render;
use anyhow::Result;
use chrono::Utc;
use std::time::Instant;

/// Runs one statement on the connection named `connection`, adds it to the connection's history, and prints its result:
/// a plain table, or JSON with `json`.
pub async fn run(connection: &ConnectionName, sql: &str, json: bool) -> Result<()> {
    let connection = super::connection(Some(connection))?;
    let (started_at, clock) = (Utc::now(), Instant::now());
    let result = run_statement(&connection, sql).await;
    let took_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
    let entry =
        Entry { statement: sql.to_owned(), connection: connection.name().clone(), started_at, took_ms, outcome: Outcome::of(&result) };
    record(entry).await;
    print(&result?, json)
}

/// Adds `entry` to its connection's history, waiting for it before pgbg exits.
async fn record(entry: Entry) {
    let Some(store) = Store::from_env() else { return };
    if let Err(err) = store.record(entry).await {
        tracing::warn!("the history writer stopped: {err}");
    }
}

async fn run_statement(connection: &Connection, sql: &str) -> Result<ResultSet> {
    let admitted = guard::admit(Statement::new(sql), connection.writes())?;
    super::on_session(connection, async |session| session.run(&admitted).await).await
}

fn print(result: &ResultSet, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        print!("{}", text(result));
    }
    Ok(())
}

fn text(result: &ResultSet) -> String {
    let header = result.columns.iter().map(|column| column.name.clone()).collect();
    let rows = result.rows.iter().map(|row| row.iter().map(ToString::to_string).collect());
    render::table(&std::iter::once(header).chain(rows).collect::<Vec<_>>()) + &format!("({})\n", render::row_count(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::{ColumnKind, Value};
    use crate::fixtures::column;

    #[test]
    fn a_result_prints_its_header_its_rows_and_their_count() {
        let result = ResultSet {
            columns: vec![column("id", ColumnKind::Int), column("customer", ColumnKind::Text)],
            rows: vec![vec![Value::Int(7), Value::Text("nina".into())], vec![Value::Int(12), Value::Null]],
            is_cut: false,
        };
        assert_eq!(text(&result), "id  customer\n7   nina\n12  NULL\n(2 rows)\n");
    }

    #[test]
    fn a_result_of_one_row_says_row() {
        let result = ResultSet { columns: vec![column("total", ColumnKind::Int)], rows: vec![vec![Value::Int(3)]], is_cut: false };
        assert!(text(&result).ends_with("(1 row)\n"));
    }
}
