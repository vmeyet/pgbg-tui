//! The SQL keywords completion offers, which no alias can be and a name must be quoted to take.

/// In upper case, as the tokenizer writes words.
const KEYWORDS: &str = "ALL AND AS ASC BETWEEN BY CASE CROSS DESC DISTINCT ELSE END EXCEPT EXISTS EXPLAIN FALSE FETCH FOR FROM FULL GROUP \
    HAVING ILIKE IN INNER INTERSECT IS JOIN LATERAL LEFT LIKE LIMIT NATURAL NOT NULL OFFSET ON OR ORDER OUTER RIGHT SELECT SHOW TABLE \
    THEN TRUE UNION USING VALUES WHEN WHERE WITH";

pub(super) fn keywords() -> impl Iterator<Item = &'static str> {
    KEYWORDS.split_ascii_whitespace()
}

/// Whether `word`, in upper case, is one of the keywords.
pub(super) fn is_keyword(word: &str) -> bool {
    keywords().any(|keyword| keyword == word)
}
