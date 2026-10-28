//! The `ctrl-r` search over the screen: a vtui palette over the statements run on the connection, newest first.
use super::app::{App, Overlay};
use super::palette_footer;
use crate::act::Pane;
use crate::history::Entry;
use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::Line;
use vtui::palette::MAX_SHOWN;
use vtui::palette::view::{self, Pick, Popup};

const WHEN_FORMAT: &str = "%m-%d %H:%M";

/// A text from the past on one line, next to its faded detail: a statement run, or a version of a query.
pub(super) fn pick<'a>(app: &App, label: &'a str, detail: &'a str) -> Pick<'a> {
    Pick { icon: "↺", colour: app.theme().accent, label, detail: view::faded(app.theme(), detail) }
}

fn detail(entry: &Entry) -> String {
    format!("{} · {}", entry.started_at.format(WHEN_FORMAT), entry.outcome)
}

pub(super) fn one_line(statement: &str) -> String {
    statement.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The search over the whole screen when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::Recall(recall)) = app.overlay() else { return };
    let theme = app.theme();
    let in_view = recall.search.in_view(MAX_SHOWN);
    let labels: Vec<String> = in_view.iter().map(|entry| one_line(entry.statement.as_str())).collect();
    let details: Vec<String> = in_view.iter().map(detail).collect();
    let picks: Vec<Pick<'_>> = labels.iter().zip(&details).map(|(label, detail)| pick(app, label, detail)).collect();
    let selected = recall.search.palette.selected - recall.search.first_in_view(MAX_SHOWN);
    let footer = palette_footer::text(Pane::Finder, f.area());
    let title = format!("History of {}", recall.connection);
    let popup = Popup { title: &title, prompt: view::prompt(theme, &recall.search.palette, "", |_| vec![]), footer: &footer };
    view::draw(f, theme, f.area(), popup, |room| {
        if recall.all.is_empty() {
            return vec![Line::styled("  no statement run on this connection yet", Style::default().fg(theme.muted))];
        }
        view::pick_lines(theme, &picks, selected, room)
    });
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{app, shop};
    use super::super::app::{Action, Answer};
    use super::*;
    use crate::db::model::Statement;
    use crate::history::Outcome;
    use chrono::DateTime;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn entry(statement: &str, outcome: Outcome) -> Entry {
        let started_at = DateTime::parse_from_rfc3339("2026-10-26T09:30:00Z").unwrap().to_utc();
        Entry { statement: Statement::new(statement), connection: shop(), started_at, took_ms: 38, outcome }
    }

    fn render(entries: Vec<Entry>) -> Terminal<TestBackend> {
        let mut app = app();
        let asked: Vec<Action> = app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(asked, [Action::ListHistory(shop())]);
        app.apply(Answer::History { connection: shop(), entries });
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        terminal
    }

    #[test]
    fn each_statement_shows_on_one_line_with_when_and_what_came_of_it() {
        let entries = vec![
            entry("SELECT total\n  FROM orders", Outcome::Rows { count: 4, is_cut: false }),
            entry("DELETE FROM orders", Outcome::Refused),
        ];
        insta::assert_snapshot!("history", render(entries).backend());
    }

    #[test]
    fn a_connection_never_run_says_so() {
        insta::assert_snapshot!("history_empty", render(vec![]).backend());
    }
}
