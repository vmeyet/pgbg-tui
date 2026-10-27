//! Drawing: the frame in the tier's colour, the panes inside it, and the key hints under them.
use super::app::{App, Fit, Overlay, SessionState, TunnelState};
use super::{
    detail_view, editor_view, finder_view, form_view, help_view, history_view, picker_view, prompt_view, result_view, sidebar_view, theme,
};
use crate::act::{self, Hint, Pane};
use crate::render::printable;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use vtui::text::truncate;

const EDITOR_SHARE_PCT: u16 = 40;
const SIDEBAR_COLUMNS: u16 = 30;
/// The status takes at most half the footer, so the hints keep the other half.
const STATUS_SHARE_OF_ROW: u16 = 2;
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
    history_view::draw(f, app);
    form_view::draw(f, app);
    let detail_last_scroll = detail_view::draw(f, app);
    help_view::draw(f, app);
    Fit { grid_rows, has_sidebar, detail_last_scroll }
}

/// The outer frame and its `● name · tier` pill in the tier's colour, so the tier reads without its colour; returns
/// the area inside it.
fn framed(f: &mut Frame<'_>, app: &App) -> Rect {
    let sheet = app.sheet();
    let tier = sheet.connection.tier();
    let colour = theme::tier(&app.theme, tier);
    let pill = Span::styled(format!(" ● {} · {tier} ", sheet.name()), Style::default().fg(colour).add_modifier(Modifier::BOLD));
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

/// The session's state on the right, as wide as it needs up to half the row, and the keys that fit on the left; an open
/// prompt takes the whole row.
fn footer(f: &mut Frame<'_>, app: &App, area: Rect) {
    if let Some(Overlay::Prompt(prompt)) = &app.overlay {
        return prompt_view::draw(f, app, prompt, area);
    }
    let status = status(app, usize::from(area.width / STATUS_SHARE_OF_ROW));
    let status_columns = u16::try_from(status.width()).unwrap_or(area.width);
    let [hints_area, status_area] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(status_columns)]).areas(area);
    let pane = if app.overlay == Some(Overlay::CopyAs) { Pane::CopyAs } else { app.focus.pane() };
    f.render_widget(Paragraph::new(hints(app, pane, usize::from(hints_area.width))), hints_area);
    f.render_widget(Paragraph::new(status), status_area);
}

fn hints(app: &App, pane: Pane, columns: usize) -> Line<'static> {
    let key = Style::default().fg(app.theme.accent).add_modifier(Modifier::BOLD);
    let does = Style::default().fg(app.theme.muted);
    let width = |hint: Hint| hint_text(hint).iter().map(|text| text.chars().count()).sum();
    let spans = act::footer_within(pane, columns, width).into_iter().flat_map(|hint| {
        let [keys, what] = hint_text(hint);
        [Span::styled(keys, key), Span::styled(what, does)]
    });
    Line::from(spans.collect::<Vec<_>>())
}

/// ` ⏎ ` and `run `: a hint's keys and what they do, as the footer writes them.
fn hint_text(hint: Hint) -> [String; 2] {
    [format!(" {} ", hint.keys), format!("{} ", hint.does)]
}

/// One chain: the tunnel then the session, `⇄ :49213 · connected`, or the one reason they are down, in `width` columns;
/// a change to the config that just failed says why first.
fn status(app: &App, width: usize) -> Line<'static> {
    if let Some(why) = &app.config_failure {
        return failure(app, why, width);
    }
    if let Some(note) = &app.note {
        return Line::styled(truncate(&format!("✓ {} ", printable(note)), width), Style::default().fg(app.theme.muted));
    }
    let sheet = app.sheet();
    let muted = Style::default().fg(app.theme.muted);
    let text = match (&sheet.tunnel, &sheet.session) {
        (Some(TunnelState::Died(how)), _) => return failure(app, how, width),
        (_, SessionState::Failed(err)) => return failure(app, err, width),
        (Some(tunnel), session) => format!("{} · {} ", tunnel_text(tunnel), session_text(session)),
        (None, session) => format!("{} ", session_text(session)),
    };
    Line::styled(truncate(&text, width), muted)
}

