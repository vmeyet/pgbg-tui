//! The config file, changed off the loop through the same `config` functions the command line uses.
//! After each change the connections are read back and sent as `ConfigAnswer::Changed`, with the id of the change.
use super::super::app::{Answer, ConfigAnswer, ConfigChangeId, KeychainPassword, SaveConnection, Saving};
use crate::config::{self, Config};
use crate::connection::ConnectionName;
use crate::secret::{Keychain, KeychainItem};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// The handle to the task that changes the config one change at a time, so answers come back in the order asked.
#[derive(Debug)]
pub(in crate::tui) struct ConfigFile {
    changes: UnboundedSender<Change>,
}

/// One change to the config, waiting for the change before it.
enum Change {
    Remove { id: ConfigChangeId, name: ConnectionName },
    Save(Box<SaveConnection>),
}

impl ConfigFile {
    /// Starts the task that changes the config at `path`, and sends the outcome of each change on `answers`.
    pub(super) fn start(path: PathBuf, answers: UnboundedSender<Answer>) -> Self {
        let (changes, queue) = mpsc::unbounded_channel();
        tokio::spawn(change_in_turn(path, answers, queue));
        Self { changes }
    }

    /// Removes the connection `name` as `pgbg connection remove` does, first deleting its keychain item when no other
    /// connection names it.
    pub(super) fn remove(&self, id: ConfigChangeId, name: ConnectionName) {
        self.queue(Change::Remove { id, name });
    }

    /// Writes the connection as `pgbg connection add|edit` does, then the typed password to its keychain item as
    /// `pgbg password set` does, so a change the config refuses never touches the keychain.
    /// An edit that names a new item then deletes the old item when no other connection names it.
    pub(super) fn save(&self, save: SaveConnection) {
        self.queue(Change::Save(Box::new(save)));
    }

    fn queue(&self, change: Change) {
        if self.changes.send(change).is_err() {
            tracing::warn!("the config task is gone: a change is dropped");
        }
    }
}

/// Makes each change on a blocking thread, since changes touch files and processes, and starts the next only once the
/// last one has answered.
async fn change_in_turn(path: PathBuf, answers: UnboundedSender<Answer>, mut queue: UnboundedReceiver<Change>) {
    while let Some(change) = queue.recv().await {
        let id = change.id();
        let path = path.clone();
        let answer = tokio::task::spawn_blocking(move || change.made(&path)).await;
        send(&answers, id, answer.unwrap_or_else(|err| ConfigAnswer::Failed { why: format!("{err}"), password: None }));
    }
}

impl Change {
    fn id(&self) -> ConfigChangeId {
        match self {
            Self::Remove { id, .. } => *id,
            Self::Save(save) => save.id,
        }
    }

    fn made(self, path: &Path) -> ConfigAnswer {
        match self {
            Self::Remove { name, .. } => changed(path, |path| remove(path, &name)),
            Self::Save(save) => save_answer(path, *save, &Keychain::PGBG),
        }
    }
}

/// The outcome of `save`: when the config refuses it, the typed password goes back so the form keeps it.
/// Once the config takes it, the connections come back, with a warning when the keychain step failed.
fn save_answer(path: &Path, save: SaveConnection, keychain: &Keychain) -> ConfigAnswer {
    let replaced = match written(path, &save) {
        Ok(replaced) => replaced,
        Err(err) => return ConfigAnswer::Failed { why: format!("{err:#}"), password: save.password.map(|typed| typed.password) },
    };
    let warning = keychain_followed(&save, replaced, keychain).err().map(|err| format!("saved `{}`, but {err:#}", save.name));
    match changed(path, |_| Ok(())) {
        ConfigAnswer::Changed { connections, .. } => ConfigAnswer::Changed { connections, warning },
        failed @ ConfigAnswer::Failed { .. } => failed,
    }
}

/// Writes the connection, and returns the keychain item that an edit stops naming and that no other connection names.
fn written(path: &Path, save: &SaveConnection) -> Result<Option<KeychainItem>> {
    let replaced = replaced_item(path, save)?;
    write_connection(path, save)?;
    Ok(replaced)
}

