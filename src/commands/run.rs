use crate::connection::ConnectionName;
use crate::db::model::{ResultSet, Statement};
use crate::db::{Session, guard};
use crate::render;
use anyhow::Result;

/// Runs one statement on the connection named `connection` and prints its result: a plain table, or JSON with `json`.
pub async fn run(connection: &ConnectionName, sql: &str, json: bool) -> Result<()> {
    let connection = super::connection(Some(connection))?;
    let admitted = guard::admit(Statement::new(sql), connection.writes())?;
    let result = Session::open(&connection).await?.run(&admitted).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print!("{}", text(&result));
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
    use crate::db::model::{Column, ColumnKind, Value};

    fn column(name: &str, kind: ColumnKind) -> Column {
        Column { name: name.to_owned(), kind, type_name: String::new() }
    }

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
