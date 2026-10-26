use crate::config;
use crate::connection::{Connection, ConnectionName};
use crate::secret::{Keychain, KeychainItem, Secret, SecretSource};
use anyhow::{Result, bail};

/// Saves the password of the connection `name` in the keychain, typed hidden or piped in; a connection
/// with no password yet gets `password = { keychain = "pgbg/<name>" }`. Safe to run twice.
pub async fn set(name: &ConnectionName) -> Result<()> {
    let connection = super::connection(Some(name))?;
    let item = keychain_item(&connection)?;
    let password = Secret::from_stdin(&format!("Password for `{name}` (hidden): ")).await?;
    Keychain::PGBG.write(&item, &password)?;
    if connection.password().is_none() {
        config::set_password(&config::path(), name, &SecretSource::Keychain(item.clone()))?;
    }
    println!("saved the password of `{name}` in the keychain as `{item}`");
    Ok(())
}

/// The keychain item the connection names, else `pgbg/<name>`; a password from elsewhere is refused.
fn keychain_item(connection: &Connection) -> Result<KeychainItem> {
    match connection.password() {
        None => format!("pgbg/{}", connection.name()).parse(),
        Some(SecretSource::Keychain(item)) => Ok(item.clone()),
        Some(other) => bail!(
            "`{}` reads its password from `{}`, not the keychain: change that there, or remove its `password` key first",
            connection.name(),
            other.kind()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn prod(extra: &str) -> Connection {
        fixtures::named("prod", &format!("tier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\n{extra}"))
    }

    #[test]
    fn a_connection_without_a_password_gets_an_item_named_after_it() {
        assert_eq!(keychain_item(&prod("")).unwrap().as_str(), "pgbg/prod");
    }

    #[test]
    fn a_connection_that_names_a_keychain_item_keeps_it() {
        assert_eq!(keychain_item(&prod("password = { keychain = \"shop/main\" }")).unwrap().as_str(), "shop/main");
    }

    #[test]
    fn a_connection_whose_password_comes_from_elsewhere_is_refused() {
        let err = keychain_item(&prod("password = { env = \"PROD_PGPASSWORD\" }")).unwrap_err().to_string();
        assert!(err.contains("`env`"), "{err}");
    }
}