/// Writes the typed password to its item, then deletes the item `replaced`.
fn keychain_followed(save: &SaveConnection, replaced: Option<KeychainItem>, keychain: &Keychain) -> Result<()> {
    if let Some(KeychainPassword { item, password }) = &save.password {
        keychain.write(item, password).context("the password did not reach the keychain")?;
    }
    let Some(item) = replaced else { return Ok(()) };
    keychain.delete(&item).with_context(|| format!("the keychain item `{item}` it no longer names was not deleted"))
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

/// The outcome of `change`: the connections read back, or why it failed.
fn changed(path: &Path, change: impl FnOnce(&Path) -> Result<()>) -> ConfigAnswer {
    match change(path).and_then(|()| Config::read(path)) {
        Ok(loaded) => ConfigAnswer::Changed { connections: loaded.config.connections, warning: None },
        Err(err) => ConfigAnswer::Failed { why: format!("{err:#}"), password: None },
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
        let ConfigAnswer::Changed { connections, .. } = answer else { panic!("{answer:?}") };
        assert_eq!(connections.iter().map(|c| c.name().to_string()).collect::<Vec<_>>(), ["local"]);
    }

    #[tokio::test]
    async fn changes_are_answered_in_the_order_they_were_asked() {
        let (_dir, path) = config(SHOP);
        let (sender, mut answers) = mpsc::unbounded_channel();
        let file = ConfigFile::start(path, sender);
        let change = config::ConnectionChange {
            tier: Some(crate::connection::Tier::Local),
            url: Some(crate::connection::Target::parse("postgres://nina@localhost/shop_copy").unwrap()),
            ..Default::default()
        };
        let name: ConnectionName = "copy".parse().unwrap();
        file.save(SaveConnection { id: ConfigChangeId::FIRST, name: name.clone(), saving: Saving::Add, change, password: None });
        file.remove(ConfigChangeId::FIRST.next(), name);
        let mut counts = vec![];
        for _ in 0..2 {
            let Some(Answer::Config { id, answer: ConfigAnswer::Changed { connections, .. } }) = answers.recv().await else {
                panic!("not changed")
            };
            counts.push((id, connections.len()));
        }
        assert_eq!(counts, [(ConfigChangeId::FIRST, 3), (ConfigChangeId::FIRST.next(), 2)]);
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
        let ConfigAnswer::Changed { connections, .. } = save_answer(&path, save, &Keychain::TEST) else { panic!("not saved") };
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
        let answer = save_answer(&path, save, &Keychain::TEST);
        let read = Keychain::TEST.read(&item).await;
        Keychain::TEST.delete(&item).unwrap();
        assert!(matches!(&answer, ConfigAnswer::Failed { why, .. } if why.contains("already")), "{answer:?}");
        assert!(read.is_err(), "the keychain item was written");
    }

    #[test]
    fn a_save_the_keychain_refuses_still_takes_the_config_change_and_says_so() {
        let (_dir, path) = config(SHOP);
        let item: KeychainItem = "probe-too-long".parse().unwrap();
        let mut typed = crate::secret::Secret::default();
        "x".repeat(5000).chars().for_each(|c| typed.push(c));
        let change = config::ConnectionChange {
            tier: Some(crate::connection::Tier::Local),
            url: Some(crate::connection::Target::parse("postgres://nina@localhost/shop_archive").unwrap()),
            password: Some(crate::secret::SecretSource::Keychain(item.clone())),
            ..Default::default()
        };
        let password = Some(KeychainPassword { item, password: typed });
        let save = SaveConnection { id: ConfigChangeId::FIRST, name: "archive".parse().unwrap(), saving: Saving::Add, change, password };
        let answer = save_answer(&path, save, &Keychain::TEST);
        let ConfigAnswer::Changed { connections, warning: Some(warning) } = answer else { panic!("{answer:?}") };
        assert_eq!(connections.len(), 3);
        assert!(warning.contains("saved `archive`") && warning.contains("too long"), "{warning}");
    }

    #[test]
    fn a_failed_save_hands_back_the_password_typed() {
        let (_dir, path) = config(SHOP);
        let mut typed = crate::secret::Secret::default();
        "hunter2".chars().for_each(|c| typed.push(c));
        let item: KeychainItem = "probe-handed-back".parse().unwrap();
        let change = config::ConnectionChange { password: Some(crate::secret::SecretSource::Keychain(item.clone())), ..Default::default() };
        let password = Some(KeychainPassword { item, password: typed });
        let save = SaveConnection { id: ConfigChangeId::FIRST, name: "local".parse().unwrap(), saving: Saving::Add, change, password };
        let answer = save_answer(&path, save, &Keychain::TEST);
        assert!(matches!(&answer, ConfigAnswer::Failed { password: Some(back), .. } if back.expose() == "hunter2"), "{answer:?}");
    }

    #[test]
    fn a_connection_the_config_does_not_hold_is_a_failure_and_the_file_stays() {
        let (_dir, path) = config(SHOP);
        let answer = changed(&path, |path| remove(path, &"prod".parse().unwrap()));
        assert!(matches!(&answer, ConfigAnswer::Failed { why, .. } if why.contains("prod")), "{answer:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SHOP);
    }
}
