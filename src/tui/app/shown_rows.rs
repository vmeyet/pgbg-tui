//! Which rows of a result the grid shows, and in which order: those holding what `/` typed, sorted by the column `s`
//! picked, among the rows loaded.
//! An index over the result's rows, so filtering and sorting never copy a row.
use super::save::typed_into;
use super::{Action, App, Overlay, Run};
use crate::act::{self, Act, Pane};
use crate::db::model::{ResultSet, Value};
use crossterm::event::KeyEvent;
use std::cmp::Ordering;
use vtui::field::Field;

/// Which way `s` sorts a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Order {
    Ascending,
    Descending,
}

/// The column the rows are sorted by, by index, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Sort {
    pub(in crate::tui) column: usize,
    pub(in crate::tui) order: Order,
}

impl Sort {
    /// What `s` on `column` gives after `sort`: ascending, then descending, then the result's own order.
    pub(super) fn after(sort: Option<Self>, column: usize) -> Option<Self> {
        match sort {
            Some(Self { column: sorted, order: Order::Ascending }) if sorted == column => Some(Self { column, order: Order::Descending }),
            Some(Self { column: sorted, order: Order::Descending }) if sorted == column => None,
            _ => Some(Self { column, order: Order::Ascending }),
        }
    }

    /// How the row `a` sorts against `b`; NULL comes last whichever way.
    fn compare(self, a: &[Value], b: &[Value]) -> Ordering {
        let (a, b) = (&a[self.column], &b[self.column]);
        match (a, b, self.order) {
            (Value::Null, Value::Null, _) => Ordering::Equal,
            (Value::Null, _, _) => Ordering::Greater,
            (_, Value::Null, _) => Ordering::Less,
            (_, _, Order::Ascending) => a.compare(b),
            (_, _, Order::Descending) => b.compare(a),
        }
    }
}

/// The rows the grid shows: the filter and the sort, and the index in the result of each row they keep.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct ShownRows {
    /// What `/` typed: a row shows when the text of one of its cells holds it, whatever the case.
    filter: String,
    sort: Option<Sort>,
    /// The index in the result of each row shown, in the order shown; none while every row shows in the result's order.
    rows: Option<Vec<usize>>,
}

impl ShownRows {
    /// The rows of `result` that hold `filter` in the text `read` gives one of their cells, sorted by `sort`.
    /// `read` takes a cell's column and value.
    pub(super) fn new(filter: String, sort: Option<Sort>, result: &ResultSet, read: impl Fn(usize, &Value) -> String) -> Self {
        if filter.is_empty() && sort.is_none() {
            return Self::default();
        }
        let needle = filter.to_lowercase();
        let mut rows: Vec<usize> = (0..result.rows().len()).filter(|&at| holds(&result.rows()[at], &needle, &read)).collect();
        if let Some(sort) = sort {
            rows.sort_by(|&a, &b| sort.compare(&result.rows()[a], &result.rows()[b]));
        }
        Self { filter, sort, rows: Some(rows) }
    }

    /// The rows shown once `filter` takes the place of this one, in the same order: typing on looks only through the
    /// rows this filter kept, so each key costs less than the one before.
    pub(super) fn filtered(&self, filter: String, result: &ResultSet, read: impl Fn(usize, &Value) -> String) -> Self {
        let needle = filter.to_lowercase();
        match &self.rows {
            Some(rows) if !self.filter.is_empty() && needle.contains(&self.filter.to_lowercase()) => {
                let rows = rows.iter().copied().filter(|&at| holds(&result.rows()[at], &needle, &read)).collect();
                Self { filter, sort: self.sort, rows: Some(rows) }
            }
            _ => Self::new(filter, self.sort, result, read),
        }
    }

    pub(in crate::tui) fn filter(&self) -> &str {
        &self.filter
    }

    pub(in crate::tui) const fn sort(&self) -> Option<Sort> {
        self.sort
    }

    /// Whether every row loaded shows, in the result's order.
    pub(super) const fn is_whole(&self) -> bool {
        self.rows.is_none()
    }

    /// How many of the `loaded` rows show.
    pub(in crate::tui) fn count(&self, loaded: usize) -> usize {
        self.rows.as_ref().map_or(loaded, Vec::len)
    }

    /// The index in the result of the row shown at `place`, among `loaded` rows.
    pub(in crate::tui) fn row_at(&self, place: usize, loaded: usize) -> Option<usize> {
        match &self.rows {
            Some(rows) => rows.get(place).copied(),
            None => (place < loaded).then_some(place),
        }
    }

    /// Where the result's row `row` shows, when it does.
    pub(super) fn place_of(&self, row: usize) -> Option<usize> {
        match &self.rows {
            Some(rows) => rows.iter().position(|&shown| shown == row),
            None => Some(row),
        }
    }
}

