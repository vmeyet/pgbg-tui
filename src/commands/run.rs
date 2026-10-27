use crate::cli::StatementArgs;
use crate::connection::{Connection, ConnectionName};
use crate::db::Session;
use crate::db::guard::{Admitted, Refusal, Write};
use crate::db::model::{ResultSet, Statement, TriedToWrite};
use crate::history::{Entry, Outcome, Store};
use crate::query::params::{self, Assignment, Given};
use crate::query::{Header, Roots};
use crate::render;
use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Instant;

/// The path that makes `--file` or `--params-file` read stdin.
const STDIN: &str = "-";

/// Runs one statement on the connection named `connection`, its `{{name}}` bound to the values of `assignments` and of
/// the `name=value` lines of `params_file`, adds it to the connection's history, and prints its result: a plain table,
/// or JSON with `json`. A write is sent only after `yes` or a yes on the terminal.
pub async fn run(
    connection: &ConnectionName,
    statement: &StatementArgs,
    assignments: &[Assignment],
    params_file: Option<&Path>,
    yes: bool,
    json: bool,
) -> Result<()> {
    let connection = super::connection(Some(connection))?;
    check_one_reader_of_stdin(statement.file.as_deref(), params_file)?;
    let sql = &statement_text(statement).await?;
    let given = given(assignments, params_file).await?;
    let admitted = admit(&connection, sql, &given, yes).await;
    let (started_at, clock) = (Utc::now(), Instant::now());
    let result = match admitted {
        Ok(admitted) => super::on_session(&connection, async |session| run_asking_for_hidden_writes(session, admitted).await).await,
        Err(err) => Err(err),
    };
    let took_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
    let entry =
        Entry { statement: sql.to_owned(), connection: connection.name().clone(), started_at, took_ms, outcome: Outcome::of(&result) };
    record(entry).await;
    print(&result?, json)
}

fn check_one_reader_of_stdin(file: Option<&Path>, params_file: Option<&Path>) -> Result<()> {
    let is_stdin = |path: Option<&Path>| path == Some(Path::new(STDIN));
    if is_stdin(file) && is_stdin(params_file) {
        bail!("stdin can hold the statement or the parameters, not both: give one of them a file");
    }
    Ok(())
}

/// The text of the statement as typed, of the saved query, or of the file `statement` names.
async fn statement_text(statement: &StatementArgs) -> Result<String> {
    if let Some(path) = &statement.query {
        let roots = Roots::find_here().await?;
        return roots.read(&roots.named(path).await?).await;
    }
    match &statement.file {
        Some(path) => read_text(path, "the statement").await,
        None => Ok(statement.sql.clone().unwrap_or_default()),
    }
}

/// Adds `entry` to its connection's history, waiting for it before pgbg exits.
async fn record(entry: Entry) {
    let Some(store) = Store::from_env() else { return };
    if let Err(err) = store.record(entry).await {
        tracing::warn!("the history writer stopped: {err}");
    }
}

/// The values of `assignments` and of `file`'s lines; a name given twice is refused, and the file may name more
/// parameters than the statement holds.
async fn given(assignments: &[Assignment], file: Option<&Path>) -> Result<Given> {
    let from_file = match file {
        Some(path) => params::assignments(&read_text(path, "the parameters").await?).map_err(|err| anyhow!("{}: {err}", path.display()))?,
        None => vec![],
    };
    Ok(Given::new(assignments.to_vec(), from_file)?)
}

/// The text of the file at `path`, or of stdin for `-`; `what` says what it holds in an error.
async fn read_text(path: &Path, what: &str) -> Result<String> {
    if path == Path::new(STDIN) {
        let read = tokio::task::spawn_blocking(|| std::io::read_to_string(std::io::stdin())).await?;
        return read.with_context(|| format!("reading {what} from stdin"));
    }
    tokio::fs::read_to_string(path).await.with_context(|| format!("reading {}", path.display()))
}

/// Clears `sql`, with `given` bound, for `connection`; a write only after `yes` or a yes on the terminal.
async fn admit(connection: &Connection, sql: &str, given: &Given, yes: bool) -> Result<Admitted> {
    let header = Header::parse(sql);
    for warning in &header.warnings {
        super::warn(warning);
    }
    let admitted = params::admit(&Statement::new(sql), given, &header.params, connection.writes())?;
    if yes {
        return Ok(admitted.confirmed());
    }
    match admitted.write() {
        Some(write) if !is_confirmed_at_the_terminal(write).await? => Err(Refusal::Unconfirmed(write.clone()).into()),
        _ => Ok(admitted),
    }
}

/// Runs `admitted`; when the server finds a write in a read, as in a function that writes, nothing ran, and it runs
/// again as a write once the user says yes on the terminal.
async fn run_asking_for_hidden_writes(session: &mut Session, admitted: Admitted) -> Result<ResultSet> {
    match session.run(&admitted).await {
        Err(err) if err.is::<TriedToWrite>() => {
            if !is_confirmed_at_the_terminal(&Write::Hidden).await? {
                return Err(Refusal::Unconfirmed(Write::Hidden).into());
            }
            session.run(&admitted.confirmed()).await
        }
        ran => ran,
    }
}

/// Whether the user answers `y` to `write` on the terminal; false when stdin is not a terminal.
async fn is_confirmed_at_the_terminal(write: &Write) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("{write}? [y/N] ");
    let answer = tokio::task::spawn_blocking(|| std::io::stdin().lines().next().transpose()).await??;
    Ok(answer.is_some_and(|answer| matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")))
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
    let header = result.columns().iter().map(|column| column.name.to_string()).collect();
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
