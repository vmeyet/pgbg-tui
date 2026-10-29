//! What the `[usage]` counts read from the app: the act a key does where it lands, and the screen on show.
//! The loop counts them and keeps the clock, so the app only answers and stays pure.
use super::{App, Overlay};
use crate::act::{self, Act, Pane};
use crossterm::event::KeyEvent;

impl App {
    /// The act `key` does in the pane or overlay that has the keys; read before `handle_key` changes them.
    /// `→` with no faded rest to take only moves the cursor, so it counts as nothing.
    pub(in crate::tui) fn act_of(&self, key: KeyEvent) -> Option<Act> {
        match act::act_for(key, self.keys_pane()?)? {
            Act::TakeGhost if self.overlay.is_none() && self.sheet().ghost.is_none() => None,
            act => Some(act),
        }
    }

    /// The pane or overlay on show, as the usage counts name its time.
    pub(in crate::tui) fn screen(&self) -> &'static str {
        match (&self.overlay, self.keys_pane()) {
            (Some(Overlay::Recall(_)), _) => "history",
            (Some(Overlay::Versions(_)), _) => "versions",
            (_, Some(pane)) => pane.title(),
            (_, None) => "help",
        }
    }

    /// The pane of the action table whose keys work now; none while the help, which reads its keys itself, is open.
    fn keys_pane(&self) -> Option<Pane> {
        let pane = match &self.overlay {
            None => self.pane(),
            Some(Overlay::Help(_)) => return None,
            Some(Overlay::Finder(_) | Overlay::Recall(_) | Overlay::Versions(_)) => Pane::Finder,
            Some(Overlay::Picker(_)) => Pane::Picker,
            Some(Overlay::Form(_)) => Pane::Form,
            Some(Overlay::Prompt(prompt)) => prompt.pane(),
            Some(Overlay::Detail(_)) => Pane::Detail,
            Some(Overlay::CopyAs) => Pane::CopyAs,
            Some(Overlay::SchemaFilter) => Pane::SchemaFilter,
            Some(Overlay::RowFilter(_)) => Pane::RowFilter,
            Some(Overlay::Completion(_)) => Pane::Completion,
            Some(Overlay::TableFinder(_)) => Pane::TableFinder,
            Some(Overlay::Ask(step)) => step.pane(),
        };
        Some(pane)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{app, press, typed, with_result};
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn control(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn a_key_is_the_act_it_does_in_the_pane_with_the_keys() {
        let mut app = with_result();
        assert_eq!(app.act_of(KeyEvent::from(KeyCode::Char('j'))), None, "j types in the editor");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.act_of(KeyEvent::from(KeyCode::Char('j'))), Some(Act::Down));
    }

    #[test]
    fn a_key_in_an_overlay_is_the_act_it_does_there() {
        let mut app = app();
        app.handle_key(control('o'));
        assert_eq!(app.act_of(KeyEvent::from(KeyCode::Esc)), Some(Act::Close));
    }

    #[test]
    fn right_with_no_faded_rest_only_moves_the_cursor() {
        assert_eq!(typed("SELECT").act_of(KeyEvent::from(KeyCode::Right)), None);
    }

    #[test]
    fn the_screen_is_the_overlay_on_show_else_the_pane_with_the_keys() {
        let mut app = with_result();
        assert_eq!(app.screen(), "editor");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.screen(), "result");
        app.handle_key(control('r'));
        assert_eq!(app.screen(), "history");
    }

    #[test]
    fn the_help_counts_as_its_own_screen_and_its_keys_as_no_act() {
        let mut app = with_result();
        press(&mut app, KeyCode::F(1));
        assert_eq!((app.screen(), app.act_of(KeyEvent::from(KeyCode::Char('j')))), ("help", None));
    }
}
