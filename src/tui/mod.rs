//! The TUI: an event loop over a pure `App`, with the database at its edge.
mod app;
mod backend;
mod chart_view;
mod completion_view;
mod detail_view;
mod editor_view;
mod finder_view;
mod form_view;
mod grid_view;
mod help_view;
mod history_view;
mod outside;
mod param_bar_view;
mod picker_view;
mod prompt_view;
mod result_view;
mod sidebar_view;
#[cfg(test)]
mod styled;
mod table_finder_view;
mod theme;
mod ui;

use crate::connection::{Connection, ConnectionName};
use crate::format::Mode;
use crate::history::Store;
use crate::query::{Query, Roots};
use crate::signal::Signals;
use anyhow::Result;
use app::{Action, Answer, App, Clock, Fit, QueryAnswer};
use backend::Backend;
use chrono::{Local, Utc};
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use std::path::PathBuf;
use std::time::Duration;
use theme::Theme;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::sleep;
use vtui::screen::Screen;

/// How long the editor's text stays still before its colours are rebuilt, so typing never waits on them.
const RECOLOUR_DELAY: Duration = Duration::from_millis(40);
/// How often the screen is drawn again while a column reads as the time from now, so it stays true to the minute.
const RELATIVE_TIME_REDRAW: Duration = Duration::from_secs(60);

/// Runs the TUI on `first` until the user quits, with `connections` to switch to and the `config` file they come from,
/// each result opening in `format`, giving the terminal back on the way out; a signal while its sessions stop drops them
/// at once, as a command does.
pub(crate) async fn run(connections: Vec<Connection>, first: Connection, config: PathBuf, format: Mode) -> Result<()> {
    let mut signals = Signals::listen()?;
    let theme = vtui::ground::ask().map_or_else(Theme::default, |ground| Theme::default().with_ground(ground));
    let mut app = App::new(connections, &first, theme).with_format_mode(format);
    let (answers, mut incoming) = mpsc::unbounded_channel();
    let mut backend = Backend::new(Roots::find_here().await?, config, answers).with_history(Store::from_env());
    for action in app.start() {
        backend.dispatch(action);
    }
    let (mut terminal, screen) = Screen::enter();
    let screens = Screens { terminal: &mut terminal, screen: &screen };
    let outcome = event_loop(screens, &mut app, &mut backend, &mut incoming, &mut signals).await;
    screen.leave();
    let stopped = signals.or_stopped(backend.stop()).await;
    outcome.and(stopped.map_err(Into::into))
}

/// The terminal the loop draws on, and the screen that lends it to another program.
struct Screens<'a> {
    terminal: &'a mut DefaultTerminal,
    screen: &'a Screen,
}

impl Screens<'_> {
    /// Lends the terminal to the user's editor on `query`'s file until it exits.
    fn edit_outside(&mut self, query: &Query, backend: &Backend) -> Result<()> {
        tokio::task::block_in_place(|| {
            let path = backend.query_file(query)?;
            self.screen.lend(self.terminal, || outside::edit(&path))
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
        app.set_clock(Clock { now: Utc::now(), zone: Local });
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
            () = sleep(RELATIVE_TIME_REDRAW), if app.sheet().shows_relative_time() => vec![],
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
