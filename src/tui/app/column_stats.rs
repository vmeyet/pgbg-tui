//! `i` on the grid: the figures of the column under the cursor over the rows loaded, in a popup: how many distinct
//! values and NULLs, the least and the greatest, and the most common.
//! Counted on the loop: sorting the index of ten thousand rows by their values takes about a millisecond.
use super::{Action, App, Overlay, Run};
use crate::act::{self, Act, Pane};
use crate::db::model::{ColumnName, ResultSet, Value};
use crate::format::Format;
use crossterm::event::KeyEvent;
use std::cmp::Ordering;

/// How many of the most common values the popup lists.
const MOST_COMMON: usize = 5;

/// A value and how many rows hold it.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Counted {
    pub(in crate::tui) value: Value,
    pub(in crate::tui) rows: usize,
}

/// The figures of one column over the rows loaded; NULL counts apart from the values.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct ColumnStats {
    pub(in crate::tui) column: ColumnName,
    /// The format the column read in on the grid, which its values show in.
    pub(in crate::tui) format: Format,
    /// The rows loaded, which the figures are counted over.
    pub(in crate::tui) rows: usize,
    pub(in crate::tui) distinct: usize,
    pub(in crate::tui) nulls: usize,
    pub(in crate::tui) least: Option<Value>,
    pub(in crate::tui) greatest: Option<Value>,
    /// The most common values, the most held first, a tie in the values' order.
    pub(in crate::tui) common: Vec<Counted>,
}

impl ColumnStats {
    /// The figures of the column at `column` of `result`, its values compared as their type does: numbers as numbers,
    /// times as times; they show in `format`.
    pub(in crate::tui) fn of(result: &ResultSet, column: usize, format: Format) -> Self {
        let value_of = |row: usize| &result.rows()[row][column];
        let mut sorted: Vec<usize> = (0..result.rows().len()).filter(|&row| *value_of(row) != Value::Null).collect();
        sorted.sort_by(|&a, &b| value_of(a).compare(value_of(b)));
        let runs: Vec<&[usize]> = sorted.chunk_by(|&a, &b| value_of(a).compare(value_of(b)) == Ordering::Equal).collect();
        let mut by_rows = runs.clone();
        by_rows.sort_by_key(|run| std::cmp::Reverse(run.len()));
        Self {
            column: result.columns()[column].name.clone(),
            format,
            rows: result.rows().len(),
            distinct: runs.len(),
            nulls: result.rows().len() - sorted.len(),
            least: sorted.first().map(|&row| value_of(row).clone()),
            greatest: sorted.last().map(|&row| value_of(row).clone()),
            common: by_rows.iter().take(MOST_COMMON).map(|run| Counted { value: value_of(run[0]).clone(), rows: run.len() }).collect(),
        }
    }
}

impl App {
    /// Opens the figures of the grid's column over the rows loaded; a column pgbg does not decode has none.
    pub(super) fn open_column_stats(&mut self) {
        if let Some(column) = self.sheet().undecoded_column() {
            self.note = Some(format!("{column} has no figures: pgbg does not decode its type"));
            return;
        }
        let Run::Done { result, grid, .. } = &self.sheet().run else { return };
        if grid.column < result.columns().len() {
            self.overlay = Some(Overlay::ColumnStats(Box::new(ColumnStats::of(result, grid.column, grid.formats[grid.column]))));
        }
    }

    /// The figures after `key`: `esc` or `i` closes them, any other key leaves them open.
    pub(super) fn column_stats_key(&mut self, stats: Box<ColumnStats>, key: KeyEvent) -> Vec<Action> {
        match act::act_for(key, Pane::ColumnStats) {
            Some(Act::Quit) => return self.quit(),
            Some(Act::Close) => {}
            _ => self.overlay = Some(Overlay::ColumnStats(stats)),
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{on_the_result, press};
    use super::*;
    use crate::db::model::ColumnKind;
    use crate::fixtures::{column, orders, undecoded};
    use crossterm::event::KeyCode;

    fn stats(kind: ColumnKind, values: Vec<Value>) -> ColumnStats {
        ColumnStats::of(
            &ResultSet::whole(vec![column("status", kind)], values.into_iter().map(|value| vec![value]).collect()),
            0,
            Format::Raw,
        )
    }

    fn texts(texts: &[&str]) -> Vec<Value> {
        texts.iter().map(|&text| Value::Text(text.into())).collect()
    }

    #[test]
    fn null_is_counted_apart_from_the_distinct_values() {
        let counted = stats(ColumnKind::Text, [texts(&["paid", "open", "paid"]), vec![Value::Null]].concat());
        assert_eq!((counted.distinct, counted.nulls, counted.rows), (2, 1, 4));
    }

    #[test]
    fn the_least_and_the_greatest_compare_numbers_as_numbers() {
        let counted = stats(ColumnKind::Numeric, ["9.5", "10", "-2"].map(|text| Value::Numeric(text.into())).into());
        assert_eq!((counted.least, counted.greatest), (Some(Value::Numeric("-2".into())), Some(Value::Numeric("10".into()))));
    }

    #[test]
    fn the_most_common_come_first_and_stop_at_five() {
        let counted = stats(ColumnKind::Text, texts(&["a", "b", "b", "c", "d", "e", "f", "f", "f"]));
        let common: Vec<(String, usize)> = counted.common.iter().map(|counted| (counted.value.to_string(), counted.rows)).collect();
        assert_eq!(common, [("f".to_owned(), 3), ("b".to_owned(), 2), ("a".to_owned(), 1), ("c".to_owned(), 1), ("d".to_owned(), 1)]);
    }

    #[test]
    fn a_column_of_only_nulls_has_no_least_and_no_common_value() {
        let counted = stats(ColumnKind::Int, vec![Value::Null; 3]);
        assert_eq!((counted.least, counted.common), (None, vec![]));
    }

    #[test]
    fn i_on_the_grid_opens_the_figures_of_the_cursors_column_and_esc_closes_them() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('i'));
        assert!(matches!(&app.overlay, Some(Overlay::ColumnStats(stats)) if stats.column.as_str() == "total"));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn i_on_a_column_pgbg_cannot_decode_opens_no_figures_and_says_so() {
        let mut app = on_the_result(undecoded());
        press(&mut app, KeyCode::Char('i'));
        assert_eq!((app.overlay.is_none(), app.note()), (true, Some("shape has no figures: pgbg does not decode its type")));
    }

    #[test]
    fn i_while_the_filter_hides_the_cursors_row_opens_no_figures() {
        let mut app = on_the_result(orders());
        "/zzz".chars().for_each(|c| press(&mut app, KeyCode::Char(c)));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('i'));
        assert!(app.overlay.is_none());
    }

    /// The wall clock, read in a release build only, by CI's own step: `cargo test --release --lib -- --ignored budget::`.
    #[cfg(not(debug_assertions))]
    mod budget {
        use super::*;
        use crate::fixtures::best_of;
        use std::time::Duration;

        /// Ten thousand texts, a tenth of them distinct, take 1 ms on a laptop: ten times that fails, and is the time
        /// to count them off the loop instead.
        const BUDGET: Duration = Duration::from_millis(10);

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn the_figures_of_ten_thousand_rows_stay_within_the_budget() {
            let rows = (0..10_000).map(|n| vec![Value::Text(format!("customer {}", n % 1_000).into())]).collect();
            let result = ResultSet::whole(vec![column("customer", ColumnKind::Text)], rows);
            let took = best_of(|| ColumnStats::of(&result, 0, Format::Raw));
            assert!(took < BUDGET, "took {took:?}");
        }
    }
}
