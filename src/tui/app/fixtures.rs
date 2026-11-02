//! Invented fixtures the app's tests and the views' tests share: the app on `shop`, typed text, and its answers.
use super::{Action, Answer, App, Fit, Focus, Grid, Incoming, Panes, Room, Run, RunId, SessionId, View};
use crate::connection::{Connection, ConnectionName};
use crate::db::model::{ColumnKind, ResultSet, Statement, Value};
use crate::failure::Failure;
use crate::fixtures::{self, keyed_catalog, keyed_orders, orders};
use crate::query::params::ParamInput;
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(in crate::tui) fn connection(tier: &str, writes: &str) -> Connection {
    fixtures::connection(&format!("tier = \"{tier}\"\nurl = \"postgres://nina@localhost/shop\"\nwrites = \"{writes}\"\n"))
}

pub(in crate::tui) fn app_on(connection: &Connection) -> App {
    App::new(vec![connection.clone()], connection, Theme::default())
}

/// The app with the keys on `focus`, whatever the frame drew.
pub(in crate::tui) fn with_focus(app: App, focus: Focus) -> App {
    App { focus, ..app }
}

pub(in crate::tui) fn app() -> App {
    app_on(&connection("local", "read-only"))
}

pub(in crate::tui) fn shop() -> ConnectionName {
    "shop".parse().unwrap()
}

pub(in crate::tui) fn to_shop(incoming: Incoming) -> Answer {
    Answer::Session { connection: shop(), session: SessionId::FIRST, incoming }
}

/// The app with `result` from `SELECT 1` on screen, the keys on it.
pub(in crate::tui) fn on_the_result(result: ResultSet) -> App {
    let mut app = typed("SELECT 1");
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result, took: Duration::from_millis(38) }));
    app.handle_key(KeyEvent::from(KeyCode::Tab));
    app
}

/// The app with [`keyed_orders`] from `SELECT * FROM orders` on screen, the schema of [`keyed_catalog`] read, the keys
/// on the grid.
pub(in crate::tui) fn on_the_keyed_orders() -> App {
    on_keyed("SELECT * FROM orders", keyed_orders())
}

/// The app with `result` of `sql` on screen, the schema of [`keyed_catalog`] read, the keys on the grid.
pub(in crate::tui) fn on_keyed(sql: &str, result: ResultSet) -> App {
    let mut app = typed(sql);
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    app.apply(to_shop(Incoming::Schema(Ok(Arc::new(keyed_catalog())))));
    app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result, took: Duration::from_millis(38) }));
    app.handle_key(KeyEvent::from(KeyCode::Tab));
    app
}

pub(in crate::tui) fn typed(text: &str) -> App {
    let mut app = app();
    for c in text.chars() {
        app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }
    app
}

/// The first run of `sql` on `connection`, with no parameter.
pub(in crate::tui) fn first_run(connection: &ConnectionName, sql: &str) -> Action {
    Action::Run {
        connection: connection.clone(),
        run: RunId::FIRST,
        statement: Statement::new(sql),
        input: ParamInput::default(),
        confirmed: None,
    }
}

pub(in crate::tui) fn enter(app: &mut App) -> Vec<Action> {
    app.handle_key(KeyEvent::from(KeyCode::Enter))
}

/// The first run answers that the server failed it at character `position`.
pub(in crate::tui) fn failed_at(app: &mut App, position: usize) {
    app.apply(to_shop(Incoming::Failed {
        run: RunId::FIRST,
        failure: Failure::Server { message: "ERROR: syntax error".into(), position: Some(position) },
    }));
}

/// The app whose first run, of `sql`, the server failed at character `position`.
pub(in crate::tui) fn failed_run(sql: &str, position: usize) -> App {
    let mut app = typed(sql);
    enter(&mut app);
    failed_at(&mut app, position);
    app
}

pub(in crate::tui) fn with_result() -> App {
    let mut app = typed("SELECT 1");
    enter(&mut app);
    app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::from_millis(38) }));
    app
}

