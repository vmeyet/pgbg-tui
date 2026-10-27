//! The tokens of SQL text, each with its place, and what kind of text a byte falls in: the guard, the parameters and
//! completion read them.
use std::ops::Range;

/// One token of SQL text and the bytes it spans.
#[derive(Debug)]
pub(crate) struct Placed {
    pub(crate) range: Range<usize>,
    pub(crate) token: Token,
}

/// What the tokenizer tells apart: comments and whitespace drop out, and string literals and quoted identifiers hide
/// what they hold.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Token {
    /// A word, in upper case.
    Word(String),
    /// A `;`.
    End,
    /// `{{name}}`, holding the name.
    Param(String),
    /// A `{{` that opens no `{{name}}`, or one glued to a word, which would read as part of it once replaced by `$n`.
    Unreadable,
    /// `$1`, `$2`: a parameter the statement numbers itself.
    Numbered(u32),
    Other,
}

/// What kind of text a byte of SQL falls in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Span {
    Code,
    /// A string literal: quoted, `E'...'` or `$tag$...$tag$`.
    String,
    Comment,
    QuotedName,
    /// A `{{name}}`, or what an unreadable `{{` opens.
    Param,
}

/// A token, a comment or a run of blanks, and whether text typed at its end would still fall in it: the end of a line
/// comment, or of a string, a comment or a `{{` never closed.
struct Lexeme {
    range: Range<usize>,
    kind: Kind,
    is_open: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Token(Token),
    Comment,
    LineComment,
    Blank,
}

impl Token {
    pub(crate) fn word(&self) -> Option<&str> {
        match self {
            Self::Word(word) => Some(word),
            Self::End | Self::Param(_) | Self::Unreadable | Self::Numbered(_) | Self::Other => None,
        }
    }
}

/// Every token of `sql`, in order.
pub(crate) fn tokens(sql: &str) -> Vec<Placed> {
    let placed = |lexeme: Lexeme| match lexeme.kind {
        Kind::Token(token) => Some(Placed { range: lexeme.range, token }),
        Kind::Comment | Kind::LineComment | Kind::Blank => None,
    };
    lexemes(sql).into_iter().filter_map(placed).collect()
}

/// What kind of text a character typed at byte `at` of `sql` would fall in.
pub(crate) fn span_at(sql: &str, at: usize) -> Span {
    let holds = |lexeme: &&Lexeme| lexeme.range.start < at && (at < lexeme.range.end || (at == lexeme.range.end && lexeme.is_open));
    lexemes(sql).iter().find(holds).map_or(Span::Code, |lexeme| lexeme.span(&sql[lexeme.range.clone()]))
}

/// The lexemes of `sql`; a helper's `None` says its literal or comment runs to the end unclosed.
fn lexemes(sql: &str) -> Vec<Lexeme> {
    let mut lexemes = vec![];
    let mut rest = sql;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        let at = sql.len() - rest.len();
        let (kind, next) = match c {
            '-' if after.starts_with('-') => (Kind::LineComment, after.find(['\n', '\r']).map(|end| &after[end..])),
            '/' if after.starts_with('*') => (Kind::Comment, after_block_comment(&after[1..])),
            c if c.is_whitespace() => (Kind::Blank, Some(after)),
            ';' => (Kind::Token(Token::End), Some(after)),
            c if c.is_alphabetic() || c == '_' => word(rest),
            '\'' | '"' => (Kind::Token(Token::Other), after_quoted(after, c)),
            '$' => dollar(rest),
            '{' if after.starts_with('{') => param(sql, at),
            _ => (Kind::Token(Token::Other), Some(after)),
        };
        let is_open = next.is_none() || kind == Kind::LineComment;
        let next = next.unwrap_or("");
        lexemes.push(Lexeme { range: at..sql.len() - next.len(), kind, is_open });
        rest = next;
    }
    lexemes
}

