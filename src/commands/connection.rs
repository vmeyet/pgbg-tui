use crate::cli::{ConnectionKeys, PasswordFlags};
use crate::config::{self, ConnectionChange};
use crate::connection::{ConnectionName, Target, Tier, TunnelSpec};
use crate::secret::{Keychain, KeychainItem, SecretSource};
use anyhow::{Context, Result, anyhow};
use std::path::Path;

/// Adds the connection `name` to the config.
pub fn add(name: &ConnectionName, tier: Tier, url: &Target, keys: &ConnectionKeys) -> Result<()> {
    let path = config::path();
    let change = ConnectionChange { tier: Some(tier), url: Some(url.clone()), ..change_from(keys)? };
    config::add_connection(&path, name, &change)?;
    println!("added `{name}` to {}", path.display());
    Ok(())
}

/// Sets the keys given on the connection `name`; a keychain item its new password replaces is deleted
/// once the config is written, unless another connection names it.
pub fn edit(name: &ConnectionName, tier: Option<Tier>, url: Option<&Target>, keys: &ConnectionKeys) -> Result<()> {
    let path = config::path();
    let change = ConnectionChange { tier, url: url.cloned(), ..change_from(keys)? };
    let replaced = match &change.password {
        Some(source) => config::replaced_keychain_item(&super::load(&path)?.connections, name, source).cloned(),
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
    match config::keychain_item_to_delete(&connections, name) {
        Some(item) => delete_keychain_item(item),
        None => Ok(()),
    }
}

fn delete_keychain_item(item: &KeychainItem) -> Result<()> {
    Keychain::PGBG.delete(item)?;
    println!("deleted the keychain item `{item}`");
    Ok(())
}

/// `text` split into words as a shell would, quotes kept together; a quoting error names `flag`.
fn words(text: &str, flag: &str) -> Result<Vec<String>> {
    shell_words::split(text).with_context(|| format!("in `{flag}`"))
}

fn tunnel_spec(command: &str) -> Result<TunnelSpec> {
    TunnelSpec::new(words(command, "--tunnel")?, None).context("in `--tunnel`")
}

fn change_from(keys: &ConnectionKeys) -> Result<ConnectionChange> {
    let tunnel = keys.tunnel.as_deref().map(tunnel_spec).transpose()?;
    let password = password_source(&keys.password)?;
    Ok(ConnectionChange {
        tunnel,
        password,
        writes: keys.writes,
        tls: keys.tls,
        statement_timeout: keys.statement_timeout,
        ..ConnectionChange::default()
    })
}

fn password_source(flags: &PasswordFlags) -> Result<Option<SecretSource>> {
    let keychain = flags.keychain.as_deref().map(|item| item.parse().context("in `--keychain`")).transpose()?;
    let env = flags.password_env.clone().map(SecretSource::Env);
    let command = flags.password_command.as_deref().map(|text| words(text, "--password-command")).transpose()?.map(SecretSource::Command);
    let source = env.or(command).or(keychain.map(SecretSource::Keychain));
    match source.as_ref().and_then(SecretSource::problem) {
        Some(problem) => Err(anyhow!(problem).context("in the password flag")),
        None => Ok(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(password_command: Option<&str>) -> PasswordFlags {
        PasswordFlags { password_env: None, password_command: password_command.map(str::to_owned), keychain: None }
    }

    #[test]
    fn a_password_command_is_split_as_a_shell_would() {
        let source = password_source(&flags(Some("pass  show 'shop main'"))).unwrap();
        assert_eq!(source, Some(SecretSource::Command(vec!["pass".into(), "show".into(), "shop main".into()])));
    }

    #[test]
    fn a_password_command_with_an_open_quote_is_refused_naming_its_flag() {
        assert!(format!("{:#}", password_source(&flags(Some("pass 'shop"))).unwrap_err()).contains("`--password-command`"));
    }

    #[test]
    fn a_keychain_item_that_is_not_one_word_is_refused_naming_its_flag() {
        let flags = PasswordFlags { keychain: Some("pgbg/prod -s other".into()), ..flags(None) };
        assert!(format!("{:#}", password_source(&flags).unwrap_err()).contains("`--keychain`"));
    }
}
