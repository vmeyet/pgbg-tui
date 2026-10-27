//! JSON colours for one line of indented JSON, by hand: the text is small and already valid, so a grammar would cost more.
use super::{Painted, Spans, Token};

/// The coloured pieces of one line of indented JSON: a key as a keyword, then strings, numbers, `true`, `false` and
/// `null`, and the punctuation. A string cut at the line's end runs to it.
pub(crate) fn highlight_json_line(line: &str) -> Spans {
    let bytes = line.as_bytes();
    let mut spans = Spans::new();
    let mut at = 0;
    while at < bytes.len() {
        let (end, token) = match bytes[at] {
            b'"' => {
                let end = string_end(bytes, at);
                (end, Some(string_token(bytes, end)))
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => (at + 1, Some(Token::Punctuation)),
            b'-' | b'0'..=b'9' => (run_end(bytes, at, |b| b.is_ascii_digit() || b"+-.eE".contains(&b)), Some(Token::Number)),
            b'a'..=b'z' => (run_end(bytes, at, |b| b.is_ascii_lowercase()), Some(Token::Constant)),
            _ => (at + 1, None),
        };
        if let Some(token) = token {
            spans.push(Painted { bytes: at..end, token });
        }
        at = end;
    }
    spans
}

/// The byte after the string opened at `start`: past its closing quote, or the line's end.
fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// A key when a `:` follows the string, else a string.
fn string_token(bytes: &[u8], end: usize) -> Token {
    let is_key = bytes.get(end..).is_some_and(|rest| rest.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b':'));
    if is_key { Token::Keyword } else { Token::String }
}

fn run_end(bytes: &[u8], start: usize, is_part: impl Fn(u8) -> bool) -> usize {
    bytes[start..].iter().position(|&b| !is_part(b)).map_or(bytes.len(), |length| start + length)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(line: &str) -> Vec<(&str, Token)> {
        highlight_json_line(line).into_iter().map(|painted| (&line[painted.bytes], painted.token)).collect()
    }

    #[test]
    fn a_key_its_value_and_the_punctuation_each_get_their_colour() {
        assert_eq!(
            tokens(r#"  "total": -12.5e3,"#),
            [("\"total\"", Token::Keyword), (":", Token::Punctuation), ("-12.5e3", Token::Number), (",", Token::Punctuation)]
        );
    }

    #[test]
    fn a_string_value_with_an_escaped_quote_is_one_string() {
        assert_eq!(tokens(r#""say \"hi\"""#), [(r#""say \"hi\"""#, Token::String)]);
    }

    #[test]
    fn true_false_and_null_are_constants() {
        assert_eq!(
            tokens("[true, null]"),
            [
                ("[", Token::Punctuation),
                ("true", Token::Constant),
                (",", Token::Punctuation),
                ("null", Token::Constant),
                ("]", Token::Punctuation)
            ]
        );
    }

    #[test]
    fn a_string_cut_at_the_end_of_the_line_runs_to_it() {
        assert_eq!(tokens(r#"  "Zoë Naka"#), [(r#""Zoë Naka"#, Token::String)]);
    }
}
