//! The idle ghost: once the editor sat a second without a key, the `[ai] command` asked for the rest of the statement
//! under the cursor, shown faded after it as the completion ghost is, and taken with the same `→`.
//! The loop owns the second; the app only says whether one would ask.
use super::editor::Revision;
use super::ghost::Ghost;
use super::{Action, App, AskFailure, AskId, Focus, Sheet};
use crate::ai::Instruction;
use crate::config::AiSettings;
use crate::connection::AiAsking;
use crate::db::model::Statement;

const INSTRUCTION: &str = "Complete the statement under the cursor: answer with it whole, the text it holds kept exactly as \
                           it is, followed by the rest it needs.";

/// Where the idle ask stands, so one text is asked about once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum IdleAsk {
    #[default]
    NotAsked,
    /// The command runs on `ask`, asked about the editor at `revision`.
    Waiting { ask: AskId, revision: Revision },
    /// The editor at this revision was asked about, or could not be.
    Asked(Revision),
}

impl IdleAsk {
    pub(super) fn is_waiting_on(self, ask: AskId) -> bool {
        matches!(self, Self::Waiting { ask: waiting, .. } if waiting == ask)
    }

    fn covers(self, revision: Revision) -> bool {
        match self {
            Self::NotAsked => false,
            Self::Waiting { .. } => true,
            Self::Asked(asked) => asked == revision,
        }
    }
}

impl App {
    /// Whether the editor sitting idle now would ask for the rest of the statement under the cursor: `[ai] ghost` is on,
    /// the editor has the keys with no overlay over it, no completion ghost shows, and this text was not asked about.
    pub(in crate::tui) fn wants_idle_ghost(&self) -> bool {
        self.idle_settings().is_some()
            && self.shows_the_bare_editor()
            && !self.asks.idle.covers(self.sheet().editor.revision())
            && self.sheet().statement_before_cursor().is_some()
    }

    /// Asks for the rest of the statement under the cursor, once the editor sat idle; nothing when it would not ask.
    pub(in crate::tui) fn ask_idle_ghost(&mut self) -> Vec<Action> {
        let Some(settings) = self.idle_settings().filter(|_| self.wants_idle_ghost()) else { return vec![] };
        let settings = settings.without_passwords_of(&self.connections);
        let revision = self.sheet().editor.revision();
        let Ok(prompt) = self.sheet().prompt(&Instruction::new(INSTRUCTION), settings.literals) else {
            self.asks.idle = IdleAsk::Asked(revision);
            return vec![];
        };
        let ask = self.asks.take_id();
        self.asks.idle = IdleAsk::Waiting { ask, revision };
        vec![Action::Ask { ask, prompt, settings }]
    }

    /// Kills the idle ask still running, since a key or a paste makes its answer stale.
    pub(super) fn drop_idle_ask(&mut self) -> Option<Action> {
        let IdleAsk::Waiting { ask, revision } = self.asks.idle else { return None };
        self.asks.idle = IdleAsk::Asked(revision);
        Some(Action::DropAsk(ask))
    }

    /// Shows the rest of the statement the idle ask's `answer` gives, while the editor is still the one it asked about;
    /// a failed ask shows nothing, since nobody asked for it.
    pub(super) fn show_idle_ghost(&mut self, answer: Result<Vec<Statement>, AskFailure>) {
        let IdleAsk::Waiting { revision, .. } = self.asks.idle else { return };
        self.asks.idle = IdleAsk::Asked(revision);
        let is_current = revision == self.sheet().editor.revision() && self.shows_the_bare_editor() && self.idle_settings().is_some();
        let Some(statements) = answer.ok().filter(|_| is_current) else { return };
        let ghost = self.sheet().rest_in(&statements);
        self.sheet_mut().ghost = ghost;
    }

    fn idle_settings(&self) -> Option<&AiSettings> {
        let settings = self.asks.settings().filter(|settings| settings.ghost)?;
        (self.sheet().connection.ai() != AiAsking::Off).then_some(settings)
    }

    fn shows_the_bare_editor(&self) -> bool {
        self.overlay.is_none() && self.focus == Focus::Editor && self.sheet().ghost.is_none()
    }
}

impl Sheet {
    /// The statement under the cursor up to the cursor, when nothing but blanks follows it there.
    fn statement_before_cursor(&self) -> Option<&str> {
        let range = self.range_under_cursor()?;
        let (before, _, _) = self.editor.split();
        let text = self.editor.text();
        let cursor = before.len().max(range.start);
        let is_at_end = text[cursor.min(range.end)..range.end].trim().is_empty();
        Some(&text[range.start..cursor]).filter(|typed| is_at_end && !typed.trim().is_empty())
    }

