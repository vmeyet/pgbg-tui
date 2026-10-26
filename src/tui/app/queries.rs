//! The saved queries in the app: the sidebar's tree and its cursor, the `ctrl-p` finder, and the query open in the editor.
use super::editor::Editor;
use super::{Action, App, Focus, Overlay};
use crate::act::{self, Act, Pane};
use crate::connection::ConnectionName;
use crate::query::{Query, TreeRow, tree_rows};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeSet;
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

/// The finder: the line typed, and the queries it matches, ranked again only when the line or the list changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::tui) struct Finder {
    pub(in crate::tui) palette: Palette<ByPath>,
    pub(in crate::tui) shown: Vec<Query>,
}

impl Finder {
    fn new(all: &[Query]) -> Self {
        Self { palette: Palette::default(), shown: found(all, "") }
    }

    /// Types the printable characters of `text`, then ranks once.
    pub(super) fn paste(&mut self, text: &str, all: &[Query]) {
        text.chars().filter(|c| !c.is_control()).for_each(|c| self.palette.type_char(c));
        self.rank(all);
    }

    fn rank(&mut self, all: &[Query]) {
        self.shown = found(all, &self.palette.input);
        self.palette.move_by(0, self.shown.len());
    }
}

/// Every saved query, the folders folded in the sidebar, and its cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct Queries {
    pub(in crate::tui) all: Vec<Query>,
    closed: BTreeSet<String>,
    pub(in crate::tui) selected: usize,
}

impl Queries {
    pub(in crate::tui) fn rows(&self) -> Vec<TreeRow> {
        tree_rows(&self.all, &self.closed)
    }
}

/// The queries the finder lists for `input`, best first.
fn found(all: &[Query], input: &str) -> Vec<Query> {
    let ranked = vtui::fuzzy::rank(input, all.iter().map(|query| (query.path.to_string(), query)));
    ranked.into_iter().take(MAX_SHOWN).map(|(_, query)| query.clone()).collect()
}

impl App {
    /// Takes the new list of queries, the sidebar's cursor kept inside it.
    pub(super) fn listed(&mut self, all: Vec<Query>) {
        self.queries.all = all;
        self.step_sidebar(0);
        if let Some(Overlay::Finder(finder)) = &mut self.overlay {
            finder.rank(&self.queries.all);
        }
    }

    /// Opens the finder on every query, and lists them again in case a file came or went.
    pub(super) fn find(&mut self) -> Vec<Action> {
        self.overlay = Some(Overlay::Finder(Finder::new(&self.queries.all)));
        vec![Action::ListQueries]
    }

    pub(super) fn step_sidebar(&mut self, delta: isize) {
        let last = self.queries.rows().len().saturating_sub(1);
        self.queries.selected = self.queries.selected.saturating_add_signed(delta).min(last);
    }

    /// Opens the query under the sidebar's cursor, or folds and unfolds the folder there.
    pub(super) fn open_in_sidebar(&mut self) -> Vec<Action> {
        match self.queries.rows().into_iter().nth(self.queries.selected) {
            Some(TreeRow::Query { query, .. }) => return vec![self.open_query(query)],
            Some(TreeRow::Folder { key, is_open: true, .. }) => _ = self.queries.closed.insert(key),
            Some(TreeRow::Folder { key, is_open: false, .. }) => _ = self.queries.closed.remove(&key),
            None => {}
        }
        vec![]
    }

