//! Which columns of a result the grid shows, in the result's order, and how many of the first stay on the left while the
//! others scroll.
//! The cursor and every act on it name a column by its index in the result; only this module knows its place on screen.
use super::grid::Room;
use super::{App, Grid};
use crate::db::model::Column;
use crate::format::counted;
use crate::keys::Keys;
use std::collections::BTreeSet;

/// The space drawn before each column.
pub(in crate::tui) const GAP: &str = "  ";

/// Which columns stay on the left while the grid scrolls.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Freeze {
    Off,
    /// The primary key's columns the result starts with, or else its first column.
    Keys,
    /// The columns up to this one, by index.
    Through(usize),
}

/// What was chosen for a result's columns, by index.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ColumnChoice {
    freeze: Freeze,
    hidden: BTreeSet<usize>,
}

/// A result's columns and what was chosen for them: the next result with the same columns takes it on, and any other
/// ignores it, since its indexes mean nothing there.
#[derive(Debug)]
pub(super) struct KeptColumns {
    columns: Vec<Column>,
    choice: ColumnChoice,
}

/// The columns the grid shows, each named by its index in the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct ShownColumns {
    choice: ColumnChoice,
    /// The last column [`Freeze::Keys`] freezes in this result.
    last_key: usize,
    /// How many columns the result has.
    count: usize,
    /// The columns not hidden, in the result's order.
    order: Vec<usize>,
    /// How many of the first in `order` are frozen.
    frozen: usize,
}

impl ShownColumns {
    /// Every column of a result of `count` columns with these `keys`, the key it starts with frozen.
    pub(super) fn new(keys: &Keys, count: usize) -> Self {
        let last_key = keys.columns().iter().take_while(|key| key.is_primary).count().saturating_sub(1);
        let choice = ColumnChoice { freeze: Freeze::Keys, hidden: BTreeSet::new() };
        let mut shown = Self { choice, last_key, count, order: vec![], frozen: 0 };
        shown.arrange();
        shown
    }

    /// What was chosen for this result's `columns`, to keep past it.
    pub(super) fn keep(&self, columns: &[Column]) -> KeptColumns {
        KeptColumns { columns: columns.to_vec(), choice: self.choice.clone() }
    }

    /// Takes on `kept` when it was chosen for these `columns`.
    pub(super) fn take_kept(&mut self, kept: &KeptColumns, columns: &[Column]) {
        if kept.columns == columns {
            self.choice = kept.choice.clone();
            self.arrange();
        }
    }

    fn arrange(&mut self) {
        self.order = (0..self.count).filter(|column| !self.choice.hidden.contains(column)).collect();
        self.frozen = match self.choice.freeze {
            Freeze::Off => 0,
            Freeze::Keys => self.order.partition_point(|&column| column <= self.last_key),
            Freeze::Through(last) => self.order.partition_point(|&column| column <= last),
        };
    }

    /// How many columns are hidden.
    pub(in crate::tui) fn hidden_count(&self) -> usize {
        self.choice.hidden.len()
    }

    /// The column drawn first; the result's first when it has no column.
    pub(super) fn first(&self) -> usize {
        self.order.first().copied().unwrap_or(0)
    }

    /// Where `column` is drawn among the columns not hidden.
    pub(super) fn place_of(&self, column: usize) -> Option<usize> {
        self.order.iter().position(|&shown| shown == column)
    }

    /// The column drawn before `column`; `column` itself when it is the first.
    pub(super) fn before(&self, column: usize) -> usize {
        self.place_of(column).map_or(column, |place| self.order[place.saturating_sub(1)])
    }

    /// The column drawn after `column`; `column` itself when it is the last.
    pub(super) fn after(&self, column: usize) -> usize {
        self.place_of(column).and_then(|place| self.order.get(place + 1).copied()).unwrap_or(column)
    }

    /// The column drawn next to `column`, the one after it if any; none when it is the only one shown.
    pub(super) fn neighbour_of(&self, column: usize) -> Option<usize> {
        [self.after(column), self.before(column)].into_iter().find(|&shown| shown != column)
    }

    /// Freezes nothing, or the key once nothing is frozen.
    pub(super) fn switch_freeze(&mut self) {
        self.choice.freeze = if self.choice.freeze == Freeze::Off { Freeze::Keys } else { Freeze::Off };
        self.arrange();
    }

    /// Freezes the columns up to `column`, itself included.
    pub(super) fn freeze_through(&mut self, column: usize) {
        self.choice.freeze = Freeze::Through(column);
        self.arrange();
    }

    pub(super) fn hide(&mut self, column: usize) {
        self.choice.hidden.insert(column);
        self.arrange();
    }

    pub(super) fn show_all(&mut self) {
        self.choice.hidden.clear();
        self.arrange();
    }

    /// The frozen columns that stay on screen in `room` terminal columns: as many as fit in half of it, so the others
    /// always have the rest; those past it scroll with the others.
    fn pinned(&self, widths: &[usize], room: usize) -> &[usize] {
        let mut taken = 0;
        let fits = |column: &&usize| {
            taken += drawn_width(widths, **column);
            2 * taken <= room
        };
        let fitting = self.order[..self.frozen].iter().take_while(fits).count();
        &self.order[..fitting]
    }

