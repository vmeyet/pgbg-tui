//! Saving the editor's text as a query, and the questions on the footer line: the path of a new query, opening another
//! query over unsaved text, and sending a write.
use super::editor::edit_line;
use super::{Action, App, Overlay, WriteToSend};
use crate::act::{self, Act, Pane};
use crate::connection::ConnectionName;
use crate::query::{Query, QueryPath};
use crossterm::event::KeyEvent;
use vtui::field::Field;

const EXTENSION: &str = ".sql";
const SAVE_FIRST: &str = "save the query with ^s before editing it in $EDITOR";

/// A question on the footer line; while it is open it takes every key.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum Prompt {
    /// The path to save the editor's text at as a new query, as typed so far, and why the last path was refused.
    SaveAs { path: Field, error: Option<String> },
    /// What to open once the user agrees to drop the editor's unsaved text.
    OpenOverUnsaved(Opening),
    /// A write waiting for a `y`; any other key drops it.
    SendWrite(Box<WriteToSend>),
}

/// What the editor opens in place of its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Opening {
    Saved(Query),
    /// An empty editor that stands for no saved query yet: the next `ctrl-s` asks for its path.
    New,
}

impl Prompt {
    /// The pane of the action table whose keys answer it.
    pub(in crate::tui) const fn pane(&self) -> Pane {
        match self {
            Self::SaveAs { .. } | Self::OpenOverUnsaved(_) => Pane::Prompt,
            Self::SendWrite(_) => Pane::Confirm,
        }
    }
}

/// `orders/daily` as `orders/daily.sql`; a path typed with its extension stays as it is.
fn with_extension(typed: &str) -> String {
    let typed = typed.trim();
    if typed.ends_with(EXTENSION) { typed.to_owned() } else { format!("{typed}{EXTENSION}") }
}

impl App {
    /// Saves the editor's text over the query it stands for, or asks where a new one goes.
    pub(super) fn save(&mut self) -> Vec<Action> {
        let sheet = self.sheet();
        let Some(query) = sheet.opened.clone() else {
            self.ask_save_as();
            return vec![];
        };
        vec![Action::SaveQuery { connection: sheet.name().clone(), query, text: sheet.editor.text().to_owned() }]
    }

    /// Asks for the path of a new query, starting from the path of the query the editor stands for, if any.
    pub(super) fn ask_save_as(&mut self) {
        let path = self.sheet().opened.as_ref().map_or_else(Field::default, |query| Field::new(query.path.as_str()));
        self.overlay = Some(Overlay::Prompt(Prompt::SaveAs { path, error: None }));
    }

    /// Opens the saved query in the user's editor; unsaved text would be lost, so it asks for a save first.
    pub(super) fn edit_outside(&mut self) -> Vec<Action> {
        let sheet = self.sheet_mut();
        let Some(query) = sheet.opened.clone().filter(|_| !sheet.is_dirty) else {
            sheet.query_error = Some(SAVE_FIRST.to_owned());
            return vec![];
        };
        vec![Action::EditOutside { connection: sheet.name().clone(), query }]
    }

    /// Opens `opening` in the editor on screen, asking first when the editor holds unsaved text.
    pub(super) fn ask_open(&mut self, opening: Opening) -> Vec<Action> {
        if !self.sheet().is_dirty {
            return self.open(opening);
        }
        self.overlay = Some(Overlay::Prompt(Prompt::OpenOverUnsaved(opening)));
        vec![]
    }

    fn open(&mut self, opening: Opening) -> Vec<Action> {
        match opening {
            Opening::Saved(query) => vec![self.open_query(query)],
            Opening::New => {
                self.new_query();
                vec![]
            }
        }
    }

    /// The prompt after `key`, `None` once it closes, and the actions to run.
    pub(super) fn prompt_after(&mut self, prompt: Prompt, key: KeyEvent) -> (Option<Prompt>, Vec<Action>) {
        match (act::act_for(key, prompt.pane()), prompt) {
            (Some(Act::Quit), _) => (None, self.quit()),
            (Some(Act::Close), _) => (None, vec![]),
            (Some(Act::Run), Prompt::SendWrite(waiting)) => (None, self.send_write(*waiting)),
            (_, Prompt::SendWrite(_)) => (None, vec![]),
            (Some(Act::Open), Prompt::OpenOverUnsaved(opening)) => (None, self.open(opening)),
            (Some(Act::Open), Prompt::SaveAs { path, .. }) => self.save_as(path),
            (_, Prompt::SaveAs { mut path, error }) => {
                edit_line(&mut path, key);
                (Some(Prompt::SaveAs { path, error }), vec![])
            }
            (_, prompt) => (Some(prompt), vec![]),
        }
    }

