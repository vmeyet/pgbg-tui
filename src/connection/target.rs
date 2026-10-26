use anyhow::{Result, anyhow, bail};
use serde::{Serialize, Serializer};
use std::fmt;
use url::Url;

const SCHEMES: [&str; 2] = ["postgres", "postgresql"];
const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];
const PASSWORD_IN_URL: &str =
    "`url` carries a password: take it out and set `password = { keychain = \"...\" }`, `{ env = \"...\" }` or `{ command = [...] }`";
const HOSTADDR_IN_URL: &str = "`url` sets `hostaddr`: pgbg keeps it for the local port its tunnels open; name the host in the URL instead";

/// Where the database is: a `postgres://` URL that never carries a password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target(Url);

impl Target {
    /// The URL in `text`; its errors never repeat it, since a refused URL may hold a password.
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let url = Url::parse(text).map_err(|e| anyhow!("`url` is not a URL: {e}"))?;
        if !SCHEMES.contains(&url.scheme()) {
            bail!("`url` must start with postgres://, not {}://", url.scheme());
        }
        if has_password(&url) {
            bail!(PASSWORD_IN_URL);
        }
        if has_query_key(&url, "hostaddr") {
            bail!(HOSTADDR_IN_URL);
        }
        Ok(Self(url))
    }

    pub(crate) fn url(&self) -> &Url {
        &self.0
    }

    /// `host/database`, as the TUI shows where a connection goes; the host may come from a `host` query key.
    pub(crate) fn place(&self) -> String {
        let queried = self.0.query_pairs().find(|(key, _)| key == "host").map(|(_, host)| host.into_owned());
        let named = self.0.host_str().filter(|host| !host.is_empty()).map(str::to_owned);
        let host = named.or(queried).unwrap_or_else(|| LOCAL_HOSTS[0].to_owned());
        format!("{host}{}", self.0.path())
    }

    /// The `sslmode` the query asks for, when it does.
    pub(crate) fn ssl_mode(&self) -> Option<String> {
        self.0.query_pairs().find(|(key, _)| key == "sslmode").map(|(_, mode)| mode.into_owned())
    }

    /// True when every host the URL names, in its authority or a `host` query key, is this machine.
    pub(crate) fn is_local(&self) -> bool {
        let queried = self.0.query_pairs().filter(|(key, _)| key == "host").map(|(_, host)| host.into_owned());
        self.0.host_str().map(str::to_owned).into_iter().chain(queried).all(|host| is_local_host(&host))
    }
}

fn is_local_host(host: &str) -> bool {
    host.starts_with('/') || LOCAL_HOSTS.contains(&host)
}

fn has_password(url: &Url) -> bool {
    url.password().is_some() || has_query_key(url, "password")
}

fn has_query_key(url: &Url, wanted: &str) -> bool {
    url.query_pairs().any(|(key, _)| key == wanted)
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
    fn a_place_is_the_host_and_the_database() {
        let place = |url: &str| Target::parse(url).unwrap().place();
        assert_eq!(place("postgres://nina@db.example.com:5432/shop?sslmode=require"), "db.example.com/shop");
        assert_eq!(place("postgres:///shop?host=db.example.com"), "db.example.com/shop");
        assert_eq!(place("postgres:///shop"), "localhost/shop");
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
    fn a_hostaddr_in_the_url_is_refused_since_tunnels_own_it() {
        assert!(refusal("postgres://nina@localhost/shop?hostaddr=203.0.113.5").contains("hostaddr"));
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
