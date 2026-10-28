//! The loop's side of `[usage]`: it reads the clock, counts each key as the act it does where it lands and the time
//! before each input as the screen it was on, and adds the counts to the usage file now and then.
use super::app::App;
use crate::usage::{Counts, UsageFile};
use crossterm::event::{Event, KeyEventKind, MouseEventKind};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use vtui::usage::{Clock, Tally};

/// The counts go to the file now and then, not on every key; a crash loses this much at most.
const FLUSH: Duration = Duration::from_secs(60);

/// What this session counted since the last flush, and where it goes.
#[derive(Debug)]
pub(super) struct Counting {
    file: UsageFile,
    tally: Tally,
    clock: Clock,
    flushed: Instant,
    /// The flush still writing, which the next one waits for, so two never rewrite the file at once.
    saving: Option<JoinHandle<()>>,
}

impl Counting {
    pub(super) fn start(file: UsageFile) -> Self {
        let now = Instant::now();
        Self { file, tally: Tally::default(), clock: Clock::starting(now), flushed: now, saving: None }
    }

    /// Counts `event` before `app` takes it: the time before an input goes to the screen it landed on, a key counts as
    /// the act it does there.
    pub(super) fn see(&mut self, event: &Event, app: &App) {
        let now = Instant::now();
        let read = match event {
            Event::FocusGained => self.clock.to_front(now),
            Event::FocusLost => self.clock.to_back(now),
            Event::Key(key) if key.kind == KeyEventKind::Release => return,
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Moved => return,
            Event::Key(_) | Event::Mouse(_) | Event::Paste(_) => self.clock.input(now),
            Event::Resize(..) => return,
        };
        self.tally.spend(app.screen(), read);
        if let Event::Key(key) = event
            && let Some(act) = app.act_of(*key)
        {
            self.tally.act(act.name());
        }
    }

    /// Adds what was counted to the file in the background, once a minute went by since the last flush.
    pub(super) fn flush_when_due(&mut self, app: &App) {
        if self.flushed.elapsed() < FLUSH {
            return;
        }
        if let Some(counts) = self.take(app) {
            self.saving = Some(tokio::spawn(save(self.saving.take(), self.file.clone(), counts)));
        }
    }

    /// Adds everything counted to the file, as pgbg quits.
    pub(super) async fn flush(mut self, app: &App) {
        if let Some(counts) = self.take(app) {
            save(self.saving.take(), self.file, counts).await;
        }
    }

    fn take(&mut self, app: &App) -> Option<Counts> {
        let now = Instant::now();
        self.flushed = now;
        self.tally.spend(app.screen(), self.clock.count(now));
        self.tally.take()
    }
}

/// Adds `counts` to today's line on a blocking thread once the save `before` it is done; a failure costs a minute of
/// counts and is logged.
async fn save(before: Option<JoinHandle<()>>, file: UsageFile, counts: Counts) {
    if let Some(before) = before
        && let Err(err) = before.await
    {
        tracing::warn!("the usage counts before were not written: {err}");
    }
    let today = chrono::Local::now().date_naive();
    let saved = tokio::task::spawn_blocking(move || file.add(today, &counts)).await.map_err(anyhow::Error::from);
    if let Err(err) = saved.and_then(|added| added) {
        tracing::warn!("the usage counts were not written: {err:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::with_result;
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};

    fn counting() -> Counting {
        Counting::start(UsageFile::in_dir(std::path::Path::new("/tmp/pgbg-never-written")))
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::from(code))
    }

    #[test]
    fn a_key_counts_as_the_act_it_does_where_it_lands() {
        let (mut counting, mut app) = (counting(), with_result());
        counting.see(&key(KeyCode::Char('j')), &app);
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        counting.see(&key(KeyCode::Char('j')), &app);
        let counts = counting.tally.take().unwrap();
        assert_eq!(counts.actions, [("down".to_owned(), 1)].into());
    }

    #[test]
    fn a_released_key_counts_as_nothing() {
        let (mut counting, mut app) = (counting(), with_result());
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        let released = KeyEvent { kind: KeyEventKind::Release, ..KeyEvent::from(KeyCode::Char('j')) };
        counting.see(&Event::Key(released), &app);
        assert_eq!(counting.tally.take(), None);
    }
}
