//! The macOS keychain through `/usr/bin/security`, service `pgbg`, one item per keychain account.
//! Reads name the service and account in argv, which hold nothing secret; the password comes back on
//! a pipe. Writes go through `security -i`, so the password never shows in argv.
//! Items are owned by Apple's signed binary, so a rebuilt pgbg never triggers an access prompt.
use super::Secret;
use anyhow::{Context, Result, bail, ensure};
use std::io::Write;
use std::process::{Command, Stdio};

const SECURITY: &str = "/usr/bin/security";
const NOT_FOUND: i32 = 44;

/// The keychain items of one service.
#[derive(Debug)]
pub(crate) struct Keychain {
    service: &'static str,
}

impl Keychain {
    /// pgbg's own items.
    pub(crate) const PGBG: Self = Self { service: "pgbg" };

    /// The password the item `account` holds.
    pub(crate) async fn read(&self, account: &str) -> Result<Secret> {
        let output = tokio::process::Command::new(SECURITY)
            .args(["find-generic-password", "-a", account, "-s", self.service, "-w"])
            .stdin(Stdio::null())
            .output()
            .await
            .context("running /usr/bin/security for the macOS keychain")?;
        if output.status.code() == Some(NOT_FOUND) {
            bail!("no keychain item `{account}`: `pgbg password set <connection>` saves one");
        }
        if !output.status.success() {
            bail!("reading the keychain item `{account}` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        let printed =
            String::from_utf8(output.stdout).map_err(|_password_bytes| anyhow::anyhow!("the keychain item `{account}` is not UTF-8"))?;
        Ok(Secret(printed.trim_end_matches(['\n', '\r']).to_owned()))
    }

    /// Saves `password` as the item `account`, over the one already there: safe to run twice.
    /// Written as hex through `security -i`, so it never shows in argv and is never read from the terminal.
    pub(crate) fn write(&self, account: &str, password: &Secret) -> Result<()> {
        let line = format!("add-generic-password -a {} -s {} -U -X {}\n", plain(account)?, self.service, hex(password.expose()));
        let mut child = Command::new(SECURITY)
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("running /usr/bin/security for the macOS keychain")?;
        child.stdin.take().context("security took no input")?.write_all(line.as_bytes())?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!("writing the keychain item `{account}` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(())
    }

    /// Deletes the item `account`; an item already gone is not an error, so it is safe to run twice.
    pub(crate) fn delete(&self, account: &str) -> Result<()> {
        let output = Command::new(SECURITY)
            .args(["delete-generic-password", "-a", account, "-s", self.service])
            .stdin(Stdio::null())
            .output()
            .context("running /usr/bin/security for the macOS keychain")?;
        if output.status.success() || output.status.code() == Some(NOT_FOUND) {
            return Ok(());
        }
        bail!("deleting the keychain item `{account}` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
}

/// Whether `security -i`, which splits its line on spaces and quotes, reads `account` as one word:
/// nothing in it can end the account and start another option.
pub(crate) fn is_plain(account: &str) -> bool {
    !account.is_empty() && account.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '/'))
}

fn plain(account: &str) -> Result<&str> {
    ensure!(is_plain(account), "not a keychain item name: {account:?}");
    Ok(account)
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_name_with_a_space_or_a_quote_is_refused_before_security_runs() {
        for account in ["pgbg/prod -s other", "pgbg/prod\"", "pgbg/prod'", "pgbg/prod\nlist-keychains", ""] {
            let err = Keychain::PGBG.write(account, &Secret("hunter2".into())).unwrap_err().to_string();
            assert!(err.starts_with("not a keychain item name"), "{account:?}: {err}");
        }
    }

    #[test]
    fn an_item_name_made_of_a_connection_name_is_plain() {
        for account in ["pgbg/prod", "pgbg/shop-eu_1.ro", "shop:5432"] {
            assert!(is_plain(account), "{account}");
        }
    }

    #[test]
    fn hex_encodes_bytes() {
        assert_eq!(hex("A{\"} é"), "417b227d20c3a9");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "touches the real login keychain"]
    async fn a_written_password_reads_back_a_second_write_replaces_it_and_a_delete_removes_it() {
        let keychain = Keychain { service: "pgbg-test" };
        keychain.write("probe", &Secret("hunter2 \"one\"".into())).unwrap();
        assert_eq!(keychain.read("probe").await.unwrap().expose(), "hunter2 \"one\"");
        keychain.write("probe", &Secret("two".into())).unwrap();
        assert_eq!(keychain.read("probe").await.unwrap().expose(), "two");
        keychain.delete("probe").unwrap();
        keychain.delete("probe").unwrap();
        assert!(keychain.read("probe").await.unwrap_err().to_string().contains("no keychain item `probe`"));
    }
}
