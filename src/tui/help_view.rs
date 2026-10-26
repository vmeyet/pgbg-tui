//! The `?` overlay over the whole screen: the keys of the focused pane, or every key after a second `?`. vtui draws it.
use super::app::App;
use crate::act;
use ratatui::Frame;

/// The overlay when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    if let Some(help) = app.help {
        vtui::help::draw(f, &app.theme, f.area(), help, &act::sections(app.focus.pane(), help.every_key));
    }
}
