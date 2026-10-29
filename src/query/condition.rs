//! A condition added to a statement: into its `WHERE` with `AND`, else as a new `WHERE` before the clause that follows
//! one, both found among the statement's `db::lex` tokens outside parentheses.
use super::params::ParamName;
use crate::db::lex::{self, Placed, Token};
use crate::db::model::sql_name;
use std::ops::Range;

/// The words that may follow a `WHERE`, outside parentheses.
const AFTER_WHERE: &[&str] =
    &["GROUP", "HAVING", "WINDOW", "ORDER", "LIMIT", "OFFSET", "FETCH", "FOR", "UNION", "INTERSECT", "EXCEPT", "RETURNING"];

/// How a column compares with a cell's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Comparison {
    Equal,
    NotEqual,
}

/// Text to put in place of some bytes of the text a statement was read from.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Splice {
    pub(crate) range: Range<usize>,
    pub(crate) text: String,
}

/// `column = {{name}}`, or `<>`, the column quoted when it must be; `IS NULL` or `IS NOT NULL` without a parameter.
pub(crate) fn condition(column: &str, comparison: Comparison, param: Option<&ParamName>) -> String {
    let column = sql_name(column);
    match (param, comparison) {
        (Some(param), Comparison::Equal) => format!("{column} = {{{{{param}}}}}"),
        (Some(param), Comparison::NotEqual) => format!("{column} <> {{{{{param}}}}}"),
        (None, Comparison::Equal) => format!("{column} IS NULL"),
        (None, Comparison::NotEqual) => format!("{column} IS NOT NULL"),
    }
}

/// The splice that adds `condition` to the statement of `tokens`, read from `sql`: after its `WHERE` with `AND`, the
/// `WHERE` in parentheses when it holds an `OR`; else a new `WHERE` before `ORDER BY`, `LIMIT` or `GROUP BY`, or at its
/// end. None when the statement has no token.
pub(crate) fn add(sql: &str, tokens: &[Placed], condition: &str) -> Option<Splice> {
    let outside = outside_parentheses(sql, tokens);
    let is_word = |placed: &Placed, words: &[&str]| placed.word(sql).is_some_and(|word| lex::is_one_of(word, words));
    let where_at = outside.iter().position(|placed| is_word(placed, &["WHERE"]));
    let from_at = outside.iter().position(|placed| is_word(placed, &["FROM"]));
    let after_from_or_where = outside.iter().skip(where_at.or(from_at).map_or(0, |at| at + 1));
    let next_clause =
        after_from_or_where.clone().find(|placed| is_word(placed, AFTER_WHERE)).map_or(sql.len(), |placed| placed.range.start);
    let end = tokens.iter().rfind(|placed| placed.range.end <= next_clause && placed.token != Token::End)?.range.end;
    let Some(where_at) = where_at else { return Some(Splice { range: end..end, text: format!(" WHERE {condition}") }) };
    let holds_or = after_from_or_where.take_while(|placed| placed.range.start < next_clause).any(|placed| is_word(placed, &["OR"]));
    if !holds_or {
        return Some(Splice { range: end..end, text: format!(" AND {condition}") });
    }
    let start = outside[where_at].range.end;
    Some(Splice { range: start..end, text: format!(" ({}) AND {condition}", sql[start..end].trim()) })
}

/// The tokens outside any parentheses, so a subquery's `WHERE` is not the statement's.
fn outside_parentheses<'a>(sql: &str, tokens: &'a [Placed]) -> Vec<&'a Placed> {
    let mut depth = 0_usize;
    let mut outside = vec![];
    for placed in tokens {
        match &sql[placed.range.clone()] {
            "(" => depth += 1,
            ")" => depth = depth.saturating_sub(1),
            _ if depth == 0 => outside.push(placed),
            _ => {}
        }
    }
    outside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added(sql: &str, condition: &str) -> String {
        let Some(splice) = add(sql, &lex::tokens(sql), condition) else { return sql.to_owned() };
        format!("{}{}{}", &sql[..splice.range.start], splice.text, &sql[splice.range.end..])
    }

    #[test]
    fn a_statement_without_where_gets_one_at_its_end() {
        assert_eq!(added("SELECT * FROM orders;", "id = 7"), "SELECT * FROM orders WHERE id = 7;");
    }

    #[test]
    fn a_new_where_goes_before_order_by_on_the_line_before_it() {
        assert_eq!(added("SELECT * FROM orders\nORDER BY id LIMIT 9", "id = 7"), "SELECT * FROM orders WHERE id = 7\nORDER BY id LIMIT 9");
    }

    #[test]
    fn a_where_takes_the_condition_with_and_before_group_by() {
        assert_eq!(
            added("SELECT status, count(*) FROM orders WHERE total > 0 GROUP BY 1", "status = 'paid'"),
            "SELECT status, count(*) FROM orders WHERE total > 0 AND status = 'paid' GROUP BY 1"
        );
    }

    #[test]
    fn a_group_in_the_select_list_is_no_clause() {
        assert_eq!(
            added("SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY total) FROM orders", "id = 7"),
            "SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY total) FROM orders WHERE id = 7"
        );
    }

    #[test]
    fn a_where_holding_or_goes_in_parentheses() {
        assert_eq!(
            added("SELECT * FROM orders WHERE id = 1 OR id = 2 LIMIT 5", "customer = 3"),
            "SELECT * FROM orders WHERE (id = 1 OR id = 2) AND customer = 3 LIMIT 5"
        );
    }

    #[test]
    fn a_where_inside_parentheses_is_a_subquerys_not_the_statements() {
        assert_eq!(
            added("SELECT * FROM orders WHERE customer IN (SELECT id FROM customers WHERE vip ORDER BY id)", "id = 7"),
            "SELECT * FROM orders WHERE customer IN (SELECT id FROM customers WHERE vip ORDER BY id) AND id = 7"
        );
    }

    #[test]
    fn an_empty_statement_takes_no_condition() {
        assert_eq!(add("  ", &lex::tokens("  "), "id = 7"), None);
    }

    #[test]
    fn a_column_is_compared_with_its_parameter_and_quoted_when_it_must_be() {
        let param = "total".parse().ok();
        assert_eq!(condition("Total", Comparison::NotEqual, param.as_ref()), "\"Total\" <> {{total}}");
    }

    #[test]
    fn a_null_cell_compares_with_is_null() {
        assert_eq!(
            [Comparison::Equal, Comparison::NotEqual].map(|comparison| condition("customer", comparison, None)),
            ["customer IS NULL", "customer IS NOT NULL"]
        );
    }
}