/// Whether the text `read` gives one of the cells of `row` holds `needle`, which is in lower case, whatever its case.
fn holds(row: &[Value], needle: &str, read: impl Fn(usize, &Value) -> String) -> bool {
    needle.is_empty() || row.iter().enumerate().any(|(column, value)| read(column, value).to_lowercase().contains(needle))
}

impl App {
    /// Opens the grid's filter on the footer line, holding the filter shown.
    pub(super) fn open_row_filter(&mut self) {
        let Run::Done { grid, .. } = &self.sheet().run else { return };
        self.overlay = Some(Overlay::RowFilter(Field::new(grid.shown().filter())));
    }

    /// Types `key` into the grid's filter, which filters the rows at once: `⏎` keeps it, `esc` drops it.
    pub(super) fn row_filter_key(&mut self, mut filter: Field, key: KeyEvent) -> Vec<Action> {
        match act::act_for(key, Pane::RowFilter) {
            Some(Act::Quit) => return self.quit(),
            Some(Act::Open) => {}
            Some(Act::Close) => self.filter_rows(String::new()),
            _ => {
                let before = filter.text().to_owned();
                typed_into(&mut filter, key);
                if filter.text() != before {
                    self.filter_rows(filter.text().to_owned());
                }
                self.overlay = Some(Overlay::RowFilter(filter));
            }
        }
        vec![]
    }

    /// Pastes `text` into the grid's filter, which filters the rows at once.
    pub(super) fn paste_in_row_filter(&mut self, text: &str) {
        let Some(Overlay::RowFilter(filter)) = &mut self.overlay else { return };
        filter.paste(text);
        let typed = filter.text().to_owned();
        self.filter_rows(typed);
    }

    fn filter_rows(&mut self, filter: String) {
        let clock = self.reading.clock;
        self.sheet_mut().filter_rows(filter, &clock);
    }

    /// Sorts the grid's rows by the column under its cursor, the next way.
    pub(super) fn step_sort(&mut self) {
        if let Some(column) = self.sheet().undecoded_column() {
            self.note = Some(format!("{column} cannot be sorted: pgbg does not decode its type"));
            return;
        }
        let clock = self.reading.clock;
        self.sheet_mut().step_sort(&clock);
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{on_the_result, press};
    use super::*;
    use crate::db::model::{ColumnKind, Extent};
    use crate::fixtures::{column, orders, page, undecoded};
    use crossterm::event::KeyCode;

    fn shown(filter: &str, sort: Option<Sort>, result: &ResultSet) -> Vec<usize> {
        let shown = ShownRows::new(filter.to_owned(), sort, result, |_, value| value.to_string());
        (0..).map_while(|place| shown.row_at(place, result.rows().len())).collect()
    }

    fn sorted(order: Order, values: Vec<Value>) -> Vec<usize> {
        let result = ResultSet::whole(vec![column("total", ColumnKind::Numeric)], values.into_iter().map(|value| vec![value]).collect());
        shown("", Some(Sort { column: 0, order }), &result)
    }

    #[test]
    fn a_filter_keeps_the_rows_with_a_cell_holding_it_whatever_the_case() {
        assert_eq!(shown("ZOË", None, &orders()), [2]);
    }

    #[test]
    fn no_filter_and_no_sort_show_every_row_in_order() {
        assert_eq!(shown("", None, &orders()), (0..orders().rows().len()).collect::<Vec<_>>());
    }

    #[test]
    fn ascending_puts_null_last() {
        let values = vec![Value::Numeric("10".into()), Value::Null, Value::Numeric("9.5".into())];
        assert_eq!(sorted(Order::Ascending, values), [2, 0, 1]);
    }

    #[test]
    fn descending_still_puts_null_last() {
        let values = vec![Value::Null, Value::Numeric("9.5".into()), Value::Numeric("10".into())];
        assert_eq!(sorted(Order::Descending, values), [2, 1, 0]);
    }

    #[test]
    fn equal_values_keep_the_results_order() {
        assert_eq!(sorted(Order::Descending, vec![Value::Int(1), Value::Int(1)]), [0, 1]);
    }

    #[test]
    fn s_on_one_column_sorts_ascending_then_descending_then_not() {
        let ascending = Sort::after(None, 1);
        let descending = Sort::after(ascending, 1);
        assert_eq!(
            [ascending, descending, Sort::after(descending, 1)].map(|sort| sort.map(|sort| sort.order)),
            [Some(Order::Ascending), Some(Order::Descending), None]
        );
    }

    #[test]
    fn s_on_another_column_sorts_it_ascending() {
        let descending = Some(Sort { column: 1, order: Order::Descending });
        assert_eq!(Sort::after(descending, 2), Some(Sort { column: 2, order: Order::Ascending }));
    }

    fn pressed(result: ResultSet, keys: &str) -> App {
        let mut app = on_the_result(result);
        keys.chars().for_each(|c| press(&mut app, KeyCode::Char(c)));
        app
    }

    fn cursor_row(app: &App) -> usize {
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        grid.row
    }

    fn shown_count(app: &App) -> usize {
        let Run::Done { grid, result, .. } = &app.sheet().run else { panic!("no result") };
        grid.shown().count(result.rows().len())
    }

    #[test]
    fn s_on_a_column_pgbg_cannot_decode_sorts_nothing_and_says_so() {
        let app = pressed(undecoded(), "s");
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!((grid.shown().sort(), app.note()), (None, Some("shape cannot be sorted: pgbg does not decode its type")));
    }

    #[test]
    fn each_key_typed_in_the_filter_filters_the_rows_at_once() {
        assert_eq!(shown_count(&pressed(orders(), "/n")), 4);
        assert_eq!(shown_count(&pressed(orders(), "/no")), 1);
    }

    #[test]
    fn enter_keeps_the_filter_and_gives_the_keys_back_to_the_grid() {
        let mut app = pressed(orders(), "/no");
        press(&mut app, KeyCode::Enter);
        assert_eq!((app.overlay.is_none(), shown_count(&app)), (true, 1));
    }

    #[test]
    fn esc_drops_the_filter() {
        let mut app = pressed(orders(), "/no");
        press(&mut app, KeyCode::Esc);
        assert_eq!((app.overlay.is_none(), shown_count(&app)), (true, 4));
    }

    #[test]
    fn slash_again_opens_the_filter_with_its_text() {
        let mut app = pressed(orders(), "/no");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.overlay, Some(Overlay::RowFilter(Field::new("no"))));
    }

