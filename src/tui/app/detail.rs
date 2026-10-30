//! `⏎` on the grid: the row under the cursor shown over the screen, one column after another, scrolled with `j`/`k`.
//! `⏎` on a foreign key's cell opens the row it points to instead, read on the session; in a row, `⇥` steps through
//! its keys and the tables pointing to it, `⏎` goes one row deeper or runs the rows that point to it, and `⌫` walks
//! the trail back.
use super::{Action, App, LookId, Overlay, Run, RunId};
use crate::act::{self, Act, Pane};
use crate::connection::ConnectionName;
use crate::db::model::{Column, ResultSet, Value};
use crate::failure::Failure;
use crate::keys::{self, Hop, Keys, Lookup, Prefilled, Referrer, TableRead, Target};
use crossterm::event::KeyEvent;
use ratatui::text::Line;

const NULL_KEY: &str = "this key is NULL: it points to no row";
const NOT_COUNTED: &str = "not counted, as no index starts with its key: ⏎ runs its rows";

/// The open detail: the grid's row or the rows followed from it, where it is scrolled to, and the key under its cursor.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Detail {
    pub(in crate::tui) scroll: DetailScroll,
    /// The key under the cursor, by its place among the targets of the row shown, then the tables pointing to it.
    pub(in crate::tui) key: usize,
    /// The rows followed from the grid's through their keys, the one shown last.
    pub(in crate::tui) trail: Vec<Followed>,
    /// The tables whose keys point to the row shown, and how many of their rows do.
    pub(in crate::tui) referrers: Vec<Counted>,
    /// The lookup on its way, whose row goes on the trail once it comes.
    waiting: Option<Waiting>,
    /// Why the last key followed nothing, until the next key.
    pub(in crate::tui) note: Option<String>,
}

/// Where the detail starts: at a column until a frame has drawn it, then at a line, which `j`/`k` move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum DetailScroll {
    /// The first line of the column at this index, or of the tables pointing to the row past its last column.
    ToColumn(usize),
    Lines(usize),
}

/// A row a key led to, alone in its result, with the keys of its columns, and where the detail stood on the row before.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Followed {
    pub(in crate::tui) look: LookId,
    pub(in crate::tui) hop: Hop,
    pub(in crate::tui) result: ResultSet,
    keys: Keys,
    back_to: Spot,
}

/// Where the detail stood on a row, and the counts of the tables pointing to it, kept for `⌫`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Spot {
    scroll: DetailScroll,
    key: usize,
    referrers: Vec<Counted>,
}

/// A lookup sent, what it reads, and the table whose row it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Waiting {
    look: LookId,
    lookup: Lookup,
    hop: Hop,
}

/// A table whose key points to the row shown, and how many of its rows do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Counted {
    pub(in crate::tui) referrer: Referrer,
    pub(in crate::tui) count: Count,
}

/// How many rows of a table point to the row shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Count {
    /// Waiting for the count before it: one goes at a time, so a run waits behind one at most.
    Queued,
    Counting(LookId),
    /// Past `keys::COUNTED_ROWS`, there are more than it.
    Rows(u64),
    /// No index starts with the key, so a count could read the whole table: `⏎` runs its rows instead.
    NotCounted,
    /// Why the count gave no number.
    Failed(String),
}

