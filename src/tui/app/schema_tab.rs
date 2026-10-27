//! The sidebar's schema tab: the schemas of the sheet on screen and their tables, folded and filtered, and its cursor.
use super::save::typed_into;
use super::{Action, App, Focus, Overlay};
use crate::act::{self, Act, Pane};
use crate::db::model::{
    Catalog, Column, ColumnKind, ColumnName, Extent, Index, Page, ResultSet, SchemaColumn, Statement, Table, TypeName, Value, qualified,
};
use crate::query::params::ParamInput;
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

/// A table the sidebar or the `ctrl-t` finder picked, by its schema and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct PickedTable {
    pub(in crate::tui) schema: String,
    pub(in crate::tui) name: String,
}

impl PickedTable {
    /// The table as a statement writes it.
    fn sql_name(&self) -> String {
        qualified(&self.schema, &self.name)
    }

    fn preview(&self) -> Statement {
        Statement::new(format!("SELECT * FROM {} LIMIT {PREVIEW_ROWS}", self.sql_name()))
    }

    fn in_catalog<'a>(&self, catalog: &'a Catalog) -> Option<&'a Table> {
        let node = catalog.schema.schemas.iter().find(|node| node.name == self.schema)?;
        node.tables.iter().find(|table| table.name == self.name)
    }
}

/// One line of the schema tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum SchemaRow<'a> {
    Schema { name: &'a str, is_open: bool },
    Table { schema: &'a str, table: &'a Table },
}

const PREVIEW_ROWS: usize = 100;
const DESCRIBED: [&str; 4] = ["", "name", "type", "detail"];

/// A table's columns, then its indexes, one a row: what it holds, its name, its type or keys, and the rest.
fn described(table: &Table) -> ResultSet {
    let columns =
        DESCRIBED.iter().map(|name| Column { name: ColumnName::new(*name), kind: ColumnKind::Text, type_name: TypeName::new("text") });
    let rows = table.columns.iter().map(column_row).chain(table.indexes.iter().map(index_row));
    ResultSet::new(columns.collect(), Page { rows: rows.collect(), extent: Extent::All })
}

/// `public.orders · 3 columns · 1 index`.
fn described_title(schema: &str, table: &Table) -> String {
    let counted = |count: usize, one: &str, many: &str| if count == 1 { format!("1 {one}") } else { format!("{count} {many}") };
    let columns = counted(table.columns.len(), "column", "columns");
    let indexes = counted(table.indexes.len(), "index", "indexes");
    format!("{} · {columns} · {indexes}", qualified(schema, &table.name))
}

fn column_row(column: &SchemaColumn) -> Vec<Value> {
    let nullable = if column.is_nullable { None } else { Some("not null".to_owned()) };
    let default = column.default.as_ref().map(|default| format!("default {default}"));
    let detail = [nullable, default].into_iter().flatten().collect::<Vec<_>>().join(" ");
    texts(["column", &column.name, column.type_name.as_str(), &detail])
}

fn index_row(index: &Index) -> Vec<Value> {
    let role = if index.is_primary {
        "primary"
    } else if index.is_unique {
        "unique"
    } else {
        ""
    };
    texts(["index", &index.name, &index.columns.join(", "), role])
}

