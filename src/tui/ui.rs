//! Drawing: the frame in the tier's colour, the panes inside it, and the key hints under them.
use super::app::{App, Fit, Link, TunnelState};
use super::{editor_view, finder_view, help_view, picker_view, result_view, sidebar_view, theme};
use crate::act;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use vtui::text::truncate;

const EDITOR_SHARE_PCT: u16 = 40;
const SIDEBAR_COLUMNS: u16 = 30;
/// A terminal narrower than this leaves the sidebar out, so the editor and the grid keep their room.
const SIDEBAR_MIN_SCREEN_COLUMNS: u16 = 80;

/// Where each part of the screen goes.
struct Areas {
    sidebar: Option<Rect>,
    editor: Rect,
    result: Rect,
    footer: Rect,
}

/// The whole screen for `app`; returns what it had room for.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) -> Fit {
    let has_sidebar = !app.queries.all.is_empty() && f.area().width >= SIDEBAR_MIN_SCREEN_COLUMNS;
    let areas = layout(framed(f, app), has_sidebar);
    if let Some(sidebar) = areas.sidebar {
        sidebar_view::draw(f, app, sidebar);
    }
    editor_view::draw(f, app, areas.editor);
    let grid_rows = result_view::draw(f, app, areas.result);
    footer(f, app, areas.footer);
    finder_view::draw(f, app);
    picker_view::draw(f, app);
    help_view::draw(f, app);
    Fit { grid_rows, has_sidebar }
}

/// The outer frame and its `● name` pill in the tier's colour; returns the area inside it.
fn framed(f: &mut Frame<'_>, app: &App) -> Rect {
    let colour = theme::tier(&app.theme, app.sheet().tier);
    let pill = Span::styled(format!(" ● {} ", app.sheet().name), Style::default().fg(colour).add_modifier(Modifier::BOLD));
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::default().fg(colour)).title(pill);
    let inside = block.inner(f.area());
    f.render_widget(block, f.area());
    inside
}

fn layout(area: Rect, has_sidebar: bool) -> Areas {
    let [panes, footer] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let sidebar_columns = if has_sidebar { SIDEBAR_COLUMNS } else { 0 };
    let [sidebar, panes] = Layout::horizontal([Constraint::Length(sidebar_columns), Constraint::Fill(1)]).areas(panes);
    let [editor, result] = Layout::vertical([Constraint::Percentage(EDITOR_SHARE_PCT), Constraint::Fill(1)]).areas(panes);
    Areas { sidebar: has_sidebar.then_some(sidebar), editor, result, footer }
}

/// The keys that exist on the left, the session's state on the right.
fn footer(f: &mut Frame<'_>, app: &App, area: Rect) {
    let key = Style::default().fg(app.theme.accent).add_modifier(Modifier::BOLD);
    let does = Style::default().fg(app.theme.muted);
    let hints: Vec<Span<'_>> = act::footer(app.focus.pane())
        .flat_map(|hint| [Span::styled(format!(" {} ", hint.keys), key), Span::styled(format!("{} ", hint.does), does)])
        .collect();
    f.render_widget(Paragraph::new(Line::from(hints)), area);
    f.render_widget(Paragraph::new(status(app, usize::from(area.width / 2))).right_aligned(), area);
}

/// The tunnel's state, when the connection has one, then the session's, in `width` columns at most.
fn status(app: &App, width: usize) -> Line<'static> {
    let tunnel = app.sheet().tunnel.as_ref().map(|tunnel| tunnel_state(app, tunnel, width / 2));
    let link = link(app, width - tunnel.as_ref().map_or(0, Span::width));
    Line::from(tunnel.into_iter().chain([link]).collect::<Vec<_>>())
}

fn tunnel_state(app: &App, tunnel: &TunnelState, width: usize) -> Span<'static> {
    match tunnel {
        TunnelState::Starting => Span::styled("⇄ tunnel starting… ", Style::default().fg(app.theme.muted)),
        TunnelState::Up(port) => Span::styled(format!("⇄ tunnel :{port} "), Style::default().fg(app.theme.muted)),
        TunnelState::Died(how) => failure(app, how, width),
    }
}

fn link(app: &App, width: usize) -> Span<'static> {
    match &app.sheet().link {
        Link::Opening => Span::styled("connecting… ", Style::default().fg(app.theme.muted)),
        Link::Open => Span::styled("connected ", Style::default().fg(app.theme.muted)),
        Link::Failed(err) => failure(app, err, width),
    }
}

