//! The config file, changed off the loop through the same `config` functions the command line uses; after each
//! change the connections are read back and sent as `ConfigAnswer::Changed`.
use super::super::app::{Answer, ConfigAnswer};
use crate::config::{self, Config};
use crate::connection::ConnectionName;
use crate::secret::Keychain;
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
    pub(super) fn remove(&self, name: ConnectionName) {
        let (path, answers) = (self.path.clone(), self.answers.clone());
        tokio::task::spawn_blocking(move || send(&answers, changed(&path, |path| remove(path, &name))));
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

fn send(answers: &UnboundedSender<Answer>, answer: ConfigAnswer) {
    if answers.send(Answer::Config(answer)).is_err() {
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
    fn a_connection_the_config_does_not_hold_is_a_failure_and_the_file_stays() {
        let (_dir, path) = config(SHOP);
        let answer = changed(&path, |path| remove(path, &"prod".parse().unwrap()));
        assert!(matches!(&answer, ConfigAnswer::Failed(why) if why.contains("prod")), "{answer:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SHOP);
    }
}
