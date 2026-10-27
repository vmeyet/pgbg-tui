use crate::connection::{Connection, ConnectionName};
use crate::db::model::{ResultSet, Statement};
use crate::history::{Entry, Outcome, Store};
use crate::query::params::{self, Assignment, Given};
use crate::render;
use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use std::path::Path;
use std::time::Instant;

/// Reads `--params-file` from stdin.
const STDIN: &str = "-";

/// Runs one statement on the connection named `connection`, its `{{name}}` bound to the values of `assignments` and of
/// the `name=value` lines of `params_file`, adds it to the connection's history, and prints its result: a plain table,
/// or JSON with `json`.
pub async fn run(connection: &ConnectionName, sql: &str, assignments: &[Assignment], params_file: Option<&Path>, json: bool) -> Result<()> {
    let connection = super::connection(Some(connection))?;
    let given = given(assignments, params_file).await?;
    let (started_at, clock) = (Utc::now(), Instant::now());
    let result = run_statement(&connection, sql, &given).await;
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

/// The values of `assignments` and of `file`'s lines, a name given twice refused; the file may name more than the
/// statement holds.
async fn given(assignments: &[Assignment], file: Option<&Path>) -> Result<Given> {
    let from_file = match file {
        Some(path) => params::assignments(&read_params_file(path).await?).map_err(|err| anyhow!("{}: {err}", path.display()))?,
        None => vec![],
    };
    Ok(Given::new(assignments.to_vec(), from_file)?)
}

async fn read_params_file(path: &Path) -> Result<String> {
    if path == Path::new(STDIN) {
        let read = tokio::task::spawn_blocking(|| std::io::read_to_string(std::io::stdin())).await?;
        return read.context("reading the parameters from stdin");
    }
    tokio::fs::read_to_string(path).await.with_context(|| format!("reading {}", path.display()))
}

async fn run_statement(connection: &Connection, sql: &str, given: &Given) -> Result<ResultSet> {
    let admitted = params::admit(&Statement::new(sql), given, connection.writes())?;
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
    let header = result.columns().iter().map(|column| column.name.clone()).collect();
    let rows = result.rows().iter().map(|row| row.iter().map(ToString::to_string).collect());
    render::table(&std::iter::once(header).chain(rows).collect::<Vec<_>>()) + &format!("({})\n", render::row_count(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::{ColumnKind, Value};
    use crate::fixtures::column;

    #[test]
    fn a_result_prints_its_header_its_rows_and_their_count() {
        let result = ResultSet::whole(
            vec![column("id", ColumnKind::Int), column("customer", ColumnKind::Text)],
            vec![vec![Value::Int(7), Value::Text("nina".into())], vec![Value::Int(12), Value::Null]],
        );
        assert_eq!(text(&result), "id  customer\n7   nina\n12  NULL\n(2 rows)\n");
    }

    #[test]
    fn a_result_of_one_row_says_row() {
        let result = ResultSet::whole(vec![column("total", ColumnKind::Int)], vec![vec![Value::Int(3)]]);
        assert!(text(&result).ends_with("(1 row)\n"));
    }
}
