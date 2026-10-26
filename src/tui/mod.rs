//! The TUI: an event loop over a pure `App`, with the terminal at its edge.
mod app;
mod editor_view;
mod theme;
mod ui;

use crate::connection::Connection;
use anyhow::Result;
use app::App;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use vtui::screen::Screen;
use vtui::theme::Theme;

/// Runs the TUI on `connection` until the user quits, giving the terminal back on the way out.
pub(crate) async fn run(connection: Connection) -> Result<()> {
    let theme = vtui::ground::ask().map_or_else(Theme::default, |ground| Theme::default().with_ground(ground));
    let mut app = App::new(&connection, theme);
    let (mut terminal, screen) = Screen::enter();
    let outcome = event_loop(&mut terminal, &mut app).await;
    screen.leave();
    outcome
}

async fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut events = EventStream::new();
    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, app))?;
        let Some(event) = events.next().await else { break };
        if let Event::Key(key) = event?
            && key.kind != KeyEventKind::Release
        {
            app.handle_key(key);
        }
    }
    Ok(())
}
