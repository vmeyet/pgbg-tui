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
mod sidebar_view;
mod theme;
mod ui;

use crate::connection::Connection;
use crate::query::{Places, Roots};
use crate::signal::Signals;
use anyhow::Result;
use app::{Action, Answer, App, Fit};
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
    let mut signals = Signals::listen()?;
    let (answers, mut incoming) = mpsc::unbounded_channel();
    let mut backend = Backend::new(find_roots().await?, answers);
    for action in std::iter::once(Action::Open(first)).chain(App::start()) {
        actions::spawn(action, &mut backend);
    }
    let (mut terminal, screen) = Screen::enter();
    let outcome = event_loop(&mut terminal, &mut app, &mut backend, &mut incoming, &mut signals).await;
    screen.leave();
    backend.stop().await;
    outcome
}

/// The folders of saved queries: the one nearest the working directory, and the one in the config folder.
async fn find_roots() -> Result<Roots> {
    let working_dir = std::env::current_dir()?;
    let (home, config_dir) = (dirs::home_dir(), dirs::config_dir());
    let found = move || Roots::find(&Places { working_dir: &working_dir, home: home.as_deref(), config_dir: config_dir.as_deref() });
    Ok(tokio::task::spawn_blocking(found).await?)
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    backend: &mut Backend,
    incoming: &mut UnboundedReceiver<Answer>,
    signals: &mut Signals,
) -> Result<()> {
    let mut events = EventStream::new();
    while !app.should_quit {
        let mut fit = Fit::default();
        terminal.draw(|f| fit = ui::draw(f, app))?;
        app.fit(fit);
        let actions = tokio::select! {
            Some(event) = events.next() => match event? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                _ => vec![],
            },
            Some(answer) = incoming.recv() => {
                app.apply(answer);
                vec![]
            }
            stopped = signals.next() => return Err(stopped.into()),
            else => break,
        };
        for action in actions {
            actions::spawn(action, backend);
        }
    }
    Ok(())
}
