//! The SQL editor: its text, the editing keys and pastes that change it, their undo, and the syntax colours that follow it.
use super::undo::{Change, Undo};
use super::wrap::{self, InView, Row};
use super::{Action, App, Focus, Sheet};
use crate::db::lex::{Lexed, Token};
use crate::query::Header;
use crate::render;
use crate::syntax::{self, Spans};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Size;
use std::ops::Range;
use vtui::click::Click;
use vtui::field::{Field, Place};

/// Whether an edit changed the text, or only moved the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edited {
    Text,
    Cursor,
}

/// How many times the editor's text changed; an answer asked about one revision fits no other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct Revision(u64);

/// The editor's text, its tokens, its colours, its undo and the first row in view.
/// The tokens are updated on each change; the colours are rebuilt by `recolour` after the text changes.
#[derive(Debug, Default)]
pub(in crate::tui) struct Editor {
    field: Field,
    revision: Revision,
    undo: Undo,
    lexed: Lexed,
    colours: Vec<Spans>,
    has_stale_colours: bool,
    /// The first row the last frame drew: it moves only when the cursor leaves the view, so a click never scrolls.
    first_row: Row,
    /// How wide the last frame drew the text, which `↑` `↓` and the mouse read rows by; 0 before the first frame.
    columns: usize,
    reveal: Option<Reveal>,
}

/// A byte kept in view in place of the cursor, as long as the cursor stays at byte `cursor`.
#[derive(Clone, Copy, Debug)]
struct Reveal {
    at: usize,
    cursor: usize,
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

    /// The bytes a drag or a double or triple click selected; `None` when nothing is.
    pub(in crate::tui) fn selection(&self) -> Option<Range<usize>> {
        self.field.selection()
    }

    /// The text a drag or a double or triple click selected.
    pub(super) fn selected(&self) -> Option<&str> {
        self.selection().map(|range| &self.field.text()[range])
    }

    pub(super) fn deselect(&mut self) {
        self.field.deselect();
    }

    /// Puts the cursor at `place`, where the mouse pressed; a double click selects the SQL word there, a triple its line.
    pub(super) fn click(&mut self, place: Place, click: Click) {
        self.undo.end_typing();
        self.field.move_to(place);
        match click {
            Click::Single => {}
            Click::Double => self.select_sql_word(),
            Click::Triple => self.field.select_line(),
        }
    }

    /// Puts the cursor at byte `at`, as a click there would.
    pub(super) fn move_cursor_to(&mut self, at: usize) {
        self.click(place_of(self.field.text(), at), Click::Single);
    }

    /// Selects the letters, digits and `_` around the cursor, the cursor at their end; nothing when there are none.
    fn select_sql_word(&mut self) {
        let text = self.field.text();
        let word = sql_word_around(text, self.split().0.len());
        if word.is_empty() {
            return;
        }
        let (start, end) = (place_of(text, word.start), place_of(text, word.end));
        self.field.move_to(start);
        self.field.select_to(end);
    }

    /// Selects from where the mouse pressed, or the selection started, to `place`.
    pub(super) fn drag_to(&mut self, place: Place) {
        self.field.select_to(place);
    }

    /// The first row in view in a pane of `size`: the one the last frame drew, moved only as far as the byte being
    /// revealed, or else the cursor, needs to show.
    pub(in crate::tui) fn first_row_in_view(&self, size: Size) -> Row {
        let at = self.live_reveal().map_or_else(|| self.split().0.len(), |reveal| reveal.at);
        wrap::first_row_for(self.text(), self.first_row, at, size)
    }

    /// The cell of the cursor in a pane of `size`; `None` when a reveal keeps it out of view.
    pub(in crate::tui) fn cursor_in_view(&self, size: Size) -> Option<InView> {
        wrap::in_view(self.text(), self.first_row_in_view(size), self.split().0.len(), size)
    }

