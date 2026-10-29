//! The clipboard of the machine pgbg runs on, through the first program that takes the copy: `pbcopy` on a Mac,
//! `wl-copy`, `xclip` or `xsel` elsewhere.
//! The loop also sends the terminal's OSC 52, which may not reach a clipboard inside tmux or from a phone, so each copy
//! tries both.
//! A paste reads it back through the first of `pbpaste`, or `wl-paste`, `xclip` and `xsel`, that prints it.
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

/// A program that puts what it reads on the machine's clipboard, or prints what the clipboard holds.
struct ClipboardProgram {
    program: &'static str,
    args: &'static [&'static str],
}

/// The programs a copy tries in turn: Wayland's first, then X11's, which also serve an X11 app under Wayland.
const COPIERS: &[ClipboardProgram] = if cfg!(target_os = "macos") {
    &[ClipboardProgram { program: "pbcopy", args: &[] }]
} else {
    &[
        ClipboardProgram { program: "wl-copy", args: &[] },
        ClipboardProgram { program: "xclip", args: &["-selection", "clipboard"] },
        ClipboardProgram { program: "xsel", args: &["--clipboard", "--input"] },
    ]
};

/// The programs a paste tries in turn, in the order of [`COPIERS`].
const PASTERS: &[ClipboardProgram] = if cfg!(target_os = "macos") {
    &[ClipboardProgram { program: "pbpaste", args: &[] }]
} else {
    &[
        ClipboardProgram { program: "wl-paste", args: &["--no-newline"] },
        ClipboardProgram { program: "xclip", args: &["-selection", "clipboard", "-out"] },
        ClipboardProgram { program: "xsel", args: &["--clipboard", "--output"] },
    ]
};

/// Which clipboards a copy reached.
#[derive(Clone, Copy)]
struct Clipboards {
    /// The program that put the copy on the machine's clipboard, if one did.
    machine: Option<&'static str>,
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

/// Hands `text` to the machine's clipboard off the loop; `done` comes back as `Answer::Copied`, noting when only one
/// clipboard got it.
pub(super) fn copy(text: String, done: String, answers: UnboundedSender<Answer>) {
    tokio::spawn(async move {
        let machine = to_the_machine(&text, COPIERS).await;
        let note = note(done, Clipboards { machine, is_in_terminal: reaches_the_terminal(&text) });
        send(&answers, Answer::Copied(note));
    });
}

/// Reads the machine's clipboard off the loop; its text comes back as `Answer::Pasted`, `None` when no program read it.
pub(super) fn paste(answers: UnboundedSender<Answer>) {
    tokio::spawn(async move {
        send(&answers, Answer::Pasted(from_the_machine(PASTERS).await));
    });
}

/// What the first of `pasters` that works prints, each failure logged.
async fn from_the_machine(pasters: &[ClipboardProgram]) -> Option<String> {
    for paster in pasters {
        match read(paster).await {
            Ok(text) => return Some(text),
            Err(err) => tracing::warn!("{} did not read the clipboard: {err:#}", paster.program),
        }
    }
    None
}

async fn read(paster: &ClipboardProgram) -> Result<String> {
    let output = Command::new(paster.program)
        .args(paster.args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("running {}", paster.program))?;
    anyhow::ensure!(output.status.success(), "{} failed", paster.program);
    String::from_utf8(output.stdout).context("the clipboard holds no UTF-8 text")
}

/// The first of `copiers` that takes `text`, each failure logged.
async fn to_the_machine(text: &str, copiers: &[ClipboardProgram]) -> Option<&'static str> {
    for copier in copiers {
        match run(copier, text).await {
            Ok(()) => return Some(copier.program),
            Err(err) => tracing::warn!("{} did not take the copy: {err:#}", copier.program),
        }
    }
    None
}

fn note(done: String, clipboards: Clipboards) -> String {
    match clipboards {
        Clipboards { machine: Some(_), is_in_terminal: true } => done,
        Clipboards { machine: Some(program), is_in_terminal: false } => format!("{done}, through {program} only"),
        Clipboards { machine: None, is_in_terminal: true } => format!("{done}, through the terminal only"),
        Clipboards { machine: None, is_in_terminal: false } => {
            format!("nothing copied: over {} KB for the terminal, and {} took none", TERMINAL_LIMIT_BYTES / 1024, names(COPIERS))
        }
    }
}

/// The programs of `copiers` as one list: `wl-copy, xclip or xsel`.
fn names(copiers: &[ClipboardProgram]) -> String {
    let programs: Vec<_> = copiers.iter().map(|copier| copier.program).collect();
    match programs.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, first)) => format!("{} or {last}", first.join(", ")),
        None => String::new(),
    }
}

async fn run(copier: &ClipboardProgram, text: &str) -> Result<()> {
    let mut child = Command::new(copier.program)
        .args(copier.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("running {}", copier.program))?;
    let mut stdin = child.stdin.take().context("the program took no input")?;
    stdin.write_all(text.as_bytes()).await?;
    drop(stdin);
    anyhow::ensure!(child.wait().await?.success(), "{} failed", copier.program);
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

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn a_copy_tries_each_clipboard_program_until_one_takes_it() {
        let programs = [ClipboardProgram { program: "pgbg-no-such-clipboard", args: &[] }, ClipboardProgram { program: "cat", args: &[] }];
        assert_eq!(to_the_machine(&"x".repeat(TERMINAL_LIMIT_BYTES + 1), &programs).await, Some("cat"));
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn a_paste_tries_each_clipboard_program_until_one_prints_it() {
        let programs = [
            ClipboardProgram { program: "pgbg-no-such-clipboard", args: &[] },
            ClipboardProgram { program: "printf", args: &["SELECT 1"] },
        ];
        assert_eq!(from_the_machine(&programs).await.as_deref(), Some("SELECT 1"));
    }

    #[test]
    fn a_copy_the_terminal_skipped_names_the_program_that_took_it() {
        assert_eq!(
            note("copied 4 rows as CSV".into(), Clipboards { machine: Some("xclip"), is_in_terminal: false }),
            "copied 4 rows as CSV, through xclip only"
        );
    }

    #[test]
    fn a_copy_neither_clipboard_took_says_it_was_too_big_for_the_terminal() {
        let note = note("copied 4 rows as CSV".into(), Clipboards { machine: None, is_in_terminal: false });
        assert_eq!(note, format!("nothing copied: over 100 KB for the terminal, and {} took none", names(COPIERS)));
    }

    #[test]
    fn the_programs_a_copy_tries_read_as_a_list() {
        let programs = [
            ClipboardProgram { program: "wl-copy", args: &[] },
            ClipboardProgram { program: "xclip", args: &[] },
            ClipboardProgram { program: "xsel", args: &[] },
        ];
        assert_eq!(names(&programs), "wl-copy, xclip or xsel");
        assert_eq!(names(&programs[..1]), "wl-copy");
    }
}