    fn save_as(&self, path: Field) -> (Option<Prompt>, Vec<Action>) {
        let sheet = self.sheet();
        match QueryPath::parse(&with_extension(path.text())) {
            Ok(parsed) => {
                let (connection, folder) = (sheet.name().clone(), self.queries_folder().clone());
                (None, vec![Action::SaveNewQuery { connection, folder, path: parsed, text: sheet.editor.text().to_owned() }])
            }
            Err(err) => (Some(Prompt::SaveAs { path, error: Some(err.to_string()) }), vec![]),
        }
    }

    /// `text` was saved as `query` from the sheet of `connection`: its editor now stands for that query, and is
    /// unsaved only if it changed since.
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
    use super::super::fixtures::{enter, failed_at, shop, typed};
    use super::super::queries::tests::{query, with_queries};
    use super::super::{Answer, Focus, QueryAnswer};
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn press(app: &mut App, key: impl Into<KeyEvent>) -> Vec<Action> {
        app.handle_key(key.into())
    }

    fn ctrl_s(app: &mut App) -> Vec<Action> {
        press(app, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
    }

    fn alt_s(app: &mut App) -> Vec<Action> {
        press(app, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT))
    }

    fn prompted_path(app: &App) -> &str {
        let Some(Overlay::Prompt(Prompt::SaveAs { path, .. })) = &app.overlay else { panic!("{:?}", app.overlay) };
        path.text()
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
    fn a_save_holds_the_text_alone_with_an_error_marked_in_it() {
        let mut app = opened("SELECT x");
        enter(&mut app);
        failed_at(&mut app, 8);
        assert_eq!(ctrl_s(&mut app), [Action::SaveQuery { connection: shop(), query: query("daily.sql"), text: "SELECT x".into() }]);
    }

    #[test]
    fn ctrl_s_on_new_text_asks_for_a_path_and_adds_the_extension() {
        let mut app = typed("SELECT 2");
        assert_eq!(ctrl_s(&mut app), []);
        "orders/weekly".chars().for_each(|c| _ = press(&mut app, KeyCode::Char(c)));
        let path = QueryPath::parse("orders/weekly.sql").unwrap();
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            [Action::SaveNewQuery { connection: shop(), folder: shop(), path, text: "SELECT 2".into() }]
        );
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn alt_s_starts_from_the_open_querys_path_and_typing_goes_on_at_its_end() {
        let mut app = opened("SELECT 1");
        assert_eq!(alt_s(&mut app), []);
        assert_eq!(prompted_path(&app), "daily.sql");
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(prompted_path(&app), "daily.sqlx");
    }

    #[test]
    fn alt_s_with_no_query_open_starts_from_an_empty_path() {
        let mut app = typed("SELECT 2");
        alt_s(&mut app);
        assert_eq!(prompted_path(&app), "");
    }

    #[test]
    fn alt_s_starts_from_the_path_on_disk_not_its_escaped_form() {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("da\x1bily.sql"), text: "SELECT 1".into() }));
        alt_s(&mut app);
        assert_eq!(prompted_path(&app), "da\x1bily.sql");
    }

    #[test]
    fn alt_b_in_the_path_moves_a_word_and_types_nothing() {
        let mut app = opened("SELECT 1");
        alt_s(&mut app);
        press(&mut app, KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(prompted_path(&app), "xdaily.sql");
    }

    #[test]
    fn a_modified_enter_adds_no_new_line_to_the_path() {
        let mut app = opened("SELECT 1");
        alt_s(&mut app);
        press(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(prompted_path(&app), "daily.sql");
    }

    #[test]
    fn alt_s_sends_the_edited_text_to_the_new_path_and_nothing_to_the_open_query() {
        let mut app = opened("SELECT 1");
        press(&mut app, KeyCode::Char(';'));
        alt_s(&mut app);
        (0.."daily.sql".len()).for_each(|_| _ = press(&mut app, KeyCode::Backspace));
        "weekly".chars().for_each(|c| _ = press(&mut app, KeyCode::Char(c)));
        let path = QueryPath::parse("weekly.sql").unwrap();
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            [Action::SaveNewQuery { connection: shop(), folder: shop(), path, text: "SELECT 1;".into() }]
        );
    }

    #[test]
    fn once_saved_under_the_new_path_the_editor_stands_for_the_new_query() {
        let mut app = opened("SELECT 1");
        press(&mut app, KeyCode::Char(';'));
        app.apply(Answer::Queries(QueryAnswer::Saved { connection: shop(), query: query("weekly.sql"), text: "SELECT 1;".into() }));
        assert_eq!((app.sheet().opened.as_ref(), app.sheet().is_dirty), (Some(&query("weekly.sql")), false));
        assert_eq!(ctrl_s(&mut app), [Action::SaveQuery { connection: shop(), query: query("weekly.sql"), text: "SELECT 1;".into() }]);
    }

    #[test]
    fn a_refused_save_as_leaves_the_editor_on_the_open_query_and_unsaved() {
        let mut app = opened("SELECT 1");
        press(&mut app, KeyCode::Char(';'));
        let path = QueryPath::parse("customers.sql").unwrap();
        app.apply(Answer::Queries(QueryAnswer::NotSaved { connection: shop(), path, error: "a query is already there".into() }));
        assert_eq!((app.sheet().opened.as_ref(), app.sheet().is_dirty), (Some(&query("daily.sql")), true));
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
    fn a_saved_query_lists_the_queries_again_so_a_new_file_shows() {
        let mut app = opened("SELECT 1");
        let saved = QueryAnswer::Saved { connection: shop(), query: query("daily.sql"), text: "SELECT 1".into() };
        assert_eq!(app.apply(Answer::Queries(saved)), [Action::ListQueries(shop())]);
    }

    #[test]
    fn opening_a_query_over_unsaved_text_asks_first() {
        let mut app = typed("SELECT 2");
        assert_eq!(app.ask_open(Opening::Saved(query("customers.sql"))), []);
        assert_eq!(press(&mut app, KeyCode::Esc), []);
        assert_eq!(app.overlay, None);
        app.ask_open(Opening::Saved(query("customers.sql")));
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery { connection: shop(), query: query("customers.sql") }]);
    }

    fn ctrl_q(app: &mut App) -> Vec<Action> {
        press(app, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL))
    }

    #[test]
    fn ctrl_q_empties_a_saved_query_and_the_next_save_asks_for_its_path() {
        let mut app = opened("SELECT 1");
        assert_eq!(ctrl_q(&mut app), []);
        assert_eq!((app.sheet().editor.text(), app.sheet().opened.as_ref(), app.sheet().is_dirty), ("", None, false));
        ctrl_s(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::Prompt(Prompt::SaveAs { .. }))), "{:?}", app.overlay);
    }

    #[test]
    fn ctrl_q_leaves_nothing_to_undo() {
        let mut app = typed("SELECT 2");
        ctrl_q(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(app.sheet().editor.text(), "");
    }

    #[test]
    fn ctrl_q_over_unsaved_text_asks_first_and_esc_keeps_the_text() {
        let mut app = typed("SELECT 2");
        ctrl_q(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::Prompt(Prompt::OpenOverUnsaved(Opening::New)))), "{:?}", app.overlay);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.sheet().editor.text(), "SELECT 2");
    }

    #[test]
    fn enter_on_the_question_drops_the_unsaved_text_for_a_new_query() {
        let mut app = typed("SELECT 2");
        ctrl_q(&mut app);
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!((app.sheet().editor.text(), app.overlay.as_ref()), ("", None));
    }

    #[test]
    fn ctrl_q_from_the_result_gives_the_new_query_the_keys() {
        let mut app = opened("SELECT 1");
        press(&mut app, KeyCode::Tab);
        ctrl_q(&mut app);
        assert_eq!(app.focus, Focus::Editor);
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
    fn ctrl_x_hands_a_saved_query_to_the_users_editor() {
        let mut app = opened("SELECT 1");
        let edit = Action::EditOutside { connection: shop(), query: query("daily.sql") };
        assert_eq!(press(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)), [edit]);
    }

    #[test]
    fn ctrl_x_on_unsaved_text_asks_for_a_save_first() {
        let mut app = opened("SELECT 1");
        press(&mut app, KeyCode::Char(';'));
        assert_eq!(press(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)), []);
        assert_eq!(app.sheet().query_error.as_deref(), Some(SAVE_FIRST));
    }
}
