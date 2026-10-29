//! The colours of one cell's text, from its value and its column's colouring.
use super::app::Colouring;
use crate::db::model::Value;
use crate::format::SHORT_UUID_DIGITS;
use ratatui::style::{Modifier, Style};
use vtui::theme::Theme;

/// How a cell's text is coloured: in one style, in two split at a byte, or token by token as JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ink {
    Whole(Style),
    Split { at: usize, head: Style, tail: Style },
    Json,
}

/// The ink of `text`, which `value` reads as in a column coloured `colouring`; a UUID dims all but its first digits.
pub(super) fn ink(text: &str, value: &Value, colouring: Colouring, theme: &Theme) -> Ink {
    let fg = |colour| Style::default().fg(colour);
    match value {
        Value::Null => Ink::Whole(fg(theme.faded).add_modifier(Modifier::ITALIC)),
        Value::Bool(true) => Ink::Whole(fg(theme.success)),
        Value::Bool(false) => Ink::Whole(fg(theme.muted)),
        _ if is_negative(value) => Ink::Whole(fg(theme.danger)),
        Value::Text(value) if colouring == Colouring::PerText => Ink::Whole(fg(theme.user(value))),
        Value::Uuid(_) => split(text, text.char_indices().nth(SHORT_UUID_DIGITS).map(|(at, _)| at), theme),
        Value::Timestamp(_) | Value::TimestampTz(_) if colouring == Colouring::DateTime => split(text, text.find(' '), theme),
        Value::Json(_) => Ink::Json,
        _ => Ink::Whole(Style::default()),
    }
}

/// `text` as it is up to byte `at`, dimmed from there; whole when there is no `at`.
fn split(text: &str, at: Option<usize>, theme: &Theme) -> Ink {
    match at {
        Some(at) if at < text.len() => Ink::Split { at, head: Style::default(), tail: Style::default().fg(theme.muted) },
        _ => Ink::Whole(Style::default()),
    }
}

fn is_negative(value: &Value) -> bool {
    match value {
        Value::Int(int) => *int < 0,
        Value::Float(float) => *float < 0.0,
        Value::Numeric(text) => text.starts_with('-'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    fn whole(colour: ratatui::style::Color) -> Ink {
        Ink::Whole(Style::default().fg(colour))
    }

    #[test]
    fn a_null_is_dimmed_and_slanted_whatever_its_column() {
        let expected = Ink::Whole(Style::default().fg(theme().faded).add_modifier(Modifier::ITALIC));
        assert_eq!(ink("NULL", &Value::Null, Colouring::PerText, &theme()), expected);
    }

    #[test]
    fn the_text_null_reads_as_any_other_text() {
        assert_eq!(ink("NULL", &Value::Text("NULL".into()), Colouring::Plain, &theme()), Ink::Whole(Style::default()));
    }

    #[test]
    fn true_takes_the_success_colour_and_false_is_dimmed() {
        let inks = [true, false].map(|value| ink("✓", &Value::Bool(value), Colouring::Plain, &theme()));
        assert_eq!(inks, [whole(theme().success), whole(theme().muted)]);
    }

    #[test]
    fn a_negative_number_takes_the_failure_colour() {
        let negatives = [Value::Int(-3), Value::Float(-0.5), Value::Numeric("-12.50".into())];
        assert!(negatives.iter().all(|value| ink("-", value, Colouring::Plain, &theme()) == whole(theme().danger)));
    }

    #[test]
    fn a_positive_number_stays_plain() {
        assert_eq!(ink("12.50", &Value::Numeric("12.50".into()), Colouring::Plain, &theme()), Ink::Whole(Style::default()));
    }

    #[test]
    fn a_repeated_text_takes_the_same_colour_in_every_result() {
        let paid = Value::Text("paid".into());
        assert_eq!(ink("paid", &paid, Colouring::PerText, &theme()), whole(theme().user("paid")));
    }

    #[test]
    fn a_uuid_dims_what_follows_its_first_eight_digits() {
        let id = Value::Uuid(uuid::Uuid::nil());
        let expected = Ink::Split { at: 8, head: Style::default(), tail: Style::default().fg(theme().muted) };
        assert_eq!(ink("00000000-0000-…", &id, Colouring::Plain, &theme()), expected);
    }

    #[test]
    fn a_uuid_cut_before_its_eighth_digit_reads_whole() {
        assert_eq!(ink("00000…", &Value::Uuid(uuid::Uuid::nil()), Colouring::Plain, &theme()), Ink::Whole(Style::default()));
    }

    #[test]
    fn a_timestamp_dims_its_time_after_its_date() {
        let at = Value::TimestampTz(chrono::DateTime::UNIX_EPOCH);
        let expected = Ink::Split { at: 10, head: Style::default(), tail: Style::default().fg(theme().muted) };
        assert_eq!(ink("1970-01-01 00:00:00+00:00", &at, Colouring::DateTime, &theme()), expected);
    }
}
