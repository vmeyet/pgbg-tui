//! The connection picker over the screen: each connection's dot in its tier's colour, where it connects, whether it
//! takes writes, and whether its session is open.
//! vtui draws the popup around it.
use super::app::{App, Note, Overlay, Search, SessionState};
use crate::connection::{Connection, WritePolicy};
use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use vtui::palette::MAX_SHOWN;
use vtui::palette::view::{self, Pick, Popup};
use vtui::text::truncate;

/// The columns vtui leaves between a pick's label and its detail.
const GAP_WIDTH: usize = 2;

const FOOTER: &str = "  ↑↓ pick · ⏎ opens · ^d removes · ^x closes · esc";

/// The picker when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::Picker(picker)) = app.overlay() else { return };
    let prompt = view::prompt(app.theme(), &picker.search.palette, "", |_| vec![]);
    let popup = Popup { title: "Connections", prompt, footer: FOOTER };
    let rows = MAX_SHOWN - usize::from(picker.note.is_some());
    view::draw(f, app.theme(), f.area(), popup, |room| {
        let mut lines = lines(app, &picker.search, rows, room);
        lines.extend(picker.note.as_ref().map(|note| note_line(app, note, room)));
        lines
    });
}

/// The question or the message below the list.
fn note_line(app: &App, note: &Note, room: usize) -> Line<'static> {
    let (text, style) = match note {
        Note::ConfirmRemove(name) => (
            format!("  remove `{name}`? y removes it, any other key keeps it"),
            Style::default().fg(app.theme().danger).add_modifier(Modifier::BOLD),
        ),
        Note::Said(said) => (format!("  {said}"), Style::default().fg(app.theme().muted)),
    };
    Line::styled(truncate(&text, room + 2), style)
}

/// The `rows` in view, scrolled so the highlighted one shows.
fn lines(app: &App, search: &Search<Connection>, rows: usize, room: usize) -> Vec<Line<'static>> {
    let selected = search.palette.selected - search.first_in_view(rows);
    let names: Vec<String> = search.in_view(rows).iter().map(|connection| connection.name().to_string()).collect();
    let picks: Vec<Pick<'_>> =
        search.in_view(rows).iter().zip(&names).map(|(connection, name)| pick(app, connection, name, room)).collect();
    view::pick_lines(app.theme(), &picks, selected, room)
}

/// `● prod  via tunnel · read only  open`: the dot in the tier's colour.
/// vtui cuts the name before the badges, and the place is cut first to the room the name leaves.
fn pick<'a>(app: &App, connection: &Connection, name: &'a str, room: usize) -> Pick<'a> {
    let badges = badges(app, connection);
    let place = connection.tunnel().map_or_else(|| connection.target().place(), |_| "via tunnel".to_owned());
    let place_room = room.saturating_sub(Span::raw(name).width() + GAP_WIDTH + badges.width());
    let mut detail = Line::styled(truncate(&place, place_room), Style::default().fg(app.theme().muted));
    detail.spans.extend(badges.spans);
    Pick { icon: "●", colour: app.theme().tier(connection.tier()), label: name, detail }
}

/// Whether the connection takes writes, then the state of its session.
fn badges(app: &App, connection: &Connection) -> Line<'static> {
    let theme = app.theme();
    let writes = match connection.writes() {
        WritePolicy::ReadOnly => Span::styled(" · read only", Style::default().fg(theme.faded)),
        WritePolicy::Allowed => Span::styled("  WRITES", Style::default().fg(theme.danger).add_modifier(Modifier::BOLD)),
    };
    let (state, state_style) = session_state(app, connection);
    Line::from(vec![writes, Span::styled(state, state_style)])
}

/// `running…` while a statement runs, or else `open`, `opening…` or `failed` once its session was asked for; nothing
/// before that.
fn session_state(app: &App, connection: &Connection) -> (&'static str, Style) {
    let Some(sheet) = app.sheet_of(connection.name()) else { return ("", Style::default()) };
    if sheet.is_running() {
        return ("  running…", Style::default().fg(app.theme().accent));
    }
    match sheet.session {
        SessionState::Opening => ("  opening…", Style::default().fg(app.theme().muted)),
        SessionState::Open => ("  open", Style::default().fg(app.theme().success)),
        SessionState::Failed(_) => ("  failed", Style::default().fg(app.theme().danger)),
        SessionState::Closed => ("  closed", Style::default().fg(app.theme().muted)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::picker::tests::{ctrl, on_local, press};
    use super::super::app::{Answer, App, Incoming, SessionId};
    use super::super::ui;
    use crate::fixtures::named;
    use crate::tui::theme::Theme;
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn drawn(app: &App, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, app)).unwrap();
        format!("{:?}", terminal.backend())
    }

    #[test]
    fn a_question_under_a_full_list_keeps_the_footer() {
        let shops: Vec<_> =
            (1..=14).map(|at| named(&format!("shop{at:02}"), "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n")).collect();
        let mut app = App::new(shops.clone(), &shops[0], Theme::default());
        ctrl(&mut app, 'o');
        (0..13).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Down)));
        ctrl(&mut app, 'd');
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        let screen = format!("{:?}", terminal.backend());
        assert!(screen.contains("▎● shop14") && screen.contains("remove `shop14`?") && screen.contains("esc"), "{screen}");
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
        app.apply(Answer::Session { connection: "local".parse().unwrap(), session: SessionId::FIRST, incoming: Incoming::Opened });
        ctrl(&mut app, 'o');
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("picker", terminal.backend());
    }

    #[test]
    fn the_picker_asks_before_removing_a_connection() {
        let mut app = on_local();
        app.apply(Answer::Session { connection: "local".parse().unwrap(), session: SessionId::FIRST, incoming: Incoming::Opened });
        ctrl(&mut app, 'o');
        app.handle_key(KeyEvent::from(KeyCode::Down));
        ctrl(&mut app, 'd');
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("picker_confirm", terminal.backend());
    }
}
