//! The statement as a prompt shows it: each string and number literal a numbered marker, its comments gone.
use crate::db::lex::{self, Token};
use serde::Deserialize;
use std::fmt;

/// Whether a prompt masks the statement's literals, the default, or sends them as typed (`[ai] literals = "sent"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LiteralPolicy {
    #[default]
    Masked,
    Sent,
}

/// The literals a masked statement left out, marker `n` at place `n - 1`.
pub(super) struct Literals(Vec<Literal>);

struct Literal {
    kind: LiteralKind,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LiteralKind {
    String,
    Number,
}

/// A statement with its literals masked and its comments stripped, and the literals it left out.
pub(super) struct Masked {
    pub(super) text: String,
    pub(super) literals: Literals,
}

/// A marker an answer holds that stands for no literal of the statement it was asked about.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct UnknownMarker(pub(super) String);

/// A marker at the start of some text: what it stands for and how many bytes it spans; a number past `usize` is none.
struct Marker {
    kind: LiteralKind,
    number: Option<usize>,
    len: usize,
}

/// `sql` with each comment a blank and, unless `policy` sends them, each string and number literal a marker; a
/// `{{name}}` stays as it is.
pub(super) fn masked(sql: &str, policy: LiteralPolicy) -> Masked {
    let mut text = String::new();
    let mut literals = vec![];
    let mut end = 0;
    for placed in lex::tokens(sql) {
        text.push_str(without_comments(&sql[end..placed.range.start]));
        let token = &sql[placed.range.clone()];
        match literal_kind(token, &placed.token).filter(|_| policy == LiteralPolicy::Masked) {
            Some(kind) => {
                literals.push(Literal { kind, text: token.to_owned() });
                text.push_str(&kind.marker(literals.len()));
            }
            None => text.push_str(token),
        }
        end = placed.range.end;
    }
    Masked { text: text.trim_matches(lex::is_blank).to_owned(), literals: Literals(literals) }
}

/// The text between two tokens, blank as it is, else a blank in place of the comments it holds.
fn without_comments(gap: &str) -> &str {
    match gap {
        gap if gap.chars().all(lex::is_blank) => gap,
        gap if gap.contains('\n') => "\n",
        _ => " ",
    }
}

/// The kind of literal the token `text` of kind `token` is, if it is one.
fn literal_kind(text: &str, token: &Token) -> Option<LiteralKind> {
    let is_escape_string = *token == Token::Word && text.starts_with(['E', 'e']) && text[1..].starts_with('\'');
    let is_string = *token == Token::Other && (text.starts_with('\'') || (text.starts_with('$') && text.len() > 1));
    match token {
        Token::Number => Some(LiteralKind::Number),
        _ if is_escape_string || is_string => Some(LiteralKind::String),
        _ => None,
    }
}

impl LiteralKind {
    const STRING_MARKER: &str = "<literal ";
    const NUMBER_MARKER: &str = "<number ";

    fn marker(self, number: usize) -> String {
        match self {
            Self::String => format!("'{}{number}>'", Self::STRING_MARKER),
            Self::Number => format!("{}{number}>", Self::NUMBER_MARKER),
        }
    }
}

impl Marker {
    /// The `<literal n>` or `<number n>` that opens `text`, if one does.
    fn at_start(text: &str) -> Option<Self> {
        let (kind, rest) = [(LiteralKind::String, LiteralKind::STRING_MARKER), (LiteralKind::Number, LiteralKind::NUMBER_MARKER)]
            .into_iter()
            .find_map(|(kind, opening)| Some((kind, text.strip_prefix(opening)?)))?;
        let digits = rest.find(|c: char| !c.is_ascii_digit()).filter(|digits| *digits > 0)?;
        let number = rest[..digits].parse().ok();
        rest[digits..].starts_with('>').then_some(Self { kind, number, len: text.len() - rest.len() + digits + 1 })
    }
}

impl Literals {
    /// `text` with each marker it holds replaced by the literal it stands for, the quotes around a string's marker
    /// included; a marker that stands for none, or for a literal of another kind, stops it.
    pub(super) fn put_back(&self, text: &str) -> Result<String, UnknownMarker> {
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find('<') {
            out.push_str(&rest[..at]);
            let tail = &rest[at..];
            let Some(marker) = Marker::at_start(tail) else {
                out.push('<');
                rest = &tail[1..];
                continue;
            };
            let literal = self.literal(&marker).ok_or_else(|| UnknownMarker(tail[..marker.len].to_owned()))?;
            rest = &tail[marker.len..];
            if marker.kind == LiteralKind::String && out.ends_with('\'') && rest.starts_with('\'') {
                out.pop();
                rest = &rest[1..];
            }
            out.push_str(&literal.text);
        }
        Ok(out + rest)
    }

