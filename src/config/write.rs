//! Rewrites `config.toml` key by key, so the user's comments and order stay; the result must load
//! as `Config::read` loads it before it replaces the file.
use super::parse;
use crate::connection::ConnectionName;
use crate::secret::SecretSource;
use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use std::path::Path;
use toml_edit::ser::ValueSerializer;
use toml_edit::{DocumentMut, Item, TableLike};

/// Sets the `password` of the connection `name` to `source`; every other key keeps its text.
pub(crate) fn set_password(path: &Path, name: &ConnectionName, source: &SecretSource) -> Result<()> {
    rewrite(path, |document| {
        let value = source.serialize(ValueSerializer::new()).context("writing the password source")?;
        connection_table(document, name)?.insert("password", Item::Value(value));
        Ok(())
    })
}

fn connection_table<'a>(document: &'a mut DocumentMut, name: &ConnectionName) -> Result<&'a mut dyn TableLike> {
    document
        .get_mut("connections")
        .and_then(Item::as_table_like_mut)
        .and_then(|connections| connections.get_mut(&name.to_string()))
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| anyhow!("no connection `{name}` in the config"))
}

fn rewrite(path: &Path, change: impl FnOnce(&mut DocumentMut) -> Result<()>) -> Result<()> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_missing| path.to_owned());
    let kept = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    parse(&kept).with_context(|| format!("in {}", path.display()))?;
    let mut document: DocumentMut = kept.parse().map_err(|_quotes_the_line| anyhow!("{} is not TOML", path.display()))?;
    change(&mut document)?;
    let text = document.to_string();
    parse(&text).context("the rewritten config would not load")?;
    write_whole(&path, &text)
}

/// Through a temp file only its owner reads, then a rename: the file is always the old one or the new one.
fn write_whole(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().context("the config path has no folder")?;
    let temp = tempfile::Builder::new().prefix(".tmp-").tempfile_in(dir).context("creating a temp file")?;
    std::fs::write(temp.path(), text)?;
    temp.persist(path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        SecretSource::Keychain(item.into())
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

    #[test]
    fn a_change_that_would_not_load_leaves_the_file_whole() {
        let (_dir, path) = typed_config(TYPED);
        assert!(set_password(&path, &prod(), &keychain("pgbg/prod -s other")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TYPED);
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
