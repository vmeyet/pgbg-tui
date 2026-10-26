//! The saved queries in the app: the `ctrl-p` finder over them, and the query open in the editor.
use super::{Action, App, Focus};
use crate::act::{self, Act, Pane};
use crate::query::QueryPath;
use crate::syntax;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::field::Field;
use vtui::palette::{MAX_SHOWN, Palette};

/// The finder's only mode: every query, by its path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct ByPath;

impl vtui::palette::Mode for ByPath {
    fn switched_by(self, _first: char) -> Option<Self> {
        None
    }

    fn takes_commands(self) -> bool {
        false
    }
}

pub(in crate::tui) type Finder = Palette<ByPath>;

/// Every saved query, and the one open in the editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct Queries {
    pub(in crate::tui) all: Vec<QueryPath>,
    pub(in crate::tui) opened: Option<QueryPath>,
}

/// The queries the finder lists for `input`, best first.
pub(in crate::tui) fn found(all: &[QueryPath], input: &str) -> Vec<QueryPath> {
    let ranked = vtui::fuzzy::rank(input, all.iter().map(|path| (path.to_string(), path)));
    ranked.into_iter().take(MAX_SHOWN).map(|(_, path)| path.clone()).collect()
}

impl App {
    /// The finder after `key`, `None` once it closed, and what to do: its own keys move and pick, the rest types.
    pub(super) fn finder_after(&self, mut finder: Finder, key: KeyEvent) -> (Option<Finder>, Vec<Action>) {
        let shown = found(&self.queries.all, &finder.input);
        match act::act_for(key, Pane::Finder) {
            Some(Act::Down) => finder.move_by(1, shown.len()),
            Some(Act::Up) => finder.move_by(-1, shown.len()),
            Some(Act::Open) => return (None, shown.into_iter().nth(finder.selected).map(Action::OpenQuery).into_iter().collect()),
            Some(Act::Close) => return (None, vec![]),
            _ => match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => finder.type_char(c),
                KeyCode::Backspace if !finder.backspace() => return (None, vec![]),
                _ => {}
            },
        }
        (Some(finder), vec![])
    }

    /// Puts the text of `query` in the editor, which it now stands for.
    pub(super) fn opened(&mut self, path: QueryPath, text: &str) {
        self.queries.opened = Some(path);
        self.editor = Field::new(text);
        self.colours = syntax::highlight(text);
        self.error_mark = None;
        self.forget_sent_place();
        self.focus = Focus::Editor;
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::tests::app;
    use super::super::{Incoming, Run};
    use super::*;

    pub(in crate::tui) fn query(path: &str) -> QueryPath {
        QueryPath::parse(path).unwrap()
    }

    pub(in crate::tui) fn with_queries() -> App {
        let mut app = app();
        app.apply(Incoming::Queries(vec![query("customers.sql"), query("orders/daily.sql"), query("orders/weekly.sql")]));
        app
    }

    fn press(app: &mut App, key: impl Into<KeyEvent>) -> Vec<Action> {
        app.handle_key(key.into())
    }

    fn find(app: &mut App, typed: &str) {
        press(app, KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for c in typed.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn ctrl_p_finds_a_query_by_a_few_letters_and_enter_opens_it() {
        let mut app = with_queries();
        find(&mut app, "wkly");
        assert_eq!(app.editor.text(), "");
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery(query("orders/weekly.sql"))]);
        assert_eq!(app.finder, None);
    }

    #[test]
    fn down_in_the_finder_picks_the_next_match() {
        let mut app = with_queries();
        find(&mut app, "orders");
        press(&mut app, KeyCode::Down);
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery(query("orders/weekly.sql"))]);
    }

    #[test]
    fn esc_closes_the_finder_and_cancels_nothing() {
        let mut app = with_queries();
        find(&mut app, "");
        assert_eq!(press(&mut app, KeyCode::Esc), []);
        assert_eq!(app.finder, None);
    }

    #[test]
    fn an_opened_query_fills_the_editor_and_takes_the_keys() {
        let mut app = with_queries();
        press(&mut app, KeyCode::Tab);
        app.apply(Incoming::QueryOpened { path: query("customers.sql"), text: "SELECT * FROM customers".into() });
        assert_eq!(app.editor.text(), "SELECT * FROM customers");
        assert_eq!(app.queries.opened, Some(query("customers.sql")));
        assert_eq!(app.colours, syntax::highlight("SELECT * FROM customers"));
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn a_query_that_cannot_be_read_says_so_in_the_result() {
        let mut app = with_queries();
        app.apply(Incoming::QueryNotOpened { path: query("customers.sql"), error: "permission denied".into() });
        assert_eq!(app.run, Run::Failed("could not open customers.sql: permission denied".into()));
    }
}
