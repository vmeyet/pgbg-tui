//! A saved query in the user's own editor, `$VISUAL` or `$EDITOR`: the loop lends it the terminal, then reads the file
//! again.
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

const FALLBACK: &str = "vi";

/// The editor to run, and the arguments it was configured with, as in `EDITOR="code -w"`.
#[derive(Debug, PartialEq, Eq)]
struct Editor {
    program: String,
    args: Vec<String>,
}

impl Editor {
    fn pick(visual: Option<String>, editor: Option<String>) -> Self {
        let command = visual.or(editor).unwrap_or_default();
        let mut words = command.split_whitespace();
        let program = words.next().unwrap_or(FALLBACK).to_owned();
        Self { program, args: words.map(str::to_owned).collect() }
    }
}

fn variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.trim().is_empty())
}

/// Runs the user's editor on `path` until it exits; it blocks the thread, so the loop calls it while the terminal is
/// lent.
pub(super) fn edit(path: &Path) -> Result<()> {
    edit_with(&Editor::pick(variable("VISUAL"), variable("EDITOR")), path)
}

fn edit_with(editor: &Editor, path: &Path) -> Result<()> {
    let status =
        Command::new(&editor.program).args(&editor.args).arg(path).status().with_context(|| format!("could not run {}", editor.program))?;
    if !status.success() {
        bail!("{} exited with {status}", editor.program);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(program: &str, args: &[&str]) -> Editor {
        Editor { program: program.into(), args: args.iter().map(|arg| (*arg).to_owned()).collect() }
    }

    #[test]
    fn visual_comes_first_then_editor_then_vi() {
        assert_eq!(Editor::pick(Some("emacs".into()), Some("nano".into())), editor("emacs", &[]));
        assert_eq!(Editor::pick(None, Some("nano".into())), editor("nano", &[]));
        assert_eq!(Editor::pick(None, None), editor("vi", &[]));
    }

    #[test]
    fn an_editor_keeps_the_arguments_it_was_given() {
        assert_eq!(Editor::pick(None, Some("code -w --new-window".into())), editor("code", &["-w", "--new-window"]));
    }

    #[test]
    fn an_editor_that_fails_says_so() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(edit_with(&editor("false", &[]), file.path()).is_err());
        assert!(edit_with(&editor("true", &[]), file.path()).is_ok());
    }
}
