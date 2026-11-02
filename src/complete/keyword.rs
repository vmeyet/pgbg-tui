//! The SQL keywords completion offers.
//! No alias can be one of them; a name that PostgreSQL reserves needs quotes.

/// In upper case.
const KEYWORDS: &str = "ALL AND AS ASC BETWEEN BY CASE CROSS DESC DISTINCT ELSE END EXCEPT EXISTS EXPLAIN FALSE FETCH FOR FROM FULL GROUP \
    HAVING ILIKE IN INNER INTERSECT IS JOIN LATERAL LEFT LIKE LIMIT NATURAL NOT NULL OFFSET ON OR ORDER OUTER RIGHT SELECT SHOW TABLE \
    THEN TRUE UNION USING VALUES WHEN WHERE WITH";

pub(super) fn keywords() -> impl Iterator<Item = &'static str> {
    KEYWORDS.split_ascii_whitespace()
}

/// Whether `word` is one of the keywords, ASCII case aside.
pub(super) fn is_keyword(word: &str) -> bool {
    keywords().any(|keyword| keyword.eq_ignore_ascii_case(word))
}

/// A keyword in lower case once what was typed of it holds a lower-case letter, or else in upper case.
pub(super) fn cased_as(keyword: &str, typed: &str) -> String {
    if typed.chars().any(char::is_lowercase) { keyword.to_lowercase() } else { keyword.to_uppercase() }
}

/// The end of a keyword after `typed`, in the case `typed` has, whatever the case of `rest`; none when the two make no keyword.
pub(crate) fn rest_of_keyword(typed: &str, rest: &str) -> Option<String> {
    is_keyword(&format!("{typed}{rest}")).then(|| cased_as(rest, typed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_of_a_keyword_takes_the_lower_case_typed() {
        assert_eq!(rest_of_keyword("sel", "ECT").as_deref(), Some("ect"));
    }

    #[test]
    fn the_end_of_a_keyword_takes_the_upper_case_typed() {
        assert_eq!(rest_of_keyword("SEL", "ect").as_deref(), Some("ECT"));
    }

    #[test]
    fn a_capital_followed_by_lower_case_counts_as_lower_case() {
        assert_eq!(rest_of_keyword("Sel", "ECT").as_deref(), Some("ect"));
    }

    #[test]
    fn a_word_that_is_no_keyword_has_no_end_to_case() {
        assert_eq!(rest_of_keyword("ord", "ERS"), None);
    }
}
