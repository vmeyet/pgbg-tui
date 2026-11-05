//! The cursor of the row's detail: `j`/`k` move it over the values, a line at a time through one too tall to fit, a click
//! moves it there, and a double click copies that value. `⇥` moves it too, in the module above, with the key it lands on.
use super::super::{Action, App, Overlay};
use super::{Detail, DetailFit, DetailScroll, DrawnDetail, Under};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use std::time::Instant;
use vtui::click::Click;

/// The column of the value under the cursor, and the line the detail starts at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cursor {
    column: usize,
    line: usize,
}

impl DetailFit {
    /// The index of the line drawn at `at`, whether a line is there or not.
    fn line_at(&self, at: Position) -> Option<usize> {
        self.body.contains(at).then(|| self.first_line + usize::from(at.y - self.body.y))
    }
}

impl DrawnDetail {
    /// The column drawn on `line`; none on the tables pointing to the row, their rule, or past them.
    fn column_on(&self, line: usize) -> Option<usize> {
        (line < self.lines.len()).then(|| self.column_starts.partition_point(|&start| start <= line).saturating_sub(1))
    }

    /// After `j`: a line further while the value runs past the bottom, or has no value after it; else on the next value,
    /// with as much of it in view as fits.
    fn after_down(&self, Cursor { column, line }: Cursor, height: usize, last_scroll: usize) -> Cursor {
        let next = self.lines_of(column + 1);
        if self.lines_of(column).end > line + height || next.is_empty() {
            return Cursor { column, line: (line + 1).min(last_scroll) };
        }
        let in_view = next.end.saturating_sub(height).min(next.start);
        Cursor { column: column + 1, line: line.max(in_view).min(last_scroll) }
    }

    /// After `k`: a line back while the value starts above the top, or has no value before it; else on the one before.
    fn after_up(&self, Cursor { column, line }: Cursor, height: usize) -> Cursor {
        if line > self.lines_of(column).start || column == 0 {
            return Cursor { column, line: line.saturating_sub(1) };
        }
        self.arriving_from_below(column - 1, line, height)
    }

    /// On the value at `column` from further down, with the end of it in view, where `k` goes on from.
    fn arriving_from_below(&self, column: usize, line: usize, height: usize) -> Cursor {
        let lines = self.lines_of(column);
        Cursor { column, line: line.min(lines.end.saturating_sub(height).max(lines.start)) }
    }
}

/// `detail` with its cursor at `moved`; with none, scrolled alone to `line`.
fn moved_to(detail: Detail, moved: Option<Cursor>, line: usize) -> Detail {
    match moved {
        Some(Cursor { column, line }) => Detail { under: Under::Value(column), scroll: DetailScroll::Lines(line), ..detail },
        None => Detail { scroll: DetailScroll::Lines(line), ..detail },
    }
}

impl App {
    /// What the last frame drew of the row `detail` shows; none until a frame drew that row.
    fn drawn_for(&self, detail: &Detail) -> Option<&DrawnDetail> {
        let shown = self.shown_row(detail)?;
        self.drawn_detail.as_ref().filter(|drawn| drawn.of.row == shown.of)
    }

    /// The line `detail` starts at, once a frame placed it.
    fn start_line(&self, detail: &Detail) -> usize {
        match detail.scroll {
            DetailScroll::Lines(line) => line,
            DetailScroll::ToColumn(_) => self.fit.detail.first_line,
        }
    }

    /// `detail` after `j`; on a table pointing to the row, or before a frame, it scrolls alone.
    pub(super) fn cursor_down(&self, detail: Detail) -> Detail {
        let (line, height, last_scroll) = (self.start_line(&detail), usize::from(self.fit.detail.body.height), self.fit.detail.last_scroll);
        let moved = detail
            .under
            .value()
            .zip(self.drawn_for(&detail))
            .map(|(column, drawn)| drawn.after_down(Cursor { column, line }, height, last_scroll));
        moved_to(detail, moved, (line + 1).min(last_scroll))
    }

