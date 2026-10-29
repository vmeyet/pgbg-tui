//! `⏎` on the grid: the row under the cursor shown over the screen, one column after another, scrolled with `j`/`k`.
//! `⏎` on a foreign key's cell opens the row it points to instead, read on the session.
use super::{Action, App, LookId, Overlay, Run, RunId};
use crate::act::{self, Act, Pane};
use crate::connection::ConnectionName;
use crate::db::model::{Column, ResultSet, Value};
use crate::failure::Failure;
use crate::keys::Lookup;
use crossterm::event::KeyEvent;
use ratatui::text::Line;

const NULL_KEY: &str = "this key is NULL: it points to no row";

/// The open detail: the grid's row or the rows followed from it, and where it is scrolled to.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Detail {
    pub(in crate::tui) scroll: DetailScroll,
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

/// A row a key led to, alone in its result.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Followed {
    pub(in crate::tui) look: LookId,
    pub(in crate::tui) table: String,
    pub(in crate::tui) result: ResultSet,
}

/// A lookup sent, and the table whose row it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Waiting {
    look: LookId,
    table: String,
}

/// The row the detail shows, and the tables of the trail that led to it.
pub(in crate::tui) struct ShownRow<'a> {
    pub(in crate::tui) columns: &'a [Column],
    pub(in crate::tui) row: &'a [Value],
    /// The tables of the rows followed, empty while the detail shows the grid's row.
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
        Self { scroll: DetailScroll::ToColumn(column), trail: vec![], waiting: None, note: None }
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

    /// The row `detail` shows.
    pub(in crate::tui) fn shown_row<'a>(&'a self, detail: &'a Detail) -> Option<ShownRow<'a>> {
        let Run::Done { run, result, grid, .. } = &self.sheet().run else { return None };
        let Some(last) = detail.trail.last() else {
            let (row, of) = (result.rows().get(grid.row)?, RowOf::Grid { run: *run, row: grid.row });
            return Some(ShownRow { columns: result.columns(), row, trail: vec![], of });
        };
        let trail = detail.trail.iter().map(|followed| followed.table.as_str()).collect();
        let (row, of) = (last.result.rows().first()?, RowOf::Followed(last.look));
        Some(ShownRow { columns: last.result.columns(), row, trail, of })
    }

    /// Opens the detail of the grid's row at the cursor's column; on a foreign key's cell, reads the row it points to.
    pub(super) fn open_detail(&mut self) -> Vec<Action> {
        let Run::Done { result, grid, .. } = &self.sheet().run else { return vec![] };
        let Some(row) = result.rows().get(grid.row) else { return vec![] };
        let detail = Detail::at_column(grid.column);
        let lookup = grid.points_to(grid.column).map(|target| (target.table.clone(), target.lookup(row)));
        let (detail, actions) = match lookup {
            Some((table, Some(lookup))) => self.look_up(detail, table, lookup),
            Some((_, None)) => (Detail { note: Some(NULL_KEY.to_owned()), ..detail }, vec![]),
            None => (detail, vec![]),
        };
        self.overlay = Some(Overlay::Detail(detail));
        actions
    }

    /// The detail after `key`: it scrolls within what the frame drew, and `esc` closes it.
    pub(super) fn detail_key(&mut self, detail: Detail, key: KeyEvent) -> Vec<Action> {
        let detail = Detail { note: None, ..detail };
        let line = match detail.scroll {
            DetailScroll::Lines(line) => line,
            DetailScroll::ToColumn(_) => self.fit.detail.first_line,
        };
        let scroll = match act::act_for(key, Pane::Detail) {
            Some(Act::Down) => DetailScroll::Lines((line + 1).min(self.fit.detail.last_scroll)),
            Some(Act::Up) => DetailScroll::Lines(line.saturating_sub(1)),
            Some(Act::Quit) => return self.quit(),
            Some(Act::Close) => return vec![],
            _ => detail.scroll,
        };
        self.overlay = Some(Overlay::Detail(Detail { scroll, ..detail }));
        vec![]
    }

    /// `detail` waiting on `lookup`, the read of a row of `table`, and the action that reads it.
    fn look_up(&mut self, detail: Detail, table: String, lookup: Lookup) -> (Detail, Vec<Action>) {
        let look = self.next_look;
        self.next_look = look.next();
        let action = Action::LookUp { connection: self.sheet().name().clone(), look, lookup };
        (Detail { waiting: Some(Waiting { look, table }), ..detail }, vec![action])
    }

    /// Takes the rows the lookup `look` read: its first row goes on the trail when the detail still waits on it.
    pub(super) fn looked_up(&mut self, look: LookId, rows: Result<ResultSet, Failure>) {
        let Some(Overlay::Detail(detail)) = &mut self.overlay else { return };
        let Some(Waiting { table, .. }) = detail.waiting.take_if(|waiting| waiting.look == look) else { return };
        match rows {
            Ok(result) if !result.rows().is_empty() => {
                detail.trail.push(Followed { look, table, result });
                detail.scroll = DetailScroll::ToColumn(0);
            }
            Ok(_) => detail.note = Some(format!("no row of {table} has this key")),
            Err(failure) => detail.note = Some(failure.to_string()),
        }
    }
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
        assert_eq!((shown.trail, shown.row[1].clone()), (vec!["customers"], Value::Text("nina".into())));
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
