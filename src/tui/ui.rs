//! Drawing: the frame in the tier's colour, the panes inside it, and the key hints under them.
use super::app::{App, Fit, Overlay, Panes, SessionState, SidebarTab, TunnelState};
use super::{
    ask_view, column_stats_view, completion_view, detail_view, editor_view, finder_view, form_view, help_view, history_view,
    json_tree_view, param_bar_view, picker_view, prompt_view, result_view, sidebar_view, table_finder_view, versions_view,
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
/// A terminal narrower than this hides the sidebar, so the editor and the grid keep their room.
const SIDEBAR_MIN_SCREEN_COLUMNS: u16 = 80;
const CUT_SCHEMA: &str = "first 20 000 tables";

/// Where each part of the screen goes.
struct Areas {
    panes: Panes,
    footer: Rect,
}

/// The whole screen for `app`; returns what it had room for.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) -> Fit {
    let has_sidebar = f.area().width >= SIDEBAR_MIN_SCREEN_COLUMNS;
    let has_params = !app.sheet().param_names().is_empty();
    let Areas { panes, footer: footer_area } = layout(framed(f, app), has_sidebar, has_params);
    let schema_rows = panes.sidebar.map(|sidebar| sidebar_view::draw(f, app, sidebar)).unwrap_or_default();
    if let Some(params) = panes.params {
        param_bar_view::draw(f, app, params);
    }
    let editor_text = editor_view::draw(f, app, panes.editor);
    let grid = result_view::draw(f, app, panes.result);
    ask_view::draw(f, app, panes.result);
    completion_view::draw(f, app, panes.editor);
    footer(f, app, footer_area);
    finder_view::draw(f, app);
    table_finder_view::draw(f, app);
    picker_view::draw(f, app);
    history_view::draw(f, app);
    versions_view::draw(f, app);
    form_view::draw(f, app);
    let detail = detail_view::draw(f, app);
    column_stats_view::draw(f, app);
    json_tree_view::draw(f, app);
    help_view::draw(f, app);
    Fit { grid, panes, editor_text, detail, schema_rows }
}

/// The outer frame and its `● name · tier` label in the tier's colour, so the tier is clear even without colour; returns
/// the area inside it.
fn framed(f: &mut Frame<'_>, app: &App) -> Rect {
    let sheet = app.sheet();
    let tier = sheet.connection.tier();
    let colour = app.theme().tier(tier);
    let pill = Span::styled(format!(" ● {} · {tier} ", sheet.name()), Style::default().fg(colour).add_modifier(Modifier::BOLD));
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::default().fg(colour)).title(pill);
    let inside = block.inner(f.area());
    f.render_widget(block, f.area());
    inside
}

/// The areas of `area`, with the sidebar when `has_sidebar`, and the parameter bar one line above the editor when
/// `has_params`.
fn layout(area: Rect, has_sidebar: bool, has_params: bool) -> Areas {
    let [panes, footer] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let sidebar_columns = if has_sidebar { SIDEBAR_COLUMNS } else { 0 };
    let [sidebar, panes] = Layout::horizontal([Constraint::Length(sidebar_columns), Constraint::Fill(1)]).areas(panes);
    let [top, result] = Layout::vertical([Constraint::Percentage(EDITOR_SHARE_PCT), Constraint::Fill(1)]).areas(panes);
    let bar_rows = u16::from(has_params);
    let [params, editor] = Layout::vertical([Constraint::Length(bar_rows), Constraint::Fill(1)]).areas(top);
    let panes = Panes { sidebar: has_sidebar.then_some(sidebar), params: has_params.then_some(params), editor, result };
    Areas { panes, footer }
}

/// The session's state on the right, as wide as it needs up to half the row, and the keys that fit on the left.
/// An open prompt takes the whole row.
fn footer(f: &mut Frame<'_>, app: &App, area: Rect) {
    match app.overlay() {
        Some(Overlay::Prompt(prompt)) => return prompt_view::draw(f, app, prompt, area),
        Some(Overlay::Ask(step)) => return ask_view::footer(f, app, step, area),
        Some(Overlay::RowFilter(filter)) => return prompt_view::draw_row_filter(f, app, filter, area),
        _ => {}
    }
    let status = status(app, usize::from(area.width / STATUS_SHARE_OF_ROW));
    let status_columns = u16::try_from(status.width()).unwrap_or(area.width);
    let [hints_area, status_area] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(status_columns)]).areas(area);
    let pane = match app.overlay() {
        Some(Overlay::CopyAs) => Pane::CopyAs,
        Some(Overlay::SchemaFilter) => Pane::SchemaFilter,
        Some(Overlay::Completion(_)) => Pane::Completion,
        _ => app.pane(),
    };
    f.render_widget(Paragraph::new(hints(app, pane, usize::from(hints_area.width))), hints_area);
    f.render_widget(Paragraph::new(status), status_area);
}

