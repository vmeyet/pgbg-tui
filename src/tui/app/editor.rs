//! The SQL editor: its text, the editing keys and pastes that change it, their undo, and the syntax colours that follow it.
use super::ghost::is_name_key;
use super::undo::{Change, Undo};
use super::{Action, Sheet};
use crate::db::lex::Lexed;
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

/// How many times the editor's text changed; an answer asked about one revision fits no other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct Revision(u64);

/// The editor's text, its tokens, its colours and its undo.
/// The tokens are updated on each change; the colours are rebuilt by `recolour` after the text changes.
#[derive(Debug, Default)]
pub(in crate::tui) struct Editor {
    field: Field,
    revision: Revision,
    undo: Undo,
    lexed: Lexed,
    colours: Vec<Spans>,
    has_stale_colours: bool,
}

impl Editor {
    /// An editor on `text`, not coloured yet: the loop colours it later, so opening a big query never waits on
    /// tree-sitter.
    pub(super) fn new(text: &str) -> Self {
        Self { field: Field::new(text), lexed: Lexed::of(text), has_stale_colours: true, ..Self::default() }
    }

    /// Puts `text` in place of the whole text, the cursor at its end, with nothing to undo.
    pub(super) fn reload(&mut self, text: &str) {
        *self = Self { revision: self.revision.next(), ..Self::new(text) };
    }

    pub(super) fn revision(&self) -> Revision {
        self.revision
    }

    pub(in crate::tui) fn text(&self) -> &str {
        self.field.text()
    }

    /// The lexed text that the guard, completion and the parameters read, as of the last change.
    pub(in crate::tui) fn lexed(&self) -> &Lexed {
        &self.lexed
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

    pub(super) fn recolour(&mut self) {
        if self.has_stale_colours {
            self.colours = syntax::highlight(self.field.text());
            self.has_stale_colours = false;
        }
    }

    pub(super) fn edit(&mut self, key: KeyEvent) -> Edited {
        self.change(change_of(key), |field| apply(field, key))
    }

    /// Inserts `text` at the cursor, cleaned as vtui cleans a paste: line ends become `\n`, tabs become spaces, and other
    /// control characters and marks that reverse text are removed.
    pub(super) fn paste(&mut self, text: &str) -> Edited {
        self.change(Change::Whole, |field| field.paste(text))
    }

    /// Puts `insert` in place of the bytes `typed`, which end at the cursor.
    pub(super) fn replace_typed(&mut self, typed: Range<usize>, insert: &str) -> Edited {
        let typed_chars = self.field.text()[typed].chars().count();
        self.change(Change::Whole, |field| {
            (0..typed_chars).for_each(|_| field.backspace());
            field.insert_str(insert);
        })
    }

    /// Puts `text` in place of the bytes `range` as one paste, cleaned as [`Editor::paste`] cleans it.
    pub(super) fn replace(&mut self, range: Range<usize>, text: &str) -> Edited {
        let mut field = Field::joined(&self.field.text()[..range.start], &self.field.text()[range.end..]);
        field.paste(text);
        self.put(field);
        Edited::Text
    }

    /// Puts `text` in place of the whole text as one step, the cursor at its end.
    pub(super) fn rewrite(&mut self, text: &str) {
        self.put(Field::new(text));
    }

    /// Takes the text back to before the last step; `Edited::Cursor` when nothing is left to undo.
    pub(super) fn undo(&mut self) -> Edited {
        let undone = self.undo.undo(&self.field);
        self.restore(undone)
    }

    /// Puts back the last step undone; `Edited::Cursor` when nothing was undone.
    pub(super) fn redo(&mut self) -> Edited {
        let redone = self.undo.redo(&self.field);
        self.restore(redone)
    }

    /// Ends the step of the characters typed so far, as when the user pauses.
    pub(super) fn end_typing(&mut self) {
        self.undo.end_typing();
    }

    /// Whether characters typed now would join the last step.
    pub(super) fn is_typing(&self) -> bool {
        self.undo.is_typing()
    }

    fn change(&mut self, change: Change, edit: impl FnOnce(&mut Field)) -> Edited {
        let before = self.field.clone();
        edit(&mut self.field);
        if self.field.revision() == before.revision() {
            self.undo.end_typing();
            return Edited::Cursor;
        }
        self.undo.changed(before, change);
        self.text_changed();
        Edited::Text
    }

    fn put(&mut self, field: Field) {
        let before = std::mem::replace(&mut self.field, field);
        self.undo.changed(before, Change::Whole);
        self.text_changed();
    }

    fn restore(&mut self, field: Option<Field>) -> Edited {
        let Some(field) = field else { return Edited::Cursor };
        self.field = field;
        self.text_changed();
        Edited::Text
    }

    fn text_changed(&mut self) {
        self.revision = self.revision.next();
        self.lexed = Lexed::of(self.field.text());
        self.has_stale_colours = true;
    }
}

impl Revision {
    const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// What `key` does to the text, for undo: a typed character, or a delete that is a step of its own.
fn change_of(key: KeyEvent) -> Change {
    match key.code {
        KeyCode::Enter => Change::Typed('\n'),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => Change::Typed(c),
        _ => Change::Whole,
    }
}

/// Applies `key` to `field` as the editor does: keys that type, delete or move the cursor.
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
    /// Applies `key` to the editor; a key that types a name asks for the next ghost.
    pub(super) fn edit(&mut self, key: KeyEvent) -> Vec<Action> {
        if self.editor.edit(key) != Edited::Text {
            return vec![];
        }
        self.text_changed();
        if is_name_key(key) { self.ask_ghost() } else { vec![] }
    }

