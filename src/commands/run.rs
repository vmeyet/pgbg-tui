use crate::cli::StatementArgs;
use crate::connection::{Connection, ConnectionName, WritePolicy};
use crate::db::Session;
use crate::db::guard::{self, Admitted, Refusal, Write};
use crate::db::model::{ResultSet, Statement, TriedToWrite};
use crate::db::preview::{self, Touched};
use crate::failure::Failure;
use crate::history::{Entry, Outcome, Store};
use crate::query::params::{self, Assignment, Filled, Given};
use crate::query::{Header, Roots};
use crate::render;
use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Instant;
use tokio::sync::oneshot;

/// The path that makes `--file` or `--params-file` read stdin.
const STDIN: &str = "-";

/// Runs one statement on the connection named `connection`, its `{{name}}` bound to the values of `assignments` and of
/// the `name=value` lines of `params_file`, adds it to the connection's history, and prints its result: a plain table,
/// or JSON with `json`. A write is sent only after `yes`, or a yes on the terminal to a question that says how many rows
/// it would touch.
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
    let sql = &statement_text(statement, connection.queries()).await?;
    let given = given(assignments, params_file).await?;
    let header = Header::parse(sql);
    let admitted = admit(&connection, sql, &given, &header, yes);
    let (started_at, clock) = (Utc::now(), Instant::now());
    let work = async |session: &mut Session, admitted| {
        let admitted = confirmed(session, admitted, count_of(sql, &given, &header), yes).await?;
        run_asking_for_hidden_writes(session, admitted).await
    };
    let result = match admitted {
        Ok(admitted) => super::on_session(&connection, async |session| work(session, admitted).await).await,
        Err(err) => Err(err),
    };
    let entry = Entry::of_run(Statement::new(sql), connection.name().clone(), started_at, clock.elapsed(), Outcome::of(&result));
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

/// The text of the statement as typed, of the saved query in `folder`, or of the file `statement` names.
async fn statement_text(statement: &StatementArgs, folder: &ConnectionName) -> Result<String> {
    if let Some(path) = &statement.query {
        let roots = Roots::from_env();
        return roots.read(&roots.named(path, folder).await?).await;
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
        let read = on_a_thread_of_its_own(|| std::io::read_to_string(std::io::stdin())).await?;
        return read.with_context(|| format!("reading {what} from stdin"));
    }
    tokio::fs::read_to_string(path).await.with_context(|| format!("reading {}", path.display()))
}

/// Clears `sql`, with `given` bound, for `connection`; a write without `yes` is refused at once when no terminal can
/// confirm it.
fn admit(connection: &Connection, sql: &str, given: &Given, header: &Header, yes: bool) -> Result<Admitted> {
    for warning in &header.warnings {
        super::warn(warning);
    }
    let admitted = params::admit(&Statement::new(sql), given, &header.params, connection.writes())?;
    match admitted.write() {
        Some(write) if !yes && !std::io::stdin().is_terminal() => Err(Refusal::Unconfirmed(write.clone()).into()),
        _ => Ok(admitted),
    }
}

/// The read that counts the rows `sql` would touch as a write, with `given` bound, admitted read only.
fn count_of(sql: &str, given: &Given, header: &Header) -> Option<Admitted> {
    let read = preview::count_of(&Statement::new(sql))?;
    let Filled { statement, params } = params::fill_cut(&read, given, &header.params).ok()?;
    let admitted = guard::admit(statement, params, WritePolicy::ReadOnly).ok()?;
    Some(admitted.with_own_timeout(Some(preview::COUNT_TIMEOUT)))
}

/// `admitted` confirmed by `yes`, or by a yes on the terminal to its write, told first how many rows `count` reads it
/// would touch; a read as it is.
async fn confirmed(session: &mut Session, admitted: Admitted, count: Option<Admitted>, yes: bool) -> Result<Admitted> {
    if yes {
        return Ok(admitted.confirmed());
    }
    let Some(write) = admitted.write() else { return Ok(admitted) };
    let rows = match count {
        Some(count) => rows_touched(session, &count).await,
        None => String::new(),
    };
    if !is_confirmed_at_the_terminal(write, &rows).await? {
        return Err(Refusal::Unconfirmed(write.clone()).into());
    }
    Ok(admitted)
}