    #[test]
    fn down_moves_to_the_next_row_shown() {
        let mut app = pressed(orders(), "/na");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(cursor_row(&app), 2);
    }

    #[test]
    fn a_sort_puts_the_cursor_on_its_first_row() {
        assert_eq!(cursor_row(&pressed(orders(), "lss")), 1);
    }

    #[test]
    fn down_moves_to_the_next_row_in_the_order_shown() {
        assert_eq!(cursor_row(&pressed(orders(), "lsj")), 2);
    }

    #[test]
    fn a_sorted_grid_asks_for_no_page_past_the_rows_loaded() {
        let mut app = pressed(ResultSet::new(vec![column("n", ColumnKind::Int)], page(500, Extent::More)), "s");
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Char('G'))), []);
    }

    #[test]
    fn a_filter_that_keeps_no_row_leaves_no_cell_to_copy() {
        let mut app = pressed(orders(), "/nobody");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Char('y'))), []);
    }

    #[test]
    fn a_filtered_grid_asks_for_no_page_past_the_rows_loaded() {
        let mut app = pressed(ResultSet::new(vec![column("n", ColumnKind::Int)], page(500, Extent::More)), "/9");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Char('G'))), []);
    }

    /// The wall clock, read in a release build only, by CI's own step: `cargo test --release --lib -- --ignored budget::`.
    #[cfg(not(debug_assertions))]
    mod budget {
        use super::*;
        use crate::fixtures::{best_of, capped};
        use crate::format::{self, Format};
        use crate::tui::app::grid::clock_at_epoch;
        use std::time::Duration;

        /// Filtering ten thousand rows of fifty columns on a text no cell holds reads every cell: 50 ms on a laptop.
        const FILTER_BUDGET: Duration = Duration::from_millis(500);
        /// Typing one more character looks only through the rows kept, here a tenth of them: 5 ms on a laptop.
        const TYPING_ON_BUDGET: Duration = Duration::from_millis(50);
        /// Sorting them by a `NUMERIC` column, read as numbers: 0.5 ms on a laptop.
        const SORT_BUDGET: Duration = Duration::from_millis(5);

        fn read(_: usize, value: &Value) -> String {
            format::cell(value, Format::Raw, &clock_at_epoch(), 200)
        }

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn filtering_ten_thousand_rows_on_a_text_no_cell_holds_stays_within_the_budget() {
            let result = capped();
            let took = best_of(|| ShownRows::new("nobody".to_owned(), None, &result, read));
            assert!(took < FILTER_BUDGET, "took {took:?}");
        }

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn sorting_ten_thousand_rows_stays_within_the_budget() {
            let result = capped();
            let sort = Some(Sort { column: 2, order: Order::Descending });
            let took = best_of(|| ShownRows::new(String::new(), sort, &result, read));
            assert!(took < SORT_BUDGET, "took {took:?}");
        }

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn typing_on_in_the_filter_of_ten_thousand_rows_stays_within_the_budget() {
            let result = capped();
            let typed = ShownRows::new("customer 9".to_owned(), None, &result, read);
            let took = best_of(|| typed.filtered("customer 99".to_owned(), &result, read));
            assert!(took < TYPING_ON_BUDGET, "took {took:?}");
        }
    }
}