    fn literal(&self, marker: &Marker) -> Option<&Literal> {
        self.0.get(marker.number?.checked_sub(1)?).filter(|literal| literal.kind == marker.kind)
    }
}

/// Literals are what the user typed, perhaps personal data: `Debug` says how many there are, never what they hold.
impl fmt::Debug for Literals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Literals({})", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(sql: &str) -> String {
        masked(sql, LiteralPolicy::Masked).text
    }

    #[test]
    fn a_string_is_a_numbered_marker() {
        assert_eq!(text("SELECT * FROM customers WHERE name = 'nina'"), "SELECT * FROM customers WHERE name = '<literal 1>'");
    }

    #[test]
    fn an_escape_string_is_a_marker_whole() {
        assert_eq!(text(r"SELECT E'nina\'s', e'x'"), "SELECT '<literal 1>', '<literal 2>'");
    }

    #[test]
    fn a_dollar_quoted_string_is_a_marker_whole() {
        assert_eq!(text("SELECT $$nina$$, $tag$ 'x' $tag$"), "SELECT '<literal 1>', '<literal 2>'");
    }

    #[test]
    fn an_unclosed_string_is_a_marker_to_the_end() {
        assert_eq!(text("SELECT 'nina"), "SELECT '<literal 1>'");
    }

    #[test]
    fn each_kind_of_number_is_one_marker() {
        assert_eq!(
            text("SELECT 42, 1.5, .5, 1e-3, 2E+4, 0x1F, 1_000, 7::text"),
            "SELECT <number 1>, <number 2>, <number 3>, <number 4>, <number 5>, <number 6>, <number 7>, <number 8>::text"
        );
    }

    #[test]
    fn strings_and_numbers_share_one_count() {
        assert_eq!(text("SELECT 'a', 2"), "SELECT '<literal 1>', <number 2>");
    }

    #[test]
    fn digits_inside_a_name_or_a_numbered_parameter_stay() {
        assert_eq!(text("SELECT col1 FROM t2 WHERE id = $1"), "SELECT col1 FROM t2 WHERE id = $1");
    }

    #[test]
    fn a_param_and_a_quoted_name_stay() {
        assert_eq!(text("SELECT \"Total\" FROM orders WHERE day > {{since}}"), "SELECT \"Total\" FROM orders WHERE day > {{since}}");
    }

    #[test]
    fn comments_are_stripped_nested_ones_whole() {
        assert_eq!(text("-- nina\nSELECT /* a /* 'b' */ c */ 1 -- 2\nFROM orders /* open"), "SELECT <number 1>\nFROM orders");
    }

    #[test]
    fn literals_sent_stay_as_typed_and_comments_still_go() {
        let sent = masked("SELECT 'nina', 42 -- big\nFROM orders", LiteralPolicy::Sent);
        assert_eq!((sent.text.as_str(), format!("{:?}", sent.literals)), ("SELECT 'nina', 42\nFROM orders", "Literals(0)".to_owned()));
    }

    #[test]
    fn what_follows_a_number_stays() {
        assert_eq!(text("SELECT 1::int, 1.2.3"), "SELECT <number 1>::int, <number 2>.<number 3>");
    }

    #[test]
    fn a_comment_between_two_words_leaves_a_blank() {
        assert_eq!(text("SELECT/**/id"), "SELECT id");
    }

    fn put_back(sql: &str, answer: &str) -> Result<String, UnknownMarker> {
        masked(sql, LiteralPolicy::Masked).literals.put_back(answer)
    }

    #[test]
    fn markers_are_put_back_as_typed() {
        let sql = "SELECT * FROM orders WHERE note = E'a\\'b' AND total > 1.5";
        let answer = "SELECT id FROM orders WHERE total > <number 2> AND note = '<literal 1>'";
        assert_eq!(put_back(sql, answer).unwrap(), "SELECT id FROM orders WHERE total > 1.5 AND note = E'a\\'b'");
    }

    #[test]
    fn a_marker_with_no_literal_stops_the_answer() {
        assert_eq!(put_back("SELECT 'a'", "SELECT '<literal 2>'"), Err(UnknownMarker("<literal 2>".into())));
        assert_eq!(put_back("SELECT 'a'", "SELECT <number 0>"), Err(UnknownMarker("<number 0>".into())));
    }

    #[test]
    fn a_marker_past_any_count_stops_the_answer() {
        let marker = "<literal 99999999999999999999999>";
        assert_eq!(put_back("SELECT 'a'", &format!("SELECT '{marker}'")), Err(UnknownMarker(marker.into())));
    }

    #[test]
    fn a_marker_of_another_kind_stops_the_answer() {
        assert_eq!(put_back("SELECT 'a'", "SELECT <number 1>"), Err(UnknownMarker("<number 1>".into())));
    }

    #[test]
    fn text_that_only_looks_like_a_marker_stays() {
        assert_eq!(put_back("SELECT 1", "SELECT a < b, '<literal x>'").unwrap(), "SELECT a < b, '<literal x>'");
    }

    #[test]
    fn debug_never_shows_a_literal() {
        assert_eq!(format!("{:?}", masked("SELECT 'nina'", LiteralPolicy::Masked).literals), "Literals(1)");
    }
}