    /// Scrolls to byte `at` and leaves the cursor where it is, until the cursor moves.
    pub(super) fn reveal(&mut self, at: usize) {
        self.reveal = Some(Reveal { at, cursor: self.split().0.len() });
    }

    /// The reveal in force: a cursor that moved ended it.
    fn live_reveal(&self) -> Option<Reveal> {
        self.reveal.filter(|reveal| reveal.cursor == self.split().0.len())
    }

    pub(super) fn stop_revealing(&mut self) {
        self.reveal = None;
    }

    /// Keeps the first row and the width a frame drew in a pane of `size`, which the next frame and the mouse start from;
    /// a pane not drawn keeps those it had.
    pub(super) fn keep_in_view(&mut self, size: Size) {
        self.reveal = self.live_reveal();
        if size.height > 0 {
            self.first_row = self.first_row_in_view(size);
            self.columns = usize::from(size.width);
        }
    }

    /// The first row the last frame drew, under the pane's first row.
    #[cfg(test)]
    pub(super) const fn first_row(&self) -> Row {
        self.first_row
    }

    /// The place in the text the last frame drew `down` rows under the pane's first, at `column`; below the last row, the
    /// text's end.
    pub(super) fn place_drawn_at(&self, down: usize, column: usize) -> Place {
        let at = wrap::byte_under(self.text(), self.first_row, down, column, self.columns);
        place_of(self.text(), at.unwrap_or(self.text().len()))
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

    /// Applies `key`; the text is copied for undo only when the key may change it, never for a move.
    pub(super) fn edit(&mut self, key: KeyEvent) -> Edited {
        if self.move_to_row(key) || move_cursor(&mut self.field, key) {
            self.undo.end_typing();
            return Edited::Cursor;
        }
        self.change(change_of(key), |field| edit_text(field, key))
    }

    /// Moves the cursor to the row drawn above or below it for `↑` `↓`, so a long line can be walked; false for any other
    /// key.
    fn move_to_row(&mut self, key: KeyEvent) -> bool {
        let at = self.split().0.len();
        let target = match key.code {
            KeyCode::Up => wrap::byte_above(self.text(), at, self.columns),
            KeyCode::Down => wrap::byte_below(self.text(), at, self.columns),
            _ => return false,
        };
        self.field.move_to(place_of(self.text(), target));
        true
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
        self.reveal = None;
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

/// Applies `key` to `field`, one line of text, as the editor does: keys that type, delete or move the cursor, those of a
/// shell line included, but `⏎` with any modifier and `↑` `↓` do nothing; `⌥b` `⌥f` are what a macOS terminal sends for
/// `⌥←` `⌥→` unless its option key is set as meta.
pub(super) fn edit_line(field: &mut Field, key: KeyEvent) {
    if key.code == KeyCode::Enter {
        return;
    }
    if !move_cursor(field, key) {
        edit_text(field, key);
    }
}

/// Moves the cursor of `field` as `key` says; false when `key` is not a move, and then `field` is as it was.
fn move_cursor(field: &mut Field, key: KeyEvent) -> bool {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('a') if control => field.line_start(),
        KeyCode::Char('e') if control => field.line_end(),
        KeyCode::Char('b') if alt => field.word_left(),
        KeyCode::Char('f') if alt => field.word_right(),
        KeyCode::Left if alt => field.word_left(),
        KeyCode::Right if alt => field.word_right(),
        KeyCode::Left => field.left(),
        KeyCode::Right => field.right(),
        KeyCode::Home => field.start(),
        KeyCode::End => field.end(),
        _ => return false,
    }
    true
}

/// Applies a `key` that is not a move: it types or deletes, or does nothing.
fn edit_text(field: &mut Field, key: KeyEvent) {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Enter => field.insert('\n'),
        KeyCode::Char('w') if control => field.delete_word(),
        KeyCode::Char('k') if control => field.delete_to_line_end(),
        KeyCode::Char('u') if control => field.delete_to_line_start(),
        KeyCode::Char(c) if !control => field.insert(c),
        KeyCode::Backspace if alt => field.delete_word(),
        KeyCode::Backspace => field.backspace(),
        KeyCode::Delete => field.delete(),
        _ => {}
    }
}

/// `held` then `statement` after a blank line; the last statement of `held` gets a `;` right after its last token,
/// before any comment, so `⏎` on `statement` runs it alone.
pub(super) fn below(held: &str, statement: &str) -> String {
    let held = held.trim_end();
    match Lexed::of(held).tokens().last() {
        None if held.is_empty() => statement.to_owned(),
        Some(last) if last.token != Token::End => format!("{};{}\n\n{statement}", &held[..last.range.end], &held[last.range.end..]),
        _ => format!("{held}\n\n{statement}"),
    }
}

impl App {
    /// Adds `statement` at the end of the editor after a blank line, with the cursor on it, as [`below`] writes it.
    pub(super) fn put_in_editor(&mut self, statement: &str) {
        let text = below(self.sheet().editor.text(), statement);
        self.replace_editor_text(&text);
    }

