//! The `ctrl-p` finder: a vtui palette over every saved query, best match first.
use super::app::{App, Overlay};
use crate::act::{self, Pane};
use crate::query::{Query, Source};
use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::Line;
use vtui::palette::view::{self, Pick, Popup};

const NO_QUERY: &str = "  no saved query yet: add .sql files under .pgbg/queries/";

/// A query as the finder lists it, `user` beside one of your own.
fn pick<'a>(app: &App, query: &Query, label: &'a str) -> Pick<'a> {
    let detail = if query.source == Source::User { "user" } else { "" };
    Pick { icon: "◆", colour: app.theme.accent, label, detail }
}

/// The finder over the whole screen when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::Finder(finder)) = &app.overlay else { return };
    let theme = &app.theme;
    let labels: Vec<String> = finder.shown.iter().map(|query| query.path.to_string()).collect();
    let picks: Vec<Pick<'_>> = finder.shown.iter().zip(&labels).map(|(query, label)| pick(app, query, label)).collect();
    let hints: Vec<String> = act::footer(Pane::Finder).map(|hint| format!("{} {}", hint.keys, hint.does)).collect();
    let footer = format!("  {}", hints.join(" · "));
    let popup = Popup { title: "Queries", prompt: view::prompt(theme, &finder.palette, "", |_| vec![]), footer: &footer };
    view::draw(f, theme, f.area(), popup, |room| {
        if app.queries.all.is_empty() {
            return vec![Line::styled(NO_QUERY, Style::default().fg(theme.muted))];
        }
        view::pick_lines(theme, &picks, finder.palette.selected, room)
    });
}
