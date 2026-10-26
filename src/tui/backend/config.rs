//! The config file, changed off the loop through the same `config` functions the command line uses; after each
//! change the connections are read back and sent as `ConfigAnswer::Changed`, with the id of the change.
use super::super::app::{Answer, ConfigAnswer, ConfigChangeId, KeychainPassword, SaveConnection, Saving};
use crate::config::{self, Config};
use crate::connection::ConnectionName;
use crate::secret::{Keychain, KeychainItem};
use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;

/// Where the config is, and the loop's channel for what changing it came to.
#[derive(Debug)]
pub(in crate::tui) struct ConfigFile {
    pub(super) path: PathBuf,
    pub(super) answers: UnboundedSender<Answer>,
}

impl ConfigFile {
    /// Removes the connection `name`, and first the keychain item no other connection names, as
    /// `pgbg connection remove` does; on a blocking thread, since both are files and processes.
    pub(super) fn remove(&self, id: ConfigChangeId, name: ConnectionName) {
        let (path, answers) = (self.path.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || send(&answers, id, changed(&path, |path| remove(path, &name))));
    }

    /// Writes the connection as `pgbg connection add|edit` does, then the password typed to its keychain item as
    /// `pgbg password set` does, so a change the config refuses never touches the keychain; an edit that names a new
    /// item then deletes the old one no other connection names.
    pub(super) fn save(&self, save: SaveConnection) {
        let (path, answers) = (self.path.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || send(&answers, save.id, changed(&path, |path| saved(path, &save, &Keychain::PGBG))));
    }
}

fn saved(path: &Path, save: &SaveConnection, keychain: &Keychain) -> Result<()> {
    let replaced = replaced_item(path, save)?;
    write_connection(path, save)?;
    if let Some(KeychainPassword { item, password }) = &save.password {
        keychain.write(item, password)?;
    }
    replaced.map_or(Ok(()), |item| keychain.delete(&item))
}

/// The keychain item an edit stops naming, which no other connection names; none for a new connection.
fn replaced_item(path: &Path, save: &SaveConnection) -> Result<Option<KeychainItem>> {
    let Some(source) = save.change.password.as_ref().filter(|_| save.saving == Saving::Edit) else { return Ok(None) };
    let connections = Config::read(path)?.config.connections;
    Ok(config::replaced_keychain_item(&connections, &save.name, source).cloned())
}

fn write_connection(path: &Path, save: &SaveConnection) -> Result<()> {
    match save.saving {
        Saving::Add => config::add_connection(path, &save.name, &save.change),
        Saving::Edit => config::edit_connection(path, &save.name, &save.change),
    }
}

fn remove(path: &Path, name: &ConnectionName) -> Result<()> {
    let connections = Config::read(path)?.config.connections;
    if let Some(item) = config::keychain_item_to_delete(&connections, name) {
        Keychain::PGBG.delete(item)?;
    }
    config::remove_connection(path, name)
}

/// What `change` came to: the connections read back, or why it failed.
fn changed(path: &Path, change: impl FnOnce(&Path) -> Result<()>) -> ConfigAnswer {
    match change(path).and_then(|()| Config::read(path)) {
        Ok(loaded) => ConfigAnswer::Changed(loaded.config.connections),
        Err(err) => ConfigAnswer::Failed(format!("{err:#}")),
    }
}

fn send(answers: &UnboundedSender<Answer>, id: ConfigChangeId, answer: ConfigAnswer) {
    if answers.send(Answer::Config { id, answer }).is_err() {
        tracing::warn!("the loop is gone: a config answer is dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = "[connections.local]\ntier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n\n[connections.replica]\ntier = \"local\"\nurl = \"postgres://nina@localhost/shop_replica\"\n";

    fn config(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn a_removed_connection_comes_back_as_the_connections_left() {
        let (_dir, path) = config(SHOP);
        let answer = changed(&path, |path| remove(path, &"replica".parse().unwrap()));
        let ConfigAnswer::Changed(connections) = answer else { panic!("{answer:?}") };
        assert_eq!(connections.iter().map(|c| c.name().to_string()).collect::<Vec<_>>(), ["local"]);
    }

    #[test]
    fn an_added_connection_comes_back_with_the_others() {
        let (_dir, path) = config(SHOP);
        let change = config::ConnectionChange {
            tier: Some(crate::connection::Tier::Staging),
            url: Some(crate::connection::Target::parse("postgres://nina@db.example.com/shop?sslmode=require").unwrap()),
            ..Default::default()
        };
        let save =
            SaveConnection { id: ConfigChangeId::FIRST, name: "staging".parse().unwrap(), saving: Saving::Add, change, password: None };
        let ConfigAnswer::Changed(connections) = changed(&path, |path| saved(path, &save, &Keychain::TEST)) else { panic!("not saved") };
        assert_eq!(connections.len(), 3);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "touches the login keychain, under a throwaway service"]
    async fn adding_a_name_already_taken_fails_before_the_keychain_is_written() {
        let (_dir, path) = config(SHOP);
        let item: KeychainItem = "probe-name-taken".parse().unwrap();
        let mut typed = crate::secret::Secret::default();
        "hunter2".chars().for_each(|c| typed.push(c));
        let password = Some(KeychainPassword { item: item.clone(), password: typed });
        let change = config::ConnectionChange { password: Some(crate::secret::SecretSource::Keychain(item.clone())), ..Default::default() };
        let save = SaveConnection { id: ConfigChangeId::FIRST, name: "local".parse().unwrap(), saving: Saving::Add, change, password };
        let answer = changed(&path, |path| saved(path, &save, &Keychain::TEST));
        let read = Keychain::TEST.read(&item).await;
        Keychain::TEST.delete(&item).unwrap();
        assert!(matches!(&answer, ConfigAnswer::Failed(why) if why.contains("already")), "{answer:?}");
        assert!(read.is_err(), "the keychain item was written");
    }

    #[test]
    fn a_connection_the_config_does_not_hold_is_a_failure_and_the_file_stays() {
        let (_dir, path) = config(SHOP);
        let answer = changed(&path, |path| remove(path, &"prod".parse().unwrap()));
        assert!(matches!(&answer, ConfigAnswer::Failed(why) if why.contains("prod")), "{answer:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SHOP);
    }
}
