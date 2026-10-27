//! The SQL editor: its text, the editing keys and pastes that change it, and the syntax colours that follow it.
use super::ghost::{is_name_key, is_take};
use super::{Action, Sheet};
use crate::query::Header;
use crate::syntax::{self, Spans};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::ops::Range;
use vtui::field::Field;

/// Whether an edit changed the text, or only moved the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edited {
    Text,
    Cursor,
}

/// The editor's text and its colours; the colours are rebuilt by `recolour` once the text changed.
#[derive(Debug, Default)]
pub(in crate::tui) struct Editor {
    field: Field,
    colours: Vec<Spans>,
    has_stale_colours: bool,
}

impl Editor {
    /// An editor on `text`, left for the loop to colour, so opening a big query never waits on tree-sitter.
    pub(super) fn new(text: &str) -> Self {
        Self { field: Field::new(text), colours: vec![], has_stale_colours: true }
    }

    pub(in crate::tui) fn text(&self) -> &str {
        self.field.text()
    }

    /// The text before the cursor, the character under it, and the rest.
    pub(in crate::tui) fn split(&self) -> (&str, &str, &str) {
        self.field.split()
    }

    /// The colours of each line, as of the last `recolour`.
    pub(in crate::tui) fn colours(&self) -> &[Spans] {
        &self.colours
    }

    /// Whether the text changed since the colours were built.
    pub(super) fn has_stale_colours(&self) -> bool {
        self.has_stale_colours
    }

    /// Builds the colours of the text as it is now.
    pub(super) fn recolour(&mut self) {
        if self.has_stale_colours {
            self.colours = syntax::highlight(self.field.text());
            self.has_stale_colours = false;
        }
    }

    /// Applies one editing key.
    pub(super) fn edit(&mut self, key: KeyEvent) -> Edited {
        self.change(|field| apply(field, key))
    }

    /// Inserts `text` at the cursor as vtui cleans a paste: its line ends as `\n`, its tabs as spaces, without other
    /// control characters or marks that reverse text.
    pub(super) fn paste(&mut self, text: &str) -> Edited {
        self.change(|field| field.paste(text))
    }

    /// Puts `insert` in place of the bytes `typed`, which end at the cursor.
    pub(super) fn replace_typed(&mut self, typed: Range<usize>, insert: &str) -> Edited {
        let typed_chars = self.field.text()[typed].chars().count();
        self.change(|field| {
            (0..typed_chars).for_each(|_| field.backspace());
            field.insert_str(insert);
        })
    }

    fn change(&mut self, edit: impl FnOnce(&mut Field)) -> Edited {
        let revision = self.field.revision();
        edit(&mut self.field);
        let edited = if self.field.revision() == revision { Edited::Cursor } else { Edited::Text };
        self.has_stale_colours |= edited == Edited::Text;
        edited
    }
}

/// Applies `key` to `field` as the editor does: a key that types, deletes or moves the cursor.
pub(super) fn apply(field: &mut Field, key: KeyEvent) {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Enter => field.insert('\n'),
        KeyCode::Char('w') if control => field.delete_word(),
        KeyCode::Char(c) if !control => field.insert(c),
        KeyCode::Backspace if alt => field.delete_word(),
        KeyCode::Backspace => field.backspace(),
        KeyCode::Delete => field.delete(),
        KeyCode::Left if alt => field.word_left(),
        KeyCode::Right if alt => field.word_right(),
        KeyCode::Left => field.left(),
        KeyCode::Right => field.right(),
        KeyCode::Up => field.up(),
        KeyCode::Down => field.down(),
        KeyCode::Home => field.start(),
        KeyCode::End => field.end(),
        _ => {}
    }
}

impl Sheet {
    /// Applies `key` to the editor: `→` takes the ghost when one shows, and a name key asks for the next one.
    pub(super) fn edit(&mut self, key: KeyEvent) -> Vec<Action> {
        if let Some(ghost) = self.ghost.take()
            && is_take(key)
        {
            self.take_ghost(ghost);
            return vec![];
        }
        if self.editor.edit(key) != Edited::Text {
            return vec![];
        }
        self.text_changed();
        if is_name_key(key) { self.ask_ghost() } else { vec![] }
    }