    /// `detail` after `k`; on a table pointing to the row, it goes back to the last value; before a frame, it scrolls alone.
    pub(super) fn cursor_up(&self, detail: Detail) -> Detail {
        let (line, height) = (self.start_line(&detail), usize::from(self.fit.detail.body.height));
        let moved = self.drawn_for(&detail).map(|drawn| match detail.under {
            Under::Value(column) => drawn.after_up(Cursor { column, line }, height),
            Under::Table(_) => drawn.arriving_from_below(drawn.column_starts.len().saturating_sub(1), line, height),
        });
        moved_to(detail, moved, line.saturating_sub(1))
    }

    /// Takes one mouse event while the detail is open: a press on a value puts the cursor there, and a double click copies it.
    pub(in crate::tui::app) fn detail_mouse(&mut self, mouse: MouseEvent, now: Instant) -> Vec<Action> {
        let at = Position::new(mouse.column, mouse.row);
        let Some(Overlay::Detail(detail)) = &self.overlay else { return vec![] };
        let Some(column) = self.value_pressed(detail, mouse.kind, at) else { return vec![] };
        let click = self.clicks.press(at, now);
        let Some(Overlay::Detail(detail)) = self.overlay.take() else { return vec![] };
        let detail = Detail { under: Under::Value(column), note: None, ..detail };
        let copy = if click == Click::Double { self.copy_detail_value(&detail) } else { vec![] };
        self.keep_open(detail, copy)
    }

