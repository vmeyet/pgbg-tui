//! `ctrl-t`: every table in the schema of the sheet on screen, narrowed as fzf does, with the schema tab's actions.
use super::schema_tab::PickedTable;
use super::search::{Search, Searched, Searching};
use super::{Action, App, Overlay};
use crate::act::{Act, Pane};
use crate::db::model::Catalog;
use crossterm::event::KeyEvent;

/// Every table of the schema and the search over them; empty until the schema is read.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct TableFinder {
    pub(in crate::tui) all: Vec<PickedTable>,
    pub(in crate::tui) search: Search<PickedTable>,
}

impl Searched for PickedTable {
    fn search_text(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}

fn tables_of(catalog: &Catalog) -> Vec<PickedTable> {
    let tables = catalog
        .schema
        .schemas
        .iter()
        .flat_map(|node| node.tables.iter().map(|table| PickedTable { schema: node.name.clone(), name: table.name.clone() }));
    tables.collect()
}

impl App {
    /// Opens the finder over the tables of the sheet on screen, and asks for its schema the first time.
    pub(super) fn find_table(&mut self) -> Vec<Action> {
        let all = self.sheet().catalog().map(tables_of).unwrap_or_default();
        let search = Search::new(&all);
        self.overlay = Some(Overlay::TableFinder(Box::new(TableFinder { all, search })));
        self.sheet_mut().want_schema()
    }

    /// Fills an open finder that was waiting for the schema, once the schema is read.
    pub(super) fn fill_table_finder(&mut self) {
        let Some(Overlay::TableFinder(finder)) = &self.overlay else { return };
        let Some(catalog) = self.sheet().catalog().filter(|_| finder.all.is_empty()) else { return };
        let all = tables_of(catalog);
        let search = Search::new(&all);
        self.overlay = Some(Overlay::TableFinder(Box::new(TableFinder { all, search })));
    }

    /// The finder after `key`: `⏎` shows the highlighted table's columns, `^r` previews it, `⇥` puts its name in the
    /// editor; each closes the finder.
    pub(super) fn table_finder_key(&mut self, finder: TableFinder, key: KeyEvent) -> Vec<Action> {
        let TableFinder { all, search } = finder;
        match search.after(key, Pane::TableFinder, &all) {
            Searching::Typed(search) => self.overlay = Some(Overlay::TableFinder(Box::new(TableFinder { all, search }))),
            Searching::Unhandled(search, act) => return self.act_on_found(TableFinder { all, search }, act),
            Searching::Picked(table) => self.describe_table(&table),
            Searching::Closed => {}
            Searching::Quit => return self.quit(),
        }
        vec![]
    }

    fn act_on_found(&mut self, finder: TableFinder, act: Act) -> Vec<Action> {
        let Some(table) = finder.search.selected().cloned() else {
            self.overlay = Some(Overlay::TableFinder(Box::new(finder)));
            return vec![];
        };
        match act {
            Act::Preview => return self.preview_table(&table),
            Act::Insert => self.insert_table(&table),
            _ => self.overlay = Some(Overlay::TableFinder(Box::new(finder))),
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app, shop, to_shop};
    use super::super::{Focus, Incoming, Run, RunId, SessionId};
    use super::*;
    use crate::db::model::Statement;
    use crate::fixtures::shop_catalog;
    use crate::query::params::ParamInput;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::sync::Arc;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn read(app: &mut App) {
        let catalog = shop_catalog();
        app.apply(to_shop(Incoming::Schema(Ok(Arc::new(catalog)))));
    }

    fn finding(typed: &str) -> App {
        let mut app = app();
        app.handle_key(ctrl('t'));
        read(&mut app);
        typed.chars().for_each(|c| _ = app.handle_key(KeyEvent::from(KeyCode::Char(c))));
        app
    }

    #[test]
    fn ctrl_t_reads_the_schema_the_first_time() {
        assert_eq!(app().handle_key(ctrl('t')), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST }]);
    }

    #[test]
    fn the_finder_lists_every_table_once_the_schema_is_read() {
        let app = finding("");
        let Some(Overlay::TableFinder(finder)) = &app.overlay else { panic!("no finder") };
        assert_eq!(finder.search.shown.len(), 3);
    }

    #[test]
    fn enter_shows_the_columns_of_the_table_a_few_letters_find() {
        let mut app = finding("cstmr");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!((app.sheet().shown.as_deref(), app.overlay.is_none()), (Some("public.customers · 2 columns · 0 indexes"), true));
    }

    #[test]
    fn ctrl_r_previews_the_table_found() {
        let statement = Statement::new("SELECT * FROM billing.invoices LIMIT 100");
        let run = Action::Run { connection: shop(), run: RunId::FIRST, statement, input: ParamInput::default(), confirmed: None };
        assert_eq!(finding("inv").handle_key(ctrl('r')), [run]);
    }

    #[test]
    fn tab_puts_the_name_of_the_table_found_in_the_editor() {
        let mut app = finding("orders");
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!((app.sheet().editor.text(), app.focus), ("public.orders", Focus::Editor));
    }

    #[test]
    fn a_finder_with_no_match_keeps_its_line_on_ctrl_r() {
        let mut app = finding("zzz");
        assert_eq!((app.handle_key(ctrl('r')), app.sheet().run.clone()), (vec![], Run::Idle));
        assert!(matches!(app.overlay, Some(Overlay::TableFinder(_))));
    }
}
