//! The TUI: an event loop over a pure `App`, with the database at its edge.
mod app;
mod backend;
mod chart_view;
mod editor_view;
mod finder_view;
mod form_view;
mod grid_view;
mod help_view;
mod picker_view;
mod result_view;
mod sidebar_view;
#[cfg(test)]
mod styled;
mod theme;
mod ui;

use crate::connection::Connection;
use crate::query::{Places, Roots};
use crate::signal::Signals;
use anyhow::Result;
use app::{Action, Answer, App, Fit};
use backend::Backend;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::sleep;
use vtui::screen::Screen;
use vtui::theme::Theme;

/// How long the editor's text stays still before its colours are rebuilt, so typing never waits on them.
const RECOLOUR_DELAY: Duration = Duration::from_millis(40);

static PASTE_OFF_ON_PANIC: Once = Once::new();

/// Runs the TUI on `first` until the user quits, with `connections` to switch to and the `config` file they come from,
/// giving the terminal back on the way out.
pub(crate) async fn run(connections: Vec<Connection>, first: Connection, config: PathBuf) -> Result<()> {
    let mut signals = Signals::listen()?;
    let theme = vtui::ground::ask().map_or_else(Theme::default, |ground| Theme::default().with_ground(ground));
    let mut app = App::new(connections, &first, theme);
    let (answers, mut incoming) = mpsc::unbounded_channel();
    let mut backend = Backend::new(find_roots().await?, config, answers);
    for action in std::iter::once(Action::Open(first)).chain(App::start()) {
        backend.dispatch(action);
    }
    let (mut terminal, screen) = Screen::enter();
    take_pastes();
    let outcome = event_loop(&mut terminal, &mut app, &mut backend, &mut incoming, &mut signals).await;
    give_pastes_back();
    screen.leave();
    backend.stop().await;
    outcome
}

/// Has the terminal send a paste as one event, so its line ends never reach the editor as `⏎`; a panic gives it back.
fn take_pastes() {
    PASTE_OFF_ON_PANIC.call_once(|| {
        let earlier = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            give_pastes_back();
            earlier(info);
        }));
    });
    if let Err(err) = crossterm::execute!(std::io::stdout(), EnableBracketedPaste) {
        tracing::warn!("the terminal will send pastes as keys: {err}");
    }
}

fn give_pastes_back() {
    if let Err(err) = crossterm::execute!(std::io::stdout(), DisableBracketedPaste) {
        tracing::warn!("the terminal may keep bracketing pastes: {err}");
    }
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
                Event::Paste(text) => app.paste(&text),
                _ => vec![],
            },
            Some(answer) = incoming.recv() => app.apply(answer),
            () = sleep(RECOLOUR_DELAY), if app.has_stale_colours() => {
                app.recolour();
                vec![]
            }
            stopped = signals.next() => {
                for action in app.quit() {
                    backend.dispatch(action);
                }
                return Err(stopped.into());
            }
            else => break,
        };
        for action in actions {
            backend.dispatch(action);
        }
    }
    Ok(())
}
