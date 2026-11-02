//! Which columns of a result the grid shows, in the result's order, and how many of the first stay on the left while the
//! others scroll.
//! The cursor and every act on it name a column by its index in the result; only this module knows its place on screen.
use super::grid::Room;
use super::{App, Grid};
use crate::format::counted;
use crate::keys::Keys;

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

/// The columns the grid shows, each named by its index in the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct ShownColumns {
    freeze: Freeze,
    /// The last column [`Freeze::Keys`] freezes in this result.
    last_key: usize,
    /// The columns, in the result's order.
    order: Vec<usize>,
    /// How many of the first in `order` are frozen.
    frozen: usize,
}

impl ShownColumns {
    /// Every column of a result of `count` columns with these `keys`, the key it starts with frozen.
    pub(super) fn new(keys: &Keys, count: usize) -> Self {
        let last_key = keys.columns().iter().take_while(|key| key.is_primary).count().saturating_sub(1);
        let mut shown = Self { freeze: Freeze::Keys, last_key, order: (0..count).collect(), frozen: 0 };
        shown.arrange();
        shown
    }

    fn arrange(&mut self) {
        self.frozen = match self.freeze {
            Freeze::Off => 0,
            Freeze::Keys => self.order.partition_point(|&column| column <= self.last_key),
            Freeze::Through(last) => self.order.partition_point(|&column| column <= last),
        };
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

    /// Freezes nothing, or the key once nothing is frozen.
    pub(super) fn switch_freeze(&mut self) {
        self.freeze = if self.freeze == Freeze::Off { Freeze::Keys } else { Freeze::Off };
        self.arrange();
    }

    /// Freezes the columns up to `column`, itself included.
    pub(super) fn freeze_through(&mut self, column: usize) {
        self.freeze = Freeze::Through(column);
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
    /// Applies `change` to the grid on screen, in the room the last frame gave it, and notes how many columns it left frozen.
    pub(super) fn change_freeze(&mut self, change: fn(&mut Grid, Room)) {
        let room = self.fit.grid;
        let Some(grid) = self.sheet_mut().grid_mut() else { return };
        change(grid, room);
        let frozen = grid.columns().frozen;
        self.note = Some(format!("{} frozen", counted(frozen, "column")));
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{cursor_name, drawn, grid, key_in_the_middle, pressed};
    use super::*;
    use crate::fixtures::{keyed_catalog, keyed_orders, orders};
    use crate::keys::keys;

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
    fn shift_z_freezes_the_columns_up_to_the_cursor_and_says_how_many() {
        assert_eq!(pressed(orders(), "lZ").note(), Some("2 columns frozen"));
    }
}
