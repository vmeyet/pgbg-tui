//! The words that mark a text as holding a secret, which then is neither kept on disk nor sent to the AI command.
use crate::connection::SECRET_KEYS;

const PASSWORD_WORD: &str = "password";
const SCHEME_END: &str = "://";
/// What marks a word of a comment as a secret anywhere inside it: `PGPASSWORD`, `db_passwd`, `x-api-key`.
const SECRETS_INSIDE_A_WORD: [&str; 4] = [PASSWORD_WORD, "passwd", "api_key", "apikey"];
/// What marks a word of a comment as a secret as a whole part of it between `_`: `pwd`, `auth_token`; `tokenize` is another word.
const SECRET_PARTS_OF_A_WORD: [&str; 3] = ["pwd", "token", "secret"];
/// What pgbg writes to disk in place of a text holding a secret: in the history and in the versions of a query.
pub(crate) const NOT_KEPT: &str = "-- not kept: it holds a password";

/// `text`, or the `NOT_KEPT` note when it holds a secret, for what pgbg writes to disk.
pub(crate) fn kept_or_note(text: &str) -> &str {
    if holds_secret(text) { NOT_KEPT } else { text }
}

/// Whether `password` or a secret key a connection refuses stands as a word of its own, as in
/// `ALTER ROLE nina PASSWORD '...'` or `sslpassword=...`, or a URL holds a password, as in
/// `postgresql://nina:...@db.example.com`; `password_hash` is another word.
pub(crate) fn holds_secret(statement: &str) -> bool {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let is_secret = |word: &str| [PASSWORD_WORD].iter().chain(&SECRET_KEYS).any(|secret| word.eq_ignore_ascii_case(secret));
    let has_secret_word = statement.split(|c: char| !is_word_char(c)).any(is_secret);
    has_secret_word || statement.match_indices(SCHEME_END).any(|(at, _)| is_url_with_password(&statement[..at], &statement[at..]))
}

/// Whether `comment`, words nobody meant to send, may hold a secret: what `holds_secret` finds, a word naming one, or any
/// `KEY=value`. Stricter than `holds_secret`, which also decides what the history and the versions keep, where a table
/// `token_usage` is no secret; a comment wrongly refused is only left out of a prompt.
pub(crate) fn comment_holds_secret(comment: &str) -> bool {
    holds_secret(comment) || comment.split(|c| !is_of_comment_word(c)).any(names_a_secret) || holds_key_value(comment)
}

fn is_of_comment_word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-')
}

fn names_a_secret(word: &str) -> bool {
    let word = word.to_lowercase().replace('-', "_");
    SECRETS_INSIDE_A_WORD.iter().any(|secret| word.contains(secret)) || word.split('_').any(|part| SECRET_PARTS_OF_A_WORD.contains(&part))
}

/// Whether a `=` of `comment` has a word right before it and a value right after it, as `PGPASSWORD=hunter2`; `a >= 1`,
/// `a = 1` and `a => 1` have no such shape.
fn holds_key_value(comment: &str) -> bool {
    comment.match_indices('=').any(|(at, _)| {
        let key = comment[..at].rsplit(|c| !is_of_comment_word(c)).next().unwrap_or_default();
        let has_key = key.starts_with(|c: char| c.is_alphabetic() || c == '_');
        let has_value = comment[at + 1..].chars().next().is_some_and(|c| !c.is_whitespace() && !matches!(c, '=' | '>'));
        has_key && has_value
    })
}

/// Whether a scheme ends `before` and `after` opens on `://user:password@`.
fn is_url_with_password(before: &str, after: &str) -> bool {
    let has_scheme = before.ends_with(|c: char| c.is_alphanumeric() || c == '_');
    let Some((user, password)) = after[SCHEME_END.len()..].split_once('@').and_then(|(user_info, _)| user_info.split_once(':')) else {
        return false;
    };
    let is_user = !user.is_empty() && !user.contains(|c: char| c == '/' || c.is_whitespace());
    let is_password = !password.is_empty() && !password.contains(char::is_whitespace);
    has_scheme && is_user && is_password
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_key_inside_a_dollar_quoted_string_is_a_secret() {
        for key in [PASSWORD_WORD].into_iter().chain(SECRET_KEYS) {
            assert!(holds_secret(&format!("SELECT dblink_connect($${key}=hunter2 host=db.example.com$$)")), "{key}");
        }
    }

    #[test]
    fn a_secret_key_inside_a_tagged_dollar_quoted_string_is_a_secret() {
        for key in [PASSWORD_WORD].into_iter().chain(SECRET_KEYS) {
            assert!(holds_secret(&format!("SELECT dblink_connect($q${key}=hunter2 host=db.example.com$q$)")), "{key}");
        }
    }

    #[test]
    fn a_column_named_after_a_password_is_no_secret() {
        assert!(!holds_secret("SELECT password_hash, $$password_hint$$ FROM customers"));
    }

    #[test]
    fn a_table_named_after_a_token_or_a_key_stays_no_secret_in_a_statement() {
        assert!(!holds_secret("SELECT token, api_key, pwd, secret, day=1 FROM token_usage"));
        assert_eq!(kept_or_note("SELECT * FROM token_usage"), "SELECT * FROM token_usage");
    }

    #[test]
    fn a_comment_naming_a_secret_holds_one() {
        for comment in [
            "PGPASSWORD hunter2",
            "the db_password of nina",
            "pwd hunter2",
            "passwd hunter2",
            "API_KEY sk-live-abc",
            "x-api-key abc",
            "token abc123",
            "auth_token abc123",
            "the client_secret",
            "secret abc",
        ] {
            assert!(comment_holds_secret(comment), "{comment}");
        }
    }

    #[test]
    fn a_comment_holding_any_key_value_holds_a_secret() {
        for comment in ["PGPASSWORD=hunter2 psql", "DB_HOST=db.example.com", "total=10", "a=\"nina smith\""] {
            assert!(comment_holds_secret(comment), "{comment}");
        }
    }

    #[test]
    fn a_comment_holding_what_a_statement_may_not_hold_holds_a_secret() {
        assert!(comment_holds_secret("from postgresql://nina:hunter2@db.example.com/shop"));
    }

    #[test]
    fn a_comment_of_plain_words_and_comparisons_holds_no_secret() {
        for comment in [
            "what's the total",
            "total >= 10",
            "total = 10",
            "a != b",
            "a == b",
            "x => y",
            "tokenize the name",
            "the secretary",
            "ends in =",
        ] {
            assert!(!comment_holds_secret(comment), "{comment}");
        }
    }
}
