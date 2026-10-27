//! The `ctrl-space` list: the candidates for the name before the cursor, prefix and fuzzy, in a popup at the cursor.
use super::ghost::is_name_key;
use super::{Action, App, Overlay, Sheet};
use crate::act::{self, Act, Pane};
use crate::complete::{Ask, Candidate, Index};
use crossterm::event::{KeyCode, KeyEvent};
use std::ops::Range;
use vtui::toward::Toward;

/// The candidates for the name before the cursor, and the one picked; empty once typing has ruled them all out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Completions {
    /// The editor bytes the taken name replaces, up to the cursor.
    typed: Range<usize>,
    pub(in crate::tui) candidates: Vec<Candidate>,
    pub(in crate::tui) selected: usize,
}

impl Completions {
    fn stepped(self, toward: Toward) -> Self {
        if self.candidates.is_empty() {
            return self;
        }
        Self { selected: toward.wrapped(self.selected, 1, self.candidates.len()), ..self }
    }
}

/// What completion is asked for at the cursor, and the index that answers it.
pub(super) struct Asked<'a> {
    pub(super) ask: Ask,
    pub(super) index: &'a Index,
}

impl Sheet {
    /// What completion is asked for at the cursor, answered from the index once there is one, or else from the
    /// keywords; none when the cursor is in a string or a comment.
    pub(super) fn ask(&self) -> Option<Asked<'_>> {
        let (before, _, _) = self.editor.split();
        let ask = Ask::at(self.editor.text(), self.editor.lexed(), before.len())?;
        Some(Asked { ask, index: self.index.as_deref().unwrap_or_else(|| Index::of_keywords()) })
    }

    /// The list for the name before the cursor.
    fn completions(&self) -> Option<Completions> {
        let Asked { ask, index } = self.ask()?;
        Some(Completions { candidates: index.candidates(&ask), typed: ask.typed, selected: 0 })
    }
}

impl App {
    /// Opens the list for the name before the cursor, empty when nothing matches it.
    pub(super) fn open_completions(&mut self) -> Vec<Action> {
        let sheet = self.sheet_mut();
        let actions = sheet.want_schema_while_open();
        sheet.ghost = None;
        let completions = sheet.completions();
        self.overlay = completions.map(Overlay::Completion);
        actions
    }

    /// Hands `key` to the open list: `↑↓` pick, and `⏎` or `⇥` take the pick.
    /// On an empty list `⏎` and `⇥` do nothing, so a half-typed name never runs.
    /// `esc` closes; any other key goes to the editor, and a key that types a name or a backspace opens the list again.
    pub(super) fn completions_key(&mut self, completions: Completions, key: KeyEvent) -> Vec<Action> {
        match act::act_for(key, Pane::Completion) {
            Some(Act::Down) => self.overlay = Some(Overlay::Completion(completions.stepped(Toward::Next))),
            Some(Act::Up) => self.overlay = Some(Overlay::Completion(completions.stepped(Toward::Previous))),
            Some(Act::Open) => self.take_completion(completions),
            Some(Act::Quit) => return self.quit(),
            Some(Act::Close) => {}
            _ => return self.type_past_completions(key),
        }
        vec![]
    }

    fn take_completion(&mut self, completions: Completions) {
        let Completions { typed, candidates, selected } = completions;
        match candidates.get(selected) {
            Some(candidate) => self.sheet_mut().put_name(typed, &candidate.insert),
            None => self.overlay = Some(Overlay::Completion(Completions { typed, candidates, selected })),
        }
    }

    /// Closes the list and pastes `text` in the editor.
    pub(super) fn paste_past_completions(&mut self, text: &str) {
        self.overlay = None;
        self.sheet_mut().paste(text);
    }

