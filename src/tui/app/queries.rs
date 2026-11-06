//! The saved queries in the app: the sidebar's tree and its cursor, the `ctrl-p` finder, the query opened in the editor,
//! and deleting one.
use super::search::{self, Search, Searching};
use super::{Action, App, Focus, Opening, Overlay, Prompt, Sheet};
use crate::act::Pane;
use crate::connection::ConnectionName;
use crate::query::{Query, QueryPath, TreeRow, tree_rows};
use crossterm::event::KeyEvent;
use std::collections::BTreeSet;

const FOLDER_NOT_DELETED: &str = "a folder is not deleted: delete its queries one by one";

/// The `ctrl-p` finder over every saved query, by its path.
pub(in crate::tui) type Finder = Search<Query>;

/// Every saved query, the folders folded in the sidebar, and the sidebar's cursor.
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

impl App {
    /// The queries folder of the connection on screen, whose queries the sidebar and the finder show.
    pub(super) fn queries_folder(&self) -> &ConnectionName {
        self.sheet().connection.queries()
    }

    /// Lists the queries of the connection on screen again.
    pub(super) fn list_queries(&self) -> Action {
        Action::ListQueries(self.queries_folder().clone())
    }

    /// Lists the queries again when the connection on screen no longer uses the `listed` folder, emptying the list
    /// until they come, so another connection's queries never show.
    pub(super) fn follow_queries_folder(&mut self, listed: &ConnectionName) -> Option<Action> {
        if self.queries_folder() == listed {
            return None;
        }
        self.take_list(vec![]);
        Some(self.list_queries())
    }

    /// Takes the list of queries of `folder`, unless the connection on screen no longer uses that folder.
    pub(super) fn listed(&mut self, folder: &ConnectionName, all: Vec<Query>) {
        if self.queries_folder() == folder {
            self.take_list(all);
        }
    }

    /// Takes the new list of queries, keeping the sidebar's cursor on its row, or else inside the list.
    fn take_list(&mut self, all: Vec<Query>) {
        let selected = self.queries.rows().into_iter().nth(self.queries.selected);
        self.queries.all = all;
        let rows = self.queries.rows();
        let kept = selected.and_then(|selected| rows.iter().position(|row| row.is_at(&selected)));
        self.queries.selected = kept.unwrap_or(self.queries.selected);
        self.step_sidebar(0);
        if let Some(Overlay::Finder(finder)) = &mut self.overlay {
            finder.rank(&self.queries.all);
        }
    }

    /// Opens the finder on every query, and lists them again in case a file was added or removed.
    pub(super) fn find(&mut self) -> Vec<Action> {
        self.overlay = Some(Overlay::Finder(Finder::new(&self.queries.all)));
        vec![self.list_queries()]
    }

    pub(super) fn step_sidebar(&mut self, delta: isize) {
        let last = self.queries.rows().len().saturating_sub(1);
        self.queries.selected = self.queries.selected.saturating_add_signed(delta).min(last);
    }

    /// Opens the query under the sidebar's cursor, or folds and unfolds the folder there.
    pub(super) fn open_in_sidebar(&mut self) -> Vec<Action> {
        match self.queries.rows().into_iter().nth(self.queries.selected) {
            Some(TreeRow::Query { query, .. }) => return self.ask_open(Opening::Saved(query)),
            Some(TreeRow::Folder { key, is_open: true, .. }) => _ = self.queries.closed.insert(key),
            Some(TreeRow::Folder { key, is_open: false, .. }) => _ = self.queries.closed.remove(&key),
            None => {}
        }
        vec![]
    }

    /// Asks whether to delete the query under the sidebar's cursor; a folder is never deleted, and the status line says so.
    pub(super) fn ask_delete_in_sidebar(&mut self) {
        match self.queries.rows().into_iter().nth(self.queries.selected) {
            Some(TreeRow::Query { query, .. }) => self.overlay = Some(Overlay::Prompt(Prompt::DeleteQuery(query))),
            Some(TreeRow::Folder { .. }) => self.note = Some(FOLDER_NOT_DELETED.to_owned()),
            None => {}
        }
    }

