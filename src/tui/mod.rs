//! The TUI: an event loop over a pure `App`, with the database at its edge.
mod actions;
mod app;
mod backend;
mod editor_view;
mod grid_view;
mod help_view;
mod result_view;
mod theme;
mod ui;

use crate::connection::Connection;
use anyhow::Result;
use app::{App, Incoming};
use backend::Backend;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use vtui::screen::Screen;
use vtui::theme::Theme;

/// Runs the TUI on `connection` until the user quits, giving the terminal back on the way out.
pub(crate) async fn run(connection: Connection) -> Result<()> {
    let theme = vtui::ground::ask().map_or_else(Theme::default, |ground| Theme::default().with_ground(ground));
    let mut app = App::new(&connection, theme);
    let (answers, mut incoming) = mpsc::unbounded_channel();
    let backend = Backend::start(connection, answers);
    let (mut terminal, screen) = Screen::enter();
    let outcome = event_loop(&mut terminal, &mut app, &backend, &mut incoming).await;
    screen.leave();
    outcome
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    backend: &Backend,
    incoming: &mut UnboundedReceiver<Incoming>,
) -> Result<()> {
    let mut events = EventStream::new();
    while !app.should_quit {
        let mut grid_rows = 0;
        terminal.draw(|f| grid_rows = ui::draw(f, app))?;
        app.fit_grid(grid_rows);
        let actions = tokio::select! {
            Some(event) = events.next() => match event? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                _ => vec![],
            },
            Some(answer) = incoming.recv() => {
                app.apply(answer);
                vec![]
            }
            else => break,
        };
        for action in actions {
            actions::spawn(action, backend);
        }
    }
    Ok(())
}
