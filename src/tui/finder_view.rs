//! The `ctrl-p` finder: a vtui palette over every saved query, best match first.
use super::app::{App, found};
use crate::act::{self, Pane};
use crate::query::QueryPath;
use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::Line;
use vtui::palette::view::{self, Pick, Popup};

const NO_QUERY: &str = "  no saved query yet: add .sql files under .pgbg/queries/";

/// The finder over the whole screen when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(finder) = &app.finder else { return };
    let theme = &app.theme;
    let labels: Vec<String> = found(&app.queries.all, &finder.input).iter().map(QueryPath::to_string).collect();
    let picks: Vec<Pick<'_>> = labels.iter().map(|label| Pick { icon: "◆", colour: theme.accent, label, detail: "" }).collect();
    let hints: Vec<String> = act::footer(Pane::Finder).map(|hint| format!("{} {}", hint.keys, hint.does)).collect();
    let footer = format!("  {}", hints.join(" · "));
    let popup = Popup { title: "Queries", prompt: view::prompt(theme, finder, "", |_| vec![]), footer: &footer };
    view::draw(f, theme, f.area(), popup, |room| {
        if app.queries.all.is_empty() {
            return vec![Line::styled(NO_QUERY, Style::default().fg(theme.muted))];
        }
        view::pick_lines(theme, &picks, finder.selected, room)
    });
}