    pub(super) fn delete_query(&self, query: Query) -> Action {
        Action::DeleteQuery { connection: self.sheet().name().clone(), query }
    }

    /// The file of `query` is gone, as the sheet of `connection` asked: each editor that stood for it keeps its text, now
    /// unsaved, and the queries are listed again.
    pub(super) fn deleted(&mut self, connection: &ConnectionName, query: &Query) -> Vec<Action> {
        if let Some(sheet) = self.sheet_of_mut(connection) {
            sheet.query_error = None;
        }
        self.sheets.iter_mut().filter(|sheet| sheet.opened.as_ref() == Some(query)).for_each(Sheet::stand_for_no_file);
        self.note = Some(format!("deleted {}", query.path));
        vec![self.list_queries()]
    }

    pub(super) fn not_deleted(&mut self, connection: &ConnectionName, path: &QueryPath, error: &str) {
        if let Some(sheet) = self.sheet_of_mut(connection) {
            sheet.query_error = Some(format!("could not delete {path}: {error}"));
        }
    }

    /// The finder after `key`, `None` once it closes, and the actions to run: a picked query opens in the editor.
    pub(super) fn finder_after(&mut self, finder: Finder, key: KeyEvent) -> (Option<Finder>, Vec<Action>) {
        match search::after(finder, key, Pane::Finder, &self.queries.all) {
            Searching::Typed(finder) | Searching::Unhandled(finder, _) => (Some(finder), vec![]),
            Searching::Closed => (None, vec![]),
            Searching::Picked(query) => (None, self.ask_open(Opening::Saved(query))),
            Searching::Quit => (None, self.quit()),
        }
    }

    /// Reads `query` into the editor on screen.
    pub(super) fn open_query(&self, query: Query) -> Action {
        Action::OpenQuery { connection: self.sheet().name().clone(), query }
    }

    /// Puts the text of `query` in the editor of `connection`, which now stands for that query; the editor gets the
    /// keys when it is on screen.
    pub(super) fn opened(&mut self, connection: &ConnectionName, query: Query, text: &str) {
        let is_on_screen = self.sheet().name() == connection;
        let Some(sheet) = self.sheet_of_mut(connection) else { return };
        sheet.stand_for(Some(query), text);
        if is_on_screen {
            self.focus = Focus::Editor;
        }
    }

    /// Empties the editor on screen, which now stands for no saved query, and gives it the keys.
    pub(super) fn new_query(&mut self) {
        self.sheet_mut().stand_for(None, "");
        self.focus = Focus::Editor;
    }
}

impl Sheet {
    /// Puts `text` in the editor, which now stands for `opened` and holds nothing unsaved.
    fn stand_for(&mut self, opened: Option<Query>, text: &str) {
        self.query_error = None;
        self.opened = opened;
        self.editor.reload(text);
        self.text_changed();
        self.is_dirty = false;
    }

