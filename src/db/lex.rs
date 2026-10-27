//! Splits SQL text into tokens, each with its bytes, and says what kind of text a byte falls in.
//! The guard, the parameters and completion all read SQL through it.
use std::ops::Range;

/// One token of SQL text and the bytes it spans.
#[derive(Debug)]
pub(crate) struct Placed {
    pub(crate) range: Range<usize>,
    pub(crate) token: Token,
}

/// The kinds of token: comments and whitespace are dropped, and what string literals and quoted names hold stays
/// hidden.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Token {
    /// A word; like PostgreSQL, pgbg compares words without regard to ASCII case.
    Word,
    /// A `;`.
    End,
    /// `{{name}}`, holding the name.
    Param(String),
    /// A `{{` that opens no `{{name}}`, or a `{{name}}` glued to a word, which would join that word once replaced by `$n`.
    Unreadable,
    /// `$1`, `$2`: a parameter the statement numbers itself.
    Numbered(u32),
    /// A number as PostgreSQL reads one: `42`, `1.5`, `.5`, `1e-3`, `0x1F`, `1_000`.
    Number,
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

/// SQL text read once: its tokens, and the lexemes that are not code, to say what kind of text a byte falls in.
#[derive(Debug, Default)]
pub(crate) struct Lexed {
    tokens: Vec<Placed>,
    not_code: Vec<SpanRun>,
}

/// A lexeme that is not code: its bytes, its kind of text, and whether text typed at its end still falls in it.
#[derive(Debug)]
struct SpanRun {
    range: Range<usize>,
    span: Span,
    is_open: bool,
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

impl Placed {
    /// The text of this token when it is a word, taken from the `sql` it was read from.
    pub(crate) fn word<'a>(&self, sql: &'a str) -> Option<&'a str> {
        (self.token == Token::Word).then(|| &sql[self.range.clone()])
    }
}

/// Whether `word` is one of `words`, ASCII case aside.
pub(crate) fn is_one_of(word: &str, words: &[&str]) -> bool {
    words.iter().any(|known| known.eq_ignore_ascii_case(word))
}

impl Lexed {
    pub(crate) fn of(sql: &str) -> Self {
        let mut lexed = Self::default();
        for lexeme in lexemes(sql) {
            let span = lexeme.span(&sql[lexeme.range.clone()]);
            if span != Span::Code {
                lexed.not_code.push(SpanRun { range: lexeme.range.clone(), span, is_open: lexeme.is_open });
            }
            if let Kind::Token(token) = lexeme.kind {
                lexed.tokens.push(Placed { range: lexeme.range, token });
            }
        }
        lexed
    }

    pub(crate) fn tokens(&self) -> &[Placed] {
        &self.tokens
    }

    /// The tokens within the bytes `range`, which starts and ends between tokens.
    pub(crate) fn tokens_in(&self, range: Range<usize>) -> &[Placed] {
        let start = self.tokens.partition_point(|placed| placed.range.start < range.start);
        let end = self.tokens.partition_point(|placed| placed.range.end <= range.end);
        &self.tokens[start..end.max(start)]
    }

    /// What kind of text a character typed at byte `at` would fall in.
    pub(crate) fn span_at(&self, at: usize) -> Span {
        let before = self.not_code.partition_point(|run| run.range.start < at);
        let holds = |run: &&SpanRun| at < run.range.end || (at == run.range.end && run.is_open);
        self.not_code[..before].last().filter(holds).map_or(Span::Code, |run| run.span)
    }
}

pub(crate) fn tokens(sql: &str) -> Vec<Placed> {
    Lexed::of(sql).tokens
}

/// The lexemes of `sql`; a helper returns `None` when its literal or comment is never closed.
fn lexemes(sql: &str) -> Vec<Lexeme> {
    let mut lexemes = vec![];
    let mut rest = sql;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        let at = sql.len() - rest.len();
        let (kind, next) = match c {
            '-' if after.starts_with('-') => (Kind::LineComment, after.find(['\n', '\r']).map(|end| &after[end..])),
            '/' if after.starts_with('*') => (Kind::Comment, after_block_comment(&after[1..])),
            c if is_blank(c) => (Kind::Blank, Some(after)),
            ';' => (Kind::Token(Token::End), Some(after)),
            c if is_word_start(c) => word(rest),
            c if c.is_ascii_digit() => (Kind::Token(Token::Number), Some(after_number(rest))),
            '.' if opens_fraction(&sql[..at], after) => (Kind::Token(Token::Number), Some(after_number(rest))),
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
            Kind::Token(Token::Word) if is_escape_string => Span::String,
            Kind::Token(Token::Other) if text.starts_with('"') => Span::QuotedName,
            Kind::Token(Token::Other) if text.starts_with('\'') || (text.starts_with('$') && text.len() > 1) => Span::String,
            Kind::Token(_) | Kind::Blank => Span::Code,
        }
    }
}

/// Whether PostgreSQL splits tokens at `c`; any other character outside ASCII, a no-break space included, is part of a word.
pub(crate) fn is_blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\x0b')
}

fn is_word_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
}

fn is_word_char(c: char) -> bool {
    is_word_start(c) || c.is_ascii_digit() || c == '$'
}

