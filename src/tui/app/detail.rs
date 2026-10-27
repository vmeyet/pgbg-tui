//! `⏎` on the grid: the row under the cursor over the screen, one column after another, scrolled with `j`/`k`.
use super::{Action, App, Overlay, Run, RunId};
use crate::act::{self, Act, Pane};
use crate::connection::ConnectionName;
use crossterm::event::KeyEvent;
use ratatui::text::Line;

/// The open detail of the grid's row, and where it is scrolled to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Detail {
    pub(in crate::tui) scroll: DetailScroll,
}

/// Where the detail starts: at a column until a frame drew it, then at a line, which `j`/`k` move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum DetailScroll {
    /// The first line of the column at this index.
    ToColumn(usize),
    Lines(usize),
}

/// Where the detail just drawn started, how far it scrolls before its last line is at the bottom, and its lines when
/// that frame had to build them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::tui) struct DetailFit {
    pub(in crate::tui) first_line: usize,
    pub(in crate::tui) last_scroll: usize,
    pub(in crate::tui) drawn: Option<DrawnDetail>,
}

/// The detail's lines as built for one row at one width, kept so the next frames build none: indenting, colouring and
/// wrapping a long value costs more than the rest of a frame.
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
    pub(in crate::tui) run: RunId,
    pub(in crate::tui) row: usize,
    pub(in crate::tui) width: u16,
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

    /// Opens the detail of the grid's row at the cursor's column, when the result has a row.
    pub(super) fn open_detail(&mut self) {
        let Run::Done { result, grid, .. } = &self.sheet().run else { return };
        if grid.row < result.rows().len() {
            self.overlay = Some(Overlay::Detail(Detail { scroll: DetailScroll::ToColumn(grid.column) }));
        }
    }

    /// The detail after `key`: it scrolls within what the frame drew, `esc` closes it.
    pub(super) fn detail_key(&mut self, detail: Detail, key: KeyEvent) -> Vec<Action> {
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
        self.overlay = Some(Overlay::Detail(Detail { scroll }));
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{on_the_result, orders, to_shop, typed};
    use super::super::{Fit, Incoming, RunId};
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::time::Duration;

    fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    #[test]
    fn enter_on_the_grid_opens_the_detail_of_its_row() {
        let mut app = on_the_result(orders());
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!(app.overlay, Some(Overlay::Detail(Detail { scroll: DetailScroll::ToColumn(0) })));
    }

    #[test]
    fn space_opens_it_too() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.overlay, Some(Overlay::Detail(Detail { scroll: DetailScroll::ToColumn(0) })));
    }

    #[test]
    fn an_empty_result_has_no_detail_to_open() {
        let mut app = typed("SELECT 1");
        press(&mut app, KeyCode::Enter);
        let empty = crate::db::model::ResultSet::whole(orders().columns().to_vec(), vec![]);
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
        assert_eq!(app.overlay, Some(Overlay::Detail(Detail { scroll: DetailScroll::Lines(1) })));
    }

    #[test]
    fn the_detail_opens_at_the_cursors_column() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Some(Overlay::Detail(Detail { scroll: DetailScroll::ToColumn(1) })));
    }

    #[test]
    fn once_drawn_the_detail_scrolls_from_the_line_the_frame_started_at() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Enter);
        app.fit(Fit { detail: DetailFit { first_line: 3, last_scroll: 5, ..DetailFit::default() }, ..Fit::default() });
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.overlay, Some(Overlay::Detail(Detail { scroll: DetailScroll::Lines(4) })));
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