    /// The finder after `key`, `None` once it closed, and what to do: its own keys move, pick and quit, the rest types.
    pub(super) fn finder_after(&mut self, mut finder: Finder, key: KeyEvent) -> (Option<Finder>, Vec<Action>) {
        match act::act_for(key, Pane::Finder) {
            Some(Act::Down) => finder.palette.move_by(1, finder.shown.len()),
            Some(Act::Up) => finder.palette.move_by(-1, finder.shown.len()),
            Some(Act::Open) => {
                let picked = finder.shown.into_iter().nth(finder.palette.selected);
                return (None, picked.map(|query| self.open_query(query)).into_iter().collect());
            }
            Some(Act::Close) => return (None, vec![]),
            Some(Act::Quit) => return (None, self.quit()),
            _ => match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    finder.palette.type_char(c);
                    finder.rank(&self.queries.all);
                }
                KeyCode::Backspace if !finder.palette.backspace() => return (None, vec![]),
                KeyCode::Backspace => finder.rank(&self.queries.all),
                _ => {}
            },
        }
        (Some(finder), vec![])
    }

    /// Reads `query` into the editor on screen.
    fn open_query(&self, query: Query) -> Action {
        Action::OpenQuery { connection: self.sheet().name.clone(), query }
    }

    /// Puts the text of `query` in the editor of `connection`, which it now stands for; the keys go to it when it is on screen.
    pub(super) fn opened(&mut self, connection: &ConnectionName, query: Query, text: &str) {
        let is_on_screen = self.sheet().name == *connection;
        let Some(sheet) = self.sheet_of_mut(connection) else { return };
        sheet.query_error = None;
        sheet.opened = Some(query);
        sheet.editor = Editor::new(text);
        sheet.text_changed();
        if is_on_screen {
            self.focus = Focus::Editor;
        }
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::tests::{app, orders, shop, to_shop, typed};
    use super::super::{Answer, Fit, Incoming, QueryAnswer, Run, RunId, View};
    use super::*;
    use crate::chart::Series;
    use crate::query::{QueryPath, Source};
    use crate::syntax;
    use std::time::Duration;

    pub(in crate::tui) fn query(path: &str) -> Query {
        Query { source: Source::Project, path: QueryPath::parse(path).unwrap() }
    }

    pub(in crate::tui) fn with_queries() -> App {
        let mut app = app();
        app.apply(Answer::Queries(QueryAnswer::All(vec![query("customers.sql"), query("orders/daily.sql"), query("orders/weekly.sql")])));
        app.fit(Fit { grid_rows: 10, has_sidebar: true });
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
        assert_eq!(app.sheet().editor.text(), "");
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery { connection: shop(), query: query("orders/weekly.sql") }]);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn down_in_the_finder_picks_the_next_match() {
        let mut app = with_queries();
        find(&mut app, "orders");
        press(&mut app, KeyCode::Down);
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery { connection: shop(), query: query("orders/weekly.sql") }]);
    }

    #[test]
    fn a_paste_in_the_finder_types_its_letters() {
        let mut app = with_queries();
        find(&mut app, "");
        app.paste("wkly\n");
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery { connection: shop(), query: query("orders/weekly.sql") }]);
    }

    #[test]
    fn esc_closes_the_finder_and_cancels_nothing() {
        let mut app = with_queries();
        find(&mut app, "");
        assert_eq!(press(&mut app, KeyCode::Esc), []);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn an_opened_query_fills_the_editor_and_takes_the_keys() {
        let mut app = with_queries();
        press(&mut app, KeyCode::Tab);
        app.apply(Answer::Queries(QueryAnswer::Opened {
            connection: shop(),
            query: query("customers.sql"),
            text: "SELECT * FROM customers".into(),
        }));
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM customers");
        assert_eq!(app.sheet().opened, Some(query("customers.sql")));
        assert_eq!(app.sheet().editor.colours(), syntax::highlight("SELECT * FROM customers"));
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn a_query_that_cannot_be_read_says_so_and_leaves_the_running_statement_alone() {
        let mut app = with_queries();
        app.sheet_mut().run = Run::Running { run: RunId::FIRST };
        app.apply(Answer::Queries(QueryAnswer::NotOpened {
            connection: shop(),
            query: query("customers.sql"),
            error: "permission denied".into(),
        }));
        assert_eq!(app.sheet().query_error.as_deref(), Some("could not open customers.sql: permission denied"));
        assert_eq!(app.sheet().run, Run::Running { run: RunId::FIRST });
    }

    #[test]
    fn ctrl_c_in_the_finder_quits() {
        let mut app = with_queries();
        find(&mut app, "o");
        press(&mut app, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_p_lists_the_queries_again_and_the_finder_follows_the_new_list() {
        let mut app = with_queries();
        assert_eq!(press(&mut app, KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)), [Action::ListQueries]);
        app.apply(Answer::Queries(QueryAnswer::All(vec![query("refunds.sql")])));
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery { connection: shop(), query: query("refunds.sql") }]);
    }

    fn to_sidebar(app: &mut App) {
        press(app, KeyCode::Tab);
        press(app, KeyCode::Tab);
    }

    #[test]
    fn tab_reaches_the_sidebar_after_the_result() {
        let mut app = with_queries();
        to_sidebar(&mut app);
        assert_eq!(app.focus, Focus::Queries);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn tab_skips_a_sidebar_that_is_not_drawn() {
        let mut app = with_queries();
        app.fit(Fit { grid_rows: 10, has_sidebar: false });
        to_sidebar(&mut app);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn a_sidebar_no_longer_drawn_gives_the_keys_to_the_editor() {
        let mut app = with_queries();
        to_sidebar(&mut app);
        app.fit(Fit { grid_rows: 10, has_sidebar: false });
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn enter_on_a_query_in_the_sidebar_opens_it() {
        let mut app = with_queries();
        to_sidebar(&mut app);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery { connection: shop(), query: query("orders/daily.sql") }]);
    }

    #[test]
    fn enter_on_a_folder_folds_it_and_again_unfolds_it() {
        let mut app = with_queries();
        to_sidebar(&mut app);
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!(app.queries.rows().len(), 2);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.queries.rows().len(), 4);
    }

    #[test]
    fn the_sidebar_cursor_stays_inside_a_shorter_list() {
        let mut app = with_queries();
        to_sidebar(&mut app);
        (0..5).for_each(|_| _ = press(&mut app, KeyCode::Char('j')));
        app.apply(Answer::Queries(QueryAnswer::All(vec![query("customers.sql")])));
        assert_eq!(app.queries.selected, 0);
    }

    fn opened_and_run(text: &str) -> (App, Vec<Action>) {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("orders/daily.sql"), text: text.into() }));
        let actions = press(&mut app, KeyCode::Enter);
        (app, actions)
    }

    #[test]
    fn a_chart_header_draws_the_result_of_the_query_as_that_chart() {
        let (mut app, _) = opened_and_run("-- chart: bar x=customer y=total\nSELECT customer, total FROM orders");
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        let spec = "bar x=customer y=total".parse().unwrap();
        let Run::Done { view, .. } = &app.sheet().run else { panic!("no result") };
        assert!(matches!(view, View::Chart(Ok(_))), "{view:?}");
        assert_eq!(view, &View::Chart(Series::from_result(&orders(), &spec)));
    }

    #[test]
    fn the_header_follows_the_text_as_it_is_typed() {
        let app = typed("-- name: daily\nSELECT 1");
        assert_eq!(app.sheet().header.name.as_deref(), Some("daily"));
    }

    #[test]
    fn a_suggested_connection_never_moves_the_run_off_the_session_on_screen() {
        let (_, actions) = opened_and_run("-- connection: prod\nSELECT 1");
        assert!(matches!(actions.as_slice(), [Action::Run { connection, .. }] if *connection == shop()), "{actions:?}");
    }
}
