//! Saving the editor's text as a query, and the prompts on the way: the path of a new query, and opening another
//! query over unsaved text.
use super::{Action, App, Overlay};
use crate::act::{self, Act, Pane};
use crate::connection::ConnectionName;
use crate::query::{Query, QueryPath};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::field::Field;

const EXTENSION: &str = ".sql";
const SAVE_FIRST: &str = "save the query with ^s before editing it in $EDITOR";

/// A question on the footer line; while it is open it takes every key.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum Prompt {
    /// The path a new query is saved at, as it is typed, and why the last one was refused.
    SaveAs { path: Field, error: Option<String> },
    /// The query to open once the user agrees to drop the editor's unsaved text.
    OpenOverUnsaved(Query),
}

/// `orders/daily` as `orders/daily.sql`; a path typed with its extension stays as it is.
fn with_extension(typed: &str) -> String {
    let typed = typed.trim();
    if typed.ends_with(EXTENSION) { typed.to_owned() } else { format!("{typed}{EXTENSION}") }
}

pub(super) fn typed_into(path: &mut Field, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => path.insert(c),
        KeyCode::Backspace => path.backspace(),
        KeyCode::Left => path.left(),
        KeyCode::Right => path.right(),
        _ => {}
    }
}

impl App {
    /// Saves the editor's text over the query it stands for, or asks where a new one goes.
    pub(super) fn save(&mut self) -> Vec<Action> {
        let sheet = self.sheet();
        let Some(query) = sheet.opened.clone() else {
            self.overlay = Some(Overlay::Prompt(Prompt::SaveAs { path: Field::default(), error: None }));
            return vec![];
        };
        vec![Action::SaveQuery { connection: sheet.name().clone(), query, text: sheet.editor.text().to_owned() }]
    }

    /// Hands the saved query to the user's editor; text not saved yet would be lost, so it asks for a save first.
    pub(super) fn edit_outside(&mut self) -> Vec<Action> {
        let sheet = self.sheet_mut();
        let Some(query) = sheet.opened.clone().filter(|_| !sheet.is_dirty) else {
            sheet.query_error = Some(SAVE_FIRST.to_owned());
            return vec![];
        };
        vec![Action::EditOutside { connection: sheet.name().clone(), query }]
    }

    /// Opens `query` in the editor on screen, after asking when it holds text not saved yet.
    pub(super) fn ask_open(&mut self, query: Query) -> Vec<Action> {
        if !self.sheet().is_dirty {
            return vec![self.open_query(query)];
        }
        self.overlay = Some(Overlay::Prompt(Prompt::OpenOverUnsaved(query)));
        vec![]
    }

    /// The prompt after `key`, `None` once it closed, and what to do.
    pub(super) fn prompt_after(&mut self, prompt: Prompt, key: KeyEvent) -> (Option<Prompt>, Vec<Action>) {
        match (act::act_for(key, Pane::Prompt), prompt) {
            (Some(Act::Quit), _) => (None, self.quit()),
            (Some(Act::Close), _) => (None, vec![]),
            (Some(Act::Open), Prompt::OpenOverUnsaved(query)) => (None, vec![self.open_query(query)]),
            (Some(Act::Open), Prompt::SaveAs { path, .. }) => self.save_as(path),
            (_, Prompt::SaveAs { mut path, error }) => {
                typed_into(&mut path, key);
                (Some(Prompt::SaveAs { path, error }), vec![])
            }
            (_, prompt) => (Some(prompt), vec![]),
        }
    }

    fn save_as(&self, path: Field) -> (Option<Prompt>, Vec<Action>) {
        let sheet = self.sheet();
        match QueryPath::parse(&with_extension(path.text())) {
            Ok(parsed) => {
                let save = Action::SaveNewQuery { connection: sheet.name().clone(), path: parsed, text: sheet.editor.text().to_owned() };
                (None, vec![save])
            }
            Err(err) => (Some(Prompt::SaveAs { path, error: Some(err.to_string()) }), vec![]),
        }
    }

    /// `text` saved as `query` from the sheet of `connection`: its editor now stands for it, unsaved only if it changed since.
    pub(super) fn saved(&mut self, connection: &ConnectionName, query: Query, text: &str) {
        let Some(sheet) = self.sheet_of_mut(connection) else { return };
        sheet.query_error = None;
        sheet.is_dirty = sheet.editor.text() != text;
        sheet.opened = Some(query);
    }

