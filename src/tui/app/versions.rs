//! `ctrl-v`: the versions of the saved query on screen that its saves replaced, newest first; picking one puts it in
//! the editor as unsaved text, which the user saves with `ctrl-s` to make it the query again.
use super::diff::{self, Change};
use super::search::{Search, Searching};
use super::{Action, App, Overlay};
use crate::act::Pane;
use crate::query::{Query, Version};
use crossterm::event::KeyEvent;

const NOT_SAVED: &str = "save the query with ^s first: each save after keeps the version it replaces";

/// The versions of one query, and the search over them; empty until the backend has read them.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct VersionList {
    pub(in crate::tui) query: Query,
    pub(in crate::tui) all: Vec<Listed>,
    pub(in crate::tui) search: Search<Listed>,
}

/// A version, and what putting it back would change in the editor's text as the list opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Listed {
    pub(in crate::tui) version: Version,
    pub(in crate::tui) added: usize,
    pub(in crate::tui) removed: usize,
    /// The first line it brings back, else the first it takes out; none when it is the editor's text.
    pub(in crate::tui) first_changed: Option<String>,
}

impl Listed {
    /// `version` against the editor's `text`; the line diff grows with both sides, so the backend runs it off the loop.
    pub(in crate::tui) fn against(version: Version, text: &str) -> Self {
        let lines = diff::lines(text, &version.text);
        let count = |change| lines.iter().filter(|line| line.change == change).count();
        let first = |change| lines.iter().find(|line| line.change == change).map(|line| line.text.clone());
        let (added, removed) = (count(Change::Added), count(Change::Removed));
        let first_changed = first(Change::Added).or_else(|| first(Change::Removed));
        Self { version, added, removed, first_changed }
    }
}

impl App {
    /// Opens the list of the versions of the query on screen, and asks the backend to read them.
    pub(super) fn list_versions(&mut self) -> Vec<Action> {
        let sheet = self.sheet_mut();
        let Some(query) = sheet.opened.clone() else {
            sheet.query_error = Some(NOT_SAVED.to_owned());
            return vec![];
        };
        self.overlay = Some(Overlay::Versions(VersionList { query: query.clone(), all: vec![], search: Search::new(&[]) }));
        let text = self.sheet().editor.text().to_owned();
        vec![Action::ListVersions { query, text }]
    }

    /// Takes the versions read for `query`, while its list is still open.
    pub(super) fn versions_read(&mut self, query: &Query, versions: Vec<Listed>) {
        let Some(Overlay::Versions(list)) = &mut self.overlay else { return };
        if list.query != *query {
            return;
        }
        list.all = versions;
        list.search.rank(&list.all);
    }

    /// The list after `key`: a picked version replaces the editor's text, unsaved.
    pub(super) fn versions_key(&mut self, list: VersionList, key: KeyEvent) -> Vec<Action> {
        let VersionList { query, all, search } = list;
        match search.after(key, Pane::Finder, &all) {
            Searching::Typed(search) | Searching::Unhandled(search, _) => {
                self.overlay = Some(Overlay::Versions(VersionList { query, all, search }));
            }
            Searching::Closed => {}
            Searching::Picked(listed) => self.replace_editor_text(&listed.version.text),
            Searching::Quit => return self.quit(),
        }
        vec![]
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::fixtures::{app, shop};
    use super::super::queries::tests::{query, with_queries};
    use super::super::{Answer, QueryAnswer};
    use super::*;
    use chrono::DateTime;
    use crossterm::event::{KeyCode, KeyModifiers};

    pub(in crate::tui) fn version(text: &str) -> Version {
        Version { replaced_at: DateTime::parse_from_rfc3339("2026-10-26T09:30:00Z").unwrap().to_utc(), text: text.to_owned() }
    }

    fn ctrl_v(app: &mut App) -> Vec<Action> {
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL))
    }

    fn on_daily(text: &str) -> App {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("daily.sql"), text: text.into() }));
        app
    }

    /// The versions of `daily.sql` with `texts`, as the backend lists them against the editor's `text`.
    pub(in crate::tui) fn versions_of_daily(texts: &[&str], text: &str) -> Answer {
        Answer::Versions { query: query("daily.sql"), versions: texts.iter().map(|old| Listed::against(version(old), text)).collect() }
    }

    fn listed(app: &mut App, texts: &[&str]) {
        ctrl_v(app);
        let text = app.sheet().editor.text().to_owned();
        app.apply(versions_of_daily(texts, &text));
    }

    fn list(app: &App) -> &VersionList {
        let Some(Overlay::Versions(list)) = &app.overlay else { panic!("no list of versions") };
        list
    }

    #[test]
    fn ctrl_v_reads_the_versions_of_the_query_on_screen() {
        assert_eq!(ctrl_v(&mut on_daily("SELECT 1")), [Action::ListVersions { query: query("daily.sql"), text: "SELECT 1".into() }]);
    }

    #[test]
    fn ctrl_v_on_text_never_saved_says_to_save_it_first() {
        let mut app = app();
        assert_eq!(ctrl_v(&mut app), []);
        assert_eq!((app.overlay.is_none(), app.sheet().query_error.as_deref()), (true, Some(NOT_SAVED)));
    }

    #[test]
    fn a_picked_version_replaces_the_editor_text_unsaved() {
        let mut app = on_daily("SELECT 2");
        listed(&mut app, &["SELECT 1"]);
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let sheet = app.sheet();
        assert_eq!((sheet.editor.text(), sheet.is_dirty, sheet.opened.clone()), ("SELECT 1", true, Some(query("daily.sql"))));
    }

    #[test]
    fn ctrl_z_takes_back_a_version_put_back() {
        let mut app = on_daily("SELECT 2");
        listed(&mut app, &["SELECT 1"]);
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(app.sheet().editor.text(), "SELECT 2");
    }

    #[test]
    fn putting_a_version_back_writes_nothing() {
        let mut app = on_daily("SELECT 2");
        listed(&mut app, &["SELECT 1"]);
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Enter)), []);
    }

    #[test]
    fn esc_closes_the_list_and_leaves_the_editor_as_it_was() {
        let mut app = on_daily("SELECT 2");
        listed(&mut app, &["SELECT 1"]);
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!((app.overlay.is_none(), app.sheet().editor.text(), app.sheet().is_dirty), (true, "SELECT 2", false));
    }

    #[test]
    fn versions_read_for_another_query_are_left_out() {
        let mut app = on_daily("SELECT 2");
        ctrl_v(&mut app);
        app.apply(Answer::Versions { query: query("weekly.sql"), versions: vec![Listed::against(version("SELECT 1"), "SELECT 2")] });
        assert_eq!(list(&app).all, []);
    }

    #[test]
    fn a_version_says_the_lines_it_would_bring_back_and_take_out() {
        let listed = Listed::against(version("SELECT id, total\nFROM orders\nLIMIT 10"), "SELECT id\nFROM orders");
        assert_eq!((listed.added, listed.removed, listed.first_changed.as_deref()), (2, 1, Some("SELECT id, total")));
    }

    #[test]
    fn a_version_with_the_editor_text_changes_nothing() {
        assert_eq!(Listed::against(version("SELECT 1"), "SELECT 1").first_changed, None);
    }
}
