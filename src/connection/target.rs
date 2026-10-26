use anyhow::{Result, anyhow, bail};
use serde::{Serialize, Serializer};
use std::fmt;
use url::Url;

const SCHEMES: [&str; 2] = ["postgres", "postgresql"];
const PASSWORD_IN_URL: &str =
    "`url` carries a password: take it out and set `password = { keychain = \"...\" }`, `{ env = \"...\" }` or `{ command = [...] }`";

/// Where the database is: a `postgres://` URL that never carries a password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target(Url);

impl Target {
    /// The URL in `text`; its errors never repeat it, since a refused URL may hold a password.
    pub(super) fn parse(text: &str) -> Result<Self> {
        let url = Url::parse(text).map_err(|e| anyhow!("`url` is not a URL: {e}"))?;
        if !SCHEMES.contains(&url.scheme()) {
            bail!("`url` must start with postgres://, not {}://", url.scheme());
        }
        if has_password(&url) {
            bail!(PASSWORD_IN_URL);
        }
        Ok(Self(url))
    }
}

fn has_password(url: &Url) -> bool {
    url.password().is_some() || url.query_pairs().any(|(key, _)| key == "password")
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl Serialize for Target {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(text: &str) -> String {
        Target::parse(text).unwrap_err().to_string()
    }

    #[test]
    fn a_target_prints_as_its_url() {
        let url = "postgres://nina@db.example.com:5432/shop?sslmode=require";
        assert_eq!(Target::parse(url).unwrap().to_string(), url);
    }

    #[test]
    fn a_password_in_the_url_is_refused_without_repeating_it() {
        let err = refusal("postgres://nina:hunter2@db.example.com/shop");
        assert!(err.contains("password") && !err.contains("hunter2"), "{err}");
    }

    #[test]
    fn a_password_in_the_query_is_refused_too() {
        let err = refusal("postgres://nina@db.example.com/shop?password=hunter2");
        assert!(err.contains("password") && !err.contains("hunter2"), "{err}");
    }

    #[test]
    fn a_url_of_another_scheme_is_refused() {
        assert!(refusal("mysql://nina@db.example.com/shop").contains("postgres://"));
    }

    #[test]
    fn a_text_that_is_no_url_is_refused() {
        assert!(refusal("db.example.com/shop").contains("not a URL"));
    }
}