    /// The ghost of what the first of `statements` adds after the statement typed before the cursor, when it starts
    /// with that statement as typed.
    fn rest_in(&self, statements: &[Statement]) -> Option<Ghost> {
        let typed = self.statement_before_cursor()?;
        let rest = statements.first()?.as_str().strip_prefix(typed)?;
        let (before, _, _) = self.editor.split();
        Ghost::at_cursor(before.len(), rest)
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::ask::tests::ai_settings;
    use super::super::fixtures::{app_on, connection, to_shop};
    use super::super::{Answer, Incoming, Overlay};
    use super::*;
    use crate::complete::Index;
    use crate::fixtures::{self, shop_catalog};
    use crate::tui::app::AskStep;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::Arc;

    fn type_in(app: &mut App, text: &str) {
        text.chars().for_each(|c| drop(app.handle_key(KeyEvent::from(KeyCode::Char(c)))));
    }

    fn with_ghost_on(connection: &crate::connection::Connection, text: &str) -> App {
        let mut app = app_on(connection).with_ai(Some(AiSettings { ghost: true, ..ai_settings() }));
        let catalog = Arc::new(shop_catalog());
        let index = Arc::new(Index::new(&catalog));
        app.apply(to_shop(Incoming::Opened));
        app.apply(to_shop(Incoming::Schema(Ok(Arc::clone(&catalog)))));
        app.apply(to_shop(Incoming::Index { catalog, index }));
        type_in(&mut app, text);
        app
    }

    /// The app with `[ai] ghost = true` on `shop`, its schema read, `text` in the editor.
    pub(in crate::tui) fn idle_on(text: &str) -> App {
        with_ghost_on(&connection("local", "read-only"), text)
    }

    /// The app showing the idle ghost `answer` gave for `text`.
    pub(in crate::tui) fn ghosted(text: &str, answer: &str) -> App {
        let mut app = idle_on(text);
        app.ask_idle_ghost();
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new(answer)]) });
        app
    }

    fn rest(app: &App) -> Option<&str> {
        app.sheet().ghost.as_ref().map(|ghost| ghost.rest.as_str())
    }

    #[test]
    fn a_second_idle_asks_for_the_rest_of_the_statement_under_the_cursor() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        let sent = app.ask_idle_ghost();
        let [Action::Ask { ask: AskId::FIRST, prompt, .. }] = &*sent else { panic!("{sent:?}") };
        assert!(prompt.text().contains("SELECT * FROM orders WHERE"), "{}", prompt.text());
    }

    #[test]
    fn the_answer_shows_the_rest_after_the_cursor() {
        let app = ghosted("SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE total > 10");
        assert_eq!(rest(&app), Some("total > 10"));
    }

    #[test]
    fn only_the_first_line_of_the_rest_shows() {
        let app = ghosted("SELECT * ", "SELECT * FROM orders\nWHERE total > 10");
        assert_eq!(rest(&app), Some("FROM orders"));
    }

    #[test]
    fn an_answer_that_changes_what_is_typed_shows_nothing() {
        assert_eq!(rest(&ghosted("SELECT * FROM orders WHERE ", "SELECT id FROM orders WHERE total > 10")), None);
    }

    #[test]
    fn right_takes_the_idle_ghost() {
        let mut app = ghosted("SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE total > 10");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.split(), ("SELECT * FROM orders WHERE total > 10", "", ""));
    }

    #[test]
    fn a_key_kills_the_idle_ask() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.ask_idle_ghost();
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Left)), [Action::DropAsk(AskId::FIRST)]);
    }

    #[test]
    fn a_paste_kills_the_idle_ask() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.ask_idle_ghost();
        assert_eq!(app.paste("id"), [Action::DropAsk(AskId::FIRST)]);
    }

    #[test]
    fn an_answer_after_a_key_shows_nothing() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.ask_idle_ghost();
        app.handle_key(KeyEvent::from(KeyCode::Backspace));
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new("SELECT * FROM orders WHERE total > 10")]) });
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn a_key_drops_the_idle_ghost() {
        let mut app = ghosted("SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE total > 10");
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn the_completion_ghost_wins_and_nothing_is_asked() {
        let mut app = idle_on("SELECT * FROM cu");
        assert_eq!((app.wants_idle_ghost(), app.ask_idle_ghost()), (false, vec![]));
    }

    #[test]
    fn the_idle_ghost_is_off_by_default() {
        let mut app = app_on(&connection("local", "read-only")).with_ai(Some(ai_settings()));
        type_in(&mut app, "SELECT * FROM orders WHERE ");
        assert_eq!((app.wants_idle_ghost(), app.ask_idle_ghost()), (false, vec![]));
    }

    #[test]
    fn a_connection_with_ai_off_never_asks() {
        let off = fixtures::connection("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nai = \"off\"\n");
        let mut app = with_ghost_on(&off, "SELECT * FROM orders WHERE ");
        assert_eq!((app.wants_idle_ghost(), app.ask_idle_ghost()), (false, vec![]));
    }

    #[test]
    fn the_same_text_is_asked_about_once() {
        let mut app = ghosted("SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE total > 10");
        app.handle_key(KeyEvent::from(KeyCode::Left));
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.ask_idle_ghost(), []);
    }

    #[test]
    fn nothing_is_asked_while_ctrl_g_is_open() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(matches!(app.overlay(), Some(Overlay::Ask(AskStep::Instruction(_)))));
        assert_eq!(app.ask_idle_ghost(), []);
    }

    #[test]
    fn nothing_is_asked_in_the_middle_of_a_statement() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.handle_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(app.ask_idle_ghost(), []);
    }

    #[test]
    fn a_statement_that_cannot_be_sent_is_not_asked_again() {
        let mut app = idle_on("SELECT * FROM orders WHERE note = 'pa");
        assert_eq!(app.ask_idle_ghost(), []);
        assert!(!app.wants_idle_ghost());
    }
}
