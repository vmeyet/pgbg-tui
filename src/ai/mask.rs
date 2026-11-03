//! The statement as a prompt shows it: each string and number literal, and each quoted name the catalog does not hold, a
//! numbered marker, its comments gone; and the comment an idle ask reads as its instruction, its quoted spans and values
//! masked the same way. A comment is English, not SQL, so it goes through a few rules of its own rather than the lexer.
use crate::db::lex::{self, Placed, Token};
use crate::db::model::{Catalog, sql_string};
use serde::Deserialize;
use std::collections::HashSet;
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
#[derive(PartialEq, Eq)]
pub(super) struct Literals(Vec<Literal>);

#[derive(PartialEq, Eq)]
struct Literal {
    kind: LiteralKind,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LiteralKind {
    String,
    Number,
    /// A quoted name the catalog does not hold: an alias, or a value quoted the wrong way, as `"nina@example.com"`.
    Name,
}

/// A statement or a comment with its literals masked and its comments stripped, and the literals it left out.
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

/// The text masked so far and the literals it left out.
struct Masking {
    text: String,
    literals: Vec<Literal>,
}

/// Each quote a comment may open and the one that closes it.
const QUOTES: [(char, char); 5] = [('\'', '\''), ('"', '"'), ('`', '`'), ('“', '”'), ('‘', '’')];
const APOSTROPHES: [char; 2] = ['\'', '’'];
const PARAM_OPENING: &str = "{{";

/// `sql` with each comment a blank and, unless `policy` sends them, each string and number literal and each quoted name
/// `catalog` does not hold a marker; a `{{name}}` stays as it is.
pub(super) fn masked(sql: &str, policy: LiteralPolicy, catalog: &Catalog) -> Masked {
    let tokens = lex::tokens(sql);
    let held = match policy {
        LiteralPolicy::Masked => catalog.held_among(&quoted_names(sql, &tokens)),
        LiteralPolicy::Sent => HashSet::new(),
    };
    let mut masking = Masking::after(Literals(vec![]));
    let mut end = 0;
    for placed in &tokens {
        masking.keep(without_comments(&sql[end..placed.range.start]));
        let token = &sql[placed.range.clone()];
        match literal_kind(token, &placed.token, &held).filter(|_| policy == LiteralPolicy::Masked) {
            Some(kind) => masking.mask(kind, token.to_owned()),
            None => masking.keep(token),
        }
        end = placed.range.end;
    }
    masking.finish()
}

/// Each name the `tokens` of `sql` quote, once.
fn quoted_names(sql: &str, tokens: &[Placed]) -> HashSet<String> {
    tokens.iter().filter_map(|placed| lex::unquoted_name(&sql[placed.range.clone()])).collect()
}

/// The character, counted from 1, of the masked `sql` that byte `at` of `sql` becomes: masking moves every character after
/// the first comment or literal, so the server's own count would point elsewhere; the `;` keeps the blanks before it.
pub(super) fn masked_position(sql: &str, at: usize, policy: LiteralPolicy, catalog: &Catalog) -> Option<usize> {
    let before = sql.get(..at)?;
    Some(masked(&format!("{before};"), policy, catalog).text.chars().count())
}

/// `comment`, the words of the `--` lines above a statement, with what each pair of quotes holds, each word holding an
/// `@` and each number a marker; its markers count on from the `before` literals, so one answer puts back the values of
/// both; none when one of its quotes is not an apostrophe inside a word nor part of a closed, non-empty pair with no
/// letter on its outside, when a `{{` opens no parameter, or when a word may hide a quoted value.
pub(super) fn masked_comment(comment: &str, before: Literals) -> Option<Masked> {
    let mut masking = Masking::after(before);
    let mut end = 0;
    while let Some(at) = next_opening(comment, end) {
        masking.mask_words(plain_words(&comment[end..at])?);
        end = masking.mask_opening(comment, at)?;
    }
    masking.mask_words(plain_words(&comment[end..])?);
    Some(masking.finish())
}

/// The byte, from `from` on, of the next `{{` or of the next quote that is no apostrophe inside a word.
fn next_opening(comment: &str, from: usize) -> Option<usize> {
    positions(comment, from).find(|&at| {
        let is_quote = comment[at..].starts_with(|c| QUOTES.iter().any(|&(open, close)| c == open || c == close));
        comment[at..].starts_with(PARAM_OPENING) || (is_quote && !is_inside_a_word(comment, at))
    })
}

/// The byte of each character of `text`, from `from` on.
fn positions(text: &str, from: usize) -> impl Iterator<Item = usize> + '_ {
    text[from..].char_indices().map(move |(offset, _)| from + offset)
}