fn tunnel_text(tunnel: &TunnelState) -> String {
    match tunnel {
        TunnelState::Starting => "⇄ starting…".to_owned(),
        TunnelState::Up(port) => format!("⇄ :{port}"),
        TunnelState::Died(_) => "⇄ died".to_owned(),
    }
}

fn session_text(session: &SessionState) -> &'static str {
    match session {
        SessionState::Opening => "connecting…",
        SessionState::Open => "connected",
        SessionState::Failed(_) => "failed",
        SessionState::Closed => "closed",
    }
}

/// The first line of `err` after a `✗`, cut to `width`.
fn failure(app: &App, err: &str, width: usize) -> Line<'static> {
    let first_line = err.lines().next().unwrap_or_default();
    Line::styled(truncate(&format!("✗ {} ", printable(first_line)), width), Style::default().fg(theme::failure(&app.theme)))
}

#[cfg(test)]
mod tests {
    use super::super::app::queries::tests::{query, with_queries};
    use super::super::app::tests::{app, app_on, connection, orders, to_shop, typed};
    use super::super::app::{Answer, App, Incoming, QueryAnswer, RunId};
    use super::super::styled;
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
    fn a_session_error_cannot_reverse_the_status_line() {
        let mut app = app();
        app.apply(to_shop(Incoming::NotOpened("role \u{202e}anin does not exist".into())));
        let drawn = render(&app, 60, 14).backend().to_string();
        assert!(drawn.contains("\\u{202e}anin") && !drawn.contains('\u{202e}'), "{drawn}");
    }

