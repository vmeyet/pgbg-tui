//! The TUI: an event loop over a pure `App`, with the database at its edge.
mod app;
mod backend;
mod chart_view;
mod detail_view;
mod editor_view;
mod finder_view;
mod form_view;
mod grid_view;
mod help_view;
mod history_view;
mod outside;
mod picker_view;
mod prompt_view;
mod result_view;
mod sidebar_view;
#[cfg(test)]
mod styled;
mod theme;
mod ui;

use crate::connection::{Connection, ConnectionName};
use crate::history::Store;
use crate::query::{Places, Query, Roots};
use crate::signal::Signals;
use anyhow::Result;
use app::{Action, Answer, App, Fit, QueryAnswer};
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
/// giving the terminal back on the way out; a signal while its sessions stop drops them at once, as a command does.
pub(crate) async fn run(connections: Vec<Connection>, first: Connection, config: PathBuf) -> Result<()> {
    let mut signals = Signals::listen()?;
    let theme = vtui::ground::ask().map_or_else(Theme::default, |ground| Theme::default().with_ground(ground));
    let mut app = App::new(connections, &first, theme);
    let (answers, mut incoming) = mpsc::unbounded_channel();
    let mut backend = Backend::new(find_roots().await?, config, answers).with_history(Store::from_env());
    for action in app.start() {
        backend.dispatch(action);
    }
    let (mut terminal, screen) = Screen::enter();
    take_pastes();
    let screens = Screens { terminal: &mut terminal, screen: &screen };
    let outcome = event_loop(screens, &mut app, &mut backend, &mut incoming, &mut signals).await;
    give_pastes_back();
    screen.leave();
    let stopped = signals.or_stopped(backend.stop()).await;
    outcome.and(stopped.map_err(Into::into))
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

/// The terminal the loop draws on, and the screen that lends it to another program.
struct Screens<'a> {
    terminal: &'a mut DefaultTerminal,
    screen: &'a Screen,
}

impl Screens<'_> {
    /// Lends the terminal to the user's editor on `query`'s file until it exits, pastes as keys as a shell has them.
    fn edit_outside(&mut self, query: &Query, backend: &Backend) -> Result<()> {
        tokio::task::block_in_place(|| {
            let path = backend.query_file(query)?;
            give_pastes_back();
            let edited = self.screen.lend(self.terminal, || outside::edit(&path));
            take_pastes();
            edited
        })
    }
}

/// Once the editor exits: the read of the query it wrote, for the sheet that asked, or the answer saying why it could not run.
fn after_editing(edit: Edit, edited: Result<()>) -> Result<Action, Answer> {
    let Edit { connection, query } = edit;
    match edited {
        Ok(()) => Ok(Action::OpenQuery { connection, query }),
        Err(err) => Err(Answer::Queries(QueryAnswer::NotOpened { connection, query, error: format!("{err:#}") })),
    }
}

/// A query the user's editor had, for the sheet that asked for it.
struct Edit {
    connection: ConnectionName,
    query: Query,
}

async fn event_loop(
    mut screens: Screens<'_>,
    app: &mut App,
    backend: &mut Backend,
    incoming: &mut UnboundedReceiver<Answer>,
    signals: &mut Signals,
) -> Result<()> {
    let mut events = EventStream::new();
    while !app.should_quit {
        let mut fit = Fit::default();
        screens.terminal.draw(|f| fit = ui::draw(f, app))?;
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
            if let Action::Copy { text, .. } = &action
                && backend::reaches_the_terminal(text)
            {
                vtui::clipboard::print(text);
            }
            let Action::EditOutside { connection, query } = action else {
                backend.dispatch(action);
                continue;
            };
            // The key stream's reader thread would steal the editor's keys: it stops with the stream, and a new one starts after.
            drop(events);
            let edited = screens.edit_outside(&query, backend);
            events = EventStream::new();
            match after_editing(Edit { connection, query }, edited) {
                Ok(read) => backend.dispatch(read),
                Err(failed) => app.apply(failed).into_iter().for_each(|action| backend.dispatch(action)),
            }
        }
    }
    Ok(())
}