/// Whether the character at byte `at` of `comment` is an apostrophe with a letter or a digit right on both sides, as in
/// `what's`.
fn is_inside_a_word(comment: &str, at: usize) -> bool {
    let Some(apostrophe) = comment[at..].chars().next().filter(|c| APOSTROPHES.contains(c)) else { return false };
    comment[..at].ends_with(is_of_a_word) && comment[at + apostrophe.len_utf8()..].starts_with(is_of_a_word)
}

fn is_of_a_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// What the quote opening at byte `at` of `comment` holds and the byte after its closing quote; none when no letter may
/// touch it and one does, when it holds nothing, or when it never closes.
fn quoted_at(comment: &str, at: usize) -> Option<(&str, usize)> {
    let opening = comment[at..].chars().next()?;
    let (_, closing) = QUOTES.into_iter().find(|&(open, _)| open == opening)?;
    let start = at + opening.len_utf8();
    let close = positions(comment, start).find(|&close| comment[close..].starts_with(closing) && !is_inside_a_word(comment, close))?;
    let end = close + closing.len_utf8();
    let is_apart = !comment[..at].ends_with(is_of_a_word) && !comment[end..].starts_with(is_of_a_word);
    (is_apart && close > start).then_some((&comment[start..close], end))
}

/// `words` when no word of them may hide a value: none when one holds two apostrophes, as `x'nina'y`, or a dollar quote,
/// as `$$nina$$`.
fn plain_words(words: &str) -> Option<&str> {
    let has_two_apostrophes =
        words.split(|c: char| !is_of_a_word(c) && !APOSTROPHES.contains(&c)).any(|word| word.matches(APOSTROPHES).count() > 1);
    let has_dollar_quote = words.split(lex::is_blank).any(|word| word.matches('$').count() > 1);
    (!has_two_apostrophes && !has_dollar_quote).then_some(words)
}

/// Whether a number opens at byte `at` of `text`: a digit that does not go on from a word, as `col1` does.
fn opens_a_number(text: &str, at: usize) -> bool {
    text[at..].starts_with(|c: char| c.is_ascii_digit()) && !text[..at].ends_with(is_of_a_word)
}

impl Masking {
    fn after(Literals(literals): Literals) -> Self {
        Self { text: String::new(), literals }
    }

    fn keep(&mut self, text: &str) {
        self.text.push_str(text);
    }

    /// Adds the marker of `text`; a name masked again keeps its number, since it stays one name for the model to read.
    fn mask(&mut self, kind: LiteralKind, text: String) {
        let same_name = self.literals.iter().position(|literal| kind == LiteralKind::Name && literal.kind == kind && literal.text == text);
        let number = same_name.unwrap_or_else(|| {
            self.literals.push(Literal { kind, text });
            self.literals.len() - 1
        }) + 1;
        self.text.push_str(&kind.marker(number));
    }

    /// Keeps the `{{name}}` or masks the quote opening at byte `at` of `comment`, and gives the byte after it.
    fn mask_opening(&mut self, comment: &str, at: usize) -> Option<usize> {
        if comment[at..].starts_with(PARAM_OPENING) {
            let end = lex::param_end(comment, at)?;
            self.keep(&comment[at..end]);
            return Some(end);
        }
        let (value, end) = quoted_at(comment, at)?;
        self.mask(LiteralKind::String, sql_string(value));
        Some(end)
    }

    fn mask_words(&mut self, words: &str) {
        words
            .split_inclusive(lex::is_blank)
            .for_each(|word| if word.contains('@') { self.mask_email(word) } else { self.mask_numbers(word) });
    }

    /// `word` with all but the punctuation around it a string marker.
    fn mask_email(&mut self, word: &str) {
        let is_around = |c: char| lex::is_blank(c) || (c.is_ascii_punctuation() && c != '@');
        let from_value = word.trim_start_matches(is_around);
        let value = from_value.trim_end_matches(is_around);
        self.keep(&word[..word.len() - from_value.len()]);
        self.mask(LiteralKind::String, sql_string(value));
        self.keep(&from_value[value.len()..]);
    }