/// The row the detail shows, the keys of its columns, and the tables of the trail that led to it.
pub(in crate::tui) struct ShownRow<'a> {
    pub(in crate::tui) columns: &'a [Column],
    pub(in crate::tui) row: &'a [Value],
    /// Each column's key, and the tables they were read from.
    keys: &'a Keys,
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
        Self { scroll: DetailScroll::ToColumn(column), key: 0, trail: vec![], referrers: vec![], waiting: None, note: None }
    }

    /// The table pointing to the row at `key`, which counts the row's `targets` first, then the tables.
    fn referrer_at(&self, key: usize, targets: usize) -> Option<&Counted> {
        self.referrers.get(key.checked_sub(targets)?)
    }

    /// The count of the first table of the row shown still queued, marked as counting by `look`; none while a count is
    /// on its way, for this row or one the trail left, or while the row a key points to is: the row shown is then
    /// about to be left.
    fn start_next_count(&mut self, look: LookId) -> Option<Lookup> {
        let is_counting = self.counts().any(|counted| matches!(counted.count, Count::Counting(_)));
        if is_counting || self.waiting.is_some() {
            return None;
        }
        let next = self.referrers.iter_mut().find(|counted| counted.count == Count::Queued)?;
        next.count = Count::Counting(look);
        Some(next.referrer.count())
    }

    /// The tables pointing to the row shown and to each row the trail left, whose counts `⌫` brings back.
    fn counts(&self) -> impl Iterator<Item = &Counted> {
        self.referrers.iter().chain(self.trail.iter().flat_map(|followed| &followed.back_to.referrers))
    }

    fn counts_mut(&mut self) -> impl Iterator<Item = &mut Counted> {
        self.referrers.iter_mut().chain(self.trail.iter_mut().flat_map(|followed| &mut followed.back_to.referrers))
    }
}

/// The number a count read: the one value of its one row.
fn count_in(read: &ResultSet) -> Option<u64> {
    match read.rows().first()?.first()? {
        Value::Int(rows) => u64::try_from(*rows).ok(),
        _ => None,
    }
}

impl<'a> ShownRow<'a> {
    /// Whether the column at `column` is part of its table's primary key.
    pub(in crate::tui) fn is_primary(&self, column: usize) -> bool {
        self.keys.columns().get(column).is_some_and(|key| key.is_primary)
    }

