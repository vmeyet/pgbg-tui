//! The connection picker over the screen: each connection's dot in its tier's colour, where it goes, whether it
//! takes writes, and whether its session is open. vtui draws the popup around it.
use super::app::{App, Overlay, SessionState};
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
    let Some(Overlay::Picker(picker)) = &app.overlay else { return };
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

/// `▎● prod  via tunnel · read only  open`: the name is cut to leave room for its badge and state, the place to the room left.
fn line(app: &App, connection: &Connection, is_selected: bool, room: usize) -> Line<'static> {
    let theme = &app.theme;
    let (writes, writes_style) = match connection.writes() {
        WritePolicy::ReadOnly => (" · read only", Style::default().fg(theme.faded)),
        WritePolicy::Allowed => ("  WRITES", Style::default().fg(theme.danger).add_modifier(Modifier::BOLD)),
    };
    let (state, state_style) = session_state(app, connection);
    let place = connection.tunnel().map_or_else(|| connection.target().place(), |_| "via tunnel".to_owned());
    let kept = ["▎● ", writes, state].iter().map(|text| text.chars().count()).sum::<usize>();
    let name = truncate(&connection.name().to_string(), room.saturating_sub(kept));
    let taken = kept + name.chars().count() + "  ".len();
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

/// `running…` while a statement runs, else `open`, `opening…` or `failed` once its session was asked for; nothing before.
fn session_state(app: &App, connection: &Connection) -> (&'static str, Style) {
    let Some(sheet) = app.sheet_of(connection.name()) else { return ("", Style::default()) };
    if sheet.is_running() {
        return ("  running…", Style::default().fg(app.theme.accent));
    }
    match sheet.session {
        SessionState::Opening => ("  opening…", Style::default().fg(app.theme.muted)),
        SessionState::Open => ("  open", Style::default().fg(app.theme.success)),
        SessionState::Failed(_) => ("  failed", Style::default().fg(app.theme.danger)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::picker::tests::{ctrl, named, on_local, press};
    use super::super::app::{Answer, App, Incoming};
    use super::super::ui;
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use vtui::theme::Theme;

    fn drawn(app: &App, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, app)).unwrap();
        format!("{:?}", terminal.backend())
    }

    #[test]
    fn a_long_name_is_cut_before_its_writes_badge() {
        let long = named(
            "staging-replica-for-the-reporting-team",
            "tier = \"staging\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\nwrites = \"allowed\"\n",
        );
        let mut app = App::new(vec![long.clone()], &long, Theme::default());
        ctrl(&mut app, 'o');
        let screen = drawn(&app, 60);
        assert!(screen.contains("▎● staging-re…  …  WRITES  opening…"), "{screen}");
    }

    #[test]
    fn a_hidden_connection_running_a_statement_says_so() {
        let mut app = on_local();
        press(&mut app, "SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        ctrl(&mut app, 'o');
        press(&mut app, "prod");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        ctrl(&mut app, 'o');
        let screen = drawn(&app, 100);
        assert!(screen.contains("local  localhost/shop · read only  running…"), "{screen}");
    }

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