    fn type_past_completions(&mut self, key: KeyEvent) -> Vec<Action> {
        let mut actions = self.handle_key(key);
        let is_narrowing = is_name_key(key) || key.code == KeyCode::Backspace;
        if is_narrowing && self.overlay.is_none() {
            actions.extend(self.open_completions());
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{to_shop, typed};
    use super::super::{Incoming, Overlay};
    use super::*;
    use crate::complete::Index;
    use crate::fixtures::shop_catalog;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::sync::Arc;

    fn listing(text: &str) -> App {
        let mut app = typed("");
        let catalog = Arc::new(shop_catalog());
        let index = Arc::new(Index::new(&catalog));
        app.apply(to_shop(Incoming::Schema(Ok(Arc::clone(&catalog)))));
        app.apply(to_shop(Incoming::Index { catalog, index }));
        for c in text.chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        app
    }

    fn listed(app: &App) -> Option<(Vec<&str>, usize)> {
        let Some(Overlay::Completion(completions)) = &app.overlay else { return None };
        let names = completions.candidates.iter().map(|candidate| candidate.name.as_str()).collect();
        Some((names, completions.selected))
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
    }

    #[test]
    fn ctrl_space_lists_prefix_matches_then_fuzzy_ones() {
        assert_eq!(listed(&listing("SELECT * FROM o")), Some((vec!["orders", "customers", "billing.invoices"], 0)));
    }

    #[test]
    fn down_and_up_pick_and_wrap() {
        let mut app = listing("SELECT * FROM o");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(listed(&app).map(|(_, selected)| selected), Some(2));
        press(&mut app, KeyCode::Down);
        assert_eq!(listed(&app).map(|(_, selected)| selected), Some(0));
        press(&mut app, KeyCode::Up);
        assert_eq!(listed(&app).map(|(_, selected)| selected), Some(2));
    }

    #[test]
    fn enter_or_tab_takes_the_pick_and_closes_the_list() {
        for take in [KeyCode::Enter, KeyCode::Tab] {
            let mut app = listing("SELECT * FROM o");
            press(&mut app, KeyCode::Down);
            press(&mut app, take);
            assert_eq!((app.sheet().editor.text(), listed(&app)), ("SELECT * FROM customers", None));
        }
    }

    #[test]
    fn esc_closes_the_list_and_keeps_the_text() {
        let mut app = listing("SELECT * FROM o");
        press(&mut app, KeyCode::Esc);
        assert_eq!((app.sheet().editor.text(), listed(&app)), ("SELECT * FROM o", None));
    }

    #[test]
    fn typing_on_narrows_the_list() {
        let mut app = listing("SELECT * FROM o");
        press(&mut app, KeyCode::Char('r'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(listed(&app), Some((vec!["orders"], 0)));
    }

    #[test]
    fn narrowing_to_nothing_keeps_the_list_open_and_empty() {
        let mut app = listing("SELECT * FROM o");
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(listed(&app), Some((vec![], 0)));
    }

    #[test]
    fn enter_or_tab_on_an_empty_list_does_nothing() {
        for take in [KeyCode::Enter, KeyCode::Tab, KeyCode::Down] {
            let mut app = listing("SELECT * FROM oz");
            assert_eq!(app.handle_key(KeyEvent::from(take)), []);
            assert_eq!((app.sheet().editor.text(), listed(&app)), ("SELECT * FROM oz", Some((vec![], 0))), "{take:?}");
        }
    }

    #[test]
    fn esc_closes_an_empty_list() {
        let mut app = listing("SELECT * FROM oz");
        press(&mut app, KeyCode::Esc);
        assert_eq!(listed(&app), None);
    }

    #[test]
    fn a_paste_while_the_list_is_open_closes_it_and_goes_to_the_editor() {
        let mut app = listing("SELECT * FROM o");
        app.paste("rders");
        assert_eq!((app.sheet().editor.text(), listed(&app)), ("SELECT * FROM orders", None));
    }

    #[test]
    fn a_key_that_types_no_name_closes_the_list_and_still_works() {
        let mut app = listing("SELECT * FROM o");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!((app.sheet().editor.text(), listed(&app)), ("SELECT * FROM o ", None));
    }

    #[test]
    fn a_string_opens_no_list() {
        assert_eq!(listed(&listing("SELECT 'o")), None);
    }

    #[test]
    fn tab_outside_the_list_still_switches_panes() {
        let mut app = typed("SELECT * FROM o");
        press(&mut app, KeyCode::Tab);
        assert_ne!(app.focus, super::super::Focus::Editor);
    }
}
