//! Which functions a statement calls, read from its tokens: what lets a read go again on its own after a lost link.
//! A best effort that fails closed: anything before a `(` that it does not know counts as an unsafe call.
//! It sees only the text, so a call hidden in a view, a custom operator or a cast stays out of its sight.
use super::lex::{Placed, is_one_of};

/// Functions with no effect outside their statement, by the names PostgreSQL gives them.
/// The type names take a `(` as functions do: `numeric(10, 2)`.
/// `pg_sleep` only spends time, and `current_setting` only reads.
const SAFE_FUNCTIONS: &str = "abs age array_agg array_length avg bool_and bool_or ceil ceiling char character coalesce concat \
    concat_ws count current_setting date_part date_trunc decimal dense_rank extract float floor format \
    generate_series greatest interval json_agg jsonb_agg lag lead least left length lower ltrim max min \
    mod now nullif numeric pg_sleep position rank replace right round row_number rtrim split_part \
    string_agg substr substring sum time timestamp to_char to_date trim unnest upper varchar";
/// Words PostgreSQL never reads as a function's name, unquoted: what follows them is no call.
const NEVER_A_FUNCTION: &str = "ALL AND ANY ARRAY AS BETWEEN CASE CAST DISTINCT ELSE EXCEPT EXISTS FETCH FROM GROUP HAVING IN \
    INTERSECT LATERAL LIMIT NOT OFFSET ON OR ROW SELECT SOME THEN TO UNION USING VALUES WHEN WHERE";
/// Words that open a clause of the call before them, and could name a function anywhere else.
const AFTER_A_CALL: [&str; 2] = ["FILTER", "OVER"];

/// Whether every `(` of the statement of `tokens`, read out of `sql`, opens no call or one of `SAFE_FUNCTIONS`.
pub(super) fn calls_only_safe_functions(sql: &str, tokens: &[Placed]) -> bool {
    let is_open = |placed: &Placed| &sql[placed.range.clone()] == "(";
    tokens.iter().enumerate().filter(|(_, placed)| is_open(placed)).all(|(at, _)| is_safe_before_a_parenthesis(sql, &tokens[..at]))
}

/// Whether what `before` ends with may stand before a `(`: a quoted or qualified name could be any function.
fn is_safe_before_a_parenthesis(sql: &str, before: &[Placed]) -> bool {
    let text = |placed: &Placed| &sql[placed.range.clone()];
    let is_in = |list: &str, word: &str| list.split_whitespace().any(|known| known.eq_ignore_ascii_case(word));
    let Some((last, earlier)) = before.split_last() else { return true };
    let Some(word) = last.word(sql) else { return !text(last).starts_with('"') };
    let previous = earlier.last().map(text);
    let opens_explain = earlier.is_empty() && word.eq_ignore_ascii_case("EXPLAIN");
    let closes_a_call = previous == Some(")") && is_one_of(word, &AFTER_A_CALL);
    previous != Some(".") && (is_in(SAFE_FUNCTIONS, word) || is_in(NEVER_A_FUNCTION, word) || opens_explain || closes_a_call)
}

#[cfg(test)]
mod tests {
    use super::super::lex;
    use super::*;

    fn is_safe(sql: &str) -> bool {
        calls_only_safe_functions(sql, &lex::tokens(sql))
    }

    #[test]
    fn a_read_of_a_table_calls_nothing() {
        assert!(is_safe("SELECT * FROM orders WHERE id IN (1, 2) AND (total > 3 OR total < 1)"));
    }

    #[test]
    fn functions_on_the_safe_list_are_safe_in_any_case() {
        assert!(is_safe("SELECT COUNT(*), lower(name), Date_Trunc('day', now()) FROM orders"));
    }

    #[test]
    fn an_unknown_function_is_not_safe() {
        assert!(!is_safe("SELECT dblink_exec('shop', 'DELETE FROM orders')"));
    }

    #[test]
    fn an_unknown_function_beside_a_safe_one_is_not_safe() {
        assert!(!is_safe("SELECT count(*), pg_terminate_backend(42) FROM orders"));
    }

    #[test]
    fn an_unknown_function_inside_a_safe_one_is_not_safe() {
        assert!(!is_safe("SELECT coalesce(nextval('orders_id_seq'), 0)"));
    }

    #[test]
    fn an_unknown_function_in_a_from_is_not_safe() {
        assert!(!is_safe("SELECT * FROM (SELECT * FROM dblink('shop', 'SELECT 1')) AS remote"));
    }

    #[test]
    fn a_quoted_name_is_not_safe_whatever_it_spells() {
        assert!(!is_safe(r#"SELECT "count"(*) FROM orders"#));
    }

    #[test]
    fn a_qualified_name_is_not_safe_whatever_it_spells() {
        assert!(!is_safe("SELECT public.lower(name) FROM orders"));
    }

    #[test]
    fn a_function_named_inside_a_string_or_a_comment_is_no_call() {
        assert!(is_safe("SELECT 'dblink_exec(1)' -- dblink_exec(2)\nFROM orders"));
    }

    #[test]
    fn a_window_over_a_call_is_safe() {
        assert!(is_safe("SELECT sum(total) FILTER (WHERE total > 1) OVER (ORDER BY day) FROM orders"));
    }

    #[test]
    fn a_function_named_like_a_window_is_not_safe() {
        assert!(!is_safe("SELECT over(1) FROM orders"));
    }

    #[test]
    fn the_options_of_an_explain_are_no_call() {
        assert!(is_safe("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT * FROM orders"));
    }

    #[test]
    fn a_function_named_explain_is_not_safe() {
        assert!(!is_safe("SELECT explain(1)"));
    }

    #[test]
    fn a_length_on_a_type_is_no_call() {
        assert!(is_safe("SELECT total::numeric(10, 2), name::varchar(20) FROM orders"));
    }
}
