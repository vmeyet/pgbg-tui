//! The `?` overlay over the whole screen: the keys of the focused pane, or every key after a second `?`. vtui draws it.
use super::app::{App, Overlay};
use crate::act;
use ratatui::Frame;

/// The overlay when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    if let Some(Overlay::Help(help)) = app.overlay {
        vtui::help::draw(f, &app.theme, f.area(), help, &act::sections(app.pane(), help.every_key));
    }
}