    /// Where the column at `column` points through a foreign key.
    pub(in crate::tui) fn points_to(&self, column: usize) -> Option<&'a Target> {
        self.keys.columns().get(column)?.points_to.as_ref()
    }

    /// Where the row's keys point, each key once, in the order of their columns.
    pub(in crate::tui) fn targets(&self) -> Vec<&'a Target> {
        let mut targets: Vec<&'a Target> = vec![];
        for target in self.keys.columns().iter().filter_map(|key| key.points_to.as_ref()) {
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
        let grid = self.grid_row()?;
        let Some(last) = detail.trail.last() else { return Some(grid) };
        let start = trail_start(&grid, detail);
        let followed = detail.trail.iter().map(|followed| followed.hop.table.as_str());
        let trail = std::iter::once(start.map_or("row", |start| start.table.as_str())).chain(followed).collect();
        let (row, of) = (last.result.rows().first()?, RowOf::Followed(last.look));
        Some(ShownRow { columns: last.result.columns(), row, keys: &last.keys, trail, of })
    }

    /// The grid's row under the cursor, with the keys of its columns.
    fn grid_row(&self) -> Option<ShownRow<'_>> {
        let Run::Done { run, result, grid, .. } = &self.sheet().run else { return None };
        let (row, of) = (result.rows().get(grid.row)?, RowOf::Grid { run: *run, row: grid.row });
        Some(ShownRow { columns: result.columns(), row, keys: grid.keys(), trail: vec![], of })
    }

    /// Opens the detail of the grid's row at the cursor's column; on a foreign key's cell, reads the row it points to.
    pub(super) fn open_detail(&mut self) -> Vec<Action> {
        let Run::Done { grid, .. } = &self.sheet().run else { return vec![] };
        let column = grid.column;
        let detail = Detail::at_column(column);
        let Some(shown) = self.shown_row(&detail) else { return vec![] };
        match shown.targets().iter().position(|target| target.pairs.iter().any(|pair| pair.at == column)) {
            Some(key) => self.follow(self.with_referrers(Detail { key, ..detail })),
            None => self.show_detail(detail),
        }
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
            Some(Act::Edit) => return self.write_join(detail),
            Some(Act::Back) => return back(detail).map(|before| self.count_on(before)).unwrap_or_default(),
            Some(Act::Quit) => return self.quit(),
            Some(Act::Close) => return vec![],
            _ => detail,
        };
        self.overlay = Some(Overlay::Detail(detail));
        vec![]
    }

    /// `detail` with the cursor `step` places further, round the row's keys and the tables pointing to it, scrolled to it;
    /// on a table not counted, or whose count failed, the note says why.
    fn on_key(&self, detail: Detail, step: isize) -> Detail {
        let Some(shown) = self.shown_row(&detail) else { return detail };
        let targets = shown.targets();
        let count = targets.len() + detail.referrers.len();
        if count == 0 {
            return detail;
        }
        let key = (detail.key + count).saturating_add_signed(step) % count;
        let column = targets.get(key).map_or(shown.columns.len(), |target| target.first_column());
        let note = match detail.referrer_at(key, targets.len()).map(|counted| &counted.count) {
            Some(Count::NotCounted) => Some(NOT_COUNTED.to_owned()),
            Some(Count::Failed(why)) => Some(format!("the count failed: {why}")),
            _ => None,
        };
        Detail { key, scroll: DetailScroll::ToColumn(column), note, ..detail }
    }

    /// Follows what is under the cursor of `detail`: reads the row its key points to, or runs the rows of a table
    /// pointing to it; notes why a key points to no row.
    fn follow(&mut self, detail: Detail) -> Vec<Action> {
        let shown = self.shown_row(&detail);
        let targets = shown.as_ref().map(ShownRow::targets).unwrap_or_default();
        let lookup = targets.get(detail.key).zip(shown.as_ref()).map(|(target, shown)| (target.hop(), target.lookup(shown.row)));
        let referrer = detail.referrer_at(detail.key, targets.len());
        if let Some(prefilled) = referrer.and_then(|counted| counted.referrer.rows()) {
            return self.run_prefilled(prefilled);
        }
        let (detail, actions) = match lookup {
            Some((table, Some(lookup))) => self.look_up(detail, table, lookup),
            Some((_, None)) => (Detail { note: Some(NULL_KEY.to_owned()), ..detail }, vec![]),
            None => (detail, vec![]),
        };
        self.overlay = Some(Overlay::Detail(detail));
        actions.into_iter().chain(self.count_next()).collect()
    }

    /// Puts `prefilled` at the end of the editor, its values in the parameter bar, and runs it as the editor runs a
    /// statement, the detail closed.
    fn run_prefilled(&mut self, prefilled: Prefilled) -> Vec<Action> {
        self.put_in_editor(&prefilled.text);
        self.sheet_mut().bar.fill(prefilled.values);
        self.run_editor()
    }

    /// Opens `detail` and counts the rows of each table pointing to the row it shows whose key an index starts with.
    fn show_detail(&mut self, detail: Detail) -> Vec<Action> {
        let detail = self.with_referrers(detail);
        self.count_on(detail)
    }

    /// `detail` with the tables pointing to the row it shows, those whose key an index starts with queued to be counted.
    fn with_referrers(&self, detail: Detail) -> Detail {
        let counted = |referrer: Referrer| Counted { count: if referrer.is_indexed { Count::Queued } else { Count::NotCounted }, referrer };
        Detail { referrers: self.referrers_of(&detail).into_iter().map(counted).collect(), ..detail }
    }

    /// Opens `detail` and sends the next count of the tables pointing to its row.
    fn count_on(&mut self, detail: Detail) -> Vec<Action> {
        self.overlay = Some(Overlay::Detail(detail));
        self.count_next()
    }

    /// Sends the count of the first table still queued, once no count is on its way.
    fn count_next(&mut self) -> Vec<Action> {
        let look = self.next_look;
        let Some(Overlay::Detail(detail)) = &mut self.overlay else { return vec![] };
        let Some(lookup) = detail.start_next_count(look) else { return vec![] };
        self.next_look = look.next();
        vec![Action::LookUp { connection: self.sheet().name().clone(), look, lookup }]
    }

    /// The tables whose keys point to the row `detail` shows, once the schema is read.
    fn referrers_of(&self, detail: &Detail) -> Vec<Referrer> {
        let (Some(shown), Some(catalog)) = (self.shown_row(detail), self.sheet().catalog()) else { return vec![] };
        keys::referrers(catalog, shown.columns, shown.row)
    }

    /// `detail` waiting on `lookup`, the read of the row `hop` leads to, and the action that reads it.
    fn look_up(&mut self, detail: Detail, hop: Hop, lookup: Lookup) -> (Detail, Vec<Action>) {
        let look = self.new_look();
        let action = Action::LookUp { connection: self.sheet().name().clone(), look, lookup: lookup.clone() };
        (Detail { waiting: Some(Waiting { look, lookup, hop }), ..detail }, vec![action])
    }

    pub(super) fn new_look(&mut self) -> LookId {
        let look = self.next_look;
        self.next_look = look.next();
        look
    }

    /// Takes the rows the lookup `look` read: a count of rows pointing to the row shown, or a row to put on the trail,
    /// with the keys its table gives its columns, when the detail still waits on it.
    pub(super) fn looked_up(&mut self, look: LookId, rows: Result<ResultSet, Failure>) -> Vec<Action> {
        let Some(Overlay::Detail(detail)) = &self.overlay else { return vec![] };
        let keys = match &detail.waiting {
            Some(waiting) if waiting.look == look => self.keys_of(rows.as_ref().ok()),
            _ => {
                self.counted(look, rows.as_ref());
                return self.count_next();
            }
        };
        let Some(Overlay::Detail(mut detail)) = self.overlay.take() else { return vec![] };
        let Some(Waiting { hop, .. }) = detail.waiting.take() else { return vec![] };
        match rows {
            Ok(result) if !result.rows().is_empty() => {
                let back_to = Spot { scroll: detail.scroll, key: detail.key, referrers: std::mem::take(&mut detail.referrers) };
                detail.trail.push(Followed { look, hop, result, keys, back_to });
                return self.show_detail(Detail { scroll: DetailScroll::ToColumn(0), key: 0, ..detail });
            }
            Ok(_) => detail.note = Some(format!("no row of {} has this key", hop.table)),
            Err(failure) => detail.note = Some(failure.to_string()),
        }
        self.count_on(detail)
    }

    /// The keys the catalog gives the columns of a row a lookup read.
    fn keys_of(&self, read: Option<&ResultSet>) -> Keys {
        self.sheet().catalog().zip(read).map(|(catalog, read)| keys::keys(catalog, read.columns())).unwrap_or_default()
    }

    /// Takes the count the lookup `look` read, when it counts the rows of a table pointing to the row shown or to one
    /// the trail left.
    fn counted(&mut self, look: LookId, read: Result<&ResultSet, &Failure>) {
        let Some(Overlay::Detail(detail)) = &mut self.overlay else { return };
        let Some(counted) = detail.counts_mut().find(|counted| counted.count == Count::Counting(look)) else { return };
        counted.count = match read {
            Ok(read) => count_in(read).map_or_else(|| Count::Failed("it read no number".to_owned()), Count::Rows),
            Err(failure) => Count::Failed(failure.to_string()),
        };
    }

    /// Puts at the end of the editor, unsaved, the statement joining the tables of `detail`'s trail from the grid's
    /// table, its row picked by its primary key, else by the key it was left by, the values in the parameter bar.
    fn write_join(&mut self, detail: Detail) -> Vec<Action> {
        let Some(prefilled) = self.trail_join(&detail) else {
            self.overlay = Some(Overlay::Detail(detail));
            return vec![];
        };
        self.put_in_editor(&prefilled.text);
        self.sheet_mut().bar.fill(prefilled.values);
        vec![]
    }

    fn trail_join(&self, detail: &Detail) -> Option<Prefilled> {
        let grid = self.grid_row()?;
        let start = trail_start(&grid, detail)?;
        let tables: Vec<Hop> = std::iter::once(start.hop()).chain(detail.trail.iter().map(|followed| followed.hop.clone())).collect();
        let left_by = || first_followed(&grid, detail)?.key(grid.row);
        let key = start.primary_key(grid.row).or_else(left_by)?;
        keys::trail_join(&tables, &key)
    }
}

