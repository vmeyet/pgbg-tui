//! The keychain items the connections of a config name, and which of them one connection owns alone.
use crate::connection::{Connection, ConnectionName};
use crate::secret::{KeychainItem, SecretSource};

fn keychain_item(connection: &Connection) -> Option<&KeychainItem> {
    match connection.password() {
        Some(SecretSource::Keychain(item)) => Some(item),
        _ => None,
    }
}

/// The keychain item of the connection `name`, when it has one no other connection names.
pub(crate) fn keychain_item_to_delete<'a>(connections: &'a [Connection], name: &ConnectionName) -> Option<&'a KeychainItem> {
    let removed = keychain_item(connections.iter().find(|c| c.name() == name)?)?;
    let is_shared = connections.iter().filter(|c| c.name() != name).any(|c| keychain_item(c) == Some(removed));
    (!is_shared).then_some(removed)
}

/// The keychain item of the connection `name` that `new` replaces, when no other connection names it.
pub(crate) fn replaced_keychain_item<'a>(
    connections: &'a [Connection],
    name: &ConnectionName,
    new: &SecretSource,
) -> Option<&'a KeychainItem> {
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
}
