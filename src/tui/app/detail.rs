//! `⏎` on the grid: the row under the cursor shown over the screen, one column after another, scrolled with `j`/`k`.
//! `⏎` on a foreign key's cell opens the row it points to instead, read on the session; in a row, `⇥` steps through
//! its keys and `⏎` goes one row deeper, on a trail that `⌫` walks back.
use super::{Action, App, LookId, Overlay, Run, RunId};
use crate::act::{self, Act, Pane};
use crate::connection::ConnectionName;
use crate::db::model::{Column, ResultSet, Value};
use crate::failure::Failure;
use crate::keys::{self, ColumnKey, Lookup, Target};
use crossterm::event::KeyEvent;
use ratatui::text::Line;

const NULL_KEY: &str = "this key is NULL: it points to no row";

/// The open detail: the grid's row or the rows followed from it, where it is scrolled to, and the key under its cursor.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Detail {
    pub(in crate::tui) scroll: DetailScroll,
    /// The key under the cursor, by its place among the targets of the row shown.
    pub(in crate::tui) key: usize,
    /// The rows followed from the grid's through their keys, the one shown last.
    pub(in crate::tui) trail: Vec<Followed>,
    /// The lookup on its way, whose row goes on the trail once it comes.
    waiting: Option<Waiting>,
    /// Why the last key followed nothing, until the next key.
    pub(in crate::tui) note: Option<String>,
}

/// Where the detail starts: at a column until a frame has drawn it, then at a line, which `j`/`k` move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum DetailScroll {
    /// The first line of the column at this index.
    ToColumn(usize),
    Lines(usize),
}

/// A row a key led to, alone in its result, with the keys of its columns, and where the detail stood on the row before.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Followed {
    pub(in crate::tui) look: LookId,
    pub(in crate::tui) table: String,
    pub(in crate::tui) result: ResultSet,
    keys: Vec<ColumnKey>,
    back_to: Spot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spot {
    scroll: DetailScroll,
    key: usize,
}

/// A lookup sent, what it reads, and the table whose row it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Waiting {
    look: LookId,
    lookup: Lookup,
    table: String,
}

/// The row the detail shows, the keys of its columns, and the tables of the trail that led to it.
pub(in crate::tui) struct ShownRow<'a> {
    pub(in crate::tui) columns: &'a [Column],
    pub(in crate::tui) row: &'a [Value],
    /// Each column's key, by index; empty when the table the row comes from is unknown.
    keys: &'a [ColumnKey],
    /// The grid's table first, empty while the detail shows the grid's row.
    pub(in crate::tui) trail: Vec<&'a str>,
    pub(in crate::tui) of: RowOf,
}

/// Which row the detail shows: the grid's, or the one a lookup read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum RowOf {
    Grid { run: RunId, row: usize },
    Followed(LookId),
}

/// Where the detail just drawn started, how far it can scroll before its last line reaches the bottom, and its lines
/// when that frame had to build them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::tui) struct DetailFit {
    pub(in crate::tui) first_line: usize,
    pub(in crate::tui) last_scroll: usize,
    pub(in crate::tui) drawn: Option<DrawnDetail>,
}

/// The detail's lines built for one row at one width, kept so the next frames do not build them again: indenting,
/// colouring and wrapping a long value costs more than the rest of a frame.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct DrawnDetail {
    pub(in crate::tui) of: DrawnRow,
    pub(in crate::tui) lines: Vec<Line<'static>>,
    /// The first line of each column.
    pub(in crate::tui) column_starts: Vec<usize>,
}

/// Which row the lines were built for, and at what width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct DrawnRow {
    pub(in crate::tui) connection: ConnectionName,
    pub(in crate::tui) row: RowOf,
    pub(in crate::tui) width: u16,
}

impl Detail {
    fn at_column(column: usize) -> Self {
        Self { scroll: DetailScroll::ToColumn(column), key: 0, trail: vec![], waiting: None, note: None }
    }
}