fn texts(cells: [&str; 4]) -> Vec<Value> {
    cells.into_iter().map(|cell| Value::Text(cell.into())).collect()
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

    /// Switches the sidebar's tab; switching to the schema reads it, opening a failed session again for it.
    pub(super) fn switch_tab(&mut self) -> Vec<Action> {
        self.sidebar = match self.sidebar {
            SidebarTab::Queries => SidebarTab::Schema,
            SidebarTab::Schema => SidebarTab::Queries,
        };
        if self.is_schema_shown() { self.sheet_mut().want_schema() } else { vec![] }
    }

    fn is_schema_shown(&self) -> bool {
        self.sidebar == SidebarTab::Schema && self.fit.has_sidebar
    }

    /// `actions`, then a read of the schema when its tab is drawn, the session on screen is open and has none, read or
    /// asked for: the schema is read the moment it shows, on whichever sheet, and a failed session stays failed.
    pub(super) fn and_the_schema_shown(&mut self, mut actions: Vec<Action>) -> Vec<Action> {
        if self.is_schema_shown() {
            actions.extend(self.sheet_mut().want_schema_while_open());
        }
        actions
    }

    /// What `act` does in the schema tab, when it does something there.
    pub(super) fn schema_act(&mut self, act: Act) -> Option<Vec<Action>> {
        match act {
            Act::Down => self.step_schema(1),
            Act::Up => self.step_schema(-1),
            Act::Open => self.open_schema_row(),
            Act::Preview => return Some(self.selected_table().map(|table| self.preview_table(&table)).unwrap_or_default()),
            Act::Insert => {
                if let Some(table) = self.selected_table() {
                    self.insert_table(&table);
                }
            }
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

    fn selected_row(&self) -> Option<SchemaRow<'_>> {
        self.schema_tab.rows(self.sheet().catalog()?).get(self.schema_tab.selected).copied()
    }

    fn selected_table(&self) -> Option<PickedTable> {
        let Some(SchemaRow::Table { schema, table }) = self.selected_row() else { return None };
        Some(PickedTable { schema: schema.to_owned(), name: table.name.clone() })
    }

    /// Folds or unfolds the schema under the cursor, or shows the columns and indexes of the table there.
    fn open_schema_row(&mut self) {
        match self.selected_row() {
            Some(SchemaRow::Schema { name, is_open }) => {
                let name = name.to_owned();
                let closed = &mut self.schema_tab.closed;
                _ = if is_open { closed.insert(name) } else { closed.remove(&name) };
            }
            Some(SchemaRow::Table { schema, table }) => {
                let picked = PickedTable { schema: schema.to_owned(), name: table.name.clone() };
                self.describe_table(&picked);
            }
            None => {}
        }
    }

    /// Shows the columns and indexes of `picked` in the result pane.
    pub(super) fn describe_table(&mut self, picked: &PickedTable) {
        let Some(table) = self.sheet().catalog().and_then(|catalog| picked.in_catalog(catalog)) else { return };
        let (title, result, reading) = (described_title(&picked.schema, table), described(table), self.reading);
        self.sheet_mut().show(title, result, &reading);
    }

    /// Runs `SELECT * FROM` `picked`, its first rows, as the editor's statements run; a read whatever the table's name,
    /// so it never asks as a write does.
    pub(super) fn preview_table(&mut self, picked: &PickedTable) -> Vec<Action> {
        self.sheet_mut().run_statement(picked.preview(), None, ParamInput::default(), None)
    }

    /// Puts the name of `picked` in the editor at its cursor, and gives the editor the keys.
    pub(super) fn insert_table(&mut self, picked: &PickedTable) {
        self.sheet_mut().paste(&picked.sql_name());
        self.focus = Focus::Editor;
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
    use super::super::sheet::{SchemaState, SessionState};
    use super::super::tests::{app, app_on, shop, to_shop};
    use super::super::{Answer, Fit, Incoming, Run, RunId, SessionId};
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::model::Params;
    use crate::db::{guard, lex};
    use crate::fixtures::{self, shop_catalog};
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
        let catalog = shop_catalog();
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
    fn a_sheet_opened_again_under_the_schema_tab_reads_its_schema_once_its_session_opens() {
        let mut app = on_the_schema_tab();
        let connection = app.sheet().connection.clone();
        app.sheet_mut().reopen(connection, SessionId::FIRST.next());
        let read = Action::ReadSchema { connection: shop(), session: SessionId::FIRST.next() };
        assert_eq!(app.apply(Answer::Copied("1 cell".to_owned())), []);
        let opened = Answer::Session { connection: shop(), session: SessionId::FIRST.next(), incoming: Incoming::Opened };
        assert_eq!(app.apply(opened), [read]);
    }

    fn failed_under_a_hidden_schema_tab() -> App {
        let mut app = app();
        app.sidebar = SidebarTab::Schema;
        app.apply(to_shop(Incoming::NotOpened("connection refused".to_owned())));
        app.fit(Fit { has_sidebar: true, ..Fit::default() });
        app
    }

    #[test]
    fn a_key_typed_once_the_schema_tab_shows_never_opens_a_failed_session_again() {
        let mut app = failed_under_a_hidden_schema_tab();
        assert_eq!(press(&mut app, KeyCode::Char('x')), []);
        assert_eq!(app.sheet().session, SessionState::Failed("connection refused".to_owned()));
    }

    #[test]
    fn switching_to_the_schema_tab_opens_a_failed_session_again_to_read_it() {
        let mut app = failed_under_a_hidden_schema_tab();
        app.sidebar = SidebarTab::Queries;
        app.focus = Focus::Queries;
        assert_eq!(press(&mut app, KeyCode::Char(']')), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST }]);
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

    fn on_orders() -> App {
        let mut app = on_the_schema_tab();
        press(&mut app, KeyCode::Char('/'));
        "public.ord".chars().for_each(|c| _ = press(&mut app, KeyCode::Char(c)));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('j'));
        app
    }

    #[test]
    fn p_on_a_table_runs_its_first_rows_as_a_statement() {
        let statement = Statement::new("SELECT * FROM public.orders LIMIT 100");
        assert_eq!(
            press(&mut on_orders(), KeyCode::Char('p')),
            [Action::Run { connection: shop(), run: RunId::FIRST, statement, input: ParamInput::default(), confirmed: None }]
        );
    }

    #[test]
    fn the_preview_of_a_table_is_a_read_whatever_its_name() {
        let names = ["delete", "update", "insert", "merge", "share", "for update", "x; DROP TABLE orders", "Orders\"; DELETE"];
        for (schema, name) in ["public", "key"].into_iter().flat_map(|schema| names.map(|name| (schema, name))) {
            let preview = PickedTable { schema: schema.into(), name: name.into() }.preview();
            let sql = preview.as_str();
            assert_eq!(guard::write_in(sql, &lex::tokens(sql)), None, "{sql}");
            assert!(guard::admit(preview.clone(), Params::default(), WritePolicy::ReadOnly).is_ok(), "{sql}");
        }
    }

    #[test]
    fn p_on_a_schema_runs_nothing() {
        assert_eq!(press(&mut on_the_schema_tab(), KeyCode::Char('p')), []);
    }

    #[test]
    fn i_on_a_table_puts_its_name_at_the_editors_cursor_and_gives_it_the_keys() {
        let mut app = on_orders();
        app.sheet_mut().paste("SELECT * FROM ");
        press(&mut app, KeyCode::Char('i'));
        assert_eq!((app.sheet().editor.text(), app.focus), ("SELECT * FROM public.orders", Focus::Editor));
    }

    #[test]
    fn enter_on_a_table_shows_its_columns_then_its_indexes() {
        let mut app = on_orders();
        press(&mut app, KeyCode::Enter);
        let Run::Done { result, .. } = &app.sheet().run else { panic!("no result") };
        let names: Vec<&Value> = result.rows().iter().map(|row| &row[1]).collect();
        let text = |name: &str| Value::Text(name.into());
        assert_eq!(names, [&text("id"), &text("customer_id"), &text("total"), &text("orders_pkey")]);
        assert_eq!(result.rows()[2][3], text("default 0"));
    }

    #[test]
    fn a_table_shown_is_titled_by_its_name_and_counts_and_a_run_after_drops_that_title() {
        let mut app = on_orders();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.sheet().shown.as_deref(), Some("public.orders · 3 columns · 1 index"));
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(app.sheet().shown, None);
    }

    #[test]
    fn enter_on_a_table_while_a_statement_runs_leaves_it_running() {
        let mut app = on_orders();
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.sheet().run, Run::Running { run: RunId::FIRST });
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
