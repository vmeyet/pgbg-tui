//! The editing keys of the SQL editor: what each one does to the text and its cursor.
use super::App;
use crate::syntax;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Applies `key` to the editor; a change to the text rebuilds its colours and drops the error mark and the place of
    /// the statement sent, which pointed into the old text.
    pub(super) fn edit(&mut self, key: KeyEvent) {
        let editor = &mut self.editor;
        // A key either inserts or deletes, never both, so the length says whether the text changed.
        let length_before = editor.text().len();
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => editor.insert('\n'),
            KeyCode::Char('w') if control => editor.delete_word(),
            KeyCode::Char(c) if !control => editor.insert(c),
            KeyCode::Backspace if alt => editor.delete_word(),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Left if alt => editor.word_left(),
            KeyCode::Right if alt => editor.word_right(),
            KeyCode::Left => editor.left(),
            KeyCode::Right => editor.right(),
            KeyCode::Up => editor.up(),
            KeyCode::Down => editor.down(),
            KeyCode::Home => editor.start(),
            KeyCode::End => editor.end(),
            _ => {}
        }
        if editor.text().len() != length_before {
            self.colours = syntax::highlight(self.editor.text());
            self.error_mark = None;
            self.forget_sent_place();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::app;
    use super::*;
    use vtui::field::Field;

    fn press(text: &str, keys: &[KeyEvent]) -> String {
        let mut app = app();
        app.editor = Field::new(text);
        for &key in keys {
            app.edit(key);
        }
        let (before, under, after) = app.editor.split();
        format!("{before}|{under}{after}")
    }

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
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
}