/// A word at the start of `rest`, and an `E'...'` literal right after an `E`.
fn word(rest: &str) -> (Kind, Option<&str>) {
    let end = rest.find(|c: char| !is_word_char(c)).unwrap_or(rest.len());
    let next = &rest[end..];
    let is_escape_string = rest[..end].eq_ignore_ascii_case("e") && next.starts_with('\'');
    (Kind::Token(Token::Word), if is_escape_string { after_escape_string(&next[1..]) } else { Some(next) })
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

/// Whether a `.` after `before` and before `after` opens a number such as `.5`, rather than a dot between two names.
fn opens_fraction(before: &str, after: &str) -> bool {
    after.starts_with(|c: char| c.is_ascii_digit()) && !before.ends_with(|c: char| is_word_char(c) || c == '"' || c == '.')
}

/// The text after the number at the start of `rest`; what follows it, `::int` or a second `.`, is left to the next token.
fn after_number(rest: &str) -> &str {
    let radix_digits = ["0x", "0X", "0o", "0O", "0b", "0B"]
        .into_iter()
        .find_map(|prefix| rest.strip_prefix(prefix).filter(|digits| digits.starts_with(|c: char| c.is_ascii_hexdigit())));
    if let Some(digits) = radix_digits {
        return digits.trim_start_matches(|c: char| c.is_ascii_hexdigit() || c == '_');
    }
    let whole = after_digits(rest);
    let fraction = whole.strip_prefix('.').filter(|fraction| !fraction.starts_with('.')).map_or(whole, after_digits);
    let exponent = fraction.strip_prefix(['e', 'E']).map(|exponent| exponent.strip_prefix(['+', '-']).unwrap_or(exponent));
    exponent.filter(|digits| digits.starts_with(|c: char| c.is_ascii_digit())).map_or(fraction, after_digits)
}

fn after_digits(text: &str) -> &str {
    text.trim_start_matches(|c: char| c.is_ascii_digit() || c == '_')
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

/// The text after a literal opened by `quote`, where a doubled quote stands for itself.
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

/// The text after an `E'...'` literal, where a backslash also escapes the next character.
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

/// The text after a `$tag$...$tag$` literal; a `$` that opens none is skipped alone.
fn after_dollar(rest: &str) -> Option<&str> {
    let tag_end = rest[1..].find(|c: char| !(is_word_start(c) || c.is_ascii_digit())).map(|at| at + 1);
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

    #[test]
    fn a_non_ascii_character_is_part_of_a_word() {
        assert_eq!(params("SELECT a\u{a0}{{since}}"), [Token::Unreadable]);
        assert_eq!(params("SELECT {{since}}\u{1f980}"), [Token::Unreadable]);
        assert_eq!(words("SELECT\u{a0}\u{1f980}x"), ["SELECT\u{a0}\u{1f980}x"]);
    }

    #[test]
    fn only_ascii_blanks_part_words() {
        assert_eq!(words("a \t\n\r\x0c\x0bb"), ["a", "b"]);
    }

    fn words(sql: &str) -> Vec<&str> {
        tokens(sql).iter().filter_map(|placed| placed.word(sql)).collect()
    }

    #[test]
    fn a_word_is_one_of_a_list_whatever_its_ascii_case() {
        assert!(is_one_of("Select", &["SELECT"]));
        assert!(!is_one_of("ſelect", &["SELECT"]));
    }

    /// The span a character typed at the `|` of `marked` would fall in.
    fn span(marked: &str) -> Span {
        Lexed::of(&marked.replace('|', "")).span_at(marked.find('|').unwrap())
    }

    #[test]
    fn the_tokens_within_a_range_are_those_it_holds_whole() {
        let lexed = Lexed::of("SELECT 1; SELECT 2");
        let ranges: Vec<Range<usize>> = lexed.tokens_in(10..18).iter().map(|placed| placed.range.clone()).collect();
        assert_eq!(ranges, [10..16, 17..18]);
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

    fn numbers(sql: &str) -> Vec<&str> {
        tokens(sql).into_iter().filter(|placed| placed.token == Token::Number).map(|placed| &sql[placed.range]).collect()
    }

    #[test]
    fn an_integer_is_one_number() {
        assert_eq!(numbers("SELECT 42"), ["42"]);
    }

    #[test]
    fn a_decimal_is_one_number() {
        assert_eq!(numbers("SELECT 1.5, 1."), ["1.5", "1."]);
    }

    #[test]
    fn a_fraction_without_its_integer_part_is_one_number() {
        assert_eq!(numbers("SELECT .5"), [".5"]);
    }

    #[test]
    fn an_exponent_belongs_to_its_number() {
        assert_eq!(numbers("SELECT 1e-3, 2E+4, 2.5e3"), ["1e-3", "2E+4", "2.5e3"]);
    }

    #[test]
    fn a_hexadecimal_octal_or_binary_integer_is_one_number() {
        assert_eq!(numbers("SELECT 0x1F, 0o17, 0b101"), ["0x1F", "0o17", "0b101"]);
    }

    #[test]
    fn underscores_between_digits_belong_to_the_number() {
        assert_eq!(numbers("SELECT 1_000"), ["1_000"]);
    }

    #[test]
    fn what_follows_a_number_is_a_token_of_its_own() {
        assert_eq!(numbers("SELECT 1::int, 5e, 1..10"), ["1", "5", "1", "10"]);
        assert_eq!(words("SELECT 1::int, 5e"), ["SELECT", "int", "e"]);
    }

    #[test]
    fn a_dot_between_names_or_after_a_number_opens_no_number() {
        assert_eq!(numbers("SELECT t.5, \"t\".5, 1.2.3"), ["5", "5", "1.2", "3"]);
    }

    #[test]
    fn digits_inside_a_word_or_a_numbered_parameter_are_no_number() {
        assert_eq!(numbers("SELECT col1, $1"), Vec::<&str>::new());
    }

    #[test]
    fn comments_and_whitespace_leave_no_token() {
        let sql = "-- note\n/* a /* b */ */ select";
        assert_eq!(tokens(sql).into_iter().map(|placed| placed.token).collect::<Vec<_>>(), [Token::Word]);
        assert_eq!(words(sql), ["select"]);
    }
}
