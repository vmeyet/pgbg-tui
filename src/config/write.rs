//! Rewrites `config.toml` key by key, so the user's comments and order stay.
//! Only the new text has to load, as `Config::read` loads it, before it replaces the file: so an edit can fix a config that does not load.
//! The helpers below change the `toml_edit` document through `&mut`, since its API only edits in place.
use super::parse;
use crate::connection::{ConnectionName, StatementTimeout, Target, Tier, Tls, TunnelSpec, WritePolicy};
use crate::files::{self, Readers};
use crate::secret::SecretSource;
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};
use toml_edit::ser::ValueSerializer;
use toml_edit::{DocumentMut, InlineTable, Item, Table, TableLike, Value};

/// The keys of a connection to set; a `None` key is left as it is.
/// Each key is already checked on its own; the load that follows checks them together.
/// It serializes as the keys it sets, in the config's spelling and order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct ConnectionChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tier: Option<Tier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<Target>,
    /// Sets the tunnel's `command`, and its `before` when it has one, else keeps the `before` the config already has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tunnel: Option<TunnelSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) password: Option<SecretSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) writes: Option<WritePolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tls: Option<Tls>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) statement_timeout: Option<StatementTimeout>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) queries: Option<ConnectionName>,
}

const TUNNEL: &str = "tunnel";

/// Adds the connection `name` at the end of the config, creating the file when there is none.
pub(crate) fn add_connection(path: &Path, name: &ConnectionName, change: &ConnectionChange) -> Result<()> {
    rewrite(path, |document| {
        let connections = connections_table(document)?;
        if connections.contains_key(&name.to_string()) {
            bail!("`{name}` is already in the config: `pgbg connection edit {name}` changes it");
        }
        let mut table = Table::new();
        apply(&mut table, change)?;
        connections.insert(&name.to_string(), Item::Table(table));
        Ok(())
    })
}

/// Sets the keys `change` gives on the connection `name`; every other key keeps its text.
pub(crate) fn edit_connection(path: &Path, name: &ConnectionName, change: &ConnectionChange) -> Result<()> {
    rewrite(path, |document| apply(connection_table(document, name)?, change))
}

/// Removes the table of the connection `name`, with the comment above it.
pub(crate) fn remove_connection(path: &Path, name: &ConnectionName) -> Result<()> {
    rewrite(path, |document| {
        let removed = connections_table(document)?.remove(&name.to_string());
        removed.map(drop).ok_or_else(|| no_connection(name))
    })
}

/// Sets the `password` of the connection `name` to `source`; every other key keeps its text.
pub(crate) fn set_password(path: &Path, name: &ConnectionName, source: &SecretSource) -> Result<()> {
    let change = ConnectionChange { password: Some(source.clone()), ..ConnectionChange::default() };
    edit_connection(path, name, &change)
}

/// Each key `change` sets, in place of the old one; a tunnel sets only the keys it has.
fn apply(table: &mut dyn TableLike, change: &ConnectionChange) -> Result<()> {
    let Value::InlineTable(keys) = value_of(change)? else { bail!("a connection change is not a table") };
    for (key, value) in &keys {
        match value.as_inline_table().filter(|_| key == TUNNEL) {
            Some(tunnel) => set_tunnel(table, tunnel),
            None => set_value(table, key, value.clone()),
        }
    }
    Ok(())
}

fn value_of(key: &impl Serialize) -> Result<Value> {
    key.serialize(ValueSerializer::new()).context("writing a key of the connection")
}

/// `value` in place of `key`, with the spacing and comment the old value had.
fn set_value(table: &mut dyn TableLike, key: &str, mut value: Value) {
    if let Some(kept) = table.get(key).and_then(Item::as_value) {
        *value.decor_mut() = kept.decor().clone();
    }
    table.insert(key, Item::Value(value));
}

/// The tunnel's keys `keys` sets, keeping the `before` it has when `keys` has none.
fn set_tunnel(table: &mut dyn TableLike, keys: &InlineTable) {
    match table.get_mut(TUNNEL).and_then(Item::as_table_like_mut) {
        Some(tunnel) => keys.iter().for_each(|(key, value)| set_value(tunnel, key, value.clone())),
        None => set_value(table, TUNNEL, Value::InlineTable(keys.clone())),
    }
}

fn connections_table(document: &mut DocumentMut) -> Result<&mut dyn TableLike> {
    let connections = document.entry("connections").or_insert_with(|| {
        let mut implicit = Table::new();
        implicit.set_implicit(true);
        Item::Table(implicit)
    });
    connections.as_table_like_mut().context("`connections` in the config is not a table")
}

fn connection_table<'a>(document: &'a mut DocumentMut, name: &ConnectionName) -> Result<&'a mut dyn TableLike> {
    document
        .get_mut("connections")
        .and_then(Item::as_table_like_mut)
        .and_then(|connections| connections.get_mut(&name.to_string()))
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| no_connection(name))
}

fn no_connection(name: &ConnectionName) -> anyhow::Error {
    anyhow!("no connection `{name}` in the config")
}

