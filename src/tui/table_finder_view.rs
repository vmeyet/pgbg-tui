//! The `ctrl-t` finder: a vtui palette over every table of the schema, best match first.
use super::app::{App, Overlay, SchemaState};
use super::palette_footer;
use crate::act::Pane;
use crate::connection::SchemaReading;
use crate::render::printable;
use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::Line;
use vtui::palette::MAX_SHOWN;
use vtui::palette::view::{self, Pick, Popup};

/// What the finder shows when it lists no table: why there is no schema yet, or that nothing matches.
fn nothing_listed(app: &App) -> String {
    let sheet = app.sheet();
    if sheet.connection.schema() == SchemaReading::Off {
        return "  schema = \"off\" on this connection".to_owned();
    }
    match &sheet.schema {
        SchemaState::Reading => "  reading the schema…".to_owned(),
        SchemaState::Failed(why) => format!("  ✗ {}", printable(why)),
        SchemaState::NotRead | SchemaState::Read(_) => "  no table holds it".to_owned(),
    }
}

/// The finder over the whole screen when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::TableFinder(finder)) = app.overlay() else { return };
    let theme = app.theme();
    let in_view = finder.search.in_view(MAX_SHOWN);
    let labels: Vec<String> = in_view.iter().map(|table| printable(&format!("{}.{}", table.schema, table.name)).into_owned()).collect();
    let picks: Vec<Pick<'_>> =
        labels.iter().map(|label| Pick { icon: "▦", colour: theme.accent, label, detail: view::faded(theme, "") }).collect();
    let selected = finder.search.palette.selected - finder.search.first_in_view(MAX_SHOWN);
    let footer = palette_footer::text(Pane::TableFinder, f.area());
    let popup = Popup { title: "Tables", prompt: view::prompt(theme, &finder.search.palette, "", |_| vec![]), footer: &footer };
    view::draw(f, theme, f.area(), popup, |room| {
        if picks.is_empty() {
            return vec![Line::styled(nothing_listed(app), Style::default().fg(theme.muted))];
        }
        view::pick_lines(theme, &picks, selected, room)
    });
}