    /// Pastes `text` into the editor as one change: a line end in it never runs a statement.
    pub(super) fn paste(&mut self, text: &str) {
        if self.editor.paste(text) == Edited::Text {
            self.text_changed();
        }
    }

    /// Reads the header of the new text, and drops the ghost, the error mark and the place of the statement sent, which
    /// pointed into the old text; the colours follow on their own.
    pub(super) fn text_changed(&mut self) {
        self.ghost = None;
        self.header = Header::parse(self.editor.text());
        self.is_dirty = true;
        self.error_mark = None;
        self.forget_sent_place();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::app;
    use super::*;
    use crate::syntax::{Painted, Token};

    fn shown(editor: &Editor) -> String {
        let (before, under, after) = editor.split();
        format!("{before}|{under}{after}")
    }

    fn press(text: &str, keys: &[KeyEvent]) -> String {
        let mut app = app();
        let sheet = app.sheet_mut();
        sheet.editor = Editor::new(text);
        for &key in keys {
            sheet.edit(key);
        }
        shown(&sheet.editor)
    }

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn pasted(text: &str, paste: &str) -> Editor {
        let mut editor = Editor::new(text);
        editor.paste(paste);
        editor
    }

    #[test]
    fn a_modified_enter_breaks_the_line() {
        assert_eq!(press("SELECT *", &[with(KeyCode::Enter, KeyModifiers::ALT)]), "SELECT *\n|");
        assert_eq!(press("SELECT *", &[with(KeyCode::Enter, KeyModifiers::SHIFT)]), "SELECT *\n|");
    }

    #[test]
    fn up_goes_to_the_line_above() {
        assert_eq!(press("SELECT *\nFROM orders", &[KeyEvent::from(KeyCode::Up)]), "SELECT *|\nFROM orders");
    }

    #[test]
    fn ctrl_w_and_alt_backspace_delete_the_word_before_the_cursor() {
        assert_eq!(press("FROM orders", &[with(KeyCode::Char('w'), KeyModifiers::CONTROL)]), "FROM |");
        assert_eq!(press("FROM orders", &[with(KeyCode::Backspace, KeyModifiers::ALT)]), "FROM |");
    }

    #[test]
    fn a_control_key_types_nothing() {
        assert_eq!(press("FROM", &[with(KeyCode::Char('x'), KeyModifiers::CONTROL)]), "FROM|");
    }

    #[test]
    fn a_paste_lands_at_the_cursor_with_its_line_ends() {
        assert_eq!(shown(&pasted("SELECT ", "*\r\nFROM orders\rWHERE id = 7")), "SELECT *\nFROM orders\nWHERE id = 7|");
    }

    #[test]
    fn a_pasted_tab_becomes_spaces_and_other_control_characters_are_dropped() {
        assert_eq!(shown(&pasted("", "\tid\u{1b}[2J")), "    id[2J|");
    }

    #[test]
    fn a_paste_drops_the_marks_that_reverse_text() {
        assert_eq!(shown(&pasted("", "id\u{202e}latot\u{2066}")), "idlatot|");
    }

    #[test]
    fn a_new_editor_waits_for_its_colours() {
        let mut editor = Editor::new("SELECT");
        assert!(editor.has_stale_colours());
        editor.recolour();
        assert_eq!(editor.colours(), [vec![Painted { bytes: 0..6, token: Token::Keyword }]]);
    }

    #[test]
    fn a_change_keeps_the_old_colours_until_they_are_rebuilt() {
        let mut editor = pasted("SELECT", " 1");
        assert!(editor.has_stale_colours());
        editor.recolour();
        assert!(!editor.has_stale_colours());
        assert_eq!(editor.colours()[0].last(), Some(&Painted { bytes: 7..8, token: Token::Number }));
    }

    #[test]
    fn a_paste_of_nothing_printable_leaves_the_colours_fresh() {
        let mut editor = Editor::new("SELECT 1");
        editor.recolour();
        assert_eq!(editor.paste("\u{1b}\u{202e}"), Edited::Cursor);
        assert!(!editor.has_stale_colours());
    }

    #[test]
    fn a_cursor_move_leaves_the_colours_fresh() {
        let mut editor = Editor::new("SELECT 1");
        editor.recolour();
        editor.edit(KeyEvent::from(KeyCode::Left));
        assert!(!editor.has_stale_colours());
    }
}