/// The table the trail of `detail` starts from: the one holding the key of `grid`, the grid's row, it first followed;
/// with no trail yet, the one holding the grid's first primary-key column.
fn trail_start<'a>(grid: &ShownRow<'a>, detail: &Detail) -> Option<&'a TableRead> {
    let column = if detail.trail.is_empty() {
        grid.keys.columns().iter().position(|key| key.is_primary)?
    } else {
        first_followed(grid, detail)?.first_column()
    };
    grid.keys.table_of(grid.columns.get(column)?)
}

/// The key of `grid`, the grid's row, that the trail of `detail` first followed; none with no trail.
fn first_followed<'a>(grid: &ShownRow<'a>, detail: &Detail) -> Option<&'a Target> {
    grid.targets().get(detail.trail.first()?.back_to.key).copied()
}

/// The detail of the row before the last one followed, where it stood, with the counts it had; none on the grid's row,
/// which `⌫` leaves.
fn back(detail: Detail) -> Option<Detail> {
    let mut trail = detail.trail;
    let Spot { scroll, key, referrers } = trail.pop()?.back_to;
    Some(Detail { scroll, key, trail, referrers, waiting: None, note: None })
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{on_keyed, on_the_keyed_orders, on_the_result, shop, to_shop, typed};
    use super::super::{Answer, Fit, Incoming, RunId, SessionId};
    use super::*;
    use crate::db::model::{Catalog, Table, TableOid};
    use crate::db::model::{ColumnKind, ColumnName, ParamValue, Params, Statement};
    use crate::fixtures::{column, keyed_catalog, keyed_customer, keyed_orders, orders, shop_column};
    use crate::query::params::Given;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::sync::Arc;
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
            own_timeout: None,
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
        press(&mut app, KeyCode::Enter);
        assert_eq!((detail(&app).waiting.as_ref(), detail(&app).note.as_deref()), (None, Some(NULL_KEY)));
    }

    #[test]
    fn the_row_of_a_lookup_left_behind_never_shows() {
        let mut app = on_a_key();
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        assert_eq!(detail(&app).trail, []);
    }

    fn customer_read_on(connection: &str, session: SessionId) -> Answer {
        let incoming = Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) };
        Answer::Session { connection: connection.parse().unwrap(), session, incoming }
    }

    #[test]
    fn a_row_read_on_another_connection_never_shows() {
        let mut app = on_a_key();
        app.apply(customer_read_on("prod", SessionId::FIRST));
        assert_eq!(detail(&app).trail, []);
    }

    #[test]
    fn a_row_read_on_a_session_the_sheet_left_never_shows_and_says_why() {
        let mut app = on_a_key();
        app.apply(customer_read_on("shop", SessionId::FIRST.next()));
        assert_eq!((detail(&app).trail.as_slice(), detail(&app).note.clone()), (&[][..], Some(Failure::StoppedWithItsSession.to_string())));
    }

    #[test]
    fn a_count_read_on_a_session_the_sheet_left_ends_as_failed() {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Enter);
        let read = ResultSet::whole(vec![column("count", ColumnKind::Int)], vec![vec![Value::Int(12)]]);
        let incoming = Incoming::LookedUp { look: LookId::FIRST, rows: Ok(read) };
        app.apply(Answer::Session { connection: shop(), session: SessionId::FIRST.next(), incoming });
        assert_eq!(detail(&app).referrers[0].count, Count::Failed(Failure::StoppedWithItsSession.to_string()));
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
        assert_eq!((detail(&app).trail.len(), detail(&app).key, detail(&app).scroll), (0, 0, DetailScroll::ToColumn(1)));
    }

    #[test]
    fn backspace_on_the_grids_row_closes_the_detail() {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.overlay, None);
    }

    const ORDER_7_WITH_ITS_CUSTOMER: &str =
        "SELECT * FROM orders;\n\nSELECT *\nFROM public.orders o\nJOIN public.customers c ON c.id = o.customer_id\nWHERE o.id = {{id}}";

    #[test]
    fn e_writes_the_join_of_the_trail_picking_the_grids_row_by_its_primary_key() {
        let mut app = on_the_customer();
        press(&mut app, KeyCode::Char('e'));
        assert_eq!((app.sheet().editor.text(), app.overlay.is_none()), (ORDER_7_WITH_ITS_CUSTOMER, true));
        assert_eq!(app.sheet().bar.given(&["id".parse().unwrap()]), Given::typed([("id".parse().unwrap(), ParamValue::new("7"))].into()));
    }

    #[test]
    fn e_joins_on_the_column_of_the_table_a_key_was_read_from_whatever_its_alias() {
        let columns = vec![
            shop_column("orders", "id", ColumnKind::Int),
            Column { name: ColumnName::new("cust"), ..shop_column("orders", "customer_id", ColumnKind::Int) },
        ];
        let mut app =
            on_keyed("SELECT id, customer_id AS cust FROM orders", ResultSet::whole(columns, vec![vec![Value::Int(7), Value::Int(3)]]));
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Enter);
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        press(&mut app, KeyCode::Char('e'));
        assert!(
            app.sheet().editor.text().ends_with("JOIN public.customers c ON c.id = o.customer_id\nWHERE o.id = {{id}}"),
            "{}",
            app.sheet().editor.text()
        );
    }

    #[test]
    fn the_join_written_is_one_step_undo_takes_back() {
        let mut app = on_the_customer();
        press(&mut app, KeyCode::Char('e'));
        app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(app.sheet().editor.text(), "SELECT * FROM orders");
    }

    #[test]
    fn esc_closes_every_row_at_once() {
        let mut app = on_the_customer();
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn tab_steps_through_the_keys_then_the_tables_pointing_to_the_row() {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Tab);
        assert_eq!((detail(&app).key, detail(&app).scroll), (1, DetailScroll::ToColumn(3)));
        press(&mut app, KeyCode::Tab);
        assert_eq!((detail(&app).key, detail(&app).scroll), (0, DetailScroll::ToColumn(1)));
    }

    #[test]
    fn opening_the_detail_counts_the_rows_of_each_table_pointing_to_it() {
        let mut app = on_the_keyed_orders();
        let Action::LookUp { lookup, .. } = press(&mut app, KeyCode::Enter).remove(0) else { panic!("no count") };
        let count = "SELECT count(*) FROM (SELECT FROM billing.invoices WHERE order_id = $1 LIMIT 1001) AS pointing";
        assert_eq!(lookup.statement, Statement::new(count));
    }

    #[test]
    fn a_count_read_shows_beside_its_table() {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Enter);
        let read = ResultSet::whole(vec![column("count", ColumnKind::Int)], vec![vec![Value::Int(12)]]);
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(read) }));
        assert_eq!(detail(&app).referrers[0].count, Count::Rows(12));
    }

    #[test]
    fn a_table_whose_key_no_index_starts_with_is_not_counted() {
        let mut app = on_a_key();
        let actions = app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        assert_eq!((actions, &detail(&app).referrers[0].count), (vec![], &Count::NotCounted));
    }

    #[test]
    fn tab_on_a_table_not_counted_says_why() {
        let mut app = on_the_customer();
        press(&mut app, KeyCode::Tab);
        assert_eq!(detail(&app).note.as_deref(), Some(NOT_COUNTED));
    }

    fn with_a_failed_count() -> App {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Enter);
        let failure = Failure::Server { message: "canceling statement due to statement timeout".into(), position: None };
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Err(failure) }));
        app
    }

    #[test]
    fn a_failed_count_keeps_why_it_failed() {
        let app = with_a_failed_count();
        assert_eq!(detail(&app).referrers[0].count, Count::Failed("canceling statement due to statement timeout".into()));
    }

    #[test]
    fn tab_on_a_failed_count_says_why_it_failed() {
        let mut app = with_a_failed_count();
        press(&mut app, KeyCode::Tab);
        assert_eq!(detail(&app).note.as_deref(), Some("the count failed: canceling statement due to statement timeout"));
    }

    /// A detail pointed to by `invoices` and by `refunds`, both counted.
    fn pointed_twice() -> Detail {
        let counted = |table: &str| Counted {
            referrer: Referrer { schema: "billing".into(), table: table.into(), pairs: vec![], is_indexed: true },
            count: Count::Queued,
        };
        Detail { referrers: vec![counted("invoices"), counted("refunds")], ..Detail::at_column(0) }
    }

    #[test]
    fn a_count_waits_while_another_is_on_its_way() {
        let mut detail = pointed_twice();
        detail.start_next_count(LookId::FIRST);
        assert_eq!(detail.start_next_count(LookId::FIRST.next()), None);
    }

    #[test]
    fn the_next_count_goes_once_the_one_before_is_read() {
        let mut detail = pointed_twice();
        detail.start_next_count(LookId::FIRST);
        detail.referrers[0].count = Count::Rows(3);
        detail.start_next_count(LookId::FIRST.next());
        assert_eq!(detail.referrers[1].count, Count::Counting(LookId::FIRST.next()));
    }

    fn counted_as(rows: i64) -> Incoming {
        let read = ResultSet::whole(vec![column("count", ColumnKind::Int)], vec![vec![Value::Int(rows)]]);
        Incoming::LookedUp { look: LookId::FIRST, rows: Ok(read) }
    }

    /// The grid's row of the keyed orders open, its count of `invoices` sent, then its customer's key followed.
    fn following_with_a_count_on_its_way(catalog: Catalog) -> App {
        let mut app = typed("SELECT * FROM orders");
        press(&mut app, KeyCode::Enter);
        app.apply(to_shop(Incoming::Schema(Ok(Arc::new(catalog)))));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: keyed_orders(), took: Duration::ZERO }));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        app
    }

    /// [`keyed_catalog`] with `billing.refunds` pointing to `orders` too, on an index of its key.
    fn pointed_by_invoices_and_refunds() -> Catalog {
        let mut catalog = keyed_catalog();
        let billing = &mut catalog.schema.schemas[0];
        let invoices = billing.tables[0].clone();
        billing.tables.push(Table { oid: TableOid::new(5), name: "refunds".to_owned(), ..invoices });
        catalog
    }

    #[test]
    fn backspace_from_a_row_opened_on_a_key_cell_counts_the_tables_pointing_to_the_grids_row() {
        let mut app = on_the_customer();
        let Action::LookUp { lookup, .. } = press(&mut app, KeyCode::Backspace).remove(0) else { panic!("no count") };
        let count = "SELECT count(*) FROM (SELECT FROM billing.invoices WHERE order_id = $1 LIMIT 1001) AS pointing";
        assert_eq!((lookup.statement, detail(&app).referrers.len()), (Statement::new(count), 1));
    }

    #[test]
    fn a_count_read_while_a_key_is_followed_sends_no_other_count_of_the_row_left() {
        let mut app = following_with_a_count_on_its_way(pointed_by_invoices_and_refunds());
        assert_eq!(app.apply(to_shop(counted_as(12))), []);
    }

    #[test]
    fn backspace_brings_back_the_counts_the_row_had() {
        let mut app = following_with_a_count_on_its_way(keyed_catalog());
        app.apply(to_shop(counted_as(12)));
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST.next(), rows: Ok(keyed_customer()) }));
        let actions = press(&mut app, KeyCode::Backspace);
        assert_eq!((actions, &detail(&app).referrers[0].count), (vec![], &Count::Rows(12)));
    }

    #[test]
    fn a_count_read_once_its_row_was_left_shows_on_the_way_back() {
        let mut app = following_with_a_count_on_its_way(keyed_catalog());
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST.next(), rows: Ok(keyed_customer()) }));
        app.apply(to_shop(counted_as(12)));
        press(&mut app, KeyCode::Backspace);
        assert_eq!(detail(&app).referrers[0].count, Count::Rows(12));
    }

    #[test]
    fn enter_on_a_table_pointing_to_the_row_runs_its_rows_with_the_value_bound() {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Tab);
        let Action::Run { statement, input, .. } = press(&mut app, KeyCode::Enter).remove(0) else { panic!("no run") };
        assert_eq!(statement, Statement::new("SELECT * FROM billing.invoices WHERE order_id = {{order_id}}"));
        assert_eq!(input.given, Given::typed([("order_id".parse().unwrap(), ParamValue::new("7"))].into()));
        assert_eq!(app.overlay, None);
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
