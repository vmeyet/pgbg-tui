//! A password given to pgbg on its standard input: typed with echo off, or piped in.
use super::{Secret, without_line_end};
use crate::signal::Signals;
use anyhow::{Result, bail};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures_util::StreamExt;
use std::io::{IsTerminal, Read, Write};

impl Secret {
    /// The password typed after `prompt` with echo off, or piped in, without its last newline.
    pub(crate) async fn from_stdin(prompt: &str) -> Result<Self> {
        let stdin = std::io::stdin();
        let password = if stdin.is_terminal() { typed(prompt).await? } else { piped(&stdin)? };
        if password.expose().is_empty() {
            bail!("no password given");
        }
        Ok(password)
    }
}

fn piped(stdin: &std::io::Stdin) -> Result<Secret> {
    let mut piped = Secret::default();
    stdin.lock().read_to_string(&mut piped.0)?;
    Ok(without_line_end(std::mem::take(&mut piped.0)))
}

async fn typed(prompt: &str) -> Result<Secret> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let password = read_hidden().await?;
    eprintln!();
    Ok(password)
}

/// Reads one line with echo off, restoring the terminal even when the read fails or a signal stops pgbg.
async fn read_hidden() -> Result<Secret> {
    let mut signals = Signals::listen()?;
    crossterm::terminal::enable_raw_mode()?;
    let line = tokio::select! {
        line = read_raw_line() => line,
        stopped = signals.next() => Err(stopped.into()),
    };
    crossterm::terminal::disable_raw_mode()?;
    line
}

async fn read_raw_line() -> Result<Secret> {
    let mut events = EventStream::new();
    let mut line = Secret::default();
    while let Some(event) = events.next().await {
        if let Event::Key(key) = event? {
            match typing(key) {
                Typing::Add(c) => line.push(c),
                Typing::Erase => line.pop(),
                Typing::Nothing => {}
                Typing::Entered => return Ok(line),
                Typing::Cancelled => bail!("cancelled"),
            }
        }
    }
    bail!("the terminal closed before the password was entered")
}

/// What one key does to the hidden line, which grows in place so no copy of it is left.
#[derive(Debug, PartialEq, Eq)]
enum Typing {
    Add(char),
    Erase,
    Nothing,
    Entered,
    Cancelled,
}

/// A letter typed with ctrl or alt is a shortcut, not part of the password.
fn typing(key: KeyEvent) -> Typing {
    let is_shortcut = key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
    match key.code {
        KeyCode::Enter => Typing::Entered,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Typing::Cancelled,
        KeyCode::Backspace => Typing::Erase,
        KeyCode::Char(c) if !is_shortcut => Typing::Add(c),
        _ => Typing::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piped_password_loses_its_newline_and_keeps_its_spaces() {
        assert_eq!(without_line_end(" hunter 2 \n".to_owned()).expose(), " hunter 2 ");
    }

    fn after(code: KeyCode, modifiers: KeyModifiers) -> Typing {
        typing(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn a_typed_letter_adds_to_the_password() {
        assert_eq!(after(KeyCode::Char('r'), KeyModifiers::NONE), Typing::Add('r'));
        assert_eq!(after(KeyCode::Char('R'), KeyModifiers::SHIFT), Typing::Add('R'));
    }

    #[test]
    fn a_letter_typed_with_ctrl_or_alt_adds_nothing() {
        assert_eq!(after(KeyCode::Char('u'), KeyModifiers::CONTROL), Typing::Nothing);
        assert_eq!(after(KeyCode::Char('x'), KeyModifiers::ALT), Typing::Nothing);
    }

    #[test]
    fn backspace_takes_the_last_character_back() {
        assert_eq!(after(KeyCode::Backspace, KeyModifiers::NONE), Typing::Erase);
    }

    #[test]
    fn enter_ends_the_password_and_ctrl_c_cancels_it() {
        assert_eq!(after(KeyCode::Enter, KeyModifiers::NONE), Typing::Entered);
        assert_eq!(after(KeyCode::Char('c'), KeyModifiers::CONTROL), Typing::Cancelled);
    }

    #[test]
    fn a_password_ending_in_line_ends_only_is_empty() {
        assert_eq!(without_line_end("\r\n".to_owned()).expose(), "");
    }
}