    /// `text` with each number a marker, along with the letters stuck to its end, as `2nd`.
    fn mask_numbers(&mut self, text: &str) {
        let mut end = 0;
        while let Some(start) = positions(text, end).find(|&at| opens_a_number(text, at)) {
            let after = lex::after_number(&text[start..]).trim_start_matches(is_of_a_word);
            self.keep(&text[end..start]);
            end = text.len() - after.len();
            self.mask(LiteralKind::Number, text[start..end].to_owned());
        }
        self.keep(&text[end..]);
    }

    fn finish(self) -> Masked {
        Masked { text: self.text.trim_matches(lex::is_blank).to_owned(), literals: Literals(self.literals) }
    }
}

/// The text between two tokens, blank as it is, else a blank in place of the comments it holds.
fn without_comments(gap: &str) -> &str {
    match gap {
        gap if gap.chars().all(lex::is_blank) => gap,
        gap if gap.contains('\n') => "\n",
        _ => " ",
    }
}

/// The kind of literal the token `text` of kind `token` is, if it is one; a quoted name is one unless `held` names it.
fn literal_kind(text: &str, token: &Token, held: &HashSet<String>) -> Option<LiteralKind> {
    match token {
        Token::Number => Some(LiteralKind::Number),
        Token::Word if text.starts_with("E'") || text.starts_with("e'") => Some(LiteralKind::String),
        Token::Other if text.starts_with('\'') || (text.starts_with('$') && text.len() > 1) => Some(LiteralKind::String),
        Token::Other if text.starts_with('"') && !lex::unquoted_name(text).is_some_and(|name| held.contains(&name)) => {
            Some(LiteralKind::Name)
        }
        _ => None,
    }
}

impl LiteralKind {
    const ALL: [Self; 3] = [Self::String, Self::Number, Self::Name];

    fn opening(self) -> &'static str {
        match self {
            Self::String => "<literal ",
            Self::Number => "<number ",
            Self::Name => "<name ",
        }
    }

    /// The quote around its marker, which stands for the value's own quotes.
    fn quote(self) -> Option<char> {
        match self {
            Self::String => Some('\''),
            Self::Number => None,
            Self::Name => Some('"'),
        }
    }

    fn marker(self, number: usize) -> String {
        let marker = format!("{}{number}>", self.opening());
        match self.quote() {
            Some(quote) => format!("{quote}{marker}{quote}"),
            None => marker,
        }
    }
}

impl Marker {
    /// The `<literal n>`, `<number n>` or `<name n>` that opens `text`, if one does.
    fn at_start(text: &str) -> Option<Self> {
        let (kind, rest) = LiteralKind::ALL.into_iter().find_map(|kind| Some((kind, text.strip_prefix(kind.opening())?)))?;
        let digits = rest.find(|c: char| !c.is_ascii_digit()).filter(|digits| *digits > 0)?;
        let number = rest[..digits].parse().ok();
        rest[digits..].starts_with('>').then_some(Self { kind, number, len: text.len() - rest.len() + digits + 1 })
    }
}