impl Lexeme {
    fn span(&self, text: &str) -> Span {
        let is_escape_string = text.starts_with(['E', 'e']) && text[1..].starts_with('\'');
        match &self.kind {
            Kind::Comment | Kind::LineComment => Span::Comment,
            Kind::Token(Token::Param(_) | Token::Unreadable) => Span::Param,
            Kind::Token(Token::Word(_)) if is_escape_string => Span::String,
            Kind::Token(Token::Other) if text.starts_with('"') => Span::QuotedName,
            Kind::Token(Token::Other) if text.starts_with('\'') || (text.starts_with('$') && text.len() > 1) => Span::String,
            Kind::Token(_) | Kind::Blank => Span::Code,
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// A word at the start of `rest`, and an `E'...'` literal right after an `E`.
fn word(rest: &str) -> (Kind, Option<&str>) {
    let end = rest.find(|c: char| !is_word_char(c)).unwrap_or(rest.len());
    let word = rest[..end].to_uppercase();
    let next = &rest[end..];
    let is_escape_string = word == "E" && next.starts_with('\'');
    (Kind::Token(Token::Word(word)), if is_escape_string { after_escape_string(&next[1..]) } else { Some(next) })
}

/// `{{name}}` at byte `at` of `sql`, spaces allowed around the name; anything else from `{{` on is unreadable.
fn param(sql: &str, at: usize) -> (Kind, Option<&str>) {
    let rest = &sql[at + 2..];
    let Some(close) = rest.find("}}") else { return (Kind::Token(Token::Unreadable), None) };
    let name = rest[..close].trim_matches(' ');
    let next = &rest[close + 2..];
    let is_glued = sql[..at].ends_with(is_word_char) || next.starts_with(is_word_char);
    let token = if is_param_name(name) && !is_glued { Token::Param(name.to_owned()) } else { Token::Unreadable };
    (Kind::Token(token), Some(next))
}

/// Whether `name` can name a parameter: a letter or `_`, then letters, digits or `_`.
pub(crate) fn is_param_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts_well = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    starts_well && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn after_block_comment(body: &str) -> Option<&str> {
    let mut depth = 1;
    let mut rest = body;
    while depth > 0 {
        let at = rest.find(['*', '/'])?;
        let mark = &rest[at..];
        rest = if let Some(after) = mark.strip_prefix("*/") {
            depth -= 1;
            after
        } else if let Some(after) = mark.strip_prefix("/*") {
            depth += 1;
            after
        } else {
            &mark[1..]
        };
    }
    Some(rest)
}

/// After a literal opened by `quote`, where a doubled quote stands for itself.
fn after_quoted(body: &str, quote: char) -> Option<&str> {
    let mut rest = body;
    while let Some(at) = rest.find(quote) {
        let after = &rest[at + 1..];
        if !after.starts_with(quote) {
            return Some(after);
        }
        rest = &after[1..];
    }
    None
}

/// After an `E'...'` literal, where a backslash also escapes the next character.
fn after_escape_string(body: &str) -> Option<&str> {
    let mut chars = body.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '\'' if body[at + 1..].starts_with('\'') => {
                chars.next();
            }
            '\'' => return Some(&body[at + 1..]),
            _ => {}
        }
    }
    None
}

/// The parameter `$1`, a `$tag$...$tag$` literal, or a lone `$`, at the start of `rest`.
fn dollar(rest: &str) -> (Kind, Option<&str>) {
    let digits_end = rest[1..].find(|c: char| !c.is_ascii_digit()).map_or(rest.len(), |at| at + 1);
    if digits_end > 1 {
        let number = rest[1..digits_end].parse().unwrap_or(u32::MAX);
        return (Kind::Token(Token::Numbered(number)), Some(&rest[digits_end..]));
    }
    (Kind::Token(Token::Other), after_dollar(rest))
}

/// After a `$tag$...$tag$` literal; a `$` that opens none is skipped alone.
fn after_dollar(rest: &str) -> Option<&str> {
    let tag_end = rest[1..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map(|at| at + 1);
    match tag_end {
        Some(end) if rest[end..].starts_with('$') => {
            let tag = &rest[..=end];
            let body = &rest[tag.len()..];
            body.find(tag).map(|at| &body[at + tag.len()..])
        }
        _ => Some(&rest[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_token_spans_its_bytes() {
        let spans: Vec<Range<usize>> = tokens("SELECT 'a;b' ;").into_iter().map(|placed| placed.range).collect();
        assert_eq!(spans, [0..6, 7..12, 13..14]);
    }

    fn numbered(sql: &str) -> Vec<Token> {
        tokens(sql).into_iter().map(|placed| placed.token).filter(|token| matches!(token, Token::Numbered(_))).collect()
    }

    #[test]
    fn a_numbered_parameter_holds_its_number() {
        assert_eq!(numbered("SELECT $1, $12"), [Token::Numbered(1), Token::Numbered(12)]);
    }

    #[test]
    fn a_dollar_quote_or_a_lone_dollar_is_no_numbered_parameter() {
        assert_eq!(numbered("SELECT $tag$ $1 $tag$, '$2', $"), []);
    }

    fn params(sql: &str) -> Vec<Token> {
        tokens(sql).into_iter().map(|placed| placed.token).filter(|token| matches!(token, Token::Param(_) | Token::Unreadable)).collect()
    }

    #[test]
    fn a_param_holds_its_name_and_spans_its_braces() {
        let placed = tokens("SELECT * FROM orders WHERE day > {{since}}");
        let last = placed.last().unwrap();
        assert_eq!((&last.token, last.range.clone()), (&Token::Param("since".into()), 33..42));
    }

    #[test]
    fn spaces_around_a_param_name_are_dropped() {
        assert_eq!(params("SELECT {{ since }}"), [Token::Param("since".into())]);
    }

    #[test]
    fn a_param_inside_a_string_a_comment_or_a_quoted_name_is_no_param() {
        assert_eq!(params("SELECT '{{since}}', \"{{owner}}\", $$ {{a}} $$ -- {{b}}\n/* {{c}} */"), []);
    }

    #[test]
    fn a_brace_pair_that_holds_no_name_is_unreadable() {
        assert_eq!(params("SELECT {{1st}}"), [Token::Unreadable]);
        assert_eq!(params("SELECT {{since"), [Token::Unreadable]);
        assert_eq!(params("SELECT {{owner name}}"), [Token::Unreadable]);
    }

    #[test]
    fn a_param_glued_to_a_word_is_unreadable() {
        assert_eq!(params("SELECT x{{since}}"), [Token::Unreadable]);
        assert_eq!(params("SELECT {{since}}e"), [Token::Unreadable]);
        assert_eq!(params("SELECT {{since}}1"), [Token::Unreadable]);
    }

    /// The span a character typed at the `|` of `marked` would fall in.
    fn span(marked: &str) -> Span {
        span_at(&marked.replace('|', ""), marked.find('|').unwrap())
    }

    #[test]
    fn a_byte_inside_a_literal_falls_in_its_kind() {
        assert_eq!(span("SELECT 'a|b'"), Span::String);
        assert_eq!(span("SELECT \"Or|ders\""), Span::QuotedName);
        assert_eq!(span("SELECT /* a| */ 1"), Span::Comment);
        assert_eq!(span("SELECT {{si|nce}}"), Span::Param);
        assert_eq!(span("SELECT $$ a| $$"), Span::String);
    }

    #[test]
    fn the_end_of_a_closed_literal_is_code_and_of_an_open_one_is_not() {
        assert_eq!(span("SELECT 'ab'|"), Span::Code);
        assert_eq!(span("SELECT 'ab|"), Span::String);
        assert_eq!(span("SELECT E'a\\'|"), Span::String);
        assert_eq!(span("SELECT /* a */|"), Span::Code);
        assert_eq!(span("SELECT /* a|"), Span::Comment);
        assert_eq!(span("SELECT {{na|"), Span::Param);
    }

    #[test]
    fn the_end_of_a_line_comment_is_in_the_comment() {
        assert_eq!(span("SELECT 1 -- note|\nFROM orders"), Span::Comment);
        assert_eq!(span("SELECT 1 -- note\n|FROM orders"), Span::Code);
    }

    #[test]
    fn words_numbers_and_blanks_are_code() {
        assert_eq!(span("SELECT or|"), Span::Code);
        assert_eq!(span("SELECT $1|"), Span::Code);
        assert_eq!(span("SELECT |"), Span::Code);
    }

    #[test]
    fn comments_and_whitespace_leave_no_token() {
        let words: Vec<Token> = tokens("-- note\n/* a /* b */ */ select").into_iter().map(|placed| placed.token).collect();
        assert_eq!(words, [Token::Word("SELECT".into())]);
    }
}
