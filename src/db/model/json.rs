use super::value::prefixed;
use serde::{Serialize, Serializer};
use serde_json::value::RawValue;
use std::fmt::{self, Write};

const INDENT: &str = "  ";

/// A JSON value kept as the compact text of what the server sent, its keys in the server's order: `jsonb` puts the
/// shorter keys first, `json` keeps them as written. One string per cell, never a tree of nodes.
#[derive(Clone, Debug)]
pub(crate) struct JsonText(Box<RawValue>);

impl JsonText {
    /// `text`, JSON as the server wrote it, with the whitespace outside its strings removed.
    pub(crate) fn compacted(text: &str) -> serde_json::Result<Self> {
        let compact: String = outside_strings(text).filter(|&(c, is_outside)| !(is_outside && is_blank(c))).map(|(c, _)| c).collect();
        RawValue::from_string(compact).map(Self)
    }

    #[cfg(test)]
    pub(crate) fn of(json: &serde_json::Value) -> Self {
        Self(serde_json::value::to_raw_value(json).unwrap())
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.get()
    }

    /// The first `chars` characters of it indented over several lines as `serde_json` pretty prints, so a row's
    /// detail can show its shape; the indenting stops once it has them.
    pub(super) fn pretty_prefix(&self, chars: usize) -> String {
        prefixed(chars, format_args!("{}", Indented(self.as_str())))
    }
}

impl PartialEq for JsonText {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for JsonText {}

/// Writes the JSON it holds as it is, so even a pretty printer keeps it on one line.
impl Serialize for JsonText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// The byte after the JSON string opened at `start` in `bytes`: past its closing quote, or the end when it is cut.
pub(crate) fn string_end(bytes: &[u8], start: usize) -> usize {
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

fn is_blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// Each character of the JSON `text`, and whether it stands outside a string, its quotes aside.
fn outside_strings(text: &str) -> impl Iterator<Item = (char, bool)> {
    let mut is_in_string = false;
    let mut is_escaped = false;
    text.chars().map(move |c| {
        let is_outside = !is_in_string && c != '"';
        match (is_in_string, is_escaped, c) {
            (true, true, _) => is_escaped = false,
            (true, false, '\\') => is_escaped = true,
            (_, false, '"') => is_in_string = !is_in_string,
            _ => {}
        }
        (c, is_outside)
    })
}

/// Compact JSON, indented as it is written out.
struct Indented<'t>(&'t str);

impl fmt::Display for Indented<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut depth = 0_usize;
        let mut chars = outside_strings(self.0).peekable();
        while let Some((c, is_outside)) = chars.next() {
            match (c, is_outside) {
                (_, false) => f.write_char(c)?,
                ('{' | '[', true) => {
                    f.write_char(c)?;
                    if let Some(&(close @ ('}' | ']'), true)) = chars.peek() {
                        chars.next();
                        f.write_char(close)?;
                    } else {
                        depth += 1;
                        new_line(f, depth)?;
                    }
                }
                ('}' | ']', true) => {
                    depth = depth.saturating_sub(1);
                    new_line(f, depth)?;
                    f.write_char(c)?;
                }
                (',', true) => {
                    f.write_char(',')?;
                    new_line(f, depth)?;
                }
                (':', true) => f.write_str(": ")?,
                _ => f.write_char(c)?,
            }
        }
        Ok(())
    }
}

fn new_line(f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
    f.write_char('\n')?;
    (0..depth).try_for_each(|_| f.write_str(INDENT))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys in name order at every depth, as `serde_json` keeps them; strings holding what is structure outside them.
    const TRICKY: &str = r#"{"a": [1, {}, [], {"q": "say \"hi\", {x: [y]}"}], "b": {"e": "back\\", "z": null}, "c": true}"#;

    #[test]
    fn a_string_ends_past_its_closing_quote_whatever_it_escapes_or_at_the_end_when_cut() {
        assert_eq!((string_end(br#""a\"b" 1"#, 0), string_end(br#""cut"#, 0)), (6, 4));
    }

    #[test]
    fn json_drops_the_blanks_outside_its_strings_and_keeps_its_keys_in_order() {
        let json = JsonText::compacted(r#"{"paid": true, "id": 7, "note": "a, b: c"}"#).unwrap();
        assert_eq!(json.as_str(), r#"{"paid":true,"id":7,"note":"a, b: c"}"#);
    }

    #[test]
    fn json_indents_as_serde_json_pretty_prints_it() {
        let tree: serde_json::Value = serde_json::from_str(TRICKY).unwrap();
        assert_eq!(JsonText::compacted(TRICKY).unwrap().pretty_prefix(usize::MAX), serde_json::to_string_pretty(&tree).unwrap());
    }

    #[test]
    fn json_indents_an_empty_object_and_array_on_one_line() {
        assert_eq!(JsonText::compacted("{\"a\": {}, \"b\": []}").unwrap().pretty_prefix(100), "{\n  \"a\": {},\n  \"b\": []\n}");
    }

    #[test]
    fn json_that_is_not_json_is_refused() {
        assert!(JsonText::compacted("{\"a\": ").is_err());
    }

    #[test]
    fn json_serializes_as_the_text_it_holds() {
        let json = JsonText::compacted("{\"b\": 1, \"a\": [2]}").unwrap();
        assert_eq!(serde_json::to_string_pretty(&[json]).unwrap(), "[\n  {\"b\":1,\"a\":[2]}\n]");
    }
}