    pub(super) fn not_saved(&mut self, connection: &ConnectionName, path: &QueryPath, error: &str) {
        if let Some(sheet) = self.sheet_of_mut(connection) {
            sheet.query_error = Some(format!("could not save {path}: {error}"));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::super::queries::tests::{query, with_queries};
    use super::super::tests::{shop, typed};
    use super::super::{Answer, QueryAnswer};
    use super::*;

    fn press(app: &mut App, key: impl Into<KeyEvent>) -> Vec<Action> {
        app.handle_key(key.into())
    }

    fn ctrl_s(app: &mut App) -> Vec<Action> {
        press(app, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
    }

    fn opened(text: &str) -> App {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("daily.sql"), text: text.into() }));
        app
    }

    #[test]
    fn ctrl_s_saves_the_text_over_the_query_it_stands_for() {
        let mut app = opened("SELECT 1");
        assert_eq!(ctrl_s(&mut app), [Action::SaveQuery { connection: shop(), query: query("daily.sql"), text: "SELECT 1".into() }]);
    }

    #[test]
    fn ctrl_s_on_new_text_asks_for_a_path_and_adds_the_extension() {
        let mut app = typed("SELECT 2");
        assert_eq!(ctrl_s(&mut app), []);
        "orders/weekly".chars().for_each(|c| _ = press(&mut app, KeyCode::Char(c)));
        let path = QueryPath::parse("orders/weekly.sql").unwrap();
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::SaveNewQuery { connection: shop(), path, text: "SELECT 2".into() }]);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn a_path_that_leaves_the_folder_is_refused_and_the_prompt_stays() {
        let mut app = typed("SELECT 2");
        ctrl_s(&mut app);
        "../x".chars().for_each(|c| _ = press(&mut app, KeyCode::Char(c)));
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert!(matches!(&app.overlay, Some(Overlay::Prompt(Prompt::SaveAs { error: Some(_), .. }))), "{:?}", app.overlay);
    }

    #[test]
    fn typing_marks_the_text_unsaved_and_saving_it_clears_the_mark() {
        let mut app = opened("SELECT 1");
        assert!(!app.sheet().is_dirty);
        press(&mut app, KeyCode::Char(';'));
        assert!(app.sheet().is_dirty);
        app.apply(Answer::Queries(QueryAnswer::Saved { connection: shop(), query: query("daily.sql"), text: "SELECT 1;".into() }));
        assert!(!app.sheet().is_dirty);
    }

    #[test]
    fn opening_a_query_over_unsaved_text_asks_first() {
        let mut app = typed("SELECT 2");
        assert_eq!(app.ask_open(query("customers.sql")), []);
        assert_eq!(press(&mut app, KeyCode::Esc), []);
        assert_eq!(app.overlay, None);
        app.ask_open(query("customers.sql"));
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery { connection: shop(), query: query("customers.sql") }]);
    }

    #[test]
    fn a_failed_save_says_why_and_keeps_the_text_unsaved() {
        let mut app = typed("SELECT 2");
        let path = QueryPath::parse("x.sql").unwrap();
        app.apply(Answer::Queries(QueryAnswer::NotSaved { connection: shop(), path, error: "a query is already there".into() }));
        assert_eq!(app.sheet().query_error.as_deref(), Some("could not save x.sql: a query is already there"));
        assert!(app.sheet().is_dirty);
    }

    #[test]
    fn a_query_picked_in_the_finder_over_unsaved_text_asks_first() {
        let mut app = with_queries();
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert!(matches!(app.overlay, Some(Overlay::Prompt(Prompt::OpenOverUnsaved(_)))), "{:?}", app.overlay);
    }

    #[test]
    fn a_save_answer_lands_on_the_sheet_that_saved() {
        let mut app = typed("SELECT 2");
        let other: ConnectionName = "prod".parse().unwrap();
        app.apply(Answer::Queries(QueryAnswer::Saved { connection: other, query: query("x.sql"), text: "SELECT 2".into() }));
        assert_eq!((app.sheet().opened.as_ref(), app.sheet().is_dirty), (None, true));
    }

    #[test]
    fn ctrl_e_hands_a_saved_query_to_the_users_editor() {
        let mut app = opened("SELECT 1");
        let edit = Action::EditOutside { connection: shop(), query: query("daily.sql") };
        assert_eq!(press(&mut app, KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)), [edit]);
    }

    #[test]
    fn ctrl_e_on_unsaved_text_asks_for_a_save_first() {
        let mut app = opened("SELECT 1");
        press(&mut app, KeyCode::Char(';'));
        assert_eq!(press(&mut app, KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)), []);
        assert_eq!(app.sheet().query_error.as_deref(), Some(SAVE_FIRST));
    }
}
