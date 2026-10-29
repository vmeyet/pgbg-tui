//! The mouse: a left click on a pane gives it the keys, as `tab` would; in the editor it also places the cursor, a
//! double click selects a word and a triple click a line, a drag selects, and letting go copies the selection, as tmux
//! does. The wheel does nothing yet.
use super::{Action, App, Focus};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use std::time::Instant;
use vtui::field::Place;

impl App {
    /// Takes one mouse event read at `now`, which tells a double click from two; a click while an overlay is open does
    /// nothing, since the overlay has the keys.
    pub(in crate::tui) fn handle_mouse(&mut self, mouse: MouseEvent, now: Instant) -> Vec<Action> {
        if self.overlay.is_some() {
            return vec![];
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

    /// Gives the keys to the pane at `at`; in the editor, also places the cursor there or selects, by the click.
    fn press(&mut self, at: Position, now: Instant) -> Vec<Action> {
        self.sheet_mut().editor.deselect();
        let Some(focus) = self.fit.panes.under(at) else { return vec![] };
        self.focus_on(focus);
        let click = self.clicks.press(at, now);
        if focus == Focus::Editor {
            let place = self.editor_place(at);
            let sheet = self.sheet_mut();
            sheet.ghost = None;
            sheet.editor.click(place, click);
        }
        self.drop_idle_ask().into_iter().collect()
    }

    /// The place in the editor's text drawn at `at`, as the last frame drew it.
    fn editor_place(&self, at: Position) -> Place {
        let inside = self.fit.editor_text;
        let first_line = self.sheet().editor.first_line_in_view(inside.height);
        Place { line: first_line + usize::from(at.y.saturating_sub(inside.y)), column: usize::from(at.x.saturating_sub(inside.x)) }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::with_every_pane;
    use super::super::{Focus, Overlay};
    use super::*;
    use crossterm::event::KeyModifiers;
    use std::time::Duration;
    use vtui::help::Help;

    const LEFT: MouseButton = MouseButton::Left;

    fn mouse_at(app: &mut App, kind: MouseEventKind, column: u16, row: u16, now: Instant) -> Vec<Action> {
        app.handle_mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }, now)
    }

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