impl ShownRow<'_> {
    /// Where the column at `column` points through a foreign key.
    pub(in crate::tui) fn points_to(&self, column: usize) -> Option<&Target> {
        self.keys.get(column)?.points_to.as_ref()
    }

    /// Where the row's keys point, each key once, in the order of their columns.
    pub(in crate::tui) fn targets(&self) -> Vec<&Target> {
        let mut targets: Vec<&Target> = vec![];
        for target in self.keys.iter().filter_map(|key| key.points_to.as_ref()) {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        targets
    }
}

impl App {
    /// Keeps the lines a frame just built, and lets them go once the detail closes.
    pub(super) fn keep_drawn_detail(&mut self, built: Option<DrawnDetail>) {
        if !matches!(self.overlay, Some(Overlay::Detail(_))) {
            self.drawn_detail = None;
        } else if built.is_some() {
            self.drawn_detail = built;
        }
    }

    /// The row `detail` shows, with the keys of its columns.
    pub(in crate::tui) fn shown_row<'a>(&'a self, detail: &'a Detail) -> Option<ShownRow<'a>> {
        let Run::Done { run, result, grid, .. } = &self.sheet().run else { return None };
        let Some(last) = detail.trail.last() else {
            let (row, of) = (result.rows().get(grid.row)?, RowOf::Grid { run: *run, row: grid.row });
            return Some(ShownRow { columns: result.columns(), row, keys: grid.keys(), trail: vec![], of });
        };
        let statement = self.sheet().shown_statement();
        let read = self.sheet().catalog().zip(statement).and_then(|(catalog, statement)| keys::one_table_read(catalog, statement));
        let followed = detail.trail.iter().map(|followed| followed.table.as_str());
        let trail = std::iter::once(read.map_or("row", |table| table.name.as_str())).chain(followed).collect();
        let (row, of) = (last.result.rows().first()?, RowOf::Followed(last.look));
        Some(ShownRow { columns: last.result.columns(), row, keys: &last.keys, trail, of })
    }

    /// Opens the detail of the grid's row at the cursor's column; on a foreign key's cell, reads the row it points to.
    pub(super) fn open_detail(&mut self) -> Vec<Action> {
        let Run::Done { grid, .. } = &self.sheet().run else { return vec![] };
        let column = grid.column;
        let detail = Detail::at_column(column);
        let Some(shown) = self.shown_row(&detail) else { return vec![] };
        let on_key = shown.targets().iter().position(|target| target.pairs.iter().any(|pair| pair.at == column));
        if let Some(key) = on_key {
            return self.follow(Detail { key, ..detail });
        }
        self.overlay = Some(Overlay::Detail(detail));
        vec![]
    }

    /// The detail after `key`: it scrolls within what the frame drew, steps through the keys of its row, follows one,
    /// goes back along the trail, and `esc` closes it.
    pub(super) fn detail_key(&mut self, detail: Detail, key: KeyEvent) -> Vec<Action> {
        let detail = Detail { note: None, ..detail };
        let line = match detail.scroll {
            DetailScroll::Lines(line) => line,
            DetailScroll::ToColumn(_) => self.fit.detail.first_line,
        };
        let detail = match act::act_for(key, Pane::Detail) {
            Some(Act::Down) => Detail { scroll: DetailScroll::Lines((line + 1).min(self.fit.detail.last_scroll)), ..detail },
            Some(Act::Up) => Detail { scroll: DetailScroll::Lines(line.saturating_sub(1)), ..detail },
            Some(Act::Right) => self.on_key(detail, 1),
            Some(Act::Left) => self.on_key(detail, -1),
            Some(Act::Open) => return self.follow(detail),
            Some(Act::Back) => match back(detail) {
                Some(before) => before,
                None => return vec![],
            },
            Some(Act::Quit) => return self.quit(),
            Some(Act::Close) => return vec![],
            _ => detail,
        };
        self.overlay = Some(Overlay::Detail(detail));
        vec![]
    }

    /// `detail` with the cursor `step` keys further, round the row's keys, scrolled to the first column of that key.
    fn on_key(&self, detail: Detail, step: isize) -> Detail {
        let Some(shown) = self.shown_row(&detail) else { return detail };
        let targets = shown.targets();
        if targets.is_empty() {
            return detail;
        }
        let key = (detail.key + targets.len()).saturating_add_signed(step) % targets.len();
        Detail { key, scroll: DetailScroll::ToColumn(targets[key].first_column()), ..detail }
    }

