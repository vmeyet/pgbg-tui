//! `⏎` on the grid: the row under the cursor over the screen, one column after another, scrolled with `j`/`k`.
use super::{Action, App, Overlay, Run};
use crate::act::{self, Act, Pane};
use crossterm::event::KeyEvent;

/// The open detail of the grid's row: how many lines it is scrolled down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct Detail {
    pub(in crate::tui) scroll: usize,
}

impl App {
    /// Opens the detail of the grid's row, when the result has one.
    pub(super) fn open_detail(&mut self) {
        let has_row = matches!(&self.sheet().run, Run::Done { result, grid, .. } if grid.row < result.rows().len());
        if has_row {
            self.overlay = Some(Overlay::Detail(Detail::default()));
        }
    }

    /// The detail after `key`: it scrolls within what the frame drew, `esc` closes it.
    pub(super) fn detail_key(&mut self, detail: Detail, key: KeyEvent) -> Vec<Action> {
        let scroll = match act::act_for(key, Pane::Detail) {
            Some(Act::Down) => (detail.scroll + 1).min(self.fit.detail_last_scroll),
            Some(Act::Up) => detail.scroll.saturating_sub(1),
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
    use super::super::tests::{orders, to_shop, typed};
    use super::super::{Fit, Incoming, RunId};
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::time::Duration;

    fn on_the_result() -> App {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app
    }

    fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    #[test]
    fn enter_on_the_grid_opens_the_detail_of_its_row() {
        let mut app = on_the_result();
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!(app.overlay, Some(Overlay::Detail(Detail::default())));
    }

    #[test]
    fn space_opens_it_too() {
        let mut app = on_the_result();
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.overlay, Some(Overlay::Detail(Detail::default())));
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
        let mut app = on_the_result();
        app.fit(Fit { detail_last_scroll: 2, ..Fit::default() });
        press(&mut app, KeyCode::Enter);
        (0..4).for_each(|_| _ = press(&mut app, KeyCode::Char('j')));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.overlay, Some(Overlay::Detail(Detail { scroll: 1 })));
    }

    #[test]
    fn esc_closes_the_detail_and_leaves_the_grid_where_it_was() {
        let mut app = on_the_result();
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!((app.overlay.is_none(), grid.row), (true, 1));
    }

    #[test]
    fn ctrl_c_in_the_detail_quits() {
        let mut app = on_the_result();
        press(&mut app, KeyCode::Enter);
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }
}