/// The first line of `err` after a `✗`, cut to `width`.
fn failure(app: &App, err: &str, width: usize) -> Span<'static> {
    let first_line = err.lines().next().unwrap_or_default();
    Span::styled(truncate(&format!("✗ {first_line} "), width), Style::default().fg(app.theme.danger))
}

#[cfg(test)]
mod tests {
    use super::super::app::queries::tests::{query, with_queries};
    use super::super::app::tests::{app, app_on, connection, orders, to_shop, typed};
    use super::super::app::{Answer, App, Incoming, QueryAnswer, RunId};
    use super::*;
    use crate::db::model::LocalPort;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::Duration;
    use vtui::theme::Theme;

    fn render(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| _ = draw(f, app)).unwrap();
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
        app.apply(to_shop(Incoming::Opened));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        insta::assert_snapshot!("result", render(&app, 60, 14).backend());
    }

    #[test]
    fn f1_opens_the_keys_of_the_editor_over_the_screen() {
        let mut app = app();
        app.handle_key(KeyEvent::from(KeyCode::F(1)));
        insta::assert_snapshot!("help", render(&app, 60, 20).backend());
    }

    #[test]
    fn a_tunnel_that_is_up_shows_its_port_beside_the_session() {
        let mut app = app();
        app.apply(to_shop(Incoming::Tunnel(TunnelState::Up(LocalPort::new(49213)))));
        app.apply(to_shop(Incoming::Opened));
        insta::assert_snapshot!("tunnel_up", render(&app, 60, 14).backend());
    }

    #[test]
    fn a_tunnel_that_died_says_how_in_the_status_line() {
        let mut app = app();
        app.apply(to_shop(Incoming::Tunnel(TunnelState::Died("the tunnel `ssh` died, exit status: 255".into()))));
        app.apply(to_shop(Incoming::Closed("its tunnel died".into())));
        insta::assert_snapshot!("tunnel_died", render(&app, 100, 14).backend());
    }

    #[test]
    fn a_session_that_cannot_open_says_so_in_the_status_line() {
        let mut app = app();
        app.apply(to_shop(Incoming::NotOpened("connecting to shop: connection refused".into())));
        insta::assert_snapshot!("not_opened", render(&app, 60, 14).backend());
    }

    #[test]
    fn a_connection_that_allows_writes_shows_a_writes_badge() {
        let app = app_on(&connection("local", "allowed"));
        insta::assert_snapshot!("writes", render(&app, 60, 6).backend());
    }

    #[test]
    fn ctrl_p_opens_the_finder_over_the_screen() {
        let mut app = with_queries();
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::from(KeyCode::Char('o')));
        insta::assert_snapshot!("finder", render(&app, 60, 16).backend());
    }

    #[test]
    fn the_sidebar_shows_the_tree_and_marks_the_open_query() {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened { query: query("orders/daily.sql"), text: "SELECT 1".into() }));
        insta::assert_snapshot!("sidebar", render(&app, 80, 10).backend());
    }

    #[test]
    fn a_narrow_terminal_leaves_the_sidebar_out() {
        assert_eq!(render(&with_queries(), 79, 10).backend().buffer()[(3, 1)].symbol(), "E", "the editor's title starts the row");
    }

    #[test]
    fn a_query_written_for_production_says_so_while_another_connection_is_on_screen() {
        let entry: crate::connection::Entry =
            toml::from_str("tier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\n").unwrap();
        let shop = connection("local", "read-only");
        let mut app = App::new(vec![shop.clone(), crate::connection::Connection::new("prod", entry).unwrap()], &shop, Theme::default());
        let text = "-- name: daily orders\n-- connection: prod\n-- owner: nina\nSELECT 1";
        app.apply(Answer::Queries(QueryAnswer::Opened { query: query("orders/daily.sql"), text: text.into() }));
        insta::assert_snapshot!("query_header", render(&app, 100, 6).backend());
    }

    #[test]
    fn an_open_query_names_the_editor() {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened { query: query("orders/daily.sql"), text: "SELECT 1".into() }));
        insta::assert_snapshot!("query_open", render(&app, 60, 6).backend());
    }

    #[test]
    fn the_frame_takes_the_colour_of_the_tier() {
        let theme = Theme::default();
        for (tier, colour) in [("local", theme.success), ("staging", theme.warn), ("production", theme.danger)] {
            let app = app_on(&connection(tier, "read-only"));
            let terminal = render(&app, 40, 6);
            assert_eq!(terminal.backend().buffer()[(0, 0)].fg, colour, "{tier}");
        }
    }
}