    /// Opens `detail` and reads the row its key under the cursor points to, or notes why it points to none.
    fn follow(&mut self, detail: Detail) -> Vec<Action> {
        let lookup = self.shown_row(&detail).and_then(|shown| {
            let target = *shown.targets().get(detail.key)?;
            Some((target.table.clone(), target.lookup(shown.row)))
        });
        let (detail, actions) = match lookup {
            Some((table, Some(lookup))) => self.look_up(detail, table, lookup),
            Some((_, None)) => (Detail { note: Some(NULL_KEY.to_owned()), ..detail }, vec![]),
            None => (detail, vec![]),
        };
        self.overlay = Some(Overlay::Detail(detail));
        actions
    }

    /// `detail` waiting on `lookup`, the read of a row of `table`, and the action that reads it.
    fn look_up(&mut self, detail: Detail, table: String, lookup: Lookup) -> (Detail, Vec<Action>) {
        let look = self.next_look;
        self.next_look = look.next();
        let action = Action::LookUp { connection: self.sheet().name().clone(), look, lookup: lookup.clone() };
        (Detail { waiting: Some(Waiting { look, lookup, table }), ..detail }, vec![action])
    }

    /// Takes the rows the lookup `look` read: its first row goes on the trail, with the keys its table gives its
    /// columns, when the detail still waits on it.
    pub(super) fn looked_up(&mut self, look: LookId, rows: Result<ResultSet, Failure>) {
        let Some(Overlay::Detail(detail)) = &self.overlay else { return };
        let Some(waiting) = detail.waiting.as_ref().filter(|waiting| waiting.look == look) else { return };
        let keys_of = |result: &ResultSet| {
            self.sheet().catalog().map(|catalog| keys::column_keys(catalog, &waiting.lookup.statement, result.columns()))
        };
        let keys = rows.as_ref().ok().and_then(keys_of).unwrap_or_default();
        let Some(Overlay::Detail(detail)) = &mut self.overlay else { return };
        let Some(Waiting { table, .. }) = detail.waiting.take() else { return };
        match rows {
            Ok(result) if !result.rows().is_empty() => {
                let back_to = Spot { scroll: detail.scroll, key: detail.key };
                detail.trail.push(Followed { look, table, result, keys, back_to });
                (detail.scroll, detail.key) = (DetailScroll::ToColumn(0), 0);
            }
            Ok(_) => detail.note = Some(format!("no row of {table} has this key")),
            Err(failure) => detail.note = Some(failure.to_string()),
        }
    }
}

