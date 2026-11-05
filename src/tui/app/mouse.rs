//! The mouse: a left click on a pane gives it the keys, as `tab` would; in the editor it also places the cursor, a
//! double click selects a word and a triple click a line, a drag selects, and letting go copies the selection, as tmux
//! does; in the schema tab it picks the row as `⏎` does; the row's detail takes its own clicks. The wheel does nothing
//! yet.
use super::{Action, App, Focus, Overlay};
use crate::act::Pane;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use std::time::Instant;
use vtui::field::Place;

impl App {
    /// Takes one mouse event read at `now`, which tells a double click from two; a click while an overlay other than the
    /// row's detail is open does nothing, since the overlay has the keys.
    pub(in crate::tui) fn handle_mouse(&mut self, mouse: MouseEvent, now: Instant) -> Vec<Action> {
        match self.overlay {
            Some(Overlay::Detail(_)) => return self.detail_mouse(mouse, now),
            Some(_) => return vec![],
            None => {}
        }
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.press(at, now),
            MouseEventKind::Drag(MouseButton::Left) if self.focus == Focus::Editor => {
                let place = self.editor_place(at);
                self.sheet_mut().editor.drag_to(place);
                vec![]
            }
            MouseEventKind::Up(MouseButton::Left) if self.focus == Focus::Editor => self.copy_selection().into_iter().collect(),
            _ => vec![],
        }
    }

    /// Gives the keys to the pane at `at`; in the editor, also places the cursor there or selects, by the click; in the
    /// schema tab, picks the row there.
    fn press(&mut self, at: Position, now: Instant) -> Vec<Action> {
        self.sheet_mut().editor.deselect();
        let Some(focus) = self.fit.panes.under(at) else { return vec![] };
        let place = self.editor_place(at);
        self.focus_on(focus);
        let click = self.clicks.press(at, now);
        if focus == Focus::Editor {
            let sheet = self.sheet_mut();
            sheet.ghost = None;
            sheet.rewrite = None;
            sheet.editor.click(place, click);
        }
        let picked = if self.pane() == Pane::Schema { self.click_schema_row(at) } else { vec![] };
        self.drop_idle_ask().into_iter().chain(picked).collect()
    }

    /// The place in the editor's text drawn at `at`, as the last frame drew it: the ghost shows only while the editor has
    /// the keys.
    fn editor_place(&self, at: Position) -> Place {
        let inside = self.fit.editor_text;
        let (down, column) = (usize::from(at.y.saturating_sub(inside.y)), usize::from(at.x.saturating_sub(inside.x)));
        let rest = self.sheet().ghost.as_ref().filter(|_| self.focus == Focus::Editor).map(|ghost| ghost.rest.as_str());
        self.sheet().editor.place_drawn_at(down, column, rest)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{every_pane, mouse_at, with_every_pane};
    use super::super::ghost::Ghost;
    use super::super::{Fit, Focus};
    use super::*;
    use ratatui::layout::Rect;
    use std::time::Duration;
    use vtui::help::Help;

    const LEFT: MouseButton = MouseButton::Left;

    fn click(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
        mouse_at(app, kind, column, row, Instant::now());
    }

    /// Every pane drawn, the editor holding `text` from its inside's corner, at column 31 and row 2.
    fn editing(text: &str) -> App {
        let mut app = with_every_pane();
        app.sheet_mut().editor.reload(text);
        app
    }

    fn selected(app: &App) -> Option<&str> {
        app.sheet().editor.selected()
    }

    /// `count` quick presses of the left button at one cell of the editor, then its release; what the release does.
    fn presses(app: &mut App, count: u32, column: u16, row: u16) -> Vec<Action> {
        let start = Instant::now();
        for press in 0..count {
            mouse_at(app, MouseEventKind::Down(LEFT), column, row, start + Duration::from_millis(100) * press);
        }
        mouse_at(app, MouseEventKind::Up(LEFT), column, row, start)
    }

    fn copied(text: &str) -> Vec<Action> {
        vec![Action::Copy { text: text.to_owned(), done: "copied the selection".to_owned() }]
    }

    #[test]
    fn a_click_in_the_editor_puts_the_cursor_there() {
        let mut app = editing("SELECT id\nFROM orders");
        click(&mut app, MouseEventKind::Down(LEFT), 36, 3);
        let (before, _, _) = app.sheet().editor.split();
        assert_eq!(before, "SELECT id\nFROM ");
    }

    /// Draws the editor ten rows high from its inside's corner, at column 31 and row 2.
    fn draw_ten_rows(app: &mut App) {
        app.fit(Fit { panes: every_pane(), editor_text: Rect::new(31, 2, 48, 10), ..Fit::default() });
    }

    /// Thirty lines, `line 0` to `line 29`, drawn ten rows high with the cursor on the last.
    fn thirty_lines() -> App {
        let mut app = editing(&(0..30).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n"));
        draw_ten_rows(&mut app);
        app
    }

    #[test]
    fn a_press_and_a_drag_select_the_lines_drawn_under_the_mouse_and_never_scroll() {
        let mut app = thirty_lines();
        click(&mut app, MouseEventKind::Down(LEFT), 31, 2);
        draw_ten_rows(&mut app);
        click(&mut app, MouseEventKind::Drag(LEFT), 31, 3);
        draw_ten_rows(&mut app);
        assert_eq!((selected(&app), app.sheet().editor.first_row().line), (Some("line 20\n"), 20));
    }

    #[test]
    fn the_keys_still_scroll_the_view_to_the_cursor() {
        let mut app = thirty_lines();
        (0..25).for_each(|_| _ = app.handle_key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Up)));
        draw_ten_rows(&mut app);
        assert_eq!(app.sheet().editor.first_row().line, 4);
    }

    /// One line of 140 columns, whose rows of the 48 columns drawn each hold 42 and end on a space, the cursor at its start.
    fn one_wrapped_line() -> App {
        let mut app = editing(&"orders ".repeat(20));
        app.handle_key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Home));
        app.fit(Fit { panes: every_pane(), editor_text: Rect::new(31, 2, 48, 7), ..Fit::default() });
        app
    }

    #[test]
    fn a_click_on_a_wrapped_row_puts_the_cursor_on_the_character_drawn_there() {
        let mut app = one_wrapped_line();
        click(&mut app, MouseEventKind::Down(LEFT), 33, 3);
        assert_eq!(app.sheet().editor.split().0.len(), 44);
    }

    #[test]
    fn a_click_past_the_end_of_a_shorter_row_puts_the_cursor_on_the_whitespace_at_its_break() {
        let mut app = one_wrapped_line();
        click(&mut app, MouseEventKind::Down(LEFT), 77, 2);
        assert_eq!(app.sheet().editor.split().0.len(), 41);
    }

    #[test]
    fn a_click_past_a_row_that_ends_on_a_tab_lands_on_that_row() {
        let mut app = editing("a\tbcdef");
        app.handle_key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Home));
        app.fit(Fit { panes: every_pane(), editor_text: Rect::new(31, 2, 4, 7), ..Fit::default() });
        click(&mut app, MouseEventKind::Down(LEFT), 34, 2);
        assert_eq!(app.sheet().editor.split().0, "");
    }

    /// `text` with the cursor at byte `at` and `rest` as its ghost, the editor 48 columns wide and having the keys.
    fn ghosted(text: &str, at: usize, rest: &str) -> App {
        let mut app = editing(text);
        app.sheet_mut().editor.move_cursor_to(at);
        app.sheet_mut().ghost = Ghost::at_cursor(at, rest);
        app.fit(Fit { panes: every_pane(), editor_text: Rect::new(31, 2, 48, 7), ..Fit::default() });
        app
    }

    fn before_click(app: &mut App, column: u16, row: u16) -> String {
        click(app, MouseEventKind::Down(LEFT), column, row);
        app.sheet().editor.split().0.to_owned()
    }

    #[test]
    fn a_click_under_a_ghost_that_wraps_lands_on_the_character_drawn_there() {
        let mut app = ghosted("SELECT * FROM cu\nWHERE x", 16, &"x".repeat(100));
        assert_eq!(before_click(&mut app, 33, 5), "SELECT * FROM cu\nWH");
    }

    #[test]
    fn a_click_under_the_later_lines_of_a_ghost_lands_on_the_character_drawn_there() {
        let mut app = ghosted("SELECT * FROM cu\nWHERE x", 16, "stomers\nLIMIT 1\nOFFSET 2");
        assert_eq!(before_click(&mut app, 33, 5), "SELECT * FROM cu\nWH");
    }

    #[test]
    fn a_click_on_the_ghost_leaves_the_cursor_where_the_ghost_starts() {
        let mut app = ghosted("SELECT * FROM cu\nWHERE x", 16, "stomers\nLIMIT 1");
        assert_eq!(before_click(&mut app, 50, 2), "SELECT * FROM cu");
        let mut app = ghosted("SELECT * FROM cu\nWHERE x", 16, "stomers\nLIMIT 1");
        assert_eq!(before_click(&mut app, 33, 3), "SELECT * FROM cu");
    }

    #[test]
    fn a_click_on_the_text_after_a_ghost_lands_on_the_character_drawn_there() {
        let mut app = ghosted("SELECT id FROM orders", 7, "total,");
        assert_eq!(before_click(&mut app, 45, 2), "SELECT i");
    }

    #[test]
    fn a_click_with_the_editor_out_of_focus_ignores_the_ghost_it_did_not_draw() {
        let mut app = ghosted("SELECT * FROM cu\nWHERE x", 16, &"x".repeat(100));
        app.focus = Focus::Result;
        assert_eq!(before_click(&mut app, 33, 3), "SELECT * FROM cu\nWH");
    }

    #[test]
    fn a_drag_across_wrapped_rows_selects_the_text_with_no_line_break() {
        let mut app = one_wrapped_line();
        click(&mut app, MouseEventKind::Down(LEFT), 77, 2);
        click(&mut app, MouseEventKind::Drag(LEFT), 33, 3);
        assert_eq!(selected(&app), Some(" or"));
    }

    #[test]
    fn a_triple_click_on_a_wrapped_row_selects_the_whole_line() {
        let mut app = one_wrapped_line();
        presses(&mut app, 3, 33, 3);
        assert_eq!(selected(&app), Some("orders ".repeat(20).as_str()));
    }

    #[test]
    fn a_click_alone_selects_and_copies_nothing() {
        let mut app = editing("SELECT id");
        assert_eq!(presses(&mut app, 1, 33, 2), []);
    }

    #[test]
    fn a_drag_in_the_editor_selects_and_letting_go_copies_it() {
        let mut app = editing("SELECT id\nFROM orders");
        click(&mut app, MouseEventKind::Down(LEFT), 38, 2);
        click(&mut app, MouseEventKind::Drag(LEFT), 35, 3);
        assert_eq!(selected(&app), Some("id\nFROM"));
        assert_eq!(mouse_at(&mut app, MouseEventKind::Up(LEFT), 35, 3, Instant::now()), copied("id\nFROM"));
    }

    #[test]
    fn a_double_click_selects_the_word_and_copies_it() {
        let mut app = editing("SELECT id\nFROM orders");
        assert_eq!(presses(&mut app, 2, 38, 3), copied("orders"));
    }

    #[test]
    fn a_triple_click_selects_the_line_and_copies_it() {
        let mut app = editing("SELECT id\nFROM orders");
        assert_eq!(presses(&mut app, 3, 33, 2), copied("SELECT id"));
    }

    #[test]
    fn any_key_drops_the_selection() {
        let mut app = editing("SELECT id");
        presses(&mut app, 2, 33, 2);
        app.handle_key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Tab));
        assert_eq!(selected(&app), None);
    }

    #[test]
    fn a_drag_that_began_outside_the_editor_selects_nothing() {
        let mut app = editing("SELECT id");
        click(&mut app, MouseEventKind::Down(LEFT), 40, 12);
        click(&mut app, MouseEventKind::Drag(LEFT), 33, 2);
        assert_eq!(selected(&app), None);
    }

    #[test]
    fn a_click_on_a_pane_gives_it_the_keys() {
        let mut app = with_every_pane();
        click(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        assert_eq!(app.focus, Focus::Result);
        click(&mut app, MouseEventKind::Down(MouseButton::Left), 5, 3);
        assert_eq!(app.focus, Focus::Queries);
    }

    #[test]
    fn the_wheel_and_the_right_button_leave_the_keys_where_they_are() {
        let mut app = with_every_pane();
        click(&mut app, MouseEventKind::ScrollDown, 40, 12);
        click(&mut app, MouseEventKind::Down(MouseButton::Right), 40, 12);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn a_click_under_an_overlay_leaves_the_keys_to_it() {
        let mut app = with_every_pane();
        app.overlay = Some(Overlay::Help(Help::default()));
        click(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        assert_eq!((app.focus, app.overlay.is_some()), (Focus::Editor, true));
    }
}
