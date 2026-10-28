//! The editor's undo and redo: the text and cursor before each step, and those a step undone left.
//! Typed characters join one step until a word starts, the cursor moves or the user pauses; any other change is a step
//! of its own.
use std::collections::VecDeque;
use vtui::field::Field;

/// How many steps undo goes back: each keeps a whole copy of the text, so a 100 kB query costs at most about 10 MB.
const STEPS_KEPT: usize = 100;

/// What a change to the editor's text was, which says whether it joins the step before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Change {
    /// One character typed at the cursor, a line end included.
    Typed(char),
    /// A paste, a name or a suggestion taken, or a delete: always a step of its own.
    Whole,
}

#[derive(Debug, Default)]
pub(super) struct Undo {
    /// The editor before each step, the latest last.
    done: VecDeque<Field>,
    /// The editor before each step undone, the latest last, until a new change drops them.
    undone: Vec<Field>,
    /// The last character typed while its step still takes more.
    typing: Option<char>,
}

impl Undo {
    /// Notes a change of the editor from `before`; a typed character joins the typing step before it, unless it starts a
    /// word.
    pub(super) fn changed(&mut self, before: Field, change: Change) {
        self.undone.clear();
        let joins = matches!((self.typing, change), (Some(last), Change::Typed(typed)) if !starts_word(last, typed));
        self.typing = match change {
            Change::Typed(typed) => Some(typed),
            Change::Whole => None,
        };
        if joins {
            return;
        }
        self.done.push_back(before);
        if self.done.len() > STEPS_KEPT {
            self.done.pop_front();
        }
    }

    /// Ends the typing step, so the next character typed starts another: the cursor moved, or the user paused.
    pub(super) fn end_typing(&mut self) {
        self.typing = None;
    }

    /// Whether a typing step still takes more characters.
    pub(super) fn is_typing(&self) -> bool {
        self.typing.is_some()
    }

    /// The editor before the last step, taking `now` to redo it; `None` when nothing is left to undo.
    pub(super) fn undo(&mut self, now: &Field) -> Option<Field> {
        let before = self.done.pop_back()?;
        self.undone.push(now.clone());
        self.typing = None;
        Some(before)
    }

    /// The editor after the last step undone, taking `now` to undo it again; `None` when nothing was undone.
    pub(super) fn redo(&mut self, now: &Field) -> Option<Field> {
        let after = self.undone.pop()?;
        self.done.push_back(now.clone());
        self.typing = None;
        Some(after)
    }
}

/// Whether typing `typed` after `last` starts a new word: a character that is not a space after one that is.
fn starts_word(last: char, typed: char) -> bool {
    last.is_whitespace() && !typed.is_whitespace()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(undo: &mut Undo, field: &mut Field, text: &str) {
        for c in text.chars() {
            let before = field.clone();
            field.insert(c);
            undo.changed(before, Change::Typed(c));
        }
    }

    fn undone(undo: &mut Undo, field: &Field) -> String {
        undo.undo(field).map(|field| field.text().to_owned()).unwrap_or_default()
    }

    #[test]
    fn undo_takes_back_the_last_word_typed_with_its_space() {
        let (mut undo, mut field) = (Undo::default(), Field::default());
        typed(&mut undo, &mut field, "SELECT id");
        assert_eq!(undone(&mut undo, &field), "SELECT ");
    }

    #[test]
    fn a_cursor_move_starts_another_typing_step() {
        let (mut undo, mut field) = (Undo::default(), Field::default());
        typed(&mut undo, &mut field, "SEL");
        undo.end_typing();
        typed(&mut undo, &mut field, "ECT");
        assert_eq!(undone(&mut undo, &field), "SEL");
    }

    #[test]
    fn a_whole_change_is_a_step_of_its_own_between_typed_characters() {
        let (mut undo, mut field) = (Undo::default(), Field::default());
        typed(&mut undo, &mut field, "SEL");
        let before = field.clone();
        field.paste("ECT");
        undo.changed(before, Change::Whole);
        typed(&mut undo, &mut field, "!");
        assert_eq!(undone(&mut undo, &field), "SELECT");
        assert_eq!(undone(&mut undo, &field), "SEL");
    }

    #[test]
    fn redo_puts_back_what_undo_took() {
        let (mut undo, mut field) = (Undo::default(), Field::default());
        typed(&mut undo, &mut field, "SELECT");
        let before = undo.undo(&field).unwrap();
        assert_eq!(undo.redo(&before).map(|field| field.text().to_owned()).as_deref(), Some("SELECT"));
    }

    #[test]
    fn a_change_after_an_undo_drops_the_redo() {
        let (mut undo, mut field) = (Undo::default(), Field::default());
        typed(&mut undo, &mut field, "SELECT");
        let mut field = undo.undo(&field).unwrap();
        typed(&mut undo, &mut field, "x");
        assert_eq!(undo.redo(&field), None);
    }

    #[test]
    fn undo_goes_back_no_further_than_the_steps_kept() {
        let (mut undo, mut field) = (Undo::default(), Field::default());
        for _ in 0..=STEPS_KEPT {
            typed(&mut undo, &mut field, " x");
        }
        let steps = std::iter::from_fn(|| undo.undo(&field)).count();
        assert_eq!(steps, STEPS_KEPT);
    }
}