impl Literals {
    /// `text` with each marker it holds replaced by the literal it stands for, the quotes around its marker included; a
    /// marker that stands for none, or for a literal of another kind, stops it.
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
            if let Some(quote) = marker.kind.quote().filter(|quote| out.ends_with(*quote) && rest.starts_with(*quote)) {
                out.pop();
                rest = &rest[quote.len_utf8()..];
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
    use crate::fixtures::shop_catalog;

    fn text(sql: &str) -> String {
        masked(sql, LiteralPolicy::Masked, &shop_catalog()).text
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
    fn a_param_and_a_quoted_name_of_the_catalog_stay() {
        let sql = "SELECT \"total\" FROM \"public\".\"orders\" WHERE day > {{since}}";
        assert_eq!(text(sql), sql);
    }

    #[test]
    fn a_quoted_name_the_catalog_does_not_hold_is_a_name_marker() {
        assert_eq!(text("SELECT * FROM customers WHERE name = \"nina@example.com\""), "SELECT * FROM customers WHERE name = \"<name 1>\"");
    }

    #[test]
    fn a_quoted_name_differing_by_case_from_the_catalog_is_a_name_marker() {
        assert_eq!(text("SELECT \"Total\" FROM orders"), "SELECT \"<name 1>\" FROM orders");
    }

    #[test]
    fn a_quoted_name_holding_a_quote_is_a_name_marker_whole() {
        assert_eq!(text("SELECT \"ni\"\"na\" FROM orders"), "SELECT \"<name 1>\" FROM orders");
    }

    #[test]
    fn a_catalog_name_between_doubled_quotes_is_another_name() {
        assert_eq!(text("SELECT \"\"\"total\"\"\" FROM orders"), "SELECT \"<name 1>\" FROM orders");
    }

    #[test]
    fn a_name_masked_twice_keeps_its_number() {
        assert_eq!(
            text("SELECT 1 AS \"Total\", 'a', 'a' FROM orders ORDER BY \"Total\""),
            "SELECT <number 1> AS \"<name 2>\", '<literal 3>', '<literal 4>' FROM orders ORDER BY \"<name 2>\""
        );
    }

    #[test]
    fn a_catalog_with_nothing_read_masks_every_quoted_name() {
        assert_eq!(masked("SELECT \"total\"", LiteralPolicy::Masked, &Catalog::default()).text, "SELECT \"<name 1>\"");
    }

    #[test]
    fn a_quoted_name_is_sent_as_typed_with_literals_sent() {
        let sql = "SELECT \"nina@example.com\"";
        assert_eq!(masked(sql, LiteralPolicy::Sent, &Catalog::default()).text, sql);
    }

    #[test]
    fn a_position_counts_in_a_statement_whose_quoted_name_is_masked() {
        let sql = "SELECT \"nina\" FROM nope";
        let at = sql.find("nope").unwrap();
        let position = masked_position(sql, at, LiteralPolicy::Masked, &shop_catalog()).unwrap();
        assert_eq!(text(sql).chars().skip(position - 1).collect::<String>(), "nope");
    }

    #[test]
    fn comments_are_stripped_nested_ones_whole() {
        assert_eq!(text("-- nina\nSELECT /* a /* 'b' */ c */ 1 -- 2\nFROM orders /* open"), "SELECT <number 1>\nFROM orders");
    }

    #[test]
    fn literals_sent_stay_as_typed_and_comments_still_go() {
        let sent = masked("SELECT 'nina', 42 -- big\nFROM orders", LiteralPolicy::Sent, &shop_catalog());
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
        masked(sql, LiteralPolicy::Masked, &shop_catalog()).literals.put_back(answer)
    }

    #[test]
    fn markers_are_put_back_as_typed() {
        let sql = "SELECT * FROM orders WHERE note = E'a\\'b' AND total > 1.5";
        let answer = "SELECT id FROM orders WHERE total > <number 2> AND note = '<literal 1>'";
        assert_eq!(put_back(sql, answer).unwrap(), "SELECT id FROM orders WHERE total > 1.5 AND note = E'a\\'b'");
    }

    #[test]
    fn a_name_marker_is_put_back_with_its_quotes_as_typed() {
        let answer = "SELECT id FROM customers WHERE name = \"<name 1>\"";
        assert_eq!(put_back("SELECT \"nina@example.com\"", answer).unwrap(), "SELECT id FROM customers WHERE name = \"nina@example.com\"");
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

    fn comment(text: &str) -> String {
        masked_comment(text, Literals(vec![])).unwrap().text
    }

    fn comment_put_back(text: &str, answer: &str) -> String {
        masked_comment(text, Literals(vec![])).unwrap().literals.put_back(answer).unwrap()
    }

    #[test]
    fn a_word_of_a_comment_holding_an_at_is_a_string_marker_whole() {
        assert_eq!(comment("orders of nina@example.com, order 4412"), "orders of '<literal 1>', order <number 2>");
    }

    #[test]
    fn an_email_of_a_comment_is_put_back_as_a_string() {
        assert_eq!(comment_put_back("by nina@example.com", "x = '<literal 1>'"), "x = 'nina@example.com'");
    }

    #[test]
    fn a_string_right_after_an_email_is_masked_on_its_own() {
        assert_eq!(comment("by nina@example.com,'x y' then"), "by '<literal 1>','<literal 2>' then");
    }

    #[test]
    fn a_number_of_a_comment_is_masked_and_a_name_with_digits_stays() {
        assert_eq!(comment("top 10 of col1, $5, 1.5 and the 2nd"), "top <number 1> of col1, $<number 2>, <number 3> and the <number 4>");
    }

    #[test]
    fn the_markers_of_a_comment_count_on_from_the_statement() {
        let statement = masked("SELECT * FROM orders WHERE total > 10", LiteralPolicy::Masked, &shop_catalog());
        let masked = masked_comment("for 'nina'", statement.literals).unwrap();
        assert_eq!(masked.text, "for '<literal 2>'");
        assert_eq!(masked.literals.put_back("SELECT <number 1>, '<literal 2>'").unwrap(), "SELECT 10, 'nina'");
    }

    const EVERY_QUOTE: [&str; 5] = ["\"nina smith\"", "`nina smith`", "“nina smith”", "‘nina smith’", "'nina smith'"];

    #[test]
    fn every_kind_of_quote_in_a_comment_is_masked() {
        for quoted in EVERY_QUOTE {
            assert_eq!(comment(&format!("for {quoted} only")), "for '<literal 1>' only", "{quoted}");
        }
    }

    #[test]
    fn every_kind_of_quote_in_a_comment_is_put_back_as_a_string() {
        for quoted in EVERY_QUOTE {
            assert_eq!(comment_put_back(&format!("for {quoted}"), "x = '<literal 1>'"), "x = 'nina smith'", "{quoted}");
        }
    }

    #[test]
    fn a_quote_after_punctuation_is_masked() {
        assert_eq!(comment("name=\"nina\", (`bob`)"), "name='<literal 1>', ('<literal 2>')");
    }

    #[test]
    fn an_apostrophe_inside_a_word_stays_and_opens_no_quote() {
        assert_eq!(comment("what's the total of l'été, what’s"), "what's the total of l'été, what’s");
    }

    #[test]
    fn a_value_after_a_word_with_an_apostrophe_is_masked() {
        assert_eq!(comment("it's 'nina'"), "it's '<literal 1>'");
    }

    #[test]
    fn an_apostrophe_inside_a_word_of_a_string_is_part_of_its_value() {
        assert_eq!(comment("for 'nina's order' only"), "for '<literal 1>' only");
        assert_eq!(comment_put_back("for 'nina's order'", "'<literal 1>'"), "'nina''s order'");
    }

    #[test]
    fn a_quote_inside_another_is_part_of_its_value() {
        assert_eq!(comment_put_back("for \"nina's 'x'\"", "'<literal 1>'"), "'nina''s ''x'''");
    }

    #[test]
    fn an_email_with_an_apostrophe_is_a_marker_whole() {
        assert_eq!(comment("by o'neil@example.com"), "by '<literal 1>'");
    }

    #[test]
    fn a_param_of_a_comment_stays() {
        assert_eq!(comment("since {{since}} or {{ day_1 }}"), "since {{since}} or {{ day_1 }}");
    }

    #[test]
    fn a_param_inside_a_quote_is_part_of_its_value() {
        assert_eq!(comment("for '{{ nina'"), "for '<literal 1>'");
    }

    fn assert_refused(comments: &[&str]) {
        for refused in comments {
            assert!(masked_comment(refused, Literals(vec![])).is_none(), "{refused}");
        }
    }

    #[test]
    fn a_comment_whose_quotes_cannot_be_paired_is_refused() {
        assert_refused(&[
            "'nina",
            "it's 'nina",
            "orders' of nina",
            "a 'nina'y",
            "note'nina' and 'bob' 'x",
            "x'nina'y",
            "5' tall '1 Main St.' x'",
            "\"nina",
            "`nina",
            "“nina",
            "nina”",
            "‘nina",
            "nina’ x",
            "“nina\"",
            "5\" tall",
            "a\"nina\"",
            "\"nina\"y",
        ]);
    }

    #[test]
    fn a_comment_holding_an_empty_string_is_refused() {
        assert_refused(&["'' nina ''", "\"\" nina \"\"", "`` nina", "“” nina", "‘’ nina", "''"]);
    }

    #[test]
    fn a_string_with_a_prefix_in_a_comment_is_refused() {
        assert_refused(&["E'nina'", "x'ff'", "N'nina' and b'101'"]);
    }

    #[test]
    fn a_comment_holding_a_dollar_quote_is_refused() {
        assert_refused(&["for $$nina$$", "for $tag$ nina $tag$"]);
    }

    #[test]
    fn a_comment_with_an_unreadable_param_is_refused() {
        assert_refused(&["{{ nina", "{{}}", "x{{since}}", "{{since}}y"]);
    }

    #[test]
    fn debug_never_shows_a_literal() {
        assert_eq!(format!("{:?}", masked("SELECT 'nina'", LiteralPolicy::Masked, &shop_catalog()).literals), "Literals(1)");
    }
}