pub(in crate::tui) fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::from(code));
}

pub(in crate::tui) fn control(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// `app` with the command palette open and `text` typed in it.
pub(in crate::tui) fn opened_on(mut app: App, text: &str) -> App {
    app.handle_key(control('l'));
    text.chars().for_each(|c| press(&mut app, KeyCode::Char(c)));
    app
}

pub(in crate::tui) fn grid(app: &App) -> &Grid {
    let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
    grid
}

/// A left click at `column` and `row`, now.
pub(in crate::tui) fn click_at(app: &mut App, column: u16, row: u16) -> Vec<Action> {
    let mouse = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: KeyModifiers::NONE };
    app.handle_mouse(mouse, Instant::now())
}

/// `keys` pressed on `app`, a frame having given the grid its room.
pub(in crate::tui) fn with_keys(mut app: App, keys: &str) -> App {
    app.fit(Fit { grid: Room { rows: 10, columns: 80 }, ..Fit::default() });
    keys.chars().for_each(|c| press(&mut app, KeyCode::Char(c)));
    app
}

/// `keys` pressed on `result`.
pub(in crate::tui) fn pressed(result: ResultSet, keys: &str) -> App {
    with_keys(on_the_result(result), keys)
}

/// The name of the column under the grid's cursor.
pub(in crate::tui) fn cursor_name(app: &App) -> String {
    let Run::Done { grid, result, .. } = &app.sheet().run else { panic!("no result") };
    result.columns()[grid.column].name.to_string()
}

/// `total`, `id`, `customer_id` of `orders`: a result that does not start with its primary key.
pub(in crate::tui) fn key_in_the_middle() -> App {
    let column = |name: &str, kind| fixtures::shop_column("orders", name, kind);
    let columns = vec![column("total", ColumnKind::Numeric), column("id", ColumnKind::Int), column("customer_id", ColumnKind::Int)];
    let row = vec![Value::Numeric("12.50".into()), Value::Int(7), Value::Int(3)];
    on_keyed("SELECT total, id, customer_id FROM orders", ResultSet::whole(columns, vec![row]))
}

/// The columns the grid draws with room for all, left to right.
pub(in crate::tui) fn drawn(app: &App) -> Vec<usize> {
    grid(app).columns_in_view(80).collect()
}

/// What the character `c` asks of the backend on `app`.
pub(in crate::tui) fn key(mut app: App, c: char) -> Vec<Action> {
    app.handle_key(KeyEvent::from(KeyCode::Char(c)))
}

/// [`keyed_orders`] with `id` hidden and `total` under the cursor: the third column, drawn second.
pub(in crate::tui) fn past_a_hidden_key() -> App {
    with_keys(on_the_keyed_orders(), "xl")
}

/// Runs the editor's statement again as the run `run`, and answers with `result`.
pub(in crate::tui) fn run_again(mut app: App, run: RunId, result: ResultSet) -> App {
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Enter);
    app.apply(to_shop(Incoming::Result { run, result, took: Duration::from_millis(38) }));
    app
}

pub(in crate::tui) fn view(app: &App) -> &View {
    let Run::Done { view, .. } = &app.sheet().run else { panic!("no result") };
    view
}

/// [`with_result`] on a screen that drew every pane.
pub(in crate::tui) fn with_every_pane() -> App {
    let mut app = with_result();
    app.fit(Fit { panes: every_pane(), editor_text: Rect::new(31, 2, 48, 7), ..Fit::default() });
    app
}

/// An 80 by 24 screen: the sidebar on the left, the bar, the editor and the result stacked on the right.
pub(in crate::tui) fn every_pane() -> Panes {
    Panes {
        sidebar: Some(Rect::new(0, 0, 30, 24)),
        params: Some(Rect::new(30, 0, 50, 1)),
        editor: Rect::new(30, 1, 50, 9),
        result: Rect::new(30, 10, 50, 14),
    }
}
