//! The sidebar's schema tab: the schemas of the sheet on screen and their tables, folded and filtered, and its cursor.
use super::save::typed_into;
use super::{Action, App, Focus, Overlay};
use crate::act::{self, Act, Pane};
use crate::db::model::{Catalog, Table};
use crossterm::event::KeyEvent;
use std::collections::BTreeSet;
use vtui::field::Field;

/// Which tab the sidebar shows; `[` and `]` switch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) enum SidebarTab {
    #[default]
    Queries,
    Schema,
}

/// The schemas folded, the filter typed with `/`, and the cursor's row.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::tui) struct SchemaTab {
    pub(in crate::tui) selected: usize,
    closed: BTreeSet<String>,
    pub(in crate::tui) filter: Field,
}

/// One line of the schema tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum SchemaRow<'a> {
    Schema { name: &'a str, is_open: bool },
    Table { schema: &'a str, table: &'a Table },
}

impl SchemaTab {
    /// The schemas holding a table the filter keeps, each with those tables unless it is folded.
    pub(in crate::tui) fn rows<'a>(&self, catalog: &'a Catalog) -> Vec<SchemaRow<'a>> {
        let filter = self.filter.text().to_lowercase();
        let mut rows = vec![];
        for node in &catalog.schema.schemas {
            let mut kept = node.tables.iter().filter(|table| node.holds(table, &filter)).peekable();
            if kept.peek().is_none() {
                continue;
            }
            let is_open = !self.closed.contains(&node.name);
            rows.push(SchemaRow::Schema { name: &node.name, is_open });
            if is_open {
                rows.extend(kept.map(|table| SchemaRow::Table { schema: &node.name, table }));
            }
        }
        rows
    }
}

impl App {
    /// The pane whose keys work: the sidebar's own depends on its tab.
    pub(in crate::tui) fn pane(&self) -> Pane {
        match (self.focus, self.sidebar) {
            (Focus::Queries, SidebarTab::Schema) => Pane::Schema,
            (focus, _) => focus.pane(),
        }
    }

    pub(super) fn switch_tab(&mut self) {
        self.sidebar = match self.sidebar {
            SidebarTab::Queries => SidebarTab::Schema,
            SidebarTab::Schema => SidebarTab::Queries,
        };
    }

    /// `actions`, then a read of the schema when its tab is drawn and the sheet on screen has none, read or asked for:
    /// the schema is read the moment it shows, on whichever sheet.
    pub(super) fn and_the_schema_shown(&mut self, mut actions: Vec<Action>) -> Vec<Action> {
        let is_shown = self.sidebar == SidebarTab::Schema && self.fit.has_sidebar;
        if is_shown {
            actions.extend(self.sheet_mut().want_schema());
        }
        actions
    }

    /// What `act` does in the schema tab, when it does something there.
    pub(super) fn schema_act(&mut self, act: Act) -> Option<Vec<Action>> {
        match act {
            Act::Down => self.step_schema(1),
            Act::Up => self.step_schema(-1),
            Act::Open => self.fold_schema(),
            Act::Filter => self.overlay = Some(Overlay::SchemaFilter),
            Act::ReadAgain => return Some(self.sheet_mut().read_schema_again()),
            _ => return None,
        }
        Some(vec![])
    }

    fn schema_rows_len(&self) -> usize {
        self.sheet().catalog().map_or(0, |catalog| self.schema_tab.rows(catalog).len())
    }

    fn step_schema(&mut self, delta: isize) {
        let last = self.schema_rows_len().saturating_sub(1);
        self.schema_tab.selected = self.schema_tab.selected.saturating_add_signed(delta).min(last);
    }

    /// Folds or unfolds the schema under the cursor.
    fn fold_schema(&mut self) {
        let Some(catalog) = self.sheet().catalog() else { return };
        let Some(SchemaRow::Schema { name, is_open }) = self.schema_tab.rows(catalog).get(self.schema_tab.selected).copied() else {
            return;
        };
        let name = name.to_owned();
        let closed = &mut self.schema_tab.closed;
        _ = if is_open { closed.insert(name) } else { closed.remove(&name) };
    }

    /// Types `key` into the schema's filter: `⏎` keeps it, `esc` drops it, and the cursor goes back to the first row.
    pub(super) fn schema_filter_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match act::act_for(key, Pane::SchemaFilter) {
            Some(Act::Quit) => return self.quit(),
            Some(Act::Open) => return vec![],
            Some(Act::Close) => _ = self.schema_tab.filter.take(),
            _ => {
                typed_into(&mut self.schema_tab.filter, key);
                self.overlay = Some(Overlay::SchemaFilter);
            }
        }
        self.schema_tab.selected = 0;
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::super::sheet::SchemaState;
    use super::super::tests::{app, app_on, shop, to_shop};
    use super::super::{Answer, Fit, Incoming, SessionId};
    use super::*;
    use crate::fixtures::{self, shop_schema};
    use crossterm::event::KeyCode;
    use std::sync::Arc;

    fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    fn on_the_sidebar(app: &mut App) {
        app.fit(Fit { has_sidebar: true, ..Fit::default() });
        app.focus = Focus::Queries;
    }

    fn on_the_schema_tab() -> App {
        let mut app = app();
        on_the_sidebar(&mut app);
        press(&mut app, KeyCode::Char(']'));
        let catalog = Catalog { schema: shop_schema(), search_path: vec!["public".to_owned()], is_cut: false };
        app.apply(to_shop(Incoming::Schema(Ok(Arc::new(catalog)))));
        app
    }

    fn shown(app: &App) -> Vec<String> {
        let rows = app.schema_tab.rows(app.sheet().catalog().unwrap());
        let named = |row: &SchemaRow<'_>| match row {
            SchemaRow::Schema { name, .. } => format!("{name}/"),
            SchemaRow::Table { table, .. } => table.name.clone(),
        };
        rows.iter().map(named).collect()
    }

    #[test]
    fn the_schema_tab_reads_the_schema_the_first_time_it_shows() {
        let mut app = app();
        on_the_sidebar(&mut app);
        assert_eq!(press(&mut app, KeyCode::Char(']')), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST }]);
        assert_eq!((app.pane(), &app.sheet().schema), (Pane::Schema, &SchemaState::Reading));
    }

    #[test]
    fn the_schema_tab_shown_again_reads_nothing() {
        let mut app = on_the_schema_tab();
        press(&mut app, KeyCode::Char('['));
        assert_eq!((app.pane(), press(&mut app, KeyCode::Char(']'))), (Pane::Queries, vec![]));
    }

    #[test]
    fn a_sheet_opened_again_under_the_schema_tab_reads_its_schema_at_once() {
        let mut app = on_the_schema_tab();
        let connection = app.sheet().connection.clone();
        app.sheet_mut().reopen(connection, SessionId::FIRST.next());
        let read = Action::ReadSchema { connection: shop(), session: SessionId::FIRST.next() };
        assert_eq!(app.apply(Answer::Copied("1 cell".to_owned())), [read]);
    }

    #[test]
    fn r_in_the_schema_tab_reads_it_again() {
        assert_eq!(
            press(&mut on_the_schema_tab(), KeyCode::Char('r')),
            [Action::ReadSchema { connection: shop(), session: SessionId::FIRST }]
        );
    }

    #[test]
    fn the_schema_tab_lists_each_schema_then_its_tables() {
        assert_eq!(shown(&on_the_schema_tab()), ["billing/", "invoices", "public/", "orders", "customers"]);
    }

    #[test]
    fn enter_on_a_schema_folds_it_and_again_unfolds_it() {
        let mut app = on_the_schema_tab();
        press(&mut app, KeyCode::Enter);
        assert_eq!(shown(&app), ["billing/", "public/", "orders", "customers"]);
        press(&mut app, KeyCode::Enter);
        assert_eq!(shown(&app).len(), 5);
    }

    #[test]
    fn j_moves_the_cursor_down_the_tables_and_stops_at_the_last() {
        let mut app = on_the_schema_tab();
        for _ in 0..9 {
            press(&mut app, KeyCode::Char('j'));
        }
        assert_eq!(app.schema_tab.selected, 4);
    }

    fn filtered(typed: &str) -> App {
        let mut app = on_the_schema_tab();
        press(&mut app, KeyCode::Char('/'));
        for c in typed.chars() {
            press(&mut app, KeyCode::Char(c));
        }
        app
    }

    #[test]
    fn a_filter_keeps_the_tables_whose_name_holds_it() {
        assert_eq!(shown(&filtered("CUST")), ["public/", "customers"]);
    }

    #[test]
    fn enter_keeps_the_filter_and_gives_the_keys_back_to_the_tab() {
        let mut app = filtered("ord");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!((shown(&app), app.schema_tab.selected), (vec!["public/".to_owned(), "orders".to_owned()], 1));
    }

    #[test]
    fn esc_drops_the_filter() {
        let mut app = filtered("ord");
        press(&mut app, KeyCode::Esc);
        assert_eq!((shown(&app).len(), app.overlay.is_none()), (5, true));
    }

    #[test]
    fn a_connection_whose_schema_is_off_shows_the_tab_without_reading_it() {
        let off = fixtures::connection("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nschema = \"off\"\n");
        let mut app = app_on(&off);
        on_the_sidebar(&mut app);
        assert_eq!(press(&mut app, KeyCode::Char(']')), []);
    }
}
