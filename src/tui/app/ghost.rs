//! The ghost: the rest of the best name for the word typed before the cursor, dimmed after it until the next key, and
//! `→` to take it.
use super::{Action, Sheet};
use crate::complete::{Ask, Index};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::ops::Range;

/// The name the ghost offers, in what the editor shows and what it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Ghost {
    /// The editor bytes the taken name replaces, up to the cursor.
    typed: Range<usize>,
    insert: String,
    /// What shows after the cursor.
    pub(in crate::tui) rest: String,
}

/// Whether `key` types a character of a name, or the `.` before one.
pub(super) fn is_name_key(key: KeyEvent) -> bool {
    let is_plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    is_plain && matches!(key.code, KeyCode::Char(c) if is_name_char(c) || c == '.')
}

/// Whether `key` takes the ghost: `→` alone.
pub(super) fn is_take(key: KeyEvent) -> bool {
    key.code == KeyCode::Right && key.modifiers.is_empty()
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

impl Sheet {
    /// Offers the ghost of the word before the cursor when the cursor ends it, from the index once there is one, else from
    /// the keywords; the first ask reads the schema.
    pub(super) fn ask_ghost(&mut self) -> Vec<Action> {
        let actions = self.want_schema();
        let (before, under, _) = self.editor.split();
        if under.starts_with(is_name_char) {
            return actions;
        }
        let index = self.index.as_deref().unwrap_or_else(|| Index::of_keywords());
        let ask = Ask::at(self.editor.text(), before.len());
        self.ghost = ask.and_then(|ask| ghost_of(index, &ask));
        actions
    }

    /// Puts the ghost's name in place of what was typed of it.
    pub(super) fn take_ghost(&mut self, ghost: Ghost) {
        self.editor.replace_typed(ghost.typed, &ghost.insert);
        self.text_changed();
    }
}

fn ghost_of(index: &Index, ask: &Ask) -> Option<Ghost> {
    let candidate = index.ghost(ask)?;
    let rest = candidate.rest_after(&ask.prefix).to_owned();
    (!rest.is_empty()).then(|| Ghost { typed: ask.typed.clone(), rest, insert: candidate.insert })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{shop, to_shop, typed};
    use super::super::{Incoming, SchemaState};
    use super::*;
    use crate::db::model::Catalog;
    use crate::fixtures::shop_schema;
    use std::sync::Arc;

    fn indexed(app: &mut super::super::App) {
        let catalog = Arc::new(Catalog { schema: shop_schema(), search_path: vec!["public".to_owned()], is_cut: false });
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
    fn the_first_name_typed_reads_the_schema() {
        let mut app = typed("");
        let actions = app.handle_key(KeyEvent::from(KeyCode::Char('s')));
        assert!(matches!(actions.as_slice(), [Action::ReadSchema { connection, .. }] if *connection == shop()), "{actions:?}");
        assert_eq!(app.sheet().schema, SchemaState::Reading);
    }

    #[test]
    fn right_takes_the_ghost_and_the_cursor_ends_it() {
        let mut app = typed_on_shop("SELECT * FROM cu");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.split(), ("SELECT * FROM customers", "", ""));
        assert_eq!(rest(&app), None);
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
