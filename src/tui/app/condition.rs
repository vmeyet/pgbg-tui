//! `=` and `!` on the grid: the cell under the cursor added to the statement under the editor's cursor, as
//! `WHERE column = {{column}}` or `<>`, its value in the parameter bar so it is bound, never spliced into the text.
use super::{App, Run, Sheet};
use crate::db::model::{ColumnName, ParamValue, Value};
use crate::query::condition::{self, Comparison};
use crate::query::params::{self, ParamName};

/// What a cell compares as: NULL, which `IS NULL` reads without a parameter, or a value bound as one.
enum Compared {
    Null,
    Bound(ParamValue),
}

impl Compared {
    /// What it compares as, or the words saying why no text binds it.
    fn of(value: &Value) -> Result<Self, &'static str> {
        match value {
            Value::Null => Ok(Self::Null),
            Value::Array(_) => Err("holds an array, which `=` and `!` do not compare"),
            other => other.as_param().map(Self::Bound).ok_or("holds a value pgbg could not decode"),
        }
    }
}

impl App {
    /// Adds the cell under the grid's cursor to the statement under the editor's cursor, and says so on the status line;
    /// the keys stay on the grid. A value pgbg could not decode is refused, as a lookup refuses it, and so is an array.
    pub(super) fn add_condition(&mut self, comparison: Comparison) {
        let Some((column, compared)) = self.cell_under_cursor() else { return };
        let note = match compared {
            Err(refusal) => format!("{column} {refusal}"),
            Ok(compared) => self
                .sheet_mut()
                .add_condition(&column, compared, comparison)
                .map_or_else(|| "no statement under the editor's cursor".to_owned(), |added| format!("added {added}")),
        };
        self.note = Some(note);
    }

    fn cell_under_cursor(&self) -> Option<(ColumnName, Result<Compared, &'static str>)> {
        let Run::Done { result, grid, .. } = &self.sheet().run else { return None };
        let value = result.rows().get(grid.row)?.get(grid.column)?;
        Some((result.columns()[grid.column].name.clone(), Compared::of(value)))
    }
}

impl Sheet {
    /// Adds `column` compared with `compared` to the statement under the editor's cursor as one step to undo, a value
    /// bound in the parameter bar under a name the statement does not hold yet; gives the condition added.
    fn add_condition(&mut self, column: &ColumnName, compared: Compared, comparison: Comparison) -> Option<String> {
        let tokens = self.editor.lexed().tokens_in(self.range_under_cursor()?);
        let taken = params::names(tokens).unwrap_or_default();
        let bound = match compared {
            Compared::Null => None,
            Compared::Bound(value) => Some((ParamName::for_column(column.as_str(), &taken), value)),
        };
        let condition = condition::condition(column.as_str(), comparison, bound.as_ref().map(|(param, _)| param));
        let splice = condition::add(self.editor.text(), tokens, &condition)?;
        self.editor.replace(splice.range, &splice.text);
        self.text_changed();
        if let Some((param, value)) = &bound {
            self.bar.put(param, value.as_str());
        }
        Some(condition)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{enter, on_the_result, press, to_shop, typed};
    use super::super::{Action, Focus, Incoming, RunId};
    use super::*;
    use crate::db::model::{ColumnKind, JsonText, ResultSet};
    use crate::fixtures::{column, orders, undecoded};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    /// The app with `sql` in the editor and the orders as its result, the keys on the grid.
    fn on_orders(sql: &str) -> App {
        let mut app = typed(sql);
        enter(&mut app);
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        press(&mut app, KeyCode::Tab);
        app
    }

    fn pressed(sql: &str, keys: &str) -> App {
        let mut app = on_orders(sql);
        keys.chars().for_each(|c| press(&mut app, KeyCode::Char(c)));
        app
    }

    #[test]
    fn equal_adds_the_cells_column_as_a_parameter_to_the_statement() {
        let app = pressed("SELECT * FROM orders ORDER BY id", "l=");
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM orders WHERE total = {{total}} ORDER BY id");
    }

    #[test]
    fn the_cells_value_waits_in_the_parameter_bar() {
        let app = pressed("SELECT * FROM orders", "l=");
        assert_eq!(app.sheet().bar.field(&"total".parse().unwrap()).map(vtui::field::Field::text), Some("12.50"));
    }

    #[test]
    fn bang_on_a_null_cell_adds_is_not_null() {
        let app = pressed("SELECT * FROM orders", "jll!");
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM orders WHERE customer IS NOT NULL");
    }

    #[test]
    fn a_second_cell_of_the_column_takes_its_own_parameter() {
        let app = pressed("SELECT * FROM orders", "=j!");
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM orders WHERE id = {{id}} AND id <> {{id_2}}");
    }

    #[test]
    fn the_keys_stay_on_the_grid_and_the_status_line_says_what_was_added() {
        let app = pressed("SELECT * FROM orders", "=");
        assert_eq!((app.focus, app.note()), (Focus::Result, Some("added id = {{id}}")));
    }

    #[test]
    fn one_undo_takes_the_condition_back() {
        let mut app = pressed("SELECT * FROM orders", "=");
        press(&mut app, KeyCode::Tab);
        app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM orders");
    }

    #[test]
    fn the_statement_runs_with_the_value_bound() {
        let mut app = pressed("SELECT * FROM orders", "=");
        press(&mut app, KeyCode::Tab);
        let [Action::Run { statement, input, .. }] = &enter(&mut app)[..] else { panic!("no run") };
        let filled = params::fill(statement, &input.given, &input.specs).unwrap();
        assert_eq!(
            (filled.statement.as_str(), filled.params.values()[0].as_ref().map(ParamValue::as_str)),
            ("SELECT * FROM orders WHERE id = $1", Some("7"))
        );
    }

    #[test]
    fn a_cell_pgbg_could_not_decode_adds_no_condition_and_says_so() {
        let mut app = on_the_result(undecoded());
        press(&mut app, KeyCode::Char('='));
        assert_eq!((app.sheet().editor.text(), app.note()), ("SELECT 1", Some("shape holds a value pgbg could not decode")));
    }

    fn tags(cell: Value) -> App {
        on_the_result(ResultSet::whole(vec![column("tags", ColumnKind::Array)], vec![vec![cell]]))
    }

    #[test]
    fn an_array_cell_adds_no_condition_and_says_so() {
        let mut app = tags(Value::Array(JsonText::of(&serde_json::json!(["tea"]))));
        press(&mut app, KeyCode::Char('='));
        assert_eq!((app.sheet().editor.text(), app.note()), ("SELECT 1", Some("tags holds an array, which `=` and `!` do not compare")));
    }

    #[test]
    fn a_null_array_cell_still_compares_with_is_null() {
        let mut app = tags(Value::Null);
        app.sheet_mut().editor.rewrite("SELECT * FROM orders");
        press(&mut app, KeyCode::Char('='));
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM orders WHERE tags IS NULL");
    }

    #[test]
    fn equal_and_bang_while_the_filter_hides_the_cursors_row_add_nothing() {
        let mut app = pressed("SELECT * FROM orders", "/zzz");
        press(&mut app, KeyCode::Enter);
        "=!".chars().for_each(|c| press(&mut app, KeyCode::Char(c)));
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM orders");
    }

    #[test]
    fn an_empty_editor_says_it_has_no_statement() {
        let mut app = on_the_result(orders());
        app.sheet_mut().editor.rewrite("");
        press(&mut app, KeyCode::Char('='));
        assert_eq!(app.note(), Some("no statement under the editor's cursor"));
    }
}
