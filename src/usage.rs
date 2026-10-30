//! What pgbg is used for, counted on this machine only: which acts, and how long on each pane or overlay.
//! Opt-in with `[usage] enabled = true`; no statement, table, row, connection or host is kept, only act names.
//! vtui counts, keeps the file's format and sums the report; pgbg names what it counts and keeps the file.
use crate::act;
use crate::config;
use crate::files::{self, Readers};
use crate::lock;
use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::path::{Path, PathBuf};
pub(crate) use vtui::usage::{Counts, Day, Rules};

const FILE: &str = "usage.jsonl";
/// How pgbg counts time: a minute after the last input at most, never while the terminal is in the background.
const COUNTING: u32 = 1;

/// What pgbg counts: every act, under the pane of its first row in the action table.
pub(crate) fn rules() -> Rules {
    let groups = act::usage_groups();
    let actions = groups.iter().flat_map(|group| group.actions.iter().copied()).collect();
    Rules { actions, groups, hints: &[], renamed: &[], counting: COUNTING }
}

/// `usage.jsonl` in pgbg's cache folder; deleting it starts the counts again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsageFile {
    path: PathBuf,
}

impl UsageFile {
    /// The file in pgbg's cache folder, where the log is; none when the system has no cache folder.
    pub(crate) fn from_env() -> Option<Self> {
        config::cache_dir().map(|dir| Self::in_dir(&dir))
    }

    /// The file in `dir`.
    pub(crate) fn in_dir(dir: &Path) -> Self {
        Self { path: dir.join(FILE) }
    }

    /// Every recorded day, oldest first; none before the first count.
    pub(crate) fn read(&self) -> Result<Vec<Day>> {
        Ok(rules().parse(&config::read_kept(&self.path)?))
    }

    /// Adds `counts` to `date`'s line under the file's lock, so two pgbg never lose each other's counts, through a temp
    /// file only its owner reads, so the file is the old one or the new.
    pub(crate) fn add(&self, date: NaiveDate, counts: &Counts) -> Result<()> {
        let dir = self.path.parent().context("the usage file has no folder")?;
        files::make_private_dir(dir).with_context(|| format!("creating {}", dir.display()))?;
        let _held = lock::open(&self.path).with_context(|| format!("locking {}", self.path.display()))?;
        self.merge(date, counts)
    }

    /// Adds `counts` to `date`'s line; only the holder of the file's lock may call it.
    fn merge(&self, date: NaiveDate, counts: &Counts) -> Result<()> {
        let text = rules().merge(&config::read_kept(&self.path)?, date, counts)?;
        files::write_whole(&self.path, &text, Readers::Owner).with_context(|| format!("writing {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    fn ran(act: &str, count: u64) -> Counts {
        Counts { actions: BTreeMap::from([(act.to_owned(), count)]), ..Counts::default() }
    }

    fn date(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    #[test]
    fn no_file_yet_reads_as_no_day() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(UsageFile::in_dir(dir.path()).read().unwrap(), []);
    }

    #[test]
    fn counts_added_twice_on_one_day_make_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let file = UsageFile::in_dir(&dir.path().join("pgbg"));
        file.add(date("2026-10-26"), &ran("run", 2)).unwrap();
        file.add(date("2026-10-26"), &ran("run", 1)).unwrap();
        let days = file.read().unwrap();
        assert_eq!((days.len(), days[0].counts.actions["run"]), (1, 3));
    }

    #[test]
    fn a_merge_waiting_for_the_lock_keeps_the_counts_of_the_one_holding_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = UsageFile::in_dir(dir.path());
        let held = lock::open(&file.path).unwrap();
        let waiting = std::thread::spawn({
            let file = file.clone();
            move || file.add(date("2026-10-26"), &ran("run", 1))
        });
        file.merge(date("2026-10-26"), &ran("run", 2)).unwrap();
        drop(held);
        waiting.join().unwrap().unwrap();
        assert_eq!(file.read().unwrap()[0].counts.actions["run"], 3);
    }

    #[test]
    fn only_its_owner_reads_the_file_and_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let file = UsageFile::in_dir(&dir.path().join("pgbg"));
        file.add(date("2026-10-26"), &ran("run", 1)).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir.path().join("pgbg")), mode(&file.path)), (0o700, 0o600));
    }

    #[test]
    fn every_act_is_listed_once_under_a_pane() {
        let actions = rules().actions;
        for act in <act::Act as strum::IntoEnumIterator>::iter() {
            assert_eq!(actions.iter().filter(|name| **name == act.name()).count(), 1, "{act:?}");
        }
    }
}
