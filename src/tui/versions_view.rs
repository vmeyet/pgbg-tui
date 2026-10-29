//! The `ctrl-v` list over the screen: a vtui palette over the versions of the query on screen, newest first.
use super::app::{App, Listed, Overlay};
use super::history_view::{one_line, pick};
use super::palette_footer;
use crate::act::Pane;
use crate::format;
use crate::secret;
use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::Line;
use vtui::palette::MAX_SHOWN;
use vtui::palette::view::{self, Pick};

const SAME_TEXT: &str = "the text in the editor";
const CANNOT_PUT_BACK: &str = "cannot be put back";

/// The first line a version changes, else that it is the editor's text; the note for one that held a password.
fn label(listed: &Listed) -> String {
    match (&listed.version.text, &listed.first_changed) {
        (None, _) => secret::NOT_KEPT.to_owned(),
        (Some(_), Some(first)) => one_line(first),
        (Some(_), None) => SAME_TEXT.to_owned(),
    }
}

/// When a save replaced it, and how many lines putting it back adds and removes, or that it cannot be put back.
fn detail(listed: &Listed, now: DateTime<Utc>) -> String {
    let when = format::relative(listed.version.replaced_at, now);
    match listed.version.text {
        Some(_) => format!("{when} · +{} −{}", listed.added, listed.removed),
        None => format!("{when} · {CANNOT_PUT_BACK}"),
    }
}

/// The list over the whole screen when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::Versions(list)) = app.overlay() else { return };
    let theme = app.theme();
    let in_view = list.search.in_view(MAX_SHOWN);
    let labels: Vec<String> = in_view.iter().map(label).collect();
    let details: Vec<String> = in_view.iter().map(|listed| detail(listed, app.clock().now)).collect();
    let picks: Vec<Pick<'_>> = labels.iter().zip(&details).map(|(label, detail)| pick(app, label, detail)).collect();
    let selected = list.search.palette.selected - list.search.first_in_view(MAX_SHOWN);
    let footer = palette_footer::text(Pane::Finder, f.area());
    let title = format!("Versions of {}", list.query.path);
    let popup = view::Popup { title: &title, prompt: view::prompt(theme, &list.search.palette, "", |_| vec![]), footer: &footer };
    view::draw(f, theme, f.area(), popup, |room| {
        if list.all.is_empty() {
            return vec![Line::styled("  none yet: ^s keeps the text it replaces", Style::default().fg(theme.muted))];
        }
        view::pick_lines(theme, &picks, selected, room)
    });
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::shop;
    use super::super::app::queries::tests::{query, with_queries};
    use super::super::app::versions::tests::{daily_versions, not_kept, version};
    use super::super::app::{Answer, Clock, QueryAnswer};
    use super::*;
    use crate::query::Version;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(versions: Vec<Version>) -> Terminal<TestBackend> {
        let mut app = with_queries();
        app.set_clock(Clock { now: DateTime::parse_from_rfc3339("2026-10-26T12:30:00Z").unwrap().to_utc(), zone: chrono::Local });
        app.apply(Answer::Queries(QueryAnswer::Opened {
            connection: shop(),
            query: query("daily.sql"),
            text: "SELECT id\nFROM orders".into(),
        }));
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        app.apply(daily_versions(versions, "SELECT id\nFROM orders"));
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        terminal
    }

    #[test]
    fn each_version_shows_its_first_changed_line_when_it_was_replaced_and_its_size() {
        insta::assert_snapshot!(
            "versions",
            render(vec![version("SELECT id, total\nFROM orders"), version("SELECT id\nFROM orders")]).backend()
        );
    }

    #[test]
    fn a_version_that_held_a_password_shows_the_note_and_that_it_cannot_be_put_back() {
        insta::assert_snapshot!("versions_not_kept", render(vec![not_kept()]).backend());
    }

    #[test]
    fn a_query_without_versions_says_how_to_get_one() {
        insta::assert_snapshot!("versions_empty", render(vec![]).backend());
    }
}