/// The detail of the row before the last one followed, where it stood; none on the grid's row, which `⌫` leaves.
fn back(detail: Detail) -> Option<Detail> {
    let mut trail = detail.trail;
    let left = trail.pop()?;
    Some(Detail { scroll: left.back_to.scroll, key: left.back_to.key, trail, waiting: None, note: None })
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{on_the_keyed_orders, on_the_result, shop, to_shop, typed};
    use super::super::{Fit, Incoming, RunId};
    use super::*;
    use crate::db::model::{ParamValue, Params, Statement};
    use crate::fixtures::{keyed_customer, orders};
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::time::Duration;

    fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    fn detail(app: &App) -> &Detail {
        let Some(Overlay::Detail(detail)) = &app.overlay else { panic!("no detail open") };
        detail
    }

    /// The keyed orders with the cursor on order 7's customer and `⏎` pressed.
    fn on_a_key() -> App {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Enter);
        app
    }

    #[test]
    fn enter_on_a_key_cell_reads_the_row_it_points_to() {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Char('l'));
        let lookup = Lookup {
            statement: Statement::new("SELECT * FROM public.customers WHERE id = $1"),
            params: Params::new(vec![Some(ParamValue::new("3"))], vec![]),
        };
        assert_eq!(press(&mut app, KeyCode::Enter), [Action::LookUp { connection: shop(), look: LookId::FIRST, lookup }]);
    }

    #[test]
    fn the_row_read_shows_with_the_trail_that_led_to_it() {
        let mut app = on_a_key();
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        let shown = app.shown_row(detail(&app)).unwrap();
        assert_eq!((shown.trail, shown.row[1].clone()), (vec!["orders", "customers"], Value::Text("nina".into())));
    }

    #[test]
    fn a_key_pointing_to_no_row_says_so_and_stays_on_the_row() {
        let mut app = on_a_key();
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(ResultSet::whole(vec![], vec![])) }));
        assert_eq!((detail(&app).trail.len(), detail(&app).note.as_deref()), (0, Some("no row of customers has this key")));
    }

    #[test]
    fn a_failed_lookup_says_why() {
        let mut app = on_a_key();
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Err(Failure::Link("connection reset".into())) }));
        assert_eq!(detail(&app).note.as_deref(), Some("connection reset"));
    }

    #[test]
    fn a_null_key_reads_nothing_and_says_why() {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!(detail(&app).note.as_deref(), Some(NULL_KEY));
    }

    #[test]
    fn the_row_of_a_lookup_left_behind_never_shows() {
        let mut app = on_a_key();
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        assert_eq!(detail(&app).trail, []);
    }

    /// The keyed orders with the detail on the customer of order 7, read.
    fn on_the_customer() -> App {
        let mut app = on_a_key();
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        app
    }

    #[test]
    fn the_row_read_has_the_keys_of_its_own_table() {
        let app = on_the_customer();
        assert_eq!(app.shown_row(detail(&app)).unwrap().points_to(2).map(|target| target.table.as_str()), Some("users"));
    }

    #[test]
    fn enter_on_a_key_of_the_row_read_goes_one_level_deeper() {
        let mut app = on_the_customer();
        let Action::LookUp { lookup, .. } = press(&mut app, KeyCode::Enter).remove(0) else { panic!("no lookup") };
        assert_eq!(lookup.statement, Statement::new("SELECT * FROM public.users WHERE id = $1"));
    }

    #[test]
    fn backspace_goes_back_to_the_row_before_where_it_stood() {
        let mut app = on_the_customer();
        press(&mut app, KeyCode::Backspace);
        assert_eq!(detail(&app), &Detail::at_column(1));
    }

    #[test]
    fn backspace_on_the_grids_row_closes_the_detail() {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn esc_closes_every_row_at_once() {
        let mut app = on_the_customer();
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn tab_steps_to_the_next_key_and_scrolls_to_it() {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Tab);
        assert_eq!((detail(&app).key, detail(&app).scroll), (0, DetailScroll::ToColumn(1)));
    }

    #[test]
    fn enter_on_the_grid_opens_the_detail_of_its_row() {
        let mut app = on_the_result(orders());
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!(detail(&app), &Detail::at_column(0));
    }

    #[test]
    fn space_opens_it_too() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(detail(&app), &Detail::at_column(0));
    }

    #[test]
    fn an_empty_result_has_no_detail_to_open() {
        let mut app = typed("SELECT 1");
        press(&mut app, KeyCode::Enter);
        let empty = ResultSet::whole(orders().columns().to_vec(), vec![]);
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: empty, took: Duration::from_millis(1) }));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn j_scrolls_down_no_further_than_the_frame_drew() {
        let mut app = on_the_result(orders());
        app.fit(Fit { detail: DetailFit { first_line: 0, last_scroll: 2, ..DetailFit::default() }, ..Fit::default() });
        press(&mut app, KeyCode::Enter);
        (0..4).for_each(|_| _ = press(&mut app, KeyCode::Char('j')));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(detail(&app).scroll, DetailScroll::Lines(1));
    }

    #[test]
    fn the_detail_opens_at_the_cursors_column() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(detail(&app).scroll, DetailScroll::ToColumn(1));
    }

    #[test]
    fn once_drawn_the_detail_scrolls_from_the_line_the_frame_started_at() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Enter);
        app.fit(Fit { detail: DetailFit { first_line: 3, last_scroll: 5, ..DetailFit::default() }, ..Fit::default() });
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(detail(&app).scroll, DetailScroll::Lines(4));
    }

    #[test]
    fn esc_closes_the_detail_and_leaves_the_grid_where_it_was() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!((app.overlay.is_none(), grid.row), (true, 1));
    }

    #[test]
    fn ctrl_c_in_the_detail_quits() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Enter);
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }
}
