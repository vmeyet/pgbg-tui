//! The pure state machine: keys in, state changed. No clock, no I/O.
use crate::connection::{Connection, ConnectionName, Tier, WritePolicy};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::field::Field;
use vtui::theme::Theme;

mod keys;

/// The whole TUI state; `handle_key` is the only way it changes.
#[derive(Debug)]
pub(super) struct App {
    pub(super) theme: Theme,
    pub(super) name: ConnectionName,
    pub(super) tier: Tier,
    pub(super) writes: WritePolicy,
    pub(super) editor: Field,
    pub(super) should_quit: bool,
}

impl App {
    /// The app on `connection`, with an empty editor.
    pub(super) fn new(connection: &Connection, theme: Theme) -> Self {
        Self {
            theme,
            name: connection.name().clone(),
            tier: connection.tier(),
            writes: connection.writes(),
            editor: Field::default(),
            should_quit: false,
        }
    }

    /// Takes one key.
    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if control => self.should_quit = true,
            _ => keys::edit(&mut self.editor, key),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::connection::Entry;

    pub(crate) fn connection(tier: &str, writes: &str) -> Connection {
        let entry: Entry =
            toml::from_str(&format!("tier = \"{tier}\"\nurl = \"postgres://nina@localhost/shop\"\nwrites = \"{writes}\"\n")).unwrap();
        Connection::new("shop", entry).unwrap()
    }

    pub(crate) fn app() -> App {
        App::new(&connection("local", "read-only"), Theme::default())
    }

    pub(crate) fn typed(text: &str) -> App {
        let mut app = app();
        for c in text.chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app
    }

    #[test]
    fn typing_goes_to_the_editor() {
        assert_eq!(typed("SELECT 1").editor.text(), "SELECT 1");
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
        assert_eq!(app.editor.text(), "SELECT 1");
    }
}
