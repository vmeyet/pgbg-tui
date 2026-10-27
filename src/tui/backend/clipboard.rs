//! The clipboard of the machine pgbg runs on, through `pbcopy`.
//! The loop also sends the terminal's OSC 52, which may not reach a clipboard inside tmux or from a phone, so each copy
//! tries both.
use super::super::app::Answer;
use super::send;
use crate::db::model::ResultSet;
use crate::export::{self, Format};
use anyhow::{Context as _, Result};
use std::process::Stdio;
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

/// The largest text the loop writes to the terminal as OSC 52: a bigger write would block the loop, and many terminals
/// drop it.
const TERMINAL_LIMIT_BYTES: usize = 100 * 1024;

/// Which clipboards a copy reached.
#[derive(Clone, Copy)]
struct Clipboards {
    is_in_pbcopy: bool,
    is_in_terminal: bool,
}

/// Whether the loop also sends `text` to the terminal's clipboard.
pub(in crate::tui) fn reaches_the_terminal(text: &str) -> bool {
    text.len() <= TERMINAL_LIMIT_BYTES
}

/// Builds the text of `result` in `format` on a blocking thread; the text comes back as `Answer::Exported`, to be copied.
pub(super) fn export(result: ResultSet, format: Format, done: String, answers: UnboundedSender<Answer>) {
    tokio::task::spawn_blocking(move || {
        send(&answers, Answer::Exported { text: export::result(&result, format), done });
    });
}

/// Hands `text` to `pbcopy` off the loop; `done` comes back as `Answer::Copied`, noting when only one clipboard got it.
pub(super) fn copy(text: String, done: String, answers: UnboundedSender<Answer>) {
    tokio::spawn(async move {
        let is_in_pbcopy = match pbcopy(&text).await {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!("pbcopy did not take the copy: {err:#}");
                false
            }
        };
        let note = note(done, Clipboards { is_in_pbcopy, is_in_terminal: reaches_the_terminal(&text) });
        send(&answers, Answer::Copied(note));
    });
}

fn note(done: String, clipboards: Clipboards) -> String {
    match clipboards {
        Clipboards { is_in_pbcopy: true, is_in_terminal: true } => done,
        Clipboards { is_in_pbcopy: true, is_in_terminal: false } => format!("{done}, through pbcopy only"),
        Clipboards { is_in_pbcopy: false, is_in_terminal: true } => format!("{done}, through the terminal only"),
        Clipboards { is_in_pbcopy: false, is_in_terminal: false } => {
            "nothing copied: too big for the terminal, and pbcopy failed".to_owned()
        }
    }
}

async fn pbcopy(text: &str) -> Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("running pbcopy")?;
    let mut stdin = child.stdin.take().context("pbcopy took no input")?;
    stdin.write_all(text.as_bytes()).await?;
    drop(stdin);
    anyhow::ensure!(child.wait().await?.success(), "pbcopy failed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::orders;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn a_result_is_exported_off_the_loop_and_comes_back_to_be_copied() {
        let (sender, mut answers) = mpsc::unbounded_channel();
        export(orders(), Format::Csv, "copied 4 rows as CSV".into(), sender);
        let Some(Answer::Exported { text, done }) = answers.recv().await else { panic!("no export") };
        assert_eq!((text.lines().next(), done.as_str()), (Some("id,total,customer"), "copied 4 rows as CSV"));
    }

    #[test]
    fn a_copy_past_the_terminal_limit_skips_the_terminal() {
        assert!(reaches_the_terminal(&"x".repeat(TERMINAL_LIMIT_BYTES)));
        assert!(!reaches_the_terminal(&"x".repeat(TERMINAL_LIMIT_BYTES + 1)));
    }

    #[test]
    fn a_copy_the_terminal_skipped_says_it_went_through_pbcopy_only() {
        assert_eq!(
            note("copied 4 rows as CSV".into(), Clipboards { is_in_pbcopy: true, is_in_terminal: false }),
            "copied 4 rows as CSV, through pbcopy only"
        );
    }

    #[test]
    fn a_copy_neither_clipboard_took_says_nothing_was_copied() {
        let note = note("copied 4 rows as CSV".into(), Clipboards { is_in_pbcopy: false, is_in_terminal: false });
        assert_eq!(note, "nothing copied: too big for the terminal, and pbcopy failed");
    }
}
