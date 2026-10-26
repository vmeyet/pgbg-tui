use crate::cli::{ConnectionKeys, PasswordFlags};
use crate::config::{self, ConnectionChange};
use crate::connection::{Connection, ConnectionName};
use crate::secret::{Keychain, SecretSource};
use anyhow::Result;

/// Adds the connection `name` to the config.
pub fn add(name: &ConnectionName, tier: &str, url: &str, keys: &ConnectionKeys) -> Result<()> {
    let path = config::path();
    let change = ConnectionChange { tier: Some(tier.to_owned()), url: Some(url.to_owned()), ..change_from(keys) };
    config::add_connection(&path, name, &change)?;
    println!("added `{name}` to {}", path.display());
    Ok(())
}

/// Sets the keys given on the connection `name`.
pub fn edit(name: &ConnectionName, tier: Option<&str>, url: Option<&str>, keys: &ConnectionKeys) -> Result<()> {
    let path = config::path();
    let change = ConnectionChange { tier: tier.map(str::to_owned), url: url.map(str::to_owned), ..change_from(keys) };
    config::edit_connection(&path, name, &change)?;
    println!("changed `{name}` in {}", path.display());
    Ok(())
}

/// Removes the connection `name`, and its keychain item unless `keep_password` or another connection
/// names it. The item goes first, so a run that stops halfway is finished by running it again.
pub fn remove(name: &ConnectionName, keep_password: bool) -> Result<()> {
    let path = config::path();
    let connections = super::load(&path)?.connections;
    if !connections.iter().any(|c| c.name() == name) {
        return Err(super::no_connection(name, &path));
    }
    let item = if keep_password { None } else { keychain_item_to_delete(&connections, name) };
    if let Some(item) = item {
        Keychain::PGBG.delete(item)?;
        println!("deleted the keychain item `{item}`");
    }
    config::remove_connection(&path, name)?;
    println!("removed `{name}` from {}", path.display());
    Ok(())
}

fn change_from(keys: &ConnectionKeys) -> ConnectionChange {
    ConnectionChange {
        tunnel: keys.tunnel.as_deref().map(words),
        password: password_source(&keys.password),
        writes: keys.writes.clone(),
        ..ConnectionChange::default()
    }
}

fn password_source(flags: &PasswordFlags) -> Option<SecretSource> {
    let env = flags.password_env.clone().map(SecretSource::Env);
    let command = flags.password_command.as_deref().map(|command| SecretSource::Command(words(command)));
    let keychain = flags.keychain.clone().map(SecretSource::Keychain);
    env.or(command).or(keychain)
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

/// The keychain item of the connection `name`, when it has one no other connection names.
fn keychain_item_to_delete<'a>(connections: &'a [Connection], name: &ConnectionName) -> Option<&'a str> {
    let item = |connection: &'a Connection| match connection.password() {
        Some(SecretSource::Keychain(item)) => Some(item.as_str()),
        _ => None,
    };
    let removed = item(connections.iter().find(|c| c.name() == name)?)?;
    let is_shared = connections.iter().filter(|c| c.name() != name).any(|c| item(c) == Some(removed));
    (!is_shared).then_some(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(name: &str, password: &str) -> Connection {
        let text = format!("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n{password}");
        Connection::new(name, toml::from_str(&text).unwrap()).unwrap()
    }

    fn name(name: &str) -> ConnectionName {
        name.parse().unwrap()
    }

    #[test]
    fn a_removed_connection_takes_its_keychain_item_with_it() {
        let connections = [connection("prod", "password = { keychain = \"pgbg/prod\" }"), connection("local", "")];
        assert_eq!(keychain_item_to_delete(&connections, &name("prod")), Some("pgbg/prod"));
    }

    #[test]
    fn a_keychain_item_another_connection_names_stays() {
        let shared = "password = { keychain = \"pgbg/shop\" }";
        let connections = [connection("prod", shared), connection("replica", shared)];
        assert_eq!(keychain_item_to_delete(&connections, &name("prod")), None);
    }

    #[test]
    fn a_password_from_elsewhere_leaves_the_keychain_alone() {
        let connections = [connection("prod", "password = { env = \"PROD_PGPASSWORD\" }")];
        assert_eq!(keychain_item_to_delete(&connections, &name("prod")), None);
    }

    #[test]
    fn a_password_command_is_split_on_spaces() {
        let flags = PasswordFlags { password_env: None, password_command: Some("pass  show shop".into()), keychain: None };
        assert_eq!(password_source(&flags), Some(SecretSource::Command(vec!["pass".into(), "show".into(), "shop".into()])));
    }
}
