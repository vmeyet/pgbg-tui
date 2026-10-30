//! The TUI: an event loop over a pure `App`, with the database at its edge.
mod app;
mod ask_view;
mod backend;
mod chart_view;
mod column_stats_view;
mod completion_view;
mod detail_view;
mod editor_view;
mod finder_view;
mod form_view;
mod grid_view;
mod help_view;
mod history_view;
mod ink;
mod outside;
mod palette_footer;
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
mod usage;
mod versions_view;

use crate::config::AiSettings;
use crate::connection::{Connection, ConnectionName};
use crate::format::Mode;
use crate::history::Store;
use crate::query::{Query, Roots, Versions};
use crate::signal::Signals;
use crate::usage::UsageFile;
use anyhow::Result;
use app::{Action, Answer, App, Clock, Fit, QueryAnswer};
use backend::Backend;
use chrono::{Local, Utc};
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use theme::Theme;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::sleep;
use usage::Counting;
use vtui::screen::Screen;

/// How long the editor's text stays still before its colours are rebuilt, so typing never waits on them.
const RECOLOUR_DELAY: Duration = Duration::from_millis(40);
/// How long the editor sits without a key to be idle: the characters typed end their undo step, and the idle ghost asks
/// the `[ai] command`.
const IDLE_DELAY: Duration = Duration::from_secs(1);
/// How often the screen is drawn again while a column shows time from now, so it stays right to the minute.
const RELATIVE_TIME_REDRAW: Duration = Duration::from_secs(60);

/// Runs the TUI on `first` until the user quits, with `connections` to switch to, read from the `config` file, each
/// result opening in `format`, `ctrl-g` asking the command of `ai`, and the usage counted into `usage` when given; it
/// gives the terminal back on the way out.
/// A signal while the sessions stop drops them at once, as a command does.
pub(crate) async fn run(
    connections: Vec<Connection>,
    first: Connection,
    config: PathBuf,
    format: Mode,
    ai: Option<AiSettings>,
    usage: Option<UsageFile>,
    warning: Option<String>,
) -> Result<()> {
    let mut signals = Signals::listen()?;
    let theme = vtui::ground::ask().map_or_else(Theme::default, |ground| Theme::default().with_ground(ground));
    let mut app = App::new(connections, &first, theme).with_format_mode(format).with_ai(ai).with_warning(warning);
    // Unbounded: each answer follows an action, a page only once the grid asks for it, and the loop takes one per turn.
    let (answers, mut incoming) = mpsc::unbounded_channel();
    let roots = Roots::from_env().with_versions(Versions::from_env());
    let mut backend = Backend::new(roots, config, answers).with_history(Store::from_env());
    for action in app.start() {
        backend.dispatch(action);
    }
    let (mut terminal, screen) = Screen::enter();
    let screens = Screens { terminal: &mut terminal, screen: &screen };
    let counting = usage.map(Counting::start);
    let outcome = event_loop(screens, &mut app, &mut backend, &mut incoming, &mut signals, counting).await;
    screen.leave();
    let stopped = signals.or_stopped(backend.stop()).await;
    outcome.and(stopped.map_err(Into::into))
}

/// The terminal the loop draws on, and the screen that can lend it to another program.
struct Screens<'a> {
    terminal: &'a mut DefaultTerminal,
    screen: &'a Screen,
}

impl Screens<'_> {
    /// Lends the terminal to the user's editor on `query`'s file until it exits, then keeps the text it replaced.
    fn edit_outside(&mut self, query: &Query, backend: &Backend) -> Result<()> {
        tokio::task::block_in_place(|| backend.edit_query(query, |path| self.screen.lend(self.terminal, || outside::edit(path))))
    }
}

/// What follows the user's editor exiting: a read of the query it wrote, for the sheet that asked, or an answer saying
/// why the editor could not run.
fn after_editing(edit: Edit, edited: Result<()>) -> Result<Action, Answer> {
    let Edit { connection, query } = edit;
    match edited {
        Ok(()) => Ok(Action::OpenQuery { connection, query }),
        Err(err) => Err(Answer::Queries(QueryAnswer::NotOpened { connection, query, error: format!("{err:#}") })),
    }
}

/// A query opened in the user's editor, for the sheet that asked for it.
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
    mut counting: Option<Counting>,
) -> Result<()> {
    let mut events = EventStream::new();
    while !app.should_quit() {
        if let Some(counting) = &mut counting {
            counting.flush_when_due(app);
        }
        app.set_clock(Clock { now: Utc::now(), zone: Local });
        let mut fit = Fit::default();
        screens.terminal.draw(|f| fit = ui::draw(f, app))?;
        app.fit(fit);
        let actions = tokio::select! {
            Some(event) = events.next() => {
                let event = event?;
                if let Some(counting) = &mut counting {
                    counting.see(&event, app);
                }
                match event {
                    Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                    Event::Paste(text) => app.paste(&text),
                    Event::Mouse(mouse) => app.handle_mouse(mouse, Instant::now()),
                    _ => vec![],
                }
            }
            Some(answer) = incoming.recv() => app.apply(answer),
            () = sleep(RECOLOUR_DELAY), if app.has_stale_colours() => {
                app.recolour();
                vec![]
            }
            () = sleep(IDLE_DELAY), if app.waits_for_idle() => app.sat_idle(),
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
            // The key stream's reader thread would steal the keys of the user's editor: it stops with the stream, and a
            // new one starts afterwards.
            drop(events);
            let edited = screens.edit_outside(&query, backend);
            events = EventStream::new();
            match after_editing(Edit { connection, query }, edited) {
                Ok(read) => backend.dispatch(read),
                Err(failed) => app.apply(failed).into_iter().for_each(|action| backend.dispatch(action)),
            }
        }
    }
    if let Some(counting) = counting {
        counting.flush(app).await;
    }
    Ok(())
}
