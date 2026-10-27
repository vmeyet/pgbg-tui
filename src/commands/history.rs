use crate::connection::ConnectionName;
use crate::history::{Entry, Store};
use crate::render;
use anyhow::{Context, Result, anyhow};

const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// Prints the `limit` newest statements run on `connection`: a plain table, or JSON with `json`.
pub fn run(connection: &ConnectionName, limit: usize, json: bool) -> Result<()> {
    let store = Store::from_env().ok_or_else(|| anyhow!("no data folder to keep the history in: set PGBG_DATA_DIR"))?;
    let history = store.read(connection).with_context(|| format!("reading the history of `{connection}`"))?;
    let newest = &history.entries[..limit.min(history.entries.len())];
    if json {
        println!("{}", serde_json::to_string_pretty(newest)?);
    } else {
        print!("{}", text(newest));
    }
    Ok(())
}

fn text(entries: &[Entry]) -> String {
    render::table(&entries.iter().map(row).collect::<Vec<_>>())
}

fn row(entry: &Entry) -> Vec<String> {
    let statement = entry.statement.as_str().split_whitespace().collect::<Vec<_>>().join(" ");
    vec![entry.started_at.format(TIME_FORMAT).to_string(), format!("{} ms", entry.took_ms), entry.outcome.to_string(), statement]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::Statement;
    use crate::history::Outcome;
    use chrono::DateTime;

    #[test]
    fn an_entry_prints_on_one_line_when_how_long_what_came_of_it_and_the_statement() {
        let entry = Entry {
            statement: Statement::new("SELECT total\n  FROM orders"),
            connection: "shop".parse().unwrap(),
            started_at: DateTime::parse_from_rfc3339("2026-10-26T09:30:00Z").unwrap().to_utc(),
            took_ms: 38,
            outcome: Outcome::Rows { count: 4, is_cut: false },
        };
        assert_eq!(text(&[entry]), "2026-10-26 09:30:00  38 ms  4 rows  SELECT total FROM orders\n");
    }
}
