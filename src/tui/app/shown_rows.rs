//! Which rows of a result the grid shows, and in which order: sorted by the column `s` picked, among the rows loaded.
//! An index over the result's rows, so sorting never copies a row.
use super::App;
use crate::db::model::{ResultSet, Value};
use std::cmp::Ordering;

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

/// The rows the grid shows: the sort, and the index in the result of each row in the order shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct ShownRows {
    sort: Option<Sort>,
    /// The index in the result of each row shown, in the order shown; none while every row shows in the result's order.
    rows: Option<Vec<usize>>,
}

impl ShownRows {
    /// The rows of `result` sorted by `sort`.
    pub(super) fn new(sort: Option<Sort>, result: &ResultSet) -> Self {
        let Some(sort) = sort else { return Self::default() };
        let mut rows: Vec<usize> = (0..result.rows().len()).collect();
        rows.sort_by(|&a, &b| sort.compare(&result.rows()[a], &result.rows()[b]));
        Self { sort: Some(sort), rows: Some(rows) }
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

impl App {
    /// Sorts the grid's rows by the column under its cursor, the next way.
    pub(super) fn step_sort(&mut self) {
        let clock = self.reading.clock;
        self.sheet_mut().step_sort(&clock);
    }
}

#[cfg(test)]
mod tests {
    use super::super::Run;
    use super::super::fixtures::{on_the_result, press};
    use super::*;
    use crate::db::model::{ColumnKind, Extent};
    use crate::fixtures::{column, orders, page};
    use crossterm::event::{KeyCode, KeyEvent};

    fn sorted(order: Order, values: Vec<Value>) -> Vec<usize> {
        let result = ResultSet::whole(vec![column("total", ColumnKind::Numeric)], values.into_iter().map(|value| vec![value]).collect());
        let shown = ShownRows::new(Some(Sort { column: 0, order }), &result);
        (0..).map_while(|place| shown.row_at(place, result.rows().len())).collect()
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

    /// The wall clock, read in a release build only, by CI's own step: `cargo test --release --lib -- --ignored budget::`.
    #[cfg(not(debug_assertions))]
    mod budget {
        use super::*;
        use crate::fixtures::{best_of, capped};
        use std::time::Duration;

        /// Sorting ten thousand rows of fifty columns by a `NUMERIC` column, read as numbers: 0.5 ms on a laptop.
        const SORT_BUDGET: Duration = Duration::from_millis(5);

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn sorting_ten_thousand_rows_stays_within_the_budget() {
            let result = capped();
            let sort = Some(Sort { column: 2, order: Order::Descending });
            let took = best_of(|| ShownRows::new(sort, &result));
            assert!(took < SORT_BUDGET, "took {took:?}");
        }
    }
}