    /// Keeps the editor's text, which now stands for no saved query, so it is unsaved.
    fn stand_for_no_file(&mut self) {
        self.opened = None;
        self.is_dirty = true;
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::fixtures::{app, control, every_pane, on_the_result, shop, to_shop, typed, with_focus};
    use super::super::{Answer, Fit, Incoming, QueryAnswer, Room, Run, RunId, View};
    use super::*;
    use crate::chart::Series;
    use crate::fixtures::orders;
    use crate::format::Format;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::time::Duration;

    pub(in crate::tui) fn query(path: &str) -> Query {
        Query { folder: shop(), path: QueryPath::parse(path).unwrap() }
    }

    /// The list of the queries of `shop`, the connection on screen.
    pub(in crate::tui) fn listed(queries: Vec<Query>) -> Answer {
        Answer::Queries(QueryAnswer::All { folder: shop(), queries })
    }

    pub(in crate::tui) fn with_queries() -> App {
        let mut app = app();
        app.apply(listed(vec![query("customers.sql"), query("orders/daily.sql"), query("orders/weekly.sql")]));
        app.fit(Fit { grid: Room { rows: 10, columns: 80 }, panes: every_pane(), ..Fit::default() });
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
    fn ctrl_k_on_the_result_finds_a_query() {
        let mut app = on_the_result(orders());
        press(&mut app, control('k'));
        assert!(matches!(app.overlay, Some(Overlay::Finder(_))));
    }

    #[test]
    fn ctrl_k_in_the_sidebar_finds_a_query() {
        let mut app = with_focus(with_queries(), Focus::Queries);
        press(&mut app, control('k'));
        assert!(matches!(app.overlay, Some(Overlay::Finder(_))));
    }

    #[test]
    fn command_k_in_the_editor_finds_a_query() {
        let mut app = with_queries();
        press(&mut app, KeyEvent::new(KeyCode::Char('k'), KeyModifiers::SUPER));
        assert!(matches!(app.overlay, Some(Overlay::Finder(_))));
    }

    #[test]
    fn ctrl_k_in_the_editor_deletes_to_the_end_of_the_line() {
        let mut app = typed("SELECT 1");
        press(&mut app, control('a'));
        press(&mut app, control('k'));
        assert_eq!((app.overlay.is_none(), app.sheet().editor.text()), (true, ""));
    }

    #[test]
    fn ctrl_k_in_the_parameter_bar_finds_nothing() {
        let mut app = with_focus(typed("SELECT {{id}}"), Focus::Params);
        press(&mut app, control('k'));
        assert!(app.overlay.is_none());
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
        assert_eq!(press(&mut app, KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)), [Action::ListQueries(shop())]);
        app.apply(listed(vec![query("refunds.sql")]));
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
        app.fit(Fit { grid: Room { rows: 10, columns: 80 }, ..Fit::default() });
        to_sidebar(&mut app);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn a_sidebar_no_longer_drawn_gives_the_keys_to_the_editor() {
        let mut app = with_queries();
        to_sidebar(&mut app);
        app.fit(Fit { grid: Room { rows: 10, columns: 80 }, ..Fit::default() });
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
    fn the_sidebar_cursor_stays_on_its_query_when_the_list_changes() {
        let mut app = with_queries();
        to_sidebar(&mut app);
        (0..2).for_each(|_| _ = press(&mut app, KeyCode::Char('j')));
        let all = vec![query("archive/old.sql"), query("customers.sql"), query("orders/daily.sql"), query("orders/weekly.sql")];
        app.apply(listed(all));
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::OpenQuery { connection: shop(), query: query("orders/weekly.sql") }]);
    }

    #[test]
    fn the_sidebar_cursor_stays_inside_a_shorter_list() {
        let mut app = with_queries();
        to_sidebar(&mut app);
        (0..5).for_each(|_| _ = press(&mut app, KeyCode::Char('j')));
        app.apply(listed(vec![query("customers.sql")]));
        assert_eq!(app.queries.selected, 0);
    }

    /// The app asking whether to delete `orders/daily.sql`, after `d` on it in the sidebar.
    pub(in crate::tui) fn asked_to_delete_daily() -> App {
        let mut app = with_queries();
        to_sidebar(&mut app);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(press(&mut app, KeyCode::Char('d')), []);
        app
    }

    #[test]
    fn y_after_d_on_a_query_in_the_sidebar_deletes_it() {
        let mut app = asked_to_delete_daily();
        assert_eq!(app.overlay, Some(Overlay::Prompt(Prompt::DeleteQuery(query("orders/daily.sql")))));
        assert_eq!(press(&mut app, KeyCode::Char('y')), [Action::DeleteQuery { connection: shop(), query: query("orders/daily.sql") }]);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn any_other_key_than_y_keeps_the_query() {
        let mut app = asked_to_delete_daily();
        assert_eq!(press(&mut app, KeyCode::Char('n')), []);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn d_on_a_folder_deletes_nothing_and_says_why() {
        let mut app = with_queries();
        to_sidebar(&mut app);
        assert_eq!(press(&mut app, KeyCode::Char('d')), []);
        assert_eq!((app.overlay.as_ref(), app.note.as_deref()), (None, Some(FOLDER_NOT_DELETED)));
    }

    #[test]
    fn a_deleted_query_lists_the_queries_again() {
        let mut app = with_queries();
        assert_eq!(
            app.apply(Answer::Queries(QueryAnswer::Deleted { connection: shop(), query: query("orders/daily.sql") })),
            [Action::ListQueries(shop())]
        );
    }

    #[test]
    fn the_editor_of_a_deleted_query_keeps_its_text_unsaved_and_stands_for_no_query() {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("customers.sql"), text: "SELECT 1".into() }));
        app.apply(Answer::Queries(QueryAnswer::Deleted { connection: shop(), query: query("customers.sql") }));
        let sheet = app.sheet();
        assert_eq!((sheet.editor.text(), sheet.opened.as_ref(), sheet.is_dirty), ("SELECT 1", None, true));
    }