    /// The columns to draw in `room` terminal columns, left to right, when the first to scroll is at place `left`.
    pub(super) fn in_view(&self, left: usize, widths: &[usize], room: usize) -> impl Iterator<Item = usize> {
        let pinned = self.pinned(widths, room);
        let scrolled = self.order.get(left.max(pinned.len())..).unwrap_or_default();
        pinned.iter().chain(scrolled).copied()
    }

    /// The first column drawn past the pinned ones in `room` terminal columns, when the first to scroll is at place `left`.
    pub(super) fn first_scrolled(&self, left: usize, widths: &[usize], room: usize) -> Option<usize> {
        self.order.get(left.max(self.pinned(widths, room).len())).copied()
    }

    /// The place to scroll from, starting at `left`, moving the least to show `column` whole beside the pinned columns
    /// in `room` terminal columns; a pinned column needs no scrolling.
    pub(super) fn left_showing(&self, column: usize, left: usize, widths: &[usize], room: usize) -> usize {
        let pinned = self.pinned(widths, room);
        let Some(place) = self.place_of(column).filter(|&place| place >= pinned.len()) else { return left };
        let first = left.max(pinned.len()).min(place);
        let total = |columns: &[usize]| columns.iter().map(|&column| drawn_width(widths, column)).sum::<usize>();
        (first..place).find(|&from| total(pinned) + total(&self.order[from..=place]) <= room).unwrap_or(place)
    }
}

/// The terminal columns the column at `column` takes, after its gap.
fn drawn_width(widths: &[usize], column: usize) -> usize {
    GAP.len() + widths[column]
}

impl App {
    /// Applies `change` to the grid, and notes how many columns it left frozen.
    pub(super) fn change_freeze(&mut self, change: fn(&mut Grid, Room)) {
        self.note = self.on_grid(|grid, room| {
            change(grid, room);
            format!("{} frozen", counted(grid.columns().frozen, "column"))
        });
    }

    /// Hides the column under the cursor, which moves next to it; the last column shown stays, and the note says so.
    pub(super) fn hide_column(&mut self) {
        if self.on_grid(Grid::hide_cursor_column) == Some(false) {
            self.note = Some("the last column stays in the grid".to_owned());
        }
    }

    pub(super) fn show_every_column(&mut self) {
        self.on_grid(Grid::show_every_column);
    }

    /// `change` on the grid on screen, in the room the last frame gave it; nothing while no result shows.
    fn on_grid<T>(&mut self, change: impl FnOnce(&mut Grid, Room) -> T) -> Option<T> {
        let room = self.fit.grid;
        self.sheet_mut().grid_mut().map(|grid| change(grid, room))
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        click_at, cursor_name, drawn, grid, key, key_in_the_middle, on_the_keyed_orders, past_a_hidden_key, press, pressed, run_again,
        with_keys,
    };
    use super::super::{Action, DetailScroll, Overlay, Run, RunId};
    use super::*;
    use crate::db::model::{ColumnKind, JsonText, ResultSet, Value};
    use crate::fixtures::{column, keyed_catalog, keyed_orders, orders};
    use crate::keys::keys;
    use crossterm::event::{KeyCode, KeyEvent};

    fn first_frozen(count: usize) -> ShownColumns {
        ShownColumns::new(&Keys::default(), count)
    }

    #[test]
    fn a_result_starting_with_its_key_freezes_every_column_of_that_run() {
        let mut catalog = keyed_catalog();
        catalog.schema.schemas[1].tables[0].indexes[0].columns.push("customer_id".to_owned());
        assert_eq!(ShownColumns::new(&keys(&catalog, keyed_orders().columns()), 3).frozen, 2);
    }

    #[test]
    fn a_result_with_its_key_further_on_freezes_its_first_column_and_opens_there() {
        let app = key_in_the_middle();
        assert_eq!((drawn(&app), grid(&app).columns().frozen, cursor_name(&app)), (vec![0, 1, 2], 1, "total".to_owned()));
    }

    #[test]
    fn right_and_left_go_through_the_columns_past_a_hidden_one() {
        assert_eq!(cursor_name(&with_keys(on_the_keyed_orders(), "lxh")), "id");
    }

    #[test]
    fn z_freezes_nothing_then_the_key_again() {
        assert_eq!(pressed(orders(), "z").note(), Some("0 columns frozen"));
        assert_eq!(pressed(orders(), "zz").note(), Some("1 column frozen"));
    }

    #[test]
    fn scrolling_to_a_column_keeps_room_for_the_frozen_one() {
        assert_eq!(first_frozen(10).left_showing(5, 0, &[8; 10], 40), 3);
    }

    #[test]
    fn a_frozen_cursor_leaves_the_scroll_where_it_is() {
        assert_eq!(first_frozen(10).left_showing(0, 6, &[8; 10], 40), 6);
    }

    #[test]
    fn shift_z_freezes_the_columns_drawn_up_to_the_cursor_and_says_how_many() {
        assert_eq!(pressed(orders(), "lZ").note(), Some("2 columns frozen"));
    }

