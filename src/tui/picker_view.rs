//! The connection picker over the screen: each connection's dot in its tier's colour, where it goes, whether it
//! takes writes, and whether its session is open. vtui draws the popup around it.
use super::app::{App, Link};
use super::theme;
use crate::connection::{Connection, WritePolicy};
use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use vtui::palette::MAX_SHOWN;
use vtui::palette::view::{self, Popup};
use vtui::text::truncate;

const FOOTER: &str = "  ↑↓ pick · ⏎ opens or switches · esc";

/// The picker when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(picker) = &app.picker else { return };
    let prompt = view::prompt(&app.theme, picker, "", |_| vec![]);
    let popup = Popup { title: "Connections", prompt, footer: FOOTER };
    let matching = app.matching(&picker.input);
    view::draw(f, &app.theme, f.area(), popup, |room| lines(app, &matching, picker.selected, room));
}

/// The rows in view, scrolled so the highlighted one shows.
fn lines(app: &App, matching: &[&Connection], selected: usize, room: usize) -> Vec<Line<'static>> {
    if matching.is_empty() {
        return vec![Line::styled("  no match", Style::default().fg(app.theme.muted))];
    }
    let first = selected.saturating_sub(MAX_SHOWN - 1);
    let shown = matching.iter().enumerate().skip(first).take(MAX_SHOWN);
    shown.map(|(at, connection)| line(app, connection, at == selected, room)).collect()
}

/// `▎● prod  via tunnel · read only  open`: the place is cut to the room the rest leaves.
fn line(app: &App, connection: &Connection, is_selected: bool, room: usize) -> Line<'static> {
    let theme = &app.theme;
    let name = connection.name().to_string();
    let (writes, writes_style) = match connection.writes() {
        WritePolicy::ReadOnly => (" · read only", Style::default().fg(theme.faded)),
        WritePolicy::Allowed => ("  WRITES", Style::default().fg(theme.danger).add_modifier(Modifier::BOLD)),
    };
    let (state, state_style) = session_state(app, connection);
    let place = connection.tunnel().map_or_else(|| connection.target().place(), |_| "via tunnel".to_owned());
    let taken = ["▎● ", &name, "  ", writes, state].iter().map(|text| text.chars().count()).sum::<usize>();
    let name_style = if is_selected { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
    Line::from(vec![
        Span::styled(if is_selected { "▎" } else { " " }, Style::default().fg(theme.accent)),
        Span::styled("● ", Style::default().fg(theme::tier(theme, connection.tier()))),
        Span::styled(name, name_style),
        Span::styled(format!("  {}", truncate(&place, room.saturating_sub(taken))), Style::default().fg(theme.muted)),
        Span::styled(writes, writes_style),
        Span::styled(state, state_style),
    ])
}

/// `open`, `opening…` or `failed` once its session was asked for; nothing before.
fn session_state(app: &App, connection: &Connection) -> (&'static str, Style) {
    let Some(sheet) = app.sheet_of(connection.name()) else { return ("", Style::default()) };
    match sheet.link {
        Link::Opening => ("  opening…", Style::default().fg(app.theme.muted)),
        Link::Open => ("  open", Style::default().fg(app.theme.success)),
        Link::Failed(_) => ("  failed", Style::default().fg(app.theme.danger)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::picker::tests::{ctrl, on_local};
    use super::super::app::{Answer, Incoming};
    use super::super::ui;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn the_picker_shows_each_connection_its_place_its_writes_and_its_session() {
        let mut app = on_local();
        app.apply(Answer::Session { connection: "local".parse().unwrap(), incoming: Incoming::Opened });
        ctrl(&mut app, 'o');
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("picker", terminal.backend());
    }
}
