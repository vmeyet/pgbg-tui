//! A password given to pgbg on its standard input: typed with echo off, or piped in.
use super::Secret;
use anyhow::{Result, bail};
use std::io::{IsTerminal, Read, Write};

impl Secret {
    /// The password typed after `prompt` with echo off, or piped in, without its last newline.
    pub(crate) fn from_stdin(prompt: &str) -> Result<Self> {
        let stdin = std::io::stdin();
        let text = if stdin.is_terminal() {
            typed(prompt)?
        } else {
            let mut piped = String::new();
            stdin.lock().read_to_string(&mut piped)?;
            piped
        };
        from_text(&text)
    }
}

fn from_text(text: &str) -> Result<Secret> {
    let password = text.trim_end_matches(['\n', '\r']);
    if password.is_empty() {
        bail!("no password given");
    }
    Ok(Secret(password.to_owned()))
}

fn typed(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let password = read_hidden()?;
    eprintln!();
    Ok(password)
}

/// Reads one line with echo off, restoring the terminal even when the read fails.
fn read_hidden() -> Result<String> {
    crossterm::terminal::enable_raw_mode()?;
    let line = read_raw_line();
    crossterm::terminal::disable_raw_mode()?;
    line
}

fn read_raw_line() -> Result<String> {
    use crossterm::event::{Event, KeyCode, KeyModifiers, read};
    let mut line = String::new();
    loop {
        if let Event::Key(key) = read()? {
            match key.code {
                KeyCode::Enter => return Ok(line),
                KeyCode::Backspace => drop(line.pop()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => bail!("cancelled"),
                KeyCode::Char(c) => line.push(c),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piped_password_loses_its_newline_and_keeps_its_spaces() {
        assert_eq!(from_text(" hunter 2 \n").unwrap().expose(), " hunter 2 ");
    }

    #[test]
    fn an_empty_password_is_refused() {
        assert!(from_text("\r\n").is_err());
    }
}
