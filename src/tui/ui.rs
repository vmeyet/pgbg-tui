//! Drawing: the frame in the tier's colour, the panes inside it, and the key hints under them.
use super::app::{App, Link};
use super::{editor_view, result_view, theme};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use vtui::text::truncate;

const EDITOR_SHARE_PCT: u16 = 40;
const HINTS: [(&str, &str); 3] = [("⏎", "run"), ("⌥⏎", "new line"), ("^c", "quit")];

/// Where each part of the screen goes.
struct Areas {
    editor: Rect,
    result: Rect,
    footer: Rect,
}

/// The whole screen for `app`.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let areas = layout(framed(f, app));
    editor_view::draw(f, app, areas.editor);
    result_view::draw(f, app, areas.result);
    footer(f, app, areas.footer);
}

/// The outer frame and its `● name` pill in the tier's colour; returns the area inside it.
fn framed(f: &mut Frame<'_>, app: &App) -> Rect {
    let colour = theme::tier(&app.theme, app.tier);
    let pill = Span::styled(format!(" ● {} ", app.name), Style::default().fg(colour).add_modifier(Modifier::BOLD));
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::default().fg(colour)).title(pill);
    let inside = block.inner(f.area());
    f.render_widget(block, f.area());
    inside
}

fn layout(area: Rect) -> Areas {
    let [panes, footer] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let [editor, result] = Layout::vertical([Constraint::Percentage(EDITOR_SHARE_PCT), Constraint::Fill(1)]).areas(panes);
    Areas { editor, result, footer }
}

/// The keys that exist on the left, the session's state on the right.
fn footer(f: &mut Frame<'_>, app: &App, area: Rect) {
    let key = Style::default().fg(app.theme.accent).add_modifier(Modifier::BOLD);
    let does = Style::default().fg(app.theme.muted);
    let hints: Vec<Span<'_>> =
        HINTS.iter().flat_map(|&(keys, what)| [Span::styled(format!(" {keys} "), key), Span::styled(format!("{what} "), does)]).collect();
    f.render_widget(Paragraph::new(Line::from(hints)), area);
    f.render_widget(Paragraph::new(link(app, usize::from(area.width / 2))).right_aligned(), area);
}

fn link(app: &App, width: usize) -> Line<'static> {
    match &app.link {
        Link::Opening => Line::styled("connecting… ", Style::default().fg(app.theme.muted)),
        Link::Open => Line::styled("connected ", Style::default().fg(app.theme.muted)),
        Link::Failed(err) => {
            let first_line = err.lines().next().unwrap_or_default();
            Line::styled(truncate(&format!("✗ {first_line} "), width), Style::default().fg(app.theme.danger))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::tests::{app, connection, orders, typed};
    use super::super::app::{App, Incoming};
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::Duration;
    use vtui::theme::Theme;

    fn render(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal
    }

    #[test]
    fn a_fresh_screen_shows_the_connection_an_empty_editor_and_the_keys() {
        insta::assert_snapshot!("fresh", render(&app(), 60, 14).backend());
    }

    #[test]
    fn a_result_shows_its_count_its_time_and_its_rows() {
        let mut app = typed("SELECT id, customer FROM orders");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(Incoming::Opened);
        app.apply(Incoming::Result { result: orders(), took: Duration::from_millis(38) });
        insta::assert_snapshot!("result", render(&app, 60, 14).backend());
    }

    #[test]
    fn a_session_that_cannot_open_says_so_in_the_status_line() {
        let mut app = app();
        app.apply(Incoming::NotOpened("connecting to shop: connection refused".into()));
        insta::assert_snapshot!("not_opened", render(&app, 60, 14).backend());
    }

    #[test]
    fn a_connection_that_allows_writes_shows_a_writes_badge() {
        let app = App::new(&connection("local", "allowed"), Theme::default());
        insta::assert_snapshot!("writes", render(&app, 60, 6).backend());
    }

    #[test]
    fn the_frame_takes_the_colour_of_the_tier() {
        let theme = Theme::default();
        for (tier, colour) in [("local", theme.success), ("staging", theme.warn), ("production", theme.danger)] {
            let app = App::new(&connection(tier, "read-only"), theme);
            let terminal = render(&app, 40, 6);
            assert_eq!(terminal.backend().buffer()[(0, 0)].fg, colour, "{tier}");
        }
    }
}