    #[test]
    fn x_hides_the_column_and_the_cursor_goes_to_the_next() {
        let app = pressed(orders(), "x");
        assert_eq!((drawn(&app), cursor_name(&app)), (vec![1, 2], "total".to_owned()));
    }

    #[test]
    fn x_leaves_the_last_column_shown_and_says_so() {
        let app = pressed(orders(), "xxx");
        assert_eq!((drawn(&app), app.note()), (vec![2], Some("the last column stays in the grid")));
    }

    #[test]
    fn shift_x_shows_every_column_and_leaves_the_cursor_on_its_column() {
        let app = pressed(orders(), "xX");
        assert_eq!((drawn(&app), cursor_name(&app)), (vec![0, 1, 2], "total".to_owned()));
    }

    #[test]
    fn s_sorts_by_the_column_under_the_cursor_past_a_hidden_one() {
        assert_eq!(grid(&with_keys(past_a_hidden_key(), "s")).shown().sort().map(|sort| sort.column), Some(2));
    }

    #[test]
    fn i_counts_the_column_under_the_cursor_past_a_hidden_one() {
        let app = with_keys(past_a_hidden_key(), "i");
        assert!(matches!(&app.overlay, Some(Overlay::ColumnStats(stats)) if stats.column.as_str() == "total"));
    }

    #[test]
    fn equal_binds_the_column_under_the_cursor_past_a_hidden_one() {
        assert!(with_keys(past_a_hidden_key(), "=").sheet().editor.text().ends_with("WHERE total = {{total}}"));
    }

    #[test]
    fn o_opens_the_cell_under_the_cursor_past_a_hidden_one() {
        let json = Value::Json(JsonText::of(&serde_json::json!({"sku": "a"})));
        let columns = vec![column("id", ColumnKind::Int), column("total", ColumnKind::Int), column("data", ColumnKind::Json)];
        let app = pressed(ResultSet::whole(columns, vec![vec![Value::Int(7), Value::Int(9), json]]), "lxo");
        assert!(matches!(&app.overlay, Some(Overlay::JsonTree(tree)) if tree.column.as_str() == "data"));
    }

    #[test]
    fn enter_follows_the_foreign_key_under_the_cursor_past_a_hidden_column() {
        let mut app = with_keys(on_the_keyed_orders(), "x");
        let actions = app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(&actions[0], Action::LookUp { lookup, .. } if lookup.statement.as_str().contains("customers")), "{actions:?}");
    }

    #[test]
    fn the_row_detail_opens_on_the_cursors_column_and_lists_the_hidden_ones() {
        let mut app = pressed(orders(), "lx");
        press(&mut app, KeyCode::Enter);
        let Some(Overlay::Detail(detail)) = &app.overlay else { panic!("no detail") };
        assert_eq!((app.shown_row(detail).map(|shown| shown.columns.len()), detail.scroll), (Some(3), DetailScroll::ToColumn(2)));
    }

    #[test]
    fn a_click_on_the_grid_leaves_the_cursor_on_its_column() {
        let mut app = pressed(orders(), "x");
        click_at(&mut app, 40, 12);
        assert_eq!(cursor_name(&app), "total");
    }

    #[test]
    fn a_filter_still_looks_through_the_hidden_columns() {
        let app = pressed(orders(), "llx/zoë");
        let Run::Done { grid, result, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(grid.shown().count(result.rows().len()), 1);
    }

    #[test]
    fn copying_the_row_still_holds_the_hidden_columns() {
        let actions = key(pressed(orders(), "x"), 'Y');
        assert!(matches!(&actions[..], [Action::Copy { text, .. }] if text.starts_with("7,12.50,")), "{actions:?}");
    }

    #[test]
    fn copying_the_result_still_holds_the_hidden_columns() {
        let actions = key(pressed(orders(), "xc"), 'c');
        assert!(matches!(&actions[..], [Action::CopyResult { result, .. }] if result.columns().len() == 3), "{actions:?}");
    }

    #[test]
    fn running_again_with_the_same_columns_keeps_what_was_hidden_and_frozen() {
        let app = run_again(pressed(orders(), "lxZ"), RunId::FIRST.next(), orders());
        assert_eq!((drawn(&app), grid(&app).columns().frozen), (vec![0, 2], 2));
    }

    #[test]
    fn running_a_statement_with_other_columns_shows_them_all_again() {
        let other = ResultSet::whole(vec![column("id", ColumnKind::Int), column("name", ColumnKind::Text)], vec![]);
        assert_eq!(grid(&run_again(pressed(orders(), "x"), RunId::FIRST.next(), other)).columns().hidden_count(), 0);
    }

    #[test]
    fn a_tables_columns_shown_between_two_runs_leave_the_choice_kept() {
        let mut app = pressed(orders(), "x");
        let reading = app.reading;
        app.sheet_mut().show("orders".to_owned(), keyed_orders(), &reading);
        assert_eq!(grid(&run_again(app, RunId::FIRST.next().next(), orders())).columns().hidden_count(), 1);
    }
}
