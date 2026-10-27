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
