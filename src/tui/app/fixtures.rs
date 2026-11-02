//! Invented fixtures the app's tests and the views' tests share: the app on `shop`, typed text, and its answers.
use super::{Action, Answer, App, Fit, Focus, Incoming, Panes, Run, RunId, SessionId, View};
use crate::connection::{Connection, ConnectionName};
use crate::db::model::{ResultSet, Statement};
use crate::failure::Failure;
use crate::fixtures::{self, keyed_catalog, keyed_orders, orders};
use crate::query::params::ParamInput;
use crate::tui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use std::sync::Arc;
use std::time::Duration;

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