    #[test]
    fn the_editor_of_another_query_is_left_as_it_is_when_one_is_deleted() {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("customers.sql"), text: "SELECT 1".into() }));
        app.apply(Answer::Queries(QueryAnswer::Deleted { connection: shop(), query: query("orders/daily.sql") }));
        assert_eq!((app.sheet().opened.as_ref(), app.sheet().is_dirty), (Some(&query("customers.sql")), false));
    }

    #[test]
    fn a_query_that_cannot_be_deleted_says_why() {
        let mut app = with_queries();
        let (path, error) = (QueryPath::parse("customers.sql").unwrap(), "permission denied".into());
        app.apply(Answer::Queries(QueryAnswer::NotDeleted { connection: shop(), path, error }));
        assert_eq!(app.sheet().query_error.as_deref(), Some("could not delete customers.sql: permission denied"));
    }

    #[test]
    fn a_query_deleted_after_all_drops_the_error_of_the_last_try() {
        let mut app = with_queries();
        let (path, error) = (QueryPath::parse("customers.sql").unwrap(), "permission denied".into());
        app.apply(Answer::Queries(QueryAnswer::NotDeleted { connection: shop(), path, error }));
        app.apply(Answer::Queries(QueryAnswer::Deleted { connection: shop(), query: query("customers.sql") }));
        assert_eq!(app.sheet().query_error, None);
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
    fn v_comes_back_to_the_chart_of_the_header() {
        use super::super::fixtures::view;
        let (mut app, _) = opened_and_run("-- chart: stacked x=customer y=total\nSELECT customer, total FROM orders");
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        let asked = view(&app).clone();
        app.sheet_mut().switch_view();
        app.sheet_mut().switch_view();
        assert_eq!(view(&app), &asked);
    }

    #[test]
    fn a_chart_header_that_does_not_fit_the_result_starts_on_the_grid() {
        let (mut app, _) = opened_and_run("-- chart: bar x=day y=refunds\nSELECT customer, total FROM orders");
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        let Run::Done { view, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(view, &View::Grid);
    }

    #[test]
    fn a_format_header_opens_the_result_of_the_query_in_its_formats() {
        let (mut app, _) = opened_and_run("-- format: total=bytes\nSELECT id, total, customer FROM orders");
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(grid.formats, [Format::Raw, Format::Bytes, Format::Raw]);
    }

    #[test]
    fn an_opened_query_is_coloured_by_the_loop_not_on_opening() {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("orders/daily.sql"), text: "SELECT 1".into() }));
        assert!(app.sheet().editor.has_stale_colours());
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