    /// Puts `text` in the editor in place of its own, unsaved and undoable, and gives it the keys.
    pub(super) fn replace_editor_text(&mut self, text: &str) {
        let sheet = self.sheet_mut();
        sheet.editor.rewrite(text);
        sheet.text_changed();
        self.focus = Focus::Editor;
    }
}

impl Sheet {
    /// Applies `key` to the editor, and offers the next ghost when it changed the text.
    pub(super) fn edit(&mut self, key: KeyEvent) -> Vec<Action> {
        if self.editor.edit(key) != Edited::Text {
            return vec![];
        }
        self.text_changed();
        self.offer_ghost(key)
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

    /// Reads the header of the new text, and drops the ghost, the rewrite, the error mark, its code and the position of the
    /// statement sent, which all pointed into the old text; the colours are rebuilt later by `recolour`.
    pub(super) fn text_changed(&mut self) {
        self.ghost = None;
        self.rewrite = None;
        self.header = Header::parse(self.editor.text());
        self.is_dirty = true;
        self.error_mark = None;
        self.error_code = None;
        self.forget_sent_place();
    }
}

fn is_sql_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The bytes of the run of SQL word characters that holds `at` or ends there.
fn sql_word_around(text: &str, at: usize) -> Range<usize> {
    let start = text[..at].char_indices().rfind(|&(_, c)| !is_sql_word_char(c)).map_or(0, |(i, c)| i + c.len_utf8());
    let end = text[at..].find(|c: char| !is_sql_word_char(c)).map_or(text.len(), |i| at + i);
    start..end
}

/// Where byte `offset` of `text` is, as `Field` reads a place: its line, and the columns of the characters before it on
/// that line, each measured alone, a control character as none.
fn place_of(text: &str, offset: usize) -> Place {
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let printable = before[line_start..].chars().filter(|c| !c.is_control());
    Place { line: before.matches('\n').count(), column: printable.map(|c| render::width(c.encode_utf8(&mut [0; 4]))).sum() }
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

    fn double_clicked(text: &str, place: Place) -> Option<String> {
        let mut editor = Editor::new(text);
        editor.click(place, Click::Double);
        editor.selected().map(str::to_owned)
    }

    #[test]
    fn a_double_click_selects_the_sql_word_and_stops_at_punctuation() {
        assert_eq!(double_clicked("SELECT count(o.total_2) FROM orders o", Place { line: 0, column: 15 }).as_deref(), Some("total_2"));
    }

    #[test]
    fn a_double_click_finds_its_word_past_wide_characters_on_a_later_line() {
        assert_eq!(double_clicked("SELECT 1\n'日本',orders", Place { line: 1, column: 8 }).as_deref(), Some("orders"));
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

    /// `text` drawn `columns` wide and two rows high, the cursor at its end, then `keys`.
    fn drawn_then_pressed(text: &str, columns: u16, keys: &[KeyCode]) -> Editor {
        let mut editor = Editor::new(text);
        let size = Size::new(columns, 2);
        editor.keep_in_view(size);
        for &key in keys {
            editor.edit(KeyEvent::from(key));
            editor.keep_in_view(size);
        }
        editor
    }

    #[test]
    fn up_goes_to_the_row_above_on_a_wrapped_line_and_changes_no_text() {
        let editor = drawn_then_pressed("customers", 4, &[KeyCode::Up]);
        assert_eq!(shown(&editor), "custo|mers");
    }

    #[test]
    fn down_goes_to_the_row_below_on_a_wrapped_line() {
        assert_eq!(shown(&drawn_then_pressed("customers", 4, &[KeyCode::Up, KeyCode::Up, KeyCode::Down])), "custo|mers");
    }

    #[test]
    fn up_past_the_end_of_a_shorter_row_lands_on_the_whitespace_at_the_break() {
        assert_eq!(shown(&drawn_then_pressed("FROM orders", 8, &[KeyCode::Up])), "FROM| orders");
    }

    #[test]
    fn down_from_the_whitespace_at_a_break_goes_to_the_cell_below_and_changes_no_text() {
        let editor = drawn_then_pressed("FROM orders", 8, &[KeyCode::Up, KeyCode::Down]);
        assert_eq!(shown(&editor), "FROM orde|rs");
        assert_eq!(editor.text(), "FROM orders");
    }

    #[test]
    fn up_and_down_are_moves_that_leave_the_colours_fresh() {
        let mut editor = Editor::new("customers\nid");
        editor.recolour();
        assert_eq!(editor.edit(KeyEvent::from(KeyCode::Up)), Edited::Cursor);
        assert_eq!(editor.edit(KeyEvent::from(KeyCode::Down)), Edited::Cursor);
        assert!(!editor.has_stale_colours());
    }

    #[test]
    fn up_finds_its_place_past_a_tab_and_a_wide_character() {
        assert_eq!(shown(&drawn_then_pressed("\t日本語\nid", 8, &[KeyCode::Up])), "\t日|本語\nid");
    }

    #[test]
    fn going_up_past_the_view_scrolls_a_long_line_by_one_row() {
        let long = "orders ".repeat(10);
        assert_eq!(drawn_then_pressed(&long, 10, &[]).first_row(), Row { line: 0, row: 8 });
        assert_eq!(drawn_then_pressed(&long, 10, &[KeyCode::Up]).first_row(), Row { line: 0, row: 8 });
        assert_eq!(drawn_then_pressed(&long, 10, &[KeyCode::Up, KeyCode::Up]).first_row(), Row { line: 0, row: 7 });
    }

    #[test]
    fn home_scrolls_a_long_line_back_to_its_first_row() {
        assert_eq!(drawn_then_pressed(&"orders ".repeat(10), 10, &[KeyCode::Home]).first_row(), Row::default());
    }

    #[test]
    fn a_reveal_scrolls_back_to_the_row_of_its_byte() {
        let mut editor = drawn_then_pressed("customers", 4, &[]);
        editor.reveal(0);
        assert_eq!(editor.first_row_in_view(Size::new(4, 2)), Row::default());
    }

    #[test]
    fn ctrl_w_and_alt_backspace_delete_the_word_before_the_cursor() {
        assert_eq!(press("FROM orders", &[with(KeyCode::Char('w'), KeyModifiers::CONTROL)]), "FROM |");
        assert_eq!(press("FROM orders", &[with(KeyCode::Backspace, KeyModifiers::ALT)]), "FROM |");
    }

    #[test]
    fn alt_b_and_alt_f_move_by_word_as_alt_and_an_arrow_do() {
        assert_eq!(press("FROM orders", &[with(KeyCode::Char('b'), KeyModifiers::ALT)]), "FROM |orders");
        assert_eq!(
            press("FROM orders", &[with(KeyCode::Home, KeyModifiers::NONE), with(KeyCode::Char('f'), KeyModifiers::ALT)]),
            "FROM| orders"
        );
    }

    #[test]
    fn ctrl_k_deletes_to_the_end_of_the_line() {
        let keys = [KeyEvent::from(KeyCode::Home), with(KeyCode::Char('k'), KeyModifiers::CONTROL)];
        assert_eq!(press("SELECT id\nFROM orders", &keys), "|\nFROM orders");
    }

    #[test]
    fn ctrl_u_deletes_to_the_start_of_the_line() {
        assert_eq!(press("SELECT id\nFROM orders", &[with(KeyCode::Char('u'), KeyModifiers::CONTROL)]), "SELECT id\n|");
    }

    #[test]
    fn ctrl_a_and_ctrl_e_go_to_the_ends_of_the_line_in_the_editor() {
        let mut app = typed("SELECT id\nFROM orders");
        control(&mut app, 'a');
        assert_eq!(shown(&app.sheet().editor), "SELECT id\n|FROM orders");
        control(&mut app, 'e');
        assert_eq!(shown(&app.sheet().editor), "SELECT id\nFROM orders|");
    }

    #[test]
    fn each_key_that_only_moves_the_cursor_is_a_move() {
        let (alt, control) = (KeyModifiers::ALT, KeyModifiers::CONTROL);
        let moves = [
            KeyEvent::from(KeyCode::Left),
            KeyEvent::from(KeyCode::Right),
            KeyEvent::from(KeyCode::Home),
            KeyEvent::from(KeyCode::End),
            with(KeyCode::Left, alt),
            with(KeyCode::Right, alt),
            with(KeyCode::Char('b'), alt),
            with(KeyCode::Char('f'), alt),
            with(KeyCode::Char('a'), control),
            with(KeyCode::Char('e'), control),
        ];
        for key in moves {
            let mut field = Field::new("SELECT id\nFROM orders");
            assert!(move_cursor(&mut field, key), "{key:?}");
            assert_eq!(field.text(), "SELECT id\nFROM orders", "{key:?}");
        }
    }

    #[test]
    fn a_key_that_types_or_deletes_is_not_a_move() {
        let (alt, control) = (KeyModifiers::ALT, KeyModifiers::CONTROL);
        let edits = [
            KeyEvent::from(KeyCode::Char('x')),
            KeyEvent::from(KeyCode::Enter),
            KeyEvent::from(KeyCode::Backspace),
            KeyEvent::from(KeyCode::Delete),
            with(KeyCode::Backspace, alt),
            with(KeyCode::Char('w'), control),
            with(KeyCode::Char('k'), control),
            with(KeyCode::Char('u'), control),
        ];
        for key in edits {
            assert!(!move_cursor(&mut Field::new("SELECT id"), key), "{key:?}");
        }
    }

    #[test]
    fn a_control_key_types_nothing() {
        assert_eq!(press("FROM", &[with(KeyCode::Char('x'), KeyModifiers::CONTROL)]), "FROM|");
    }

    fn line_after(text: &str, keys: &[KeyEvent]) -> String {
        let mut editor = Editor::new(text);
        for &key in keys {
            edit_line(&mut editor.field, key);
        }
        shown(&editor)
    }

    #[test]
    fn alt_b_in_a_line_moves_a_word_and_types_nothing() {
        assert_eq!(line_after("late orders", &[with(KeyCode::Char('b'), KeyModifiers::ALT)]), "late |orders");
    }

    #[test]
    fn alt_f_in_a_line_moves_a_word_and_types_nothing() {
        let keys = [KeyEvent::from(KeyCode::Home), with(KeyCode::Char('f'), KeyModifiers::ALT)];
        assert_eq!(line_after("late orders", &keys), "late| orders");
    }

    #[test]
    fn alt_and_an_arrow_in_a_line_move_a_word() {
        let keys =
            [with(KeyCode::Left, KeyModifiers::ALT), with(KeyCode::Left, KeyModifiers::ALT), with(KeyCode::Right, KeyModifiers::ALT)];
        assert_eq!(line_after("late orders", &keys), "late| orders");
    }

    #[test]
    fn home_goes_to_the_start_of_a_line() {
        assert_eq!(line_after("orders", &[KeyEvent::from(KeyCode::Home)]), "|orders");
    }

    #[test]
    fn end_goes_to_the_end_of_a_line() {
        assert_eq!(line_after("orders", &[KeyEvent::from(KeyCode::Home), KeyEvent::from(KeyCode::End)]), "orders|");
    }

    #[test]
    fn ctrl_a_goes_to_the_start_of_a_line() {
        assert_eq!(line_after("orders", &[with(KeyCode::Char('a'), KeyModifiers::CONTROL)]), "|orders");
    }

    #[test]
    fn ctrl_e_goes_to_the_end_of_a_line() {
        let keys = [KeyEvent::from(KeyCode::Home), with(KeyCode::Char('e'), KeyModifiers::CONTROL)];
        assert_eq!(line_after("orders", &keys), "orders|");
    }

    #[test]
    fn delete_takes_the_character_under_the_cursor_of_a_line() {
        assert_eq!(line_after("orders", &[KeyEvent::from(KeyCode::Home), KeyEvent::from(KeyCode::Delete)]), "|rders");
    }

    #[test]
    fn ctrl_w_deletes_the_word_before_the_cursor_of_a_line() {
        assert_eq!(line_after("late orders", &[with(KeyCode::Char('w'), KeyModifiers::CONTROL)]), "late |");
    }

    #[test]
    fn alt_backspace_deletes_the_word_before_the_cursor_of_a_line() {
        assert_eq!(line_after("late orders", &[with(KeyCode::Backspace, KeyModifiers::ALT)]), "late |");
    }

    #[test]
    fn ctrl_u_deletes_to_the_start_of_a_line() {
        let keys = [KeyEvent::from(KeyCode::Left), with(KeyCode::Char('u'), KeyModifiers::CONTROL)];
        assert_eq!(line_after("orders", &keys), "|s");
    }

    #[test]
    fn ctrl_k_deletes_to_the_end_of_a_line() {
        let keys = [KeyEvent::from(KeyCode::Left), with(KeyCode::Char('k'), KeyModifiers::CONTROL)];
        assert_eq!(line_after("orders", &keys), "order|");
    }

    #[test]
    fn enter_with_any_modifier_adds_no_new_line_to_a_line() {
        for modifiers in [KeyModifiers::NONE, KeyModifiers::ALT, KeyModifiers::SHIFT, KeyModifiers::CONTROL] {
            assert_eq!(line_after("orders", &[with(KeyCode::Enter, modifiers)]), "orders|", "{modifiers:?}");
        }
    }

    #[test]
    fn up_does_nothing_in_a_line_a_paste_broke_in_two() {
        assert_eq!(line_after("orders\ncustomers", &[KeyEvent::from(KeyCode::Up)]), "orders\ncustomers|");
    }

    #[test]
    fn down_does_nothing_in_a_line_a_paste_broke_in_two() {
        let keys = [KeyEvent::from(KeyCode::Home), KeyEvent::from(KeyCode::Down)];
        assert_eq!(line_after("orders\ncustomers", &keys), "|orders\ncustomers");
    }

    #[test]
    fn an_option_character_other_than_b_and_f_types_in_a_line() {
        assert_eq!(line_after("order", &[with(KeyCode::Char('@'), KeyModifiers::ALT)]), "order@|");
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
    fn a_delete_to_the_end_of_the_line_is_one_step() {
        let mut app = typed("SELECT id, total");
        app.handle_key(KeyEvent::from(KeyCode::Home));
        control(&mut app, 'k');
        control(&mut app, 'z');
        assert_eq!(app.sheet().editor.text(), "SELECT id, total");
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