fn rewrite(path: &Path, change: impl FnOnce(&mut DocumentMut) -> Result<()>) -> Result<()> {
    let path = resolved(path)?;
    let kept = read_kept(&path)?;
    let mut document: DocumentMut = kept.parse().map_err(|_quotes_the_line| anyhow!("{} is not TOML", path.display()))?;
    change(&mut document)?;
    let text = document.to_string();
    parse(&text).context("the rewritten config would not load")?;
    write_whole(&path, &text)
}

/// The file `path` names once its links are followed, or `path` itself when there is no file yet;
/// a link to no file is refused, since writing would replace the link.
fn resolved(path: &Path) -> Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && path.is_symlink() => {
            bail!("{} links to {}, which does not exist", path.display(), std::fs::read_link(path)?.display())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(path.to_owned()),
        resolved => resolved.with_context(|| format!("finding {}", path.display())),
    }
}

/// The text of the file at `path`, empty when there is no file yet.
pub(crate) fn read_kept(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        read => read.with_context(|| format!("reading {}", path.display())),
    }
}

/// Writes `text` as the config at `path`, which only its owner reads, making its folder when needed.
fn write_whole(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().with_context(|| format!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    files::write_whole(path, text, Readers::Owner).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPED: &str = "# my databases\n[connections.local]\ntier = \"local\" # docker\nurl = \"postgres://nina@localhost:5432/shop\"\n\n# the real one\n[connections.prod]\ntier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\nwrites = \"read-only\" # always\n";

    fn typed_config(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    fn prod() -> ConnectionName {
        "prod".parse().unwrap()
    }

    fn keychain(item: &str) -> SecretSource {
        SecretSource::Keychain(item.parse().unwrap())
    }

    #[test]
    fn setting_a_password_adds_one_key_and_keeps_the_comments_and_order() {
        let (dir, path) = typed_config(TYPED);
        set_password(&path, &prod(), &keychain("pgbg/prod")).unwrap();
        let expected = format!("{TYPED}password = {{ keychain = \"pgbg/prod\" }}\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file is left behind");
    }

    #[test]
    fn setting_the_same_password_twice_changes_nothing_the_second_time() {
        let (_dir, path) = typed_config(TYPED);
        set_password(&path, &prod(), &keychain("pgbg/prod")).unwrap();
        let once = std::fs::read_to_string(&path).unwrap();
        set_password(&path, &prod(), &keychain("pgbg/prod")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), once);
    }

    #[test]
    fn a_password_already_there_is_replaced_where_it_stands() {
        let (_dir, path) = typed_config(
            "[connections.prod]\npassword = { env = \"PROD_PGPASSWORD\" }\ntier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n",
        );
        set_password(&path, &prod(), &SecretSource::Command(vec!["pass".into(), "shop".into()])).unwrap();
        assert!(
            std::fs::read_to_string(&path).unwrap().starts_with("[connections.prod]\npassword = { command = [\"pass\", \"shop\"] }\ntier")
        );
    }

    #[test]
    fn a_connection_the_config_does_not_hold_is_an_error_and_the_file_stays() {
        let (_dir, path) = typed_config(TYPED);
        assert!(set_password(&path, &"staging".parse().unwrap(), &keychain("pgbg/staging")).unwrap_err().to_string().contains("`staging`"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TYPED);
    }

    fn url(text: &str) -> Target {
        Target::parse(text).unwrap()
    }

    #[test]
    fn a_change_that_would_not_load_leaves_the_file_whole() {
        let (_dir, path) = typed_config(TYPED);
        let without_tls = change(|c| c.url = Some(url("postgres://nina@db.example.com/shop")));
        assert!(edit_connection(&path, &prod(), &without_tls).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TYPED);
    }

    #[test]
    fn an_edit_fixes_a_config_that_does_not_load() {
        let (_dir, path) = typed_config(&TYPED.replace("/shop?sslmode=require", "/shop"));
        edit_connection(&path, &prod(), &change(|c| c.url = Some(url("postgres://nina@db.example.com/shop?sslmode=require")))).unwrap();
        assert_eq!(read(&path), TYPED);
    }

    fn change(edit: impl FnOnce(&mut ConnectionChange)) -> ConnectionChange {
        let mut change = ConnectionChange::default();
        edit(&mut change);
        change
    }

    fn staging() -> ConnectionChange {
        change(|c| {
            c.tier = Some(Tier::Staging);
            c.url = Some(url("postgres://nina@localhost:5433/shop?sslmode=require"));
            c.tunnel = Some(tunnel(&["ssh", "-N", "-L", "{port}:localhost:5432", "db.example.com"]));
            c.password = Some(SecretSource::Env("STAGING_PGPASSWORD".into()));
            c.writes = Some(WritePolicy::Allowed);
        })
    }

    fn tunnel(command: &[&str]) -> TunnelSpec {
        TunnelSpec::new(command.iter().map(|&word| word.to_owned()).collect(), None).unwrap()
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn an_added_connection_lands_at_the_end_with_its_keys_in_order() {
        let (_dir, path) = typed_config(TYPED);
        add_connection(&path, &"staging".parse().unwrap(), &staging()).unwrap();
        let added = "\n[connections.staging]\ntier = \"staging\"\nurl = \"postgres://nina@localhost:5433/shop?sslmode=require\"\ntunnel = { command = [\"ssh\", \"-N\", \"-L\", \"{port}:localhost:5432\", \"db.example.com\"] }\npassword = { env = \"STAGING_PGPASSWORD\" }\nwrites = \"allowed\"\n";
        assert_eq!(read(&path), format!("{TYPED}{added}"));
    }

    #[test]
    fn adding_to_no_file_creates_its_folder_and_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pgbg").join("config.toml");
        add_connection(&path, &"staging".parse().unwrap(), &staging()).unwrap();
        assert!(read(&path).starts_with("[connections.staging]\ntier = \"staging\""), "{}", read(&path));
    }

    #[test]
    fn adding_a_connection_already_there_is_refused_and_the_file_stays() {
        let (_dir, path) = typed_config(TYPED);
        let err = add_connection(&path, &prod(), &staging()).unwrap_err().to_string();
        assert!(err.contains("pgbg connection edit prod"), "{err}");
        assert_eq!(read(&path), TYPED);
    }

    #[test]
    fn adding_a_connection_that_would_not_load_leaves_the_file_whole() {
        let (_dir, path) = typed_config(TYPED);
        let without_url = change(|c| c.tier = Some(Tier::Staging));
        assert!(add_connection(&path, &"staging".parse().unwrap(), &without_url).is_err());
        assert_eq!(read(&path), TYPED);
    }

    #[test]
    fn editing_sets_only_the_keys_given_and_keeps_their_comments() {
        let (_dir, path) = typed_config(TYPED);
        edit_connection(&path, &"local".parse().unwrap(), &change(|c| c.tier = Some(Tier::Staging))).unwrap();
        assert_eq!(read(&path), TYPED.replace("tier = \"local\" # docker", "tier = \"staging\" # docker"));
    }

    #[test]
    fn editing_the_statement_timeout_writes_it_as_the_config_spells_it() {
        let (_dir, path) = typed_config(TYPED);
        edit_connection(&path, &prod(), &change(|c| c.statement_timeout = Some("2min".parse().unwrap()))).unwrap();
        assert!(read(&path).ends_with("statement_timeout = \"2min\"\n"), "{}", read(&path));
    }

    #[test]
    fn editing_queries_writes_the_name_of_the_connection_it_shares() {
        let (_dir, path) = typed_config(TYPED);
        edit_connection(&path, &"local".parse().unwrap(), &change(|c| c.queries = Some(prod()))).unwrap();
        assert!(read(&path).contains("url = \"postgres://nina@localhost:5432/shop\"\nqueries = \"prod\"\n"), "{}", read(&path));
    }

    #[test]
    fn editing_tls_writes_it_as_the_config_spells_it() {
        let (_dir, path) = typed_config(TYPED);
        edit_connection(&path, &"local".parse().unwrap(), &change(|c| c.tls = Some(Tls::On))).unwrap();
        assert!(read(&path).contains("url = \"postgres://nina@localhost:5432/shop\"\ntls = \"on\"\n"), "{}", read(&path));
    }

    #[test]
    fn editing_the_tunnel_command_keeps_its_before_command() {
        let (_dir, path) = typed_config(&format!("{TYPED}tunnel = {{ command = [\"proxy\", \"{{port}}\"], before = [\"wake\"] }}\n"));
        edit_connection(&path, &prod(), &change(|c| c.tunnel = Some(tunnel(&["ssh", "{port}"])))).unwrap();
        assert!(read(&path).ends_with("tunnel = { command = [\"ssh\", \"{port}\"], before = [\"wake\"] }\n"), "{}", read(&path));
    }

    #[test]
    fn editing_the_tunnel_with_a_before_command_sets_both() {
        let (_dir, path) = typed_config(&format!("{TYPED}tunnel = {{ command = [\"proxy\", \"{{port}}\"], before = [\"wake\"] }}\n"));
        let tunnel = TunnelSpec::new(vec!["ssh".into(), "{port}".into()], Some(vec!["start".into()])).unwrap();
        edit_connection(&path, &prod(), &change(|c| c.tunnel = Some(tunnel))).unwrap();
        assert!(read(&path).ends_with("tunnel = { command = [\"ssh\", \"{port}\"], before = [\"start\"] }\n"), "{}", read(&path));
    }

    #[test]
    fn removing_a_connection_drops_its_table_and_the_comment_above_it() {
        let (_dir, path) = typed_config(TYPED);
        remove_connection(&path, &prod()).unwrap();
        assert_eq!(read(&path), TYPED.split("\n# the real one").next().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn a_config_path_that_links_to_no_file_is_refused_and_the_link_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::os::unix::fs::symlink(dir.path().join("dotfiles").join("config.toml"), &path).unwrap();
        let err = add_connection(&path, &"staging".parse().unwrap(), &staging()).unwrap_err().to_string();
        assert!(err.contains("links to"), "{err}");
        assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn the_rewritten_file_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = typed_config(TYPED);
        set_password(&path, &prod(), &keychain("pgbg/prod")).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
