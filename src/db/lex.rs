//! The tokens of SQL text, each with its place: the guard and the parameters read them, and completion will.
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
    let mut tokens = vec![];
    let mut rest = sql;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        let at = sql.len() - rest.len();
        let (token, next) = match c {
            '-' if after.starts_with('-') => (None, after.find(['\n', '\r']).map_or("", |end| &after[end..])),
            '/' if after.starts_with('*') => (None, after_block_comment(&after[1..])),
            c if c.is_whitespace() => (None, after),
            ';' => (Some(Token::End), after),
            c if c.is_alphabetic() || c == '_' => word(rest),
            '\'' | '"' => (Some(Token::Other), after_quoted(after, c)),
            '$' => dollar(rest),
            '{' if after.starts_with('{') => param(sql, at),
            _ => (Some(Token::Other), after),
        };
        if let Some(token) = token {
            tokens.push(Placed { range: at..sql.len() - next.len(), token });
        }
        rest = next;
    }
    tokens
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// A word at the start of `rest`, and an `E'...'` literal right after an `E`.
fn word(rest: &str) -> (Option<Token>, &str) {
    let end = rest.find(|c: char| !is_word_char(c)).unwrap_or(rest.len());
    let word = rest[..end].to_uppercase();
    let next = &rest[end..];
    let is_escape_string = word == "E" && next.starts_with('\'');
    (Some(Token::Word(word)), if is_escape_string { after_escape_string(&next[1..]) } else { next })
}

/// `{{name}}` at byte `at` of `sql`, spaces allowed around the name; anything else from `{{` on is unreadable.
fn param(sql: &str, at: usize) -> (Option<Token>, &str) {
    let rest = &sql[at + 2..];
    let Some(close) = rest.find("}}") else { return (Some(Token::Unreadable), "") };
    let name = rest[..close].trim_matches(' ');
    let next = &rest[close + 2..];
    let is_glued = sql[..at].ends_with(is_word_char) || next.starts_with(is_word_char);
    let token = if is_param_name(name) && !is_glued { Token::Param(name.to_owned()) } else { Token::Unreadable };
    (Some(token), next)
}

/// Whether `name` can name a parameter: a letter or `_`, then letters, digits or `_`.
pub(crate) fn is_param_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts_well = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    starts_well && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn after_block_comment(body: &str) -> &str {
    let mut depth = 1;
    let mut rest = body;
    while depth > 0 {
        let Some(at) = rest.find(['*', '/']) else { return "" };
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
    rest
}

/// After a literal opened by `quote`, where a doubled quote stands for itself.
fn after_quoted(body: &str, quote: char) -> &str {
    let mut rest = body;
    while let Some(at) = rest.find(quote) {
        let after = &rest[at + 1..];
        if !after.starts_with(quote) {
            return after;
        }
        rest = &after[1..];
    }
    ""
}

/// After an `E'...'` literal, where a backslash also escapes the next character.
fn after_escape_string(body: &str) -> &str {
    let mut chars = body.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '\'' if body[at + 1..].starts_with('\'') => {
                chars.next();
            }
            '\'' => return &body[at + 1..],
            _ => {}
        }
    }
    ""
}

/// The parameter `$1`, a `$tag$...$tag$` literal, or a lone `$`, at the start of `rest`.
fn dollar(rest: &str) -> (Option<Token>, &str) {
    let digits_end = rest[1..].find(|c: char| !c.is_ascii_digit()).map_or(rest.len(), |at| at + 1);
    if digits_end > 1 {
        let number = rest[1..digits_end].parse().unwrap_or(u32::MAX);
        return (Some(Token::Numbered(number)), &rest[digits_end..]);
    }
    (Some(Token::Other), after_dollar(rest))
}

/// After a `$tag$...$tag$` literal; a `$` that opens none is skipped alone.
fn after_dollar(rest: &str) -> &str {
    let tag_end = rest[1..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map(|at| at + 1);
    match tag_end {
        Some(end) if rest[end..].starts_with('$') => {
            let tag = &rest[..=end];
            let body = &rest[tag.len()..];
            body.find(tag).map_or("", |at| &body[at + tag.len()..])
        }
        _ => &rest[1..],
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

    #[test]
    fn comments_and_whitespace_leave_no_token() {
        let words: Vec<Token> = tokens("-- note\n/* a /* b */ */ select").into_iter().map(|placed| placed.token).collect();
        assert_eq!(words, [Token::Word("SELECT".into())]);
    }
}
