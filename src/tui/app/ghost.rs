//! The ghost: the rest of the best name for the word before the cursor, dimmed after the cursor until the next key.
//! `→` takes it.
use super::{Action, SessionState, Sheet};
use crate::complete::{Ask, Index};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::ops::Range;
use vtui::field::Field;

/// The name the ghost offers: what the editor shows, and the bytes it replaces when taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Ghost {
    /// The editor bytes the taken name replaces, up to the cursor.
    typed: Range<usize>,
    insert: String,
    /// What shows after the cursor.
    pub(in crate::tui) rest: String,
}

impl Ghost {
    /// The first line of `rest`, cleaned as a paste is, offered at the cursor `at`; none when nothing printable is left.
    pub(super) fn at_cursor(at: usize, rest: &str) -> Option<Self> {
        let mut cleaned = Field::default();
        cleaned.paste(rest);
        let line = cleaned.text().lines().next().filter(|line| !line.trim().is_empty())?;
        Some(Self { typed: at..at, insert: line.to_owned(), rest: line.to_owned() })
    }
}

/// Whether `key` types a character of a name, or the `.` before one.
pub(super) fn is_name_key(key: KeyEvent) -> bool {
    let is_plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    is_plain && matches!(key.code, KeyCode::Char(c) if is_name_char(c) || c == '.')
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

impl Sheet {
    /// Offers the ghost of the word before the cursor when the cursor is at its end, from the index once there is one,
    /// or else from the keywords; the first ask reads the schema.
    pub(super) fn ask_ghost(&mut self) -> Vec<Action> {
        let actions = self.want_schema_while_open();
        let (_, under, _) = self.editor.split();
        if under.starts_with(is_name_char) {
            return actions;
        }
        self.ghost = self.ask().and_then(|asked| ghost_of(asked.index, &asked.ask));
        actions
    }

    /// Asks for the schema that completion needs, only while the session is open: typing never opens a session again.
    pub(super) fn want_schema_while_open(&mut self) -> Vec<Action> {
        if self.session == SessionState::Open { self.want_schema() } else { vec![] }
    }

    /// Replaces the typed part of the ghost's name with the whole name.
    pub(super) fn take_ghost(&mut self, ghost: Ghost) {
        self.put_name(ghost.typed, &ghost.insert);
    }

    /// Puts `insert` in place of the bytes `typed`, which end at the cursor.
    pub(super) fn put_name(&mut self, typed: Range<usize>, insert: &str) {
        self.editor.replace_typed(typed, insert);
        self.text_changed();
    }
}

fn ghost_of(index: &Index, ask: &Ask) -> Option<Ghost> {
    let ghosted = index.ghost(ask)?;
    (!ghosted.rest.is_empty()).then(|| Ghost { typed: ask.typed.clone(), rest: ghosted.rest, insert: ghosted.insert })
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{shop, to_shop, typed};
    use super::super::{Incoming, SchemaState};
    use super::*;
    use crate::db::model::Catalog;
    use crate::failure::Failure;
    use crate::fixtures::shop_catalog;
    use std::sync::Arc;

    fn indexed(app: &mut super::super::App) {
        let catalog = Arc::new(shop_catalog());
        let index = Arc::new(Index::new(&catalog));
        app.apply(to_shop(Incoming::Schema(Ok(Arc::clone(&catalog)))));
        app.apply(to_shop(Incoming::Index { catalog, index }));
    }

    fn typed_on_shop(text: &str) -> super::super::App {
        let mut app = typed("");
        indexed(&mut app);
        for c in text.chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app
    }

    fn rest(app: &super::super::App) -> Option<&str> {
        app.sheet().ghost.as_ref().map(|ghost| ghost.rest.as_str())
    }

    #[test]
    fn typing_a_name_shows_the_rest_of_the_best_one() {
        assert_eq!(rest(&typed_on_shop("SELECT * FROM cu")), Some("stomers"));
    }

    #[test]
    fn a_dot_shows_a_column_of_the_table_before_it() {
        assert_eq!(rest(&typed_on_shop("SELECT * FROM orders o WHERE o.")), Some("id"));
    }

    #[test]
    fn before_the_schema_is_read_keywords_still_show() {
        assert_eq!(rest(&typed("sel")), Some("ect"));
    }

    #[test]
    fn the_first_name_typed_on_an_open_session_reads_the_schema() {
        let mut app = typed("");
        app.apply(to_shop(Incoming::Opened));
        let actions = app.handle_key(KeyEvent::from(KeyCode::Char('s')));
        assert!(matches!(actions.as_slice(), [Action::ReadSchema { connection, .. }] if *connection == shop()), "{actions:?}");
        assert_eq!(app.sheet().schema, SchemaState::Reading);
    }

    #[test]
    fn typing_never_opens_a_failed_session_again() {
        let mut app = typed("");
        app.apply(to_shop(Incoming::NotOpened(Failure::Login("connection refused".into()))));
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Char('s'))), []);
        assert_eq!((&app.sheet().schema, rest(&app)), (&SchemaState::NotRead, Some("how")));
    }

    #[test]
    fn right_takes_the_ghost_and_the_cursor_ends_it() {
        let mut app = typed_on_shop("SELECT * FROM cu");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.split(), ("SELECT * FROM customers", "", ""));
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn right_after_join_takes_the_table_with_its_alias_and_condition() {
        let mut app = typed_on_shop("SELECT * FROM orders o JOIN cu");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM orders o JOIN customers c ON c.id = o.customer_id");
    }

    #[test]
    fn any_other_key_drops_the_ghost() {
        let mut app = typed_on_shop("SELECT * FROM cu");
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(rest(&app), None);
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM cu");
    }

    #[test]
    fn a_key_that_acts_drops_the_ghost() {
        for key in [KeyEvent::from(KeyCode::Enter), KeyEvent::from(KeyCode::Esc), KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)]
        {
            let mut app = typed_on_shop("SELECT * FROM cu");
            app.handle_key(key);
            assert_eq!(rest(&app), None, "{key:?}");
        }
    }

    #[test]
    fn a_space_shows_no_ghost() {
        assert_eq!(rest(&typed_on_shop("SELECT * FROM ")), None);
    }

    #[test]
    fn a_name_typed_in_the_middle_of_a_word_shows_no_ghost() {
        let mut app = typed_on_shop("SELECT * FROM rders");
        for _ in 0..5 {
            app.handle_key(KeyEvent::from(KeyCode::Left));
        }
        app.handle_key(KeyEvent::from(KeyCode::Char('o')));
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn an_index_of_a_replaced_catalog_is_dropped() {
        let mut app = typed("");
        let old = Arc::new(Catalog::default());
        app.apply(to_shop(Incoming::Schema(Ok(Arc::new(Catalog::default())))));
        app.apply(to_shop(Incoming::Index { index: Arc::new(Index::new(&old)), catalog: old }));
        assert_eq!(app.sheet().index, None);
    }
}
