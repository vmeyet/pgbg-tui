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
        let text = if stdin.is_terminal() {
            typed(prompt).await?
        } else {
            let mut piped = String::new();
            stdin.lock().read_to_string(&mut piped)?;
            piped
        };
        from_text(&text)
    }
}

fn from_text(text: &str) -> Result<Secret> {
    let password = without_line_end(text);
    if password.is_empty() {
        bail!("no password given");
    }
    Ok(Secret(password.to_owned()))
}

async fn typed(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let password = read_hidden().await?;
    eprintln!();
    Ok(password)
}

/// Reads one line with echo off, restoring the terminal even when the read fails or a signal stops pgbg.
async fn read_hidden() -> Result<String> {
    let mut signals = Signals::listen()?;
    crossterm::terminal::enable_raw_mode()?;
    let line = tokio::select! {
        line = read_raw_line() => line,
        stopped = signals.next() => Err(stopped.into()),
    };
    crossterm::terminal::disable_raw_mode()?;
    line
}

async fn read_raw_line() -> Result<String> {
    let mut events = EventStream::new();
    let mut line = String::new();
    while let Some(event) = events.next().await {
        if let Event::Key(key) = event? {
            match typed_key(&line, key) {
                Typing::Line(typed) => line = typed,
                Typing::Entered => return Ok(line),
                Typing::Cancelled => bail!("cancelled"),
            }
        }
    }
    bail!("the terminal closed before the password was entered")
}

/// Where the hidden line stands after one key.
#[derive(Debug, PartialEq, Eq)]
enum Typing {
    Line(String),
    Entered,
    Cancelled,
}

/// The line after `key`; a letter typed with ctrl or alt is a shortcut, not part of the password.
fn typed_key(line: &str, key: KeyEvent) -> Typing {
    let is_shortcut = key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
    match key.code {
        KeyCode::Enter => Typing::Entered,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Typing::Cancelled,
        KeyCode::Backspace => Typing::Line(without_last_character(line).to_owned()),
        KeyCode::Char(c) if !is_shortcut => Typing::Line(format!("{line}{c}")),
        _ => Typing::Line(line.to_owned()),
    }
}

fn without_last_character(line: &str) -> &str {
    line.char_indices().next_back().map_or(line, |(last, _)| &line[..last])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piped_password_loses_its_newline_and_keeps_its_spaces() {
        assert_eq!(from_text(" hunter 2 \n").unwrap().expose(), " hunter 2 ");
    }

    fn after(line: &str, code: KeyCode, modifiers: KeyModifiers) -> Typing {
        typed_key(line, KeyEvent::new(code, modifiers))
    }

    #[test]
    fn a_typed_letter_adds_to_the_password() {
        assert_eq!(after("hunte", KeyCode::Char('r'), KeyModifiers::NONE), Typing::Line("hunter".into()));
        assert_eq!(after("hunte", KeyCode::Char('R'), KeyModifiers::SHIFT), Typing::Line("hunteR".into()));
    }

    #[test]
    fn a_letter_typed_with_ctrl_or_alt_adds_nothing() {
        assert_eq!(after("hunter", KeyCode::Char('u'), KeyModifiers::CONTROL), Typing::Line("hunter".into()));
        assert_eq!(after("hunter", KeyCode::Char('x'), KeyModifiers::ALT), Typing::Line("hunter".into()));
    }

    #[test]
    fn backspace_takes_the_last_character_back() {
        assert_eq!(after("hunté", KeyCode::Backspace, KeyModifiers::NONE), Typing::Line("hunt".into()));
    }

    #[test]
    fn enter_ends_the_password_and_ctrl_c_cancels_it() {
        assert_eq!(after("hunter", KeyCode::Enter, KeyModifiers::NONE), Typing::Entered);
        assert_eq!(after("hunter", KeyCode::Char('c'), KeyModifiers::CONTROL), Typing::Cancelled);
    }

    #[test]
    fn an_empty_password_is_refused() {
        assert!(from_text("\r\n").is_err());
    }
}