fn hints(app: &App, pane: Pane, columns: usize) -> Line<'static> {
    let key = Style::default().fg(app.theme().accent).add_modifier(Modifier::BOLD);
    let does = Style::default().fg(app.theme().muted);
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

/// The tunnel then the session as one line, `⇄ :49213 · connected`, or the one reason they are down, in `width`
/// columns; a change to the config that just failed shows why first.
fn status(app: &App, width: usize) -> Line<'static> {
    if let Some(why) = app.config_failure() {
        return failure(app, why, width);
    }
    if let Some(warning) = app.warning() {
        return failure(app, warning, width);
    }
    if let Some(why) = app.ask_failure() {
        return failure(app, &why.to_string(), width);
    }
    if let Some(note) = app.note() {
        return Line::styled(truncate(&format!("✓ {} ", printable(note)), width), Style::default().fg(app.theme().muted));
    }
    if app.is_asking_idle() {
        return Line::styled(truncate(ask_view::asking(app).trim_start(), width), Style::default().fg(app.theme().warn));
    }
    let sheet = app.sheet();
    if let Some(why) = &sheet.page_failure {
        return failure(app, why, width);
    }
    let muted = Style::default().fg(app.theme().muted);
    let text = match (&sheet.tunnel, &sheet.session) {
        (Some(TunnelState::Died(how)), _) => return failure(app, how, width),
        (_, SessionState::Failed(err)) => return failure(app, err, width),
        (Some(tunnel), session) => format!("{} · {} ", tunnel_text(tunnel), session_text(session)),
        (None, session) => format!("{} ", session_text(session)),
    };
    let text = if is_cut_schema_shown(app) { format!("{CUT_SCHEMA} · {text}") } else { text };
    Line::styled(truncate(&text, width), muted)
}

