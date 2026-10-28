//! The mouse: a left click on a pane gives it the keys, as `tab` would; the wheel and drags do nothing yet.
use super::{Action, App};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

impl App {
    /// Takes one mouse event; a click while an overlay is open does nothing, since the overlay has the keys.
    pub(in crate::tui) fn handle_mouse(&mut self, mouse: MouseEvent) -> Vec<Action> {
        let MouseEventKind::Down(MouseButton::Left) = mouse.kind else { return vec![] };
        if self.overlay.is_some() {
            return vec![];
        }
        let Some(focus) = self.fit.panes.under(Position::new(mouse.column, mouse.row)) else { return vec![] };
        self.focus_on(focus);
        self.drop_idle_ask().into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::with_every_pane;
    use super::super::{Focus, Overlay};
    use super::*;
    use crossterm::event::KeyModifiers;
    use vtui::help::Help;

    fn click(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
        app.handle_mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE });
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
