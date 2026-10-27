//! `y`, `Y` and `c` on the grid: the cell, the row or the whole result to the clipboard, its values raw.
use super::{Action, App, Overlay, Run};
use crate::act::{self, Act, Pane};
use crate::db::model::{ResultSet, Value};
use crate::export::{self, Format};
use crossterm::event::KeyEvent;

/// What a copy's note adds when some of its cells pgbg cannot decode, which copy as `<unsupported>`.
fn undecodable<'v>(values: impl IntoIterator<Item = &'v Value>) -> String {
    match values.into_iter().filter(|value| matches!(value, Value::Unsupported)).count() {
        0 => String::new(),
        1 => " · 1 cell not decodable, copied as <unsupported>".to_owned(),
        count => format!(" · {count} cells not decodable, copied as <unsupported>"),
    }
}

impl App {
    fn shown_result(&self) -> Option<&ResultSet> {
        match &self.sheet().run {
            Run::Done { result, .. } => Some(result),
            _ => None,
        }
    }

    /// The row under the grid's cursor, counted from 1, and its values.
    fn current_row(&self) -> Option<(usize, &[Value])> {
        let Run::Done { result, grid, .. } = &self.sheet().run else { return None };
        result.rows().get(grid.row).map(|values| (grid.row + 1, values.as_slice()))
    }

    /// Copies the value of the cell under the cursor.
    pub(super) fn copy_cell(&self) -> Vec<Action> {
        let Run::Done { result, grid, .. } = &self.sheet().run else { return vec![] };
        let Some(value) = result.rows().get(grid.row).and_then(|values| values.get(grid.column)) else { return vec![] };
        let done = format!("copied {} of row {}{}", result.columns()[grid.column].name, grid.row + 1, undecodable([value]));
        vec![Action::Copy { text: export::cell(value), done }]
    }

    /// Copies the cursor's row as a CSV line.
    pub(super) fn copy_row(&self) -> Vec<Action> {
        let Some((number, values)) = self.current_row() else { return vec![] };
        vec![Action::Copy { text: export::row(values), done: format!("copied row {number} as CSV{}", undecodable(values)) }]
    }

    /// Asks on the footer line which format the whole result is copied as.
    pub(super) fn ask_copy_format(&mut self) {
        if self.shown_result().is_some() {
            self.overlay = Some(Overlay::CopyAs);
        }
    }

    /// The question after `key`: a format copies the result, `esc` copies nothing, any other key asks again.
    pub(super) fn copy_as_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let format = match act::act_for(key, Pane::CopyAs) {
            Some(Act::CopyCsv) => Format::Csv,
            Some(Act::CopyJson) => Format::Json,
            Some(Act::CopyMarkdown) => Format::Markdown,
            Some(Act::Quit) => return self.quit(),
            Some(Act::Close) => return vec![],
            _ => {
                self.overlay = Some(Overlay::CopyAs);
                return vec![];
            }
        };
        self.copy_result(format)
    }

    fn copy_result(&self, format: Format) -> Vec<Action> {
        let Some(result) = self.shown_result() else { return vec![] };
        let cells = undecodable(result.rows().iter().flatten());
        let done = format!("copied {} as {}{cells}", crate::render::row_count(result), format.name());
        vec![Action::Copy { text: export::result(result, format), done }]
    }

    /// Takes what a copy came to, for the status line until the next key.
    pub(super) fn copied(&mut self, note: String) {
        self.note = Some(note);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{orders, to_shop, typed};
    use super::super::{Answer, Incoming, RunId};
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::time::Duration;

    fn on_the_result() -> App {
        on(orders())
    }

    fn on(result: ResultSet) -> App {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result, took: Duration::from_millis(38) }));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app
    }

    fn with_an_undecodable_name() -> App {
        on(orders().with_cell(0, 2, Value::Unsupported))
    }

    #[test]
    fn a_copied_cell_pgbg_cannot_decode_says_so() {
        let mut app = with_an_undecodable_name();
        press(&mut app, 'l');
        press(&mut app, 'l');
        assert_eq!(
            press(&mut app, 'y'),
            copied("<unsupported>", "copied customer of row 1 · 1 cell not decodable, copied as <unsupported>")
        );
    }

    #[test]
    fn a_copied_row_says_how_many_of_its_cells_pgbg_cannot_decode() {
        let mut app = with_an_undecodable_name();
        assert_eq!(
            press(&mut app, 'Y'),
            copied("7,12.50,<unsupported>\n", "copied row 1 as CSV · 1 cell not decodable, copied as <unsupported>")
        );
    }

    #[test]
    fn a_copied_result_says_how_many_of_its_cells_pgbg_cannot_decode() {
        let mut app = on(orders().with_cell(0, 2, Value::Unsupported).with_cell(1, 2, Value::Unsupported));
        press(&mut app, 'c');
        let actions = press(&mut app, 'c');
        assert!(
            matches!(&*actions, [Action::Copy { done, .. }] if done.ends_with("· 2 cells not decodable, copied as <unsupported>")),
            "{actions:?}"
        );
    }

    fn press(app: &mut App, c: char) -> Vec<Action> {
        app.handle_key(KeyEvent::from(KeyCode::Char(c)))
    }

    fn copied(text: &str, done: &str) -> Vec<Action> {
        vec![Action::Copy { text: text.to_owned(), done: done.to_owned() }]
    }

    #[test]
    fn y_copies_the_cell_under_the_cursor() {
        let mut app = on_the_result();
        press(&mut app, 'j');
        press(&mut app, 'l');
        assert_eq!(press(&mut app, 'y'), copied("1499.00", "copied total of row 2"));
    }

    #[test]
    fn shift_y_copies_the_row_as_csv() {
        let mut app = on_the_result();
        assert_eq!(press(&mut app, 'Y'), copied("7,12.50,nina\n", "copied row 1 as CSV"));
    }

    #[test]
    fn c_then_a_letter_copies_the_whole_result_in_that_format() {
        let mut app = on_the_result();
        press(&mut app, 'c');
        let actions = press(&mut app, 'm');
        assert!(matches!(&*actions, [Action::Copy { text, done }] if text.starts_with("| id |") && done == "copied 4 rows as Markdown"));
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn esc_after_c_copies_nothing() {
        let mut app = on_the_result();
        press(&mut app, 'c');
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Esc)), []);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn another_key_after_c_asks_again() {
        let mut app = on_the_result();
        press(&mut app, 'c');
        assert_eq!(press(&mut app, 'x'), []);
        assert_eq!(app.overlay, Some(Overlay::CopyAs));
    }

    #[test]
    fn y_in_the_editor_types_a_y() {
        let mut app = typed("SELECT ");
        assert_eq!(press(&mut app, 'y'), []);
        assert_eq!(app.sheet().editor.text(), "SELECT y");
    }

    #[test]
    fn nothing_to_copy_before_a_result() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!([press(&mut app, 'y'), press(&mut app, 'Y'), press(&mut app, 'c')], [vec![], vec![], vec![]]);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn the_note_of_a_copy_stays_until_the_next_key() {
        let mut app = on_the_result();
        app.apply(Answer::Copied("copied row 1 as CSV".into()));
        assert_eq!(app.note.as_deref(), Some("copied row 1 as CSV"));
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.note, None);
    }
}
