use crate::cli::{ConnectionKeys, PasswordFlags};
use crate::config::{self, ConnectionChange};
use crate::connection::{Connection, ConnectionName, Target, Tier, TunnelSpec};
use crate::secret::{Keychain, KeychainItem, SecretSource};
use anyhow::{Context, Result, anyhow};
use std::path::Path;

/// Adds the connection `name` to the config.
pub fn add(name: &ConnectionName, tier: Tier, url: &str, keys: &ConnectionKeys) -> Result<()> {
    let path = config::path();
    let change = ConnectionChange { tier: Some(tier), url: Some(target(url)?), ..change_from(keys)? };
    config::add_connection(&path, name, &change)?;
    println!("added `{name}` to {}", path.display());
    Ok(())
}

/// Sets the keys given on the connection `name`; a keychain item its new password replaces is deleted
/// once the config is written, unless another connection names it.
pub fn edit(name: &ConnectionName, tier: Option<Tier>, url: Option<&str>, keys: &ConnectionKeys) -> Result<()> {
    let path = config::path();
    let change = ConnectionChange { tier, url: url.map(target).transpose()?, ..change_from(keys)? };
    let replaced = match &change.password {
        Some(source) => replaced_keychain_item(&super::load(&path)?.connections, name, source).cloned(),
        None => None,
    };
    config::edit_connection(&path, name, &change)?;
    println!("changed `{name}` in {}", path.display());
    if let Some(item) = replaced {
        delete_keychain_item(&item)?;
    }
    Ok(())
}

/// Removes the connection `name`, and its keychain item unless `keep_password` or another connection
/// names it. The item goes first, so a run that stops halfway is finished by running it again.
/// With `keep_password` the rest of the config need not load, so a broken connection can go.
pub fn remove(name: &ConnectionName, keep_password: bool) -> Result<()> {
    let path = config::path();
    if !keep_password {
        delete_its_keychain_item(&path, name)?;
    }
    config::remove_connection(&path, name)?;
    println!("removed `{name}` from {}", path.display());
    Ok(())
}

fn delete_its_keychain_item(path: &Path, name: &ConnectionName) -> Result<()> {
    let connections = super::load(path).context("add `--keep-password` to remove it without its keychain item")?.connections;
    if !connections.iter().any(|c| c.name() == name) {
        return Err(super::no_connection(name, path));
    }
    match keychain_item_to_delete(&connections, name) {
        Some(item) => delete_keychain_item(item),
        None => Ok(()),
    }
}

fn delete_keychain_item(item: &KeychainItem) -> Result<()> {
    Keychain::PGBG.delete(item)?;
    println!("deleted the keychain item `{item}`");
    Ok(())
}

fn target(url: &str) -> Result<Target> {
    Target::parse(url).context("in `--url`")
}

fn change_from(keys: &ConnectionKeys) -> Result<ConnectionChange> {
    let tunnel = match keys.tunnel.as_slice() {
        [] => None,
        command => Some(TunnelSpec::new(command.to_vec()).context("in `--tunnel`")?),
    };
    Ok(ConnectionChange { tunnel, password: password_source(&keys.password)?, writes: keys.writes, ..ConnectionChange::default() })
}

fn password_source(flags: &PasswordFlags) -> Result<Option<SecretSource>> {
    let keychain = flags.keychain.as_deref().map(|item| item.parse().context("in `--keychain`")).transpose()?;
    let env = flags.password_env.clone().map(SecretSource::Env);
    let command = (!flags.password_command.is_empty()).then(|| SecretSource::Command(flags.password_command.clone()));
    let source = env.or(command).or(keychain.map(SecretSource::Keychain));
    match source.as_ref().and_then(SecretSource::problem) {
        Some(problem) => Err(anyhow!(problem).context("in the password flag")),
        None => Ok(source),
    }
}

fn keychain_item(connection: &Connection) -> Option<&KeychainItem> {
    match connection.password() {
        Some(SecretSource::Keychain(item)) => Some(item),
        _ => None,
    }
}

/// The keychain item of the connection `name`, when it has one no other connection names.
fn keychain_item_to_delete<'a>(connections: &'a [Connection], name: &ConnectionName) -> Option<&'a KeychainItem> {
    let removed = keychain_item(connections.iter().find(|c| c.name() == name)?)?;
    let is_shared = connections.iter().filter(|c| c.name() != name).any(|c| keychain_item(c) == Some(removed));
    (!is_shared).then_some(removed)
}

/// The keychain item of the connection `name` that `new` replaces, when no other connection names it.
fn replaced_keychain_item<'a>(connections: &'a [Connection], name: &ConnectionName, new: &SecretSource) -> Option<&'a KeychainItem> {
    keychain_item_to_delete(connections, name).filter(|&old| *new != SecretSource::Keychain(old.clone()))
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

    fn item(name: &str) -> KeychainItem {
        name.parse().unwrap()
    }

    fn flags(password_command: &[&str]) -> PasswordFlags {
        let password_command = password_command.iter().map(|&word| word.to_owned()).collect();
        PasswordFlags { password_env: None, password_command, keychain: None }
    }

    #[test]
    fn a_removed_connection_takes_its_keychain_item_with_it() {
        let connections = [connection("prod", "password = { keychain = \"pgbg/prod\" }"), connection("local", "")];
        assert_eq!(keychain_item_to_delete(&connections, &name("prod")), Some(&item("pgbg/prod")));
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
    fn a_new_password_source_replaces_the_keychain_item_it_had() {
        let connections = [connection("prod", "password = { keychain = \"pgbg/prod\" }")];
        let new = SecretSource::Keychain(item("shop/main"));
        assert_eq!(replaced_keychain_item(&connections, &name("prod"), &new), Some(&item("pgbg/prod")));
    }

    #[test]
    fn the_same_keychain_item_again_replaces_nothing() {
        let connections = [connection("prod", "password = { keychain = \"pgbg/prod\" }")];
        assert_eq!(replaced_keychain_item(&connections, &name("prod"), &SecretSource::Keychain(item("pgbg/prod"))), None);
    }

    #[test]
    fn a_password_command_keeps_each_word_as_given() {
        let source = password_source(&flags(&["pass", "show", "shop main"])).unwrap();
        assert_eq!(source, Some(SecretSource::Command(vec!["pass".into(), "show".into(), "shop main".into()])));
    }

    #[test]
    fn a_keychain_item_that_is_not_one_word_is_refused_naming_its_flag() {
        let flags = PasswordFlags { keychain: Some("pgbg/prod -s other".into()), ..flags(&[]) };
        assert!(format!("{:#}", password_source(&flags).unwrap_err()).contains("`--keychain`"));
    }
}
