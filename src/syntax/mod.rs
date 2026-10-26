//! SQL colours from the tree-sitter grammar, built on first use so startup never pays for it.
use std::ops::Range;
use std::sync::OnceLock;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// What a piece of SQL is, as far as colour goes; the theme gives every token a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Constant,
    Punctuation,
}

/// The coloured byte ranges of one line of text.
pub(crate) type Spans = Vec<(Range<usize>, Token)>;

/// `None` inside when the grammar's query does not build: the editor then stays uncoloured.
static CONFIGURATION: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();

/// The capture names the grammar's query uses and the token each paints.
/// tree-sitter matches a capture to the longest listed name that prefixes it, so `string` also
/// covers `string.special`.
const CAPTURES: [(&str, Token); 13] = [
    ("keyword", Token::Keyword),
    ("conditional", Token::Keyword),
    ("storageclass", Token::Keyword),
    ("string", Token::String),
    ("comment", Token::Comment),
    ("number", Token::Number),
    ("float", Token::Number),
    ("boolean", Token::Constant),
    ("type", Token::Type),
    ("attribute", Token::Type),
    ("function", Token::Function),
    ("operator", Token::Punctuation),
    ("punctuation", Token::Punctuation),
];

/// The spans of each line of `source`, split on `\n`; one entry per line, empty for uncoloured ones.
/// A grammar that fails leaves every line uncoloured rather than failing the editor.
pub(crate) fn highlight(source: &str) -> Vec<Spans> {
    let starts: Vec<usize> = std::iter::once(0).chain(source.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let mut lines: Vec<Spans> = vec![Vec::new(); starts.len()];
    let Some(config) = configuration() else { return lines };
    let mut highlighter = Highlighter::new();
    let Ok(events) = highlighter.highlight(config, source.as_bytes(), None, None, |_| None) else { return lines };
    let mut stack: Vec<Token> = Vec::new();
    for event in events {
        match event {
            Ok(HighlightEvent::HighlightStart(capture)) => stack.push(CAPTURES[capture.0].1),
            Ok(HighlightEvent::HighlightEnd) => {
                stack.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                if let Some(&token) = stack.last() {
                    push_span(&mut lines, &starts, start..end, token);
                }
            }
            Err(_) => return vec![Vec::new(); starts.len()],
        }
    }
    lines
}

/// Splits a source range across the lines it spans, as byte ranges within each line.
fn push_span(lines: &mut [Spans], starts: &[usize], range: Range<usize>, token: Token) {
    let first = starts.partition_point(|&s| s <= range.start).saturating_sub(1);
    for (index, &line_start) in starts.iter().enumerate().skip(first) {
        if line_start >= range.end {
            break;
        }
        let line_end = starts.get(index + 1).map_or(usize::MAX, |next| next - 1);
        let start = range.start.max(line_start);
        let end = range.end.min(line_end);
        if start < end {
            lines[index].push((start - line_start..end - line_start, token));
        }
    }
}

fn configuration() -> Option<&'static HighlightConfiguration> {
    CONFIGURATION
        .get_or_init(|| {
            let mut config = sql()?;
            config.configure(&CAPTURES.map(|(name, _)| name));
            Some(config)
        })
        .as_ref()
}

/// The grammar's query is written for Neovim: its number patterns use Lua's `%d`, which the Rust
/// highlighter reads as a literal `%`, so every literal stayed a string; comments also carry a
/// Neovim-only `@spell`. Here a literal is a string only when quoted, a number when it is digits.
fn sql() -> Option<HighlightConfiguration> {
    const LITERALS: &str = r#"((literal) @string (#match? @string "^['\"]"))
((literal) @number (#match? @number "^[-+]?[0-9]*[.]?[0-9]+$"))
"#;
    let upstream = tree_sitter_sequel::HIGHLIGHTS_QUERY.replace("@comment @spell", "@comment");
    let kept: Vec<String> = upstream
        .split("\n\n")
        .filter(|block| !block.contains("%d"))
        .map(|block| block.lines().filter(|line| line.trim() != "(literal) @string").collect::<Vec<_>>().join("\n"))
        .collect();
    let highlights = format!("{LITERALS}{}", kept.join("\n\n"));
    HighlightConfiguration::new(tree_sitter_sequel::LANGUAGE.into(), "sql", &highlights, "", "").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens_of<'a>(line: &'a str, spans: &Spans) -> Vec<(&'a str, Token)> {
        spans.iter().map(|(range, token)| (&line[range.clone()], *token)).collect()
    }

    #[test]
    fn the_grammar_query_builds() {
        assert!(sql().is_some());
    }

    #[test]
    fn keywords_strings_numbers_and_comments_land_on_their_bytes() {
        let sql = "SELECT id FROM orders WHERE status = 'paid' AND total > 10; -- recent";
        let tokens = tokens_of(sql, &highlight(sql)[0]);
        assert!(tokens.contains(&("SELECT", Token::Keyword)) && tokens.contains(&("WHERE", Token::Keyword)), "{tokens:?}");
        assert!(tokens.contains(&("'paid'", Token::String)), "{tokens:?}");
        assert!(tokens.contains(&("10", Token::Number)), "{tokens:?}");
        assert!(tokens.contains(&("-- recent", Token::Comment)), "{tokens:?}");
    }

    #[test]
    fn each_line_gets_its_own_spans_with_bytes_counted_from_its_start() {
        let sql = "SELECT *\nFROM orders\n";
        let lines = highlight(sql);
        assert_eq!(lines.len(), 3);
        assert!(tokens_of("FROM orders", &lines[1]).contains(&("FROM", Token::Keyword)), "{:?}", lines[1]);
    }

    #[test]
    fn a_string_across_lines_is_split_per_line() {
        let sql = "SELECT 'one\ntwo' FROM orders";
        let lines = highlight(sql);
        assert!(tokens_of("SELECT 'one", &lines[0]).contains(&("'one", Token::String)), "{:?}", lines[0]);
        assert!(tokens_of("two' FROM orders", &lines[1]).contains(&("two'", Token::String)), "{:?}", lines[1]);
    }

    #[test]
    fn an_unfinished_statement_still_colours_what_it_can() {
        let sql = "SELECT id FROM";
        assert!(tokens_of(sql, &highlight(sql)[0]).contains(&("SELECT", Token::Keyword)));
    }
}
