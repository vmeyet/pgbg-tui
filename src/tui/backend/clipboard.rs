//! The clipboard of the machine pgbg runs on, through `pbcopy`: the loop also sends the terminal's OSC 52, which inside
//! tmux or from a phone may not reach a clipboard, so each copy tries both.
use super::super::app::Answer;
use super::send;
use anyhow::{Context as _, Result};
use std::process::Stdio;
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

/// Hands `text` to `pbcopy` off the loop; `done` comes back as `Answer::Copied`, saying when only the terminal had it.
pub(super) fn copy(text: String, done: String, answers: UnboundedSender<Answer>) {
    tokio::spawn(async move {
        let note = match pbcopy(&text).await {
            Ok(()) => done,
            Err(err) => {
                tracing::warn!("pbcopy did not take the copy: {err:#}");
                format!("{done}, through the terminal only")
            }
        };
        send(&answers, Answer::Copied(note));
    });
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
