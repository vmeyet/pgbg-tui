//! The TUI: an event loop over a pure `App`, with the database at its edge.
mod actions;
mod app;
mod backend;
mod editor_view;
mod finder_view;
mod grid_view;
mod help_view;
mod picker_view;
mod result_view;
mod theme;
mod ui;

use crate::connection::Connection;
use crate::query::Roots;
use anyhow::Result;
use app::{Action, Answer, App};
use backend::Backend;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use vtui::screen::Screen;
use vtui::theme::Theme;

/// Runs the TUI on `first` until the user quits, with `connections` to switch to, giving the terminal back on the way out.
pub(crate) async fn run(connections: Vec<Connection>, first: Connection) -> Result<()> {
    let theme = vtui::ground::ask().map_or_else(Theme::default, |ground| Theme::default().with_ground(ground));
    let mut app = App::new(connections, &first, theme);
    let (answers, mut incoming) = mpsc::unbounded_channel();
    let mut backend = Backend::new(find_roots().await?, answers);
    for action in std::iter::once(Action::Open(first)).chain(App::start()) {
        actions::spawn(action, &mut backend);
    }
    let (mut terminal, screen) = Screen::enter();
    let outcome = event_loop(&mut terminal, &mut app, &mut backend, &mut incoming).await;
    screen.leave();
    backend.stop().await;
    outcome
}

/// The folders of saved queries: the one nearest the working directory, and the one in the config folder.
async fn find_roots() -> Result<Roots> {
    let working_dir = std::env::current_dir()?;
    Ok(tokio::task::spawn_blocking(move || Roots::find(&working_dir, dirs::config_dir().as_deref())).await?)
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    backend: &mut Backend,
    incoming: &mut UnboundedReceiver<Answer>,
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
