//! The words that mark a statement as holding a secret, which then is neither kept in the history nor sent to the AI command.
use crate::connection::SECRET_KEYS;

const PASSWORD_WORD: &str = "password";
const SCHEME_END: &str = "://";

/// Whether `password` or a secret key a connection refuses stands as a word of its own, as in
/// `ALTER ROLE nina PASSWORD '...'` or `sslpassword=...`, or a URL holds a password, as in
/// `postgresql://nina:...@db.example.com`; `password_hash` is another word.
pub(crate) fn holds_secret(statement: &str) -> bool {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let is_secret = |word: &str| [PASSWORD_WORD].iter().chain(&SECRET_KEYS).any(|secret| word.eq_ignore_ascii_case(secret));
    let has_secret_word = statement.split(|c: char| !is_word_char(c)).any(is_secret);
    has_secret_word || statement.match_indices(SCHEME_END).any(|(at, _)| is_url_with_password(&statement[..at], &statement[at..]))
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
}