    /// The column of the value drawn at `at`, when `kind` presses the left button there.
    fn value_pressed(&self, detail: &Detail, kind: MouseEventKind, at: Position) -> Option<usize> {
        if kind != MouseEventKind::Down(MouseButton::Left) {
            return None;
        }
        self.drawn_for(detail)?.column_on(self.fit.detail.line_at(at)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::{ColumnKind, JsonText, ResultSet, Statement, Value};
    use crate::fixtures::{column, keyed_customer, keyed_orders, orders};
    use crate::tui::app::fixtures::{click_at, copied, detail, mouse_at, on_a_key, on_keyed, on_the_keyed_orders, on_the_result, to_shop};
    use crate::tui::app::{Fit, Incoming, LookId};
    use crate::tui::detail_view;
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::Duration;

    const TALL_LINES: usize = 30;
    /// A screen on which the detail holds 13 lines.
    const SCREEN_HEIGHT: u16 = 20;
    /// A screen on which the detail holds 3 lines, fewer than the keyed orders' values and the tables under them.
    const SHORT_SCREEN_HEIGHT: u16 = 8;

    fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    /// Draws the detail on a screen `height` rows high as the loop does, and tells the app what the frame drew.
    fn frame_at(app: &mut App, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(60, height)).unwrap();
        let mut fit = DetailFit::default();
        terminal.draw(|f| fit = detail_view::draw(f, app)).unwrap();
        app.fit(Fit { detail: fit, ..Fit::default() });
    }

    fn frame(app: &mut App) {
        frame_at(app, SCREEN_HEIGHT);
    }

    /// Presses `code`, then lets a frame draw what it did.
    fn key(app: &mut App, code: KeyCode) -> Vec<Action> {
        let actions = press(app, code);
        frame(app);
        actions
    }

    /// The detail of the first row of `result`, drawn.
    fn open_on(result: ResultSet) -> App {
        let mut app = on_the_result(result);
        press(&mut app, KeyCode::Enter);
        frame(&mut app);
        app
    }

    /// The keyed orders' detail, drawn: `id`, `customer_id` then `total`, and one table pointing to the row.
    fn open_on_keyed_orders() -> App {
        let mut app = on_the_keyed_orders();
        press(&mut app, KeyCode::Enter);
        frame(&mut app);
        app
    }

    /// The customer of order 7, followed from the keyed orders' detail, drawn.
    fn on_a_followed_row() -> App {
        let mut app = on_a_key();
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        frame(&mut app);
        app
    }

    /// A text of [`TALL_LINES`] lines, taller than the detail.
    fn tall_text() -> Value {
        Value::Text((0..TALL_LINES).map(|line| format!("line {line}")).collect::<Vec<_>>().join("\n").into())
    }

    /// `id`, a text of [`TALL_LINES`] lines, then `total`: the middle value is taller than the detail.
    fn with_a_tall_value() -> App {
        let columns = vec![column("id", ColumnKind::Int), column("note", ColumnKind::Text), column("total", ColumnKind::Numeric)];
        open_on(ResultSet::whole(columns, vec![vec![Value::Int(7), tall_text(), Value::Numeric("12.50".into())]]))
    }

    fn first_line(app: &App) -> usize {
        app.fit.detail.first_line
    }

    /// Whether the last frame drew the line `line` of the detail.
    fn line_in_view(app: &App, line: usize) -> bool {
        (first_line(app)..first_line(app) + usize::from(app.fit.detail.body.height)).contains(&line)
    }

    /// Whether the last frame drew the first line of the value at `column`.
    fn value_in_view(app: &App, column: usize) -> bool {
        line_in_view(app, app.drawn_detail.as_ref().unwrap().lines_of(column).start)
    }

    const LEFT_PRESS: MouseEventKind = MouseEventKind::Down(MouseButton::Left);

    /// A cell of the line `down` lines under the top of the detail's lines.
    fn on_line(app: &App, down: u16) -> Position {
        let body = app.fit.detail.body;
        Position::new(body.x + 2, body.y + down)
    }

    #[test]
    fn a_followed_row_opens_with_its_first_key_in_view_below_a_tall_value() {
        let mut app = on_a_key();
        let customer = keyed_customer().with_cell(0, 1, tall_text());
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(customer) }));
        frame(&mut app);
        assert!(value_in_view(&app, 2), "first line {}", first_line(&app));
    }

    #[test]
    fn backspace_after_scrolling_during_a_lookup_brings_the_key_followed_back_into_view() {
        let mut app = on_keyed("SELECT * FROM orders", keyed_orders().with_cell(0, 2, tall_text()));
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Enter);
        frame(&mut app);
        (0..5).for_each(|_| _ = key(&mut app, KeyCode::Char('j')));
        assert!(!value_in_view(&app, 1));
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        frame(&mut app);
        key(&mut app, KeyCode::Backspace);
        assert!(value_in_view(&app, 1), "first line {}", first_line(&app));
    }

    #[test]
    fn shift_tab_onto_a_later_table_on_a_short_pane_brings_that_table_into_view() {
        let mut app = open_on_keyed_orders();
        let Some(Overlay::Detail(open)) = &mut app.overlay else { panic!("no detail open") };
        let invoices = open.referrers[0].clone();
        open.referrers = vec![invoices; 4];
        press(&mut app, KeyCode::BackTab);
        frame_at(&mut app, SHORT_SCREEN_HEIGHT);
        assert_eq!(detail(&app).under, Under::Table(3));
        let lines = app.drawn_detail.as_ref().unwrap().lines.len();
        assert!(line_in_view(&app, lines + 1 + 3), "first line {}", first_line(&app));
    }

    #[test]
    fn a_click_a_key_and_a_click_on_the_same_cell_copy_nothing() {
        let mut app = with_a_tall_value();
        let (at, start) = (on_line(&app, 0), Instant::now());
        mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start);
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(first_line(&app), 1);
        assert_eq!(mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start + Duration::from_millis(100)), []);
    }

    #[test]
    fn a_click_a_row_arriving_and_a_click_on_the_same_cell_copy_nothing() {
        let mut app = on_a_key();
        frame(&mut app);
        let (at, start) = (on_line(&app, 0), Instant::now());
        mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start);
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        frame(&mut app);
        assert_eq!(mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start + Duration::from_millis(100)), []);
    }

    #[test]
    fn j_and_k_move_the_cursor_across_the_values() {
        let mut app = open_on(orders());
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(detail(&app).under, Under::Value(1));
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(detail(&app).under, Under::Value(0));
    }

    #[test]
    fn the_cursor_starts_on_the_column_the_grid_was_on() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(detail(&app).under, Under::Value(1));
    }

    #[test]
    fn j_goes_line_by_line_through_a_value_too_tall_until_its_last_line_shows_then_on() {
        let mut app = with_a_tall_value();
        key(&mut app, KeyCode::Char('j'));
        let height = usize::from(app.fit.detail.body.height);
        let last_start = 1 + TALL_LINES - height;
        while first_line(&app) < last_start {
            let before = first_line(&app);
            key(&mut app, KeyCode::Char('j'));
            assert_eq!((detail(&app).under, first_line(&app)), (Under::Value(1), before + 1));
        }
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(detail(&app).under, Under::Value(2));
    }

    #[test]
    fn k_comes_into_a_value_too_tall_at_its_end_and_goes_through_it_to_its_first_line_then_on() {
        let mut app = with_a_tall_value();
        (0..30).for_each(|_| _ = key(&mut app, KeyCode::Char('j')));
        assert_eq!(detail(&app).under, Under::Value(2));
        let height = usize::from(app.fit.detail.body.height);
        key(&mut app, KeyCode::Char('k'));
        assert_eq!((detail(&app).under, first_line(&app)), (Under::Value(1), 1 + TALL_LINES - height));
        while first_line(&app) > 1 {
            key(&mut app, KeyCode::Char('k'));
            assert_eq!(detail(&app).under, Under::Value(1));
        }
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(detail(&app).under, Under::Value(0));
    }

    #[test]
    fn j_on_the_last_value_stays_on_it_and_scrolls_to_the_tables_pointing_to_the_row() {
        let mut app = open_on_keyed_orders();
        frame_at(&mut app, SHORT_SCREEN_HEIGHT);
        for _ in 0..4 {
            press(&mut app, KeyCode::Char('j'));
            frame_at(&mut app, SHORT_SCREEN_HEIGHT);
        }
        assert_eq!((detail(&app).under, first_line(&app)), (Under::Value(2), app.fit.detail.last_scroll));
        assert!(first_line(&app) > 0);
    }

    /// The statement of the lookup `⏎` sends now.
    fn followed_by_enter(app: &mut App) -> Statement {
        let Action::LookUp { lookup, .. } = press(app, KeyCode::Enter).remove(0) else { panic!("no lookup") };
        lookup.statement
    }

    const READ_THE_CUSTOMER: &str = "SELECT * FROM public.customers WHERE id = $1";

    #[test]
    fn j_onto_a_column_holding_a_key_lets_enter_follow_it() {
        let mut app = open_on_keyed_orders();
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(followed_by_enter(&mut app), Statement::new(READ_THE_CUSTOMER));
    }

    #[test]
    fn j_off_a_key_onto_a_plain_value_leaves_enter_nothing_to_follow() {
        let mut app = open_on_keyed_orders();
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!(detail(&app).note.as_deref(), Some(super::super::NO_KEY));
    }

    #[test]
    fn a_click_on_a_column_holding_a_key_lets_enter_follow_it() {
        let mut app = open_on_keyed_orders();
        let at = on_line(&app, 1);
        click_at(&mut app, at.x, at.y);
        assert_eq!(followed_by_enter(&mut app), Statement::new(READ_THE_CUSTOMER));
    }

    #[test]
    fn e_on_a_plain_value_of_a_followed_row_joins_the_trail_and_no_key_it_is_not_on() {
        let mut app = on_a_followed_row();
        key(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('e'));
        let text = app.sheet().editor.text();
        assert!(text.contains("JOIN public.customers") && !text.contains("users"), "{text}");
    }

    #[test]
    fn tab_from_a_plain_value_goes_to_the_next_key_after_it() {
        let mut app = open_on_keyed_orders();
        key(&mut app, KeyCode::Tab);
        assert_eq!(detail(&app).under, Under::Value(1));
    }

    #[test]
    fn tab_from_a_plain_value_past_the_last_key_goes_to_the_first_table() {
        let mut app = open_on_keyed_orders();
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Tab);
        assert_eq!(detail(&app).under, Under::Table(0));
    }

    #[test]
    fn shift_tab_from_a_plain_value_goes_to_the_key_before_it() {
        let mut app = open_on_keyed_orders();
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::BackTab);
        assert_eq!(detail(&app).under, Under::Value(1));
    }

    #[test]
    fn shift_tab_from_a_plain_value_before_the_first_key_goes_round_to_the_last_table() {
        let mut app = open_on_keyed_orders();
        key(&mut app, KeyCode::BackTab);
        assert_eq!(detail(&app).under, Under::Table(0));
    }

    #[test]
    fn tab_on_a_key_goes_on_from_that_key() {
        let mut app = open_on_keyed_orders();
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Tab);
        assert_eq!(detail(&app).under, Under::Table(0));
    }

    #[test]
    fn tab_then_shift_tab_comes_back_to_the_key() {
        let mut app = on_a_followed_row();
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::BackTab);
        assert_eq!(detail(&app).under, Under::Value(2));
    }

    #[test]
    fn k_on_a_table_goes_back_to_the_last_value() {
        let mut app = open_on_keyed_orders();
        key(&mut app, KeyCode::BackTab);
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(detail(&app).under, Under::Value(2));
    }

    #[test]
    fn y_on_a_table_copies_nothing_and_leaves_the_detail_open() {
        let mut app = open_on_keyed_orders();
        key(&mut app, KeyCode::BackTab);
        assert_eq!(press(&mut app, KeyCode::Char('y')), []);
        assert!(matches!(app.overlay, Some(Overlay::Detail(_))));
    }

    #[test]
    fn y_copies_the_value_under_the_cursor_and_leaves_the_detail_open() {
        let mut app = open_on(orders());
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(press(&mut app, KeyCode::Char('y')), copied("12.50", "copied total of row 1"));
        assert!(matches!(app.overlay, Some(Overlay::Detail(_))));
    }

    #[test]
    fn y_copies_a_json_value_as_stored_not_as_the_detail_indents_it() {
        let json = JsonText::of(&serde_json::json!({"sku": "TEA-01"}));
        let mut app = open_on(ResultSet::whole(vec![column("items", ColumnKind::Json)], vec![vec![Value::Json(json)]]));
        assert_eq!(press(&mut app, KeyCode::Char('y')), copied(r#"{"sku":"TEA-01"}"#, "copied items of row 1"));
    }

    #[test]
    fn y_copies_the_whole_of_a_value_the_detail_cuts() {
        let long = "x".repeat(70_000);
        let mut app = open_on(ResultSet::whole(vec![column("note", ColumnKind::Text)], vec![vec![Value::Text(long.clone().into())]]));
        assert_eq!(press(&mut app, KeyCode::Char('y')), copied(&long, "copied note of row 1"));
    }

    #[test]
    fn y_on_a_value_pgbg_cannot_decode_says_so() {
        let mut app = open_on(orders().with_cell(0, 0, Value::Unsupported));
        assert_eq!(
            press(&mut app, KeyCode::Char('y')),
            copied("<unsupported>", "copied id of row 1 · 1 cell not decodable, copied as <unsupported>")
        );
    }

    #[test]
    fn shift_y_copies_the_row_as_csv_and_leaves_the_detail_open() {
        let mut app = open_on(orders());
        assert_eq!(press(&mut app, KeyCode::Char('Y')), copied("7,12.50,nina\n", "copied row 1 as CSV"));
        assert!(matches!(app.overlay, Some(Overlay::Detail(_))));
    }

    #[test]
    fn y_on_a_followed_row_copies_from_that_row_and_names_its_table() {
        let mut app = on_a_followed_row();
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(press(&mut app, KeyCode::Char('y')), copied("nina", "copied name of the customers row"));
    }

    #[test]
    fn shift_y_on_a_followed_row_copies_that_row_and_names_its_table() {
        let mut app = on_a_followed_row();
        assert_eq!(press(&mut app, KeyCode::Char('Y')), copied("3,nina,5\n", "copied the customers row as CSV"));
    }

    #[test]
    fn the_cursor_on_a_followed_row_starts_on_its_first_key() {
        assert_eq!(detail(&on_a_followed_row()).under, Under::Value(2));
    }

    #[test]
    fn backspace_brings_the_cursor_back_to_where_it_was_on_the_row_before() {
        let mut app = on_a_followed_row();
        key(&mut app, KeyCode::Backspace);
        assert_eq!(detail(&app).under, Under::Value(1));
    }

    #[test]
    fn a_click_on_a_value_moves_the_cursor_there_and_copies_nothing() {
        let mut app = open_on(orders());
        let at = on_line(&app, 2);
        assert_eq!(click_at(&mut app, at.x, at.y), []);
        assert_eq!(detail(&app).under, Under::Value(2));
    }

    #[test]
    fn a_click_on_a_later_line_of_a_value_moves_the_cursor_to_that_value() {
        let mut app = with_a_tall_value();
        let at = on_line(&app, 5);
        click_at(&mut app, at.x, at.y);
        assert_eq!(detail(&app).under, Under::Value(1));
    }

    #[test]
    fn a_click_finds_the_value_on_the_line_a_scrolled_frame_drew_there() {
        let mut app = with_a_tall_value();
        (0..19).for_each(|_| _ = key(&mut app, KeyCode::Char('j')));
        assert!(first_line(&app) > 1);
        let height = app.fit.detail.body.height;
        let at = on_line(&app, height - 1);
        click_at(&mut app, at.x, at.y);
        assert_eq!(detail(&app).under, Under::Value(2));
    }

    #[test]
    fn a_double_click_copies_the_value_straight_away() {
        let mut app = open_on(orders());
        let (at, start) = (on_line(&app, 2), Instant::now());
        mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start);
        assert_eq!(
            mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start + Duration::from_millis(100)),
            copied("nina", "copied customer of row 1")
        );
    }

    #[test]
    fn a_double_click_on_a_followed_row_copies_from_that_row() {
        let mut app = on_a_followed_row();
        let (at, start) = (on_line(&app, 1), Instant::now());
        mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start);
        assert_eq!(
            mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start + Duration::from_millis(100)),
            copied("nina", "copied name of the customers row")
        );
    }

    #[test]
    fn two_clicks_far_apart_copy_nothing() {
        let mut app = open_on(orders());
        let (at, start) = (on_line(&app, 2), Instant::now());
        mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start);
        assert_eq!(mouse_at(&mut app, LEFT_PRESS, at.x, at.y, start + Duration::from_secs(2)), []);
    }

    #[test]
    fn a_click_outside_the_modal_does_nothing() {
        let mut app = open_on(orders());
        let body = app.fit.detail.body;
        let actions = click_at(&mut app, body.x, body.y - 2);
        assert_eq!((actions, detail(&app).under), (vec![], Under::Value(0)));
        assert!(matches!(app.overlay, Some(Overlay::Detail(_))));
    }

    #[test]
    fn a_click_on_the_rule_before_the_tables_does_nothing() {
        let mut app = open_on_keyed_orders();
        let at = on_line(&app, 3);
        click_at(&mut app, at.x, at.y);
        assert_eq!(detail(&app).under, Under::Value(0));
    }

    #[test]
    fn a_click_on_a_table_pointing_to_the_row_does_nothing() {
        let mut app = open_on_keyed_orders();
        let at = on_line(&app, 4);
        click_at(&mut app, at.x, at.y);
        assert_eq!(detail(&app).under, Under::Value(0));
    }

    #[test]
    fn a_click_past_the_last_line_does_nothing() {
        let mut app = open_on(orders());
        let at = on_line(&app, 8);
        click_at(&mut app, at.x, at.y);
        assert_eq!(detail(&app).under, Under::Value(0));
    }

    #[test]
    fn the_right_button_and_a_release_leave_the_cursor_where_it_is() {
        let mut app = open_on(orders());
        let at = on_line(&app, 2);
        mouse_at(&mut app, MouseEventKind::Down(MouseButton::Right), at.x, at.y, Instant::now());
        mouse_at(&mut app, MouseEventKind::Up(MouseButton::Left), at.x, at.y, Instant::now());
        assert_eq!(detail(&app).under, Under::Value(0));
    }
}