    #[test]
    fn a_failed_run_cannot_reverse_the_result_pane() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, error: "column \u{2066}latot does not exist".into(), position: None }));
        let drawn = render(&app, 60, 14).backend().to_string();
        assert!(drawn.contains("\\u{2066}latot") && !drawn.contains('\u{2066}'), "{drawn}");
    }

    #[test]
    fn a_connection_that_allows_writes_shows_a_writes_badge() {
        let app = app_on(&connection("local", "allowed"));
        insta::assert_snapshot!("writes", render(&app, 60, 6).backend());
    }

    #[test]
    fn a_finder_with_more_matches_than_fit_scrolls_and_keeps_its_footer() {
        let mut app = with_queries();
        let many = (1..=20).map(|day| query(&format!("orders/day{day:02}.sql"))).collect();
        app.apply(Answer::Queries(QueryAnswer::All(many)));
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        (0..14).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Down)));
        let screen = format!("{:?}", render(&app, 60, 20).backend());
        assert!(screen.contains("▎◆ orders/day15.sql") && screen.contains("esc"), "{screen}");
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
        app.apply(Answer::Queries(QueryAnswer::Opened {
            connection: app.sheet().name().clone(),
            query: query("orders/daily.sql"),
            text: "SELECT 1".into(),
        }));
        insta::assert_snapshot!("sidebar", render(&app, 80, 10).backend());
    }

    #[test]
    fn a_narrow_terminal_leaves_the_sidebar_out() {
        assert_eq!(render(&with_queries(), 79, 10).backend().buffer()[(3, 1)].symbol(), "E", "the editor's title starts the row");
    }

    #[test]
    fn a_query_written_for_production_says_so_while_another_connection_is_on_screen() {
        let prod = crate::fixtures::named("prod", "tier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\n");
        let shop = connection("local", "read-only");
        let mut app = App::new(vec![shop.clone(), prod], &shop, Theme::default());
        let text = "-- name: daily orders\n-- connection: prod\n-- owner: nina\nSELECT 1";
        app.apply(Answer::Queries(QueryAnswer::Opened {
            connection: app.sheet().name().clone(),
            query: query("orders/daily.sql"),
            text: text.into(),
        }));
        insta::assert_snapshot!("query_header", render(&app, 100, 6).backend());
    }

    #[test]
    fn ctrl_s_on_new_text_asks_for_its_path_on_the_footer_line() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        "orders/".chars().for_each(|c| _ = app.handle_key(KeyEvent::from(KeyCode::Char(c))));
        insta::assert_snapshot!("save_as", render(&app, 60, 6).backend());
    }

    #[test]
    fn an_open_query_names_the_editor() {
        let mut app = with_queries();
        app.apply(Answer::Queries(QueryAnswer::Opened {
            connection: app.sheet().name().clone(),
            query: query("orders/daily.sql"),
            text: "SELECT 1".into(),
        }));
        insta::assert_snapshot!("query_open", render(&app, 60, 6).backend());
    }

    fn row_of(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buffer = terminal.backend().buffer();
        (buffer.area.left()..buffer.area.right()).map(|x| buffer[(x, y)].symbol()).collect()
    }

    fn footer_of(terminal: &Terminal<TestBackend>) -> String {
        row_of(terminal, terminal.backend().buffer().area.bottom() - 2)
    }

    fn connected_through_a_tunnel() -> App {
        let mut app = app();
        app.apply(to_shop(Incoming::Tunnel(TunnelState::Up(LocalPort::new(49213)))));
        app.apply(to_shop(Incoming::Opened));
        app
    }

    #[test]
    fn the_status_chains_the_tunnel_and_the_session() {
        assert!(footer_of(&render(&connected_through_a_tunnel(), 100, 8)).contains("⇄ :49213 · connected"));
    }

    #[test]
    fn quit_and_the_keys_stay_in_the_footer_when_the_hints_do_not_fit() {
        for width in [100, 60, 44] {
            let footer = footer_of(&render(&connected_through_a_tunnel(), width, 8));
            assert!(footer.contains("^c quit") && footer.contains("F1 keys") && footer.contains("connected"), "{width}: {footer}");
        }
    }

    #[test]
    fn a_dead_tunnel_gives_the_one_reason_the_session_is_down() {
        let mut app = app();
        app.apply(to_shop(Incoming::Tunnel(TunnelState::Died("the tunnel `ssh` died, exit status: 255".into()))));
        app.apply(to_shop(Incoming::Closed("its tunnel died".into())));
        let footer = footer_of(&render(&app, 100, 8));
        assert_eq!(footer.matches('✗').count(), 1, "{footer}");
        assert!(footer.contains("✗ the tunnel `ssh` died"), "{footer}");
    }

    fn result_in_focus() -> App {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app
    }

    #[test]
    fn c_on_the_result_asks_for_the_format_on_the_footer() {
        let mut app = result_in_focus();
        app.handle_key(KeyEvent::from(KeyCode::Char('c')));
        let footer = footer_of(&render(&app, 100, 8));
        assert!(footer.contains("c CSV  j JSON  m Markdown  esc cancel"), "{footer}");
    }

    #[test]
    fn a_copy_says_what_it_copied_in_the_status_line() {
        let mut app = result_in_focus();
        app.apply(Answer::Copied("copied row 1 as CSV".into()));
        assert!(footer_of(&render(&app, 100, 8)).contains("✓ copied row 1 as CSV"));
    }

    #[test]
    fn a_starting_tunnel_fits_a_narrow_screen() {
        let mut app = app();
        app.apply(to_shop(Incoming::Tunnel(TunnelState::Starting)));
        insta::assert_snapshot!("tunnel_starting_narrow", render(&app, 36, 8).backend());
    }

    #[test]
    fn the_pill_names_the_tier() {
        let terminal = render(&app_on(&connection("production", "read-only")), 60, 6);
        assert!(row_of(&terminal, 0).contains("● shop · production"));
    }

    #[test]
    fn a_production_frame_and_an_error_never_share_a_colour() {
        let mut app = app_on(&connection("production", "read-only"));
        app.apply(to_shop(Incoming::NotOpened("connecting to shop: connection refused".into())));
        insta::assert_snapshot!("production_error_styled", styled::runs(render(&app, 60, 6).backend().buffer()));
    }

    #[test]
    fn the_pane_with_the_keys_has_the_focused_frame() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        insta::assert_snapshot!("focus_styled", styled::runs(render(&app, 40, 10).backend().buffer()));
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