/// Whether the schema tab shows a schema whose read kept only the first tables.
fn is_cut_schema_shown(app: &App) -> bool {
    let is_cut = app.sheet().catalog().is_some_and(|catalog| catalog.is_cut);
    app.sidebar() == SidebarTab::Schema && is_cut
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
    Line::styled(truncate(&format!("✗ {} ", printable(first_line)), width), Style::default().fg(app.theme().danger))
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{app, app_on, connection, every_pane, on_the_result, to_shop, typed};
    use super::super::app::queries::tests::{listed, query, with_queries};
    use super::super::app::{Answer, App, Incoming, QueryAnswer, RunId};
    use super::super::styled;
    use super::*;
    use crate::db::model::{Catalog, Extent, LocalPort, Page, ResultSet};
    use crate::failure::{Failure, Lost};
    use crate::fixtures::orders;
    use crate::tui::theme::Theme;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use std::sync::Arc;
    use std::time::Duration;

    fn render(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| _ = draw(f, app)).unwrap();
        terminal
    }

    /// The wall clock, read in a release build only, by CI's own step: `cargo test --release --lib -- --ignored budget::`.
    #[cfg(not(debug_assertions))]
    mod budget {
        use super::*;
        use crate::fixtures::best_of;

        const STATEMENT: &str = "SELECT id, customer, total\nFROM orders\nWHERE customer = {{owner}} AND total > 10 -- recent\nLIMIT 50;\n";
        const KEYS_TYPED: u32 = 200;
        /// One key and its frame take 0.17 ms on a laptop, CI about four times that: ten times slower fails.
        const KEY_AND_FRAME_BUDGET: Duration = Duration::from_micros(1_500);

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn one_key_and_its_frame_on_a_thousand_line_editor_stay_within_the_budget() {
            let mut app = app();
            app.paste(&STATEMENT.repeat(250));
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            let mut type_keys = || {
                for _ in 0..KEYS_TYPED {
                    app.handle_key(KeyEvent::from(KeyCode::Char('x')));
                    let mut fit = Fit::default();
                    terminal.draw(|f| fit = draw(f, &app)).unwrap();
                    app.fit(fit);
                }
            };
            let took = best_of(&mut type_keys) / KEYS_TYPED;
            assert!(took < KEY_AND_FRAME_BUDGET, "took {took:?}");
        }
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

    fn dracula() -> Theme {
        Theme::from(vtui::theme::Theme::named("dracula").unwrap())
    }

    /// The app in `theme` with the result of `SELECT id, customer FROM orders` on screen.
    fn a_result_in(theme: Theme) -> App {
        let shop = connection("local", "read-only");
        let mut app = App::new(vec![shop.clone()], &shop, theme);
        "SELECT id, customer FROM orders".chars().for_each(|c| _ = app.handle_key(KeyEvent::from(KeyCode::Char(c))));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Opened));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
        app
    }

    #[test]
    fn a_named_theme_paints_the_result_screen_in_its_own_colours() {
        insta::assert_snapshot!("result_dracula", styled::runs(render(&a_result_in(dracula()), 60, 14).backend().buffer()));
    }

    #[test]
    fn a_palette_with_its_own_ground_paints_no_colour_of_the_terminal_on_any_screen() {
        let mut app = a_result_in(dracula());
        let result = render(&app, 60, 14);
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Char('v')));
        let chart = render(&app, 60, 14);
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        let picker = render(&app, 60, 14);
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        app.handle_key(KeyEvent::from(KeyCode::F(1)));
        let help = render(&app, 60, 20);
        for terminal in [result, chart, picker, help] {
            let painted = terminal.backend().buffer().content().iter().map(|cell| cell.fg).filter(|fg| *fg != Color::Reset);
            let terminal_colours: Vec<Color> = painted.filter(|fg| !matches!(fg, Color::Rgb(..))).collect();
            assert_eq!(terminal_colours, vec![], "a colour that bypasses the theme");
        }
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
        app.apply(to_shop(Incoming::Closed(Lost::TunnelDied)));
        insta::assert_snapshot!("tunnel_died", render(&app, 100, 14).backend());
    }

    #[test]
    fn a_session_that_cannot_open_says_so_in_the_status_line() {
        let mut app = app();
        app.apply(to_shop(Incoming::NotOpened(Failure::Login("connecting to shop: connection refused".into()))));
        insta::assert_snapshot!("not_opened", render(&app, 60, 14).backend());
    }

    #[test]
    fn a_session_error_cannot_reverse_the_status_line() {
        let mut app = app();
        app.apply(to_shop(Incoming::NotOpened(Failure::Login("role \u{202e}anin does not exist".into()))));
        let drawn = render(&app, 60, 14).backend().to_string();
        assert!(drawn.contains("\\u{202e}anin") && !drawn.contains('\u{202e}'), "{drawn}");
    }

    #[test]
    fn a_failed_run_cannot_reverse_the_result_pane() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Failed { run: RunId::FIRST, failure: Failure::Link("column \u{2066}latot does not exist".into()) }));
        let drawn = render(&app, 60, 14).backend().to_string();
        assert!(drawn.contains("\\u{2066}latot") && !drawn.contains('\u{2066}'), "{drawn}");
    }

    #[test]
    fn a_connection_that_allows_writes_shows_a_writes_badge() {
        let app = app_on(&connection("local", "allowed"));
        insta::assert_snapshot!("writes", render(&app, 60, 6).backend());
    }

    #[test]
    fn a_write_asks_on_the_footer_line_before_it_is_sent() {
        let mut app = app_on(&connection("production", "allowed"));
        "DELETE FROM orders".chars().for_each(|c| _ = app.handle_key(KeyEvent::from(KeyCode::Char(c))));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        insta::assert_snapshot!("send_write", render(&app, 60, 6).backend());
    }

    #[test]
    fn a_finder_with_more_matches_than_fit_scrolls_and_keeps_its_footer() {
        let mut app = with_queries();
        let many = (1..=20).map(|day| query(&format!("orders/day{day:02}.sql"))).collect();
        app.apply(listed(many));
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

    fn on_the_schema_tab(is_cut: bool) -> App {
        let mut app = app();
        app.fit(Fit { panes: every_pane(), ..Fit::default() });
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Char(']')));
        app.apply(to_shop(Incoming::Opened));
        let catalog = Catalog { schema: crate::fixtures::shop_schema(), search_path: vec!["public".to_owned()], is_cut };
        app.apply(to_shop(Incoming::Schema(Ok(Arc::new(catalog)))));
        app
    }

    #[test]
    fn the_schema_tab_shows_each_schema_and_its_tables() {
        insta::assert_snapshot!("schema_tab", render(&on_the_schema_tab(false), 80, 10).backend());
    }

    #[test]
    fn the_schema_tab_shows_the_filter_being_typed_above_the_tables_it_keeps() {
        let mut app = on_the_schema_tab(false);
        "/ord".chars().for_each(|c| _ = app.handle_key(KeyEvent::from(KeyCode::Char(c))));
        insta::assert_snapshot!("schema_filter", render(&app, 80, 10).backend());
    }

    #[test]
    fn d_on_a_table_shows_its_columns_and_indexes_in_the_result_pane() {
        let mut app = on_the_schema_tab(false);
        for _ in 0..3 {
            app.handle_key(KeyEvent::from(KeyCode::Char('j')));
        }
        app.handle_key(KeyEvent::from(KeyCode::Char('d')));
        insta::assert_snapshot!("schema_table", render(&app, 100, 14).backend());
    }

    #[test]
    fn ctrl_t_opens_the_table_finder_over_the_screen() {
        let mut app = app();
        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        let catalog = Catalog { schema: crate::fixtures::shop_schema(), search_path: vec!["public".to_owned()], is_cut: false };
        app.apply(to_shop(Incoming::Schema(Ok(Arc::new(catalog)))));
        app.handle_key(KeyEvent::from(KeyCode::Char('o')));
        insta::assert_snapshot!("table_finder", render(&app, 60, 16).backend());
    }

    #[test]
    fn the_table_finder_footer_at_sixty_columns_drops_whole_hints_and_keeps_esc() {
        let mut app = app();
        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        let screen = format!("{}", render(&app, 60, 16).backend());
        assert!(screen.contains("   ⏎ rows · ^d columns · esc closes   │"), "{screen}");
    }

    #[test]
    fn a_cut_schema_says_so_in_the_status_line() {
        let screen = format!("{:?}", render(&on_the_schema_tab(true), 100, 10).backend());
        assert!(screen.contains("first 20 000 tables · connected"), "{screen}");
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
        app.apply(to_shop(Incoming::Closed(Lost::TunnelDied)));
        let footer = footer_of(&render(&app, 100, 8));
        assert_eq!(footer.matches('✗').count(), 1, "{footer}");
        assert!(footer.contains("✗ the tunnel `ssh` died"), "{footer}");
    }

    #[test]
    fn c_on_the_result_asks_for_the_format_on_the_footer() {
        let mut app = on_the_result(orders());
        app.handle_key(KeyEvent::from(KeyCode::Char('c')));
        let footer = footer_of(&render(&app, 100, 8));
        assert!(footer.contains("c CSV  j JSON  m Markdown  esc cancel"), "{footer}");
    }

    #[test]
    fn a_failed_page_says_why_in_the_status_line() {
        let first = Page { rows: orders().rows().to_vec(), extent: Extent::More };
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let result = ResultSet::new(orders().columns().to_vec(), first);
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result, took: Duration::from_millis(38) }));
        app.apply(to_shop(Incoming::PageFailed { run: RunId::FIRST, failure: Failure::Link("ERROR: division by zero".into()) }));
        assert!(footer_of(&render(&app, 100, 8)).contains("✗ the next page failed: ERROR: division by zero"));
    }

    #[test]
    fn a_copy_says_what_it_copied_in_the_status_line() {
        let mut app = on_the_result(orders());
        app.apply(Answer::Copied("copied row 1 as CSV".into()));
        assert!(footer_of(&render(&app, 100, 8)).contains("✓ copied row 1 as CSV"));
    }

    #[test]
    fn a_warning_given_at_start_shows_in_the_status_line_until_a_key() {
        let mut app = app().with_warning(Some("old.toml sits in a folder pgbg no longer reads".into()));
        assert!(footer_of(&render(&app, 100, 8)).contains("✗ old.toml sits in a folder pgbg no longer reads"));
        app.handle_key(KeyEvent::from(KeyCode::Char('x')));
        assert!(!footer_of(&render(&app, 100, 8)).contains("old.toml"));
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
        app.apply(to_shop(Incoming::NotOpened(Failure::Login("connecting to shop: connection refused".into()))));
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
        for (tier, colour) in [("local", theme.success), ("staging", theme.warn), ("production", theme.production)] {
            let app = app_on(&connection(tier, "read-only"));
            let terminal = render(&app, 40, 6);
            assert_eq!(terminal.backend().buffer()[(0, 0)].fg, colour, "{tier}");
        }
    }
}
