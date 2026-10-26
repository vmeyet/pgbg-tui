//! Keys a config holds that this pgbg does not read: written for a newer pgbg, or a secret.
//! A newer pgbg's config must not stop an older one, so each is a warning; only a secret fails.
use anyhow::{Result, bail};
use serde::de::DeserializeOwned;

/// Words no setting is named after, since a password never goes in the config.
const SECRET_WORDS: [&str; 4] = ["key", "token", "secret", "password"];

/// What `text` holds, and one warning for each key it ignored.
pub(super) fn parse<T: DeserializeOwned>(text: &str) -> Result<(T, Vec<String>)> {
    let mut ignored = vec![];
    let value = serde_ignored::deserialize(toml::Deserializer::parse(text)?, |path| ignored.push(path.to_string()))?;
    if let Some(secret) = ignored.iter().find(|key| is_secret(key)) {
        bail!("`{secret}` looks like a secret: a password comes from the keychain, an environment variable or a command, never the config");
    }
    Ok((value, ignored.iter().map(|key| warning(key)).collect()))
}

fn is_secret(key: &str) -> bool {
    let name = key.rsplit('.').next().unwrap_or(key).to_ascii_lowercase();
    SECRET_WORDS.iter().any(|word| name.strip_suffix(word).is_some_and(|rest| rest.is_empty() || rest.ends_with('_')))
}

fn warning(key: &str) -> String {
    format!("`{key}` is not a setting this pgbg knows; it is ignored — written for a newer pgbg?")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Debug, serde::Deserialize)]
    struct Shop {
        #[serde(default)]
        tables: BTreeMap<String, Table>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct Table {
        rows: u32,
    }

    fn warned(text: &str) -> Vec<String> {
        parse::<Shop>(text).unwrap().1
    }

    #[test]
    fn an_unknown_key_is_ignored_with_a_warning_naming_its_full_path() {
        let (shop, warnings) = parse::<Shop>("[tables.orders]\nrows = 3\ncolour = \"red\"\n").unwrap();
        assert_eq!(shop.tables["orders"].rows, 3, "what it knows still loads");
        assert_eq!(warnings, ["`tables.orders.colour` is not a setting this pgbg knows; it is ignored — written for a newer pgbg?"]);
    }

    #[test]
    fn a_config_of_known_keys_warns_of_nothing() {
        assert_eq!(warned("[tables.orders]\nrows = 3\n"), [] as [String; 0]);
    }

    #[test]
    fn a_key_that_looks_like_a_secret_fails_to_load() {
        let err = parse::<Shop>("[tables.orders]\nrows = 3\napi_token = \"x\"\n").unwrap_err().to_string();
        assert!(err.contains("`tables.orders.api_token`") && err.contains("keychain"), "{err}");
        assert!(parse::<Shop>("password = \"hunter2\"\n").is_err());
        assert!(parse::<Shop>("keyboard = \"azerty\"\n").is_ok(), "only a whole word counts");
    }
}