    /// Takes the editor's text back to before its last step.
    pub(super) fn undo(&mut self) {
        if self.editor.undo() == Edited::Text {
            self.text_changed();
        }
    }

    /// Puts back the editor's last step undone.
    pub(super) fn redo(&mut self) {
        if self.editor.redo() == Edited::Text {
            self.text_changed();
        }
    }

    /// Pastes `text` into the editor as one change: a line end in it never runs a statement.
    pub(super) fn paste(&mut self, text: &str) {
        if self.editor.paste(text) == Edited::Text {
            self.text_changed();
        }
    }

    /// Reads the header of the new text, and drops the ghost, the error mark and the position of the statement sent,
    /// which all pointed into the old text; the colours are rebuilt later by `recolour`.
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
    use super::super::fixtures::{app, typed};
    use super::super::queries::tests::query;
    use super::super::{Answer, App, QueryAnswer};
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

    fn control(app: &mut App, c: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    fn type_in(app: &mut App, text: &str) {
        text.chars().for_each(|c| _ = app.handle_key(KeyEvent::from(KeyCode::Char(c))));
    }

    #[test]
    fn ctrl_z_takes_back_the_last_word_typed() {
        let mut app = typed("SELECT id");
        control(&mut app, 'z');
        assert_eq!(app.sheet().editor.text(), "SELECT ");
    }

    #[test]
    fn ctrl_y_puts_back_what_ctrl_z_took() {
        let mut app = typed("SELECT id");
        control(&mut app, 'z');
        control(&mut app, 'y');
        assert_eq!(app.sheet().editor.text(), "SELECT id");
    }

    #[test]
    fn ctrl_shift_z_redoes_as_ctrl_y_does() {
        let mut app = typed("SELECT id");
        control(&mut app, 'z');
        app.handle_key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT));
        assert_eq!(app.sheet().editor.text(), "SELECT id");
    }

    #[test]
    fn a_paste_is_one_step() {
        let mut app = typed("SELECT ");
        app.paste("id, total");
        control(&mut app, 'z');
        assert_eq!(app.sheet().editor.text(), "SELECT ");
    }

    #[test]
    fn a_name_taken_is_one_step() {
        let mut app = typed("SELECT * FROM ord");
        app.sheet_mut().put_name(14..17, "orders");
        control(&mut app, 'z');
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM ord");
    }

    #[test]
    fn each_delete_is_one_step() {
        let mut app = typed("SELECT id");
        (0..2).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Backspace)));
        control(&mut app, 'z');
        assert_eq!(app.sheet().editor.text(), "SELECT i");
    }

    #[test]
    fn a_pause_ends_the_step_of_the_characters_typed() {
        let mut app = typed("SEL");
        app.sat_idle();
        type_in(&mut app, "ECT");
        control(&mut app, 'z');
        assert_eq!(app.sheet().editor.text(), "SEL");
    }

    #[test]
    fn a_cursor_move_ends_the_step_of_the_characters_typed() {
        let mut app = typed("SEL");
        app.handle_key(KeyEvent::from(KeyCode::Left));
        type_in(&mut app, "x");
        control(&mut app, 'z');
        assert_eq!(shown(&app.sheet().editor), "SE|L");
    }

    #[test]
    fn opening_a_query_leaves_nothing_to_undo() {
        let mut app = typed("SELECT 1");
        app.apply(Answer::Queries(QueryAnswer::Opened {
            connection: app.sheet().name().clone(),
            query: query("daily.sql"),
            text: "SELECT 2".into(),
        }));
        control(&mut app, 'z');
        assert_eq!((app.sheet().editor.text(), app.sheet().is_dirty), ("SELECT 2", false));
    }
}
