//! The log file, `pgbg.log` in pgbg's cache folder; never the terminal, which the TUI owns.
//! `PGBG_LOG` sets the level. A line never holds a secret or a row.
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use tracing::level_filters::LevelFilter;

const FILE: &str = "pgbg.log";
const ROLLED_FILE: &str = "pgbg.log.1";
const LEVEL: &str = "PGBG_LOG";
const DEFAULT_LEVEL: LevelFilter = LevelFilter::WARN;
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// Logs to the file for the rest of the process; a file that cannot open leaves pgbg unlogged, not stopped.
pub fn start() {
    let level = level(std::env::var(LEVEL).ok().as_deref());
    if level == LevelFilter::OFF {
        return;
    }
    let Some(dir) = crate::config::cache_dir() else { return };
    let Ok(file) = Rolling::open(dir, MAX_BYTES) else { return };
    #[expect(clippy::let_underscore_must_use, reason = "`start` runs once per process, so no subscriber is set before")]
    let _ = tracing::subscriber::set_global_default(subscriber(std::sync::Mutex::new(file), level));
}

/// The log file, rolled over to `pgbg.log.1` whenever it passes `max` bytes, so it stays under twice `max`.
struct Rolling {
    dir: PathBuf,
    max: u64,
    file: File,
    size: u64,
}

impl Rolling {
    fn open(dir: PathBuf, max: u64) -> std::io::Result<Self> {
        DirBuilder::new().recursive(true).mode(DIR_MODE).create(&dir)?;
        let path = dir.join(FILE);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > max) {
            std::fs::rename(&path, dir.join(ROLLED_FILE))?;
        }
        let file = OpenOptions::new().create(true).append(true).mode(FILE_MODE).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self { dir, max, file, size })
    }

    fn roll_over(&mut self) {
        match Self::open(self.dir.clone(), self.max) {
            Ok(next) => *self = next,
            // A log that cannot roll over has nowhere to say so: it goes on in the same file and tries again later.
            Err(_) => self.size = 0,
        }
    }
}

impl Write for Rolling {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.size > self.max {
            self.roll_over();
        }
        let written = self.file.write(bytes)?;
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Only pgbg's own events: the libraries below log wire details at their debug levels.
fn subscriber<W>(writer: W, level: LevelFilter) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'w> tracing_subscriber::fmt::MakeWriter<'w> + Send + Sync + 'static,
{
    use tracing_subscriber::layer::SubscriberExt;
    let only_pgbg = tracing_subscriber::filter::Targets::new().with_target(env!("CARGO_CRATE_NAME"), level);
    tracing_subscriber::fmt().with_writer(writer).with_ansi(false).with_max_level(level).finish().with(only_pgbg)
}

/// A level `PGBG_LOG` names (`error` to `trace`, or `off`); anything else keeps the default.
fn level(variable: Option<&str>) -> LevelFilter {
    variable.and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_LEVEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_comes_from_the_variable_else_warn() {
        assert_eq!(level(Some("debug")), LevelFilter::DEBUG);
        assert_eq!(level(Some(" off ")), LevelFilter::OFF);
        assert_eq!(level(Some("loud")), LevelFilter::WARN);
        assert_eq!(level(None), LevelFilter::WARN);
    }

    #[test]
    fn the_log_rolls_over_mid_session_once_past_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = Rolling::open(dir.path().to_owned(), 8).unwrap();
        log.write_all(b"first").unwrap();
        log.write_all(b"second").unwrap();
        log.write_all(b"third").unwrap();
        assert_eq!(std::fs::read(dir.path().join(ROLLED_FILE)).unwrap(), b"firstsecond", "a line is never cut");
        assert_eq!(std::fs::read(dir.path().join(FILE)).unwrap(), b"third");
    }

    #[test]
    fn the_log_is_private_to_its_user() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        Rolling::open(dir.path().join("pgbg"), 8).unwrap();
        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.path().join("pgbg")), DIR_MODE);
        assert_eq!(mode(dir.path().join("pgbg").join(FILE)), FILE_MODE);
    }
}