/// How many rows `count` reads, in words, or why it read none.
async fn rows_touched(session: &mut Session, count: &Admitted) -> String {
    match session.run(count).await {
        Ok(counted) => {
            Touched::read(&counted).map_or_else(|| "rows not counted: it read no number".to_owned(), |touched| touched.to_string())
        }
        Err(err) => format!("rows not counted: {}", Failure::from(&err)),
    }
}

/// Runs `admitted`; when the server finds a write in a read, as in a function that writes, the write was refused, and
/// it runs again as a write once the user says yes on the terminal: what it did before the write may then run twice.
async fn run_asking_for_hidden_writes(session: &mut Session, admitted: Admitted) -> Result<ResultSet> {
    match session.run(&admitted).await {
        Err(err) if err.is::<TriedToWrite>() => {
            if !is_confirmed_at_the_terminal(&Write::Hidden, "").await? {
                return Err(Refusal::Unconfirmed(Write::Hidden).into());
            }
            session.run(&admitted.confirmed()).await
        }
        ran => ran,
    }
}

/// Whether the user answers `y` to `write`, told it would touch `rows`, on the terminal; false when stdin is not a
/// terminal.
async fn is_confirmed_at_the_terminal(write: &Write, rows: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("{}", question(write, rows));
    let answer = on_a_thread_of_its_own(|| std::io::stdin().lines().next().transpose()).await??;
    Ok(answer.is_some_and(|answer| matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")))
}

/// The question that asks for `write`, saying the `rows` it would touch when they were counted, its names and the
/// server's words escaped so none can rewrite the terminal line.
fn question(write: &Write, rows: &str) -> String {
    let rows = if rows.is_empty() { String::new() } else { format!("{} ", render::printable(rows)) };
    format!("{}? {rows}[y/N] ", render::printable(&write.to_string()))
}

/// What `read` gives, read on a thread of its own: dropped on a signal, it leaves the thread blocked and lets pgbg exit,
/// where the runtime would wait for a blocking task.
async fn on_a_thread_of_its_own<T: Send + 'static>(read: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    let (sender, read_back) = oneshot::channel();
    std::thread::Builder::new().name("pgbg-stdin".into()).spawn(move || sender.send(read()))?;
    Ok(read_back.await?)
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
    use crate::db::{guard, lex};
    use crate::fixtures::column;
    use std::time::Duration;

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

    #[test]
    fn the_question_escapes_a_name_that_would_rewrite_the_terminal_line() {
        let sql = "DELETE FROM \"orders\r\x1b[2K\u{202e}SELECT from nothing\"";
        let write = guard::write_in(sql, &lex::tokens(sql)).unwrap();
        assert_eq!(question(&write, ""), "DELETE from \"orders\\r\\u{1b}[2K\\u{202e}SELECT from nothing\"? [y/N] ");
    }

    #[test]
    fn the_question_says_how_many_rows_the_write_would_touch() {
        let sql = "DELETE FROM orders";
        let write = guard::write_in(sql, &lex::tokens(sql)).unwrap();
        assert_eq!(question(&write, "42 rows"), "DELETE from orders? 42 rows [y/N] ");
    }

    #[test]
    fn a_read_dropped_halfway_leaves_the_runtime_free_to_shut_down() {
        let (blocked, _never_written) = std::io::pipe().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
        let reading = on_a_thread_of_its_own(move || std::io::read_to_string(blocked));
        let read = runtime.block_on(async { tokio::time::timeout(Duration::ZERO, reading).await });
        assert!(read.is_err(), "nothing was written");
        drop(runtime);
    }
}
