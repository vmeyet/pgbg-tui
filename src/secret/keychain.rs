//! The macOS keychain through `/usr/bin/security`, service `pgbg`, one item per keychain account.
//! Reads name the service and account in argv, which hold nothing secret; the password comes back on
//! a pipe. Writes go through `security -i`, so the password never shows in argv.
//! Items are owned by Apple's signed binary, so a rebuilt pgbg never triggers an access prompt.
//! An error gives `security`'s exit status only: what it prints may hold the password.
use super::Secret;
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};
use std::str::FromStr;
use zeroize::Zeroizing;

const SECURITY: &str = "/usr/bin/security";
const RUNNING_SECURITY: &str = "running /usr/bin/security for the macOS keychain";
const NOT_FOUND: i32 = 44;
/// `security -i` cuts a longer line and runs the rest as a command.
const LINE_LIMIT_BYTES: usize = 4096;
const PASSWORD_LINE: &str = "password: ";
const HEX_PREFIX: &str = "0x";

const NO_ITEM: &str = "`keychain` names no item";
const NOT_AN_ITEM: &str = "`keychain` names an item with other than letters, digits, `-`, `_`, `.`, `:` and `/`";

/// The name of one keychain item, which `security -i` reads as one word: nothing in it can end the
/// account and start another option.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct KeychainItem(String);

impl KeychainItem {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for KeychainItem {
    type Error = &'static str;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        if name.is_empty() {
            return Err(NO_ITEM);
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '/')) {
            return Err(NOT_AN_ITEM);
        }
        Ok(Self(name))
    }
}

impl FromStr for KeychainItem {
    type Err = anyhow::Error;

    fn from_str(name: &str) -> Result<Self> {
        Self::try_from(name.to_owned()).map_err(|problem| anyhow!(problem))
    }
}

impl From<KeychainItem> for String {
    fn from(item: KeychainItem) -> Self {
        item.0
    }
}

impl fmt::Display for KeychainItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The keychain items of one service.
#[derive(Debug)]
pub(crate) struct Keychain {
    service: &'static str,
}

impl Keychain {
    /// pgbg's own items.
    pub(crate) const PGBG: Self = Self { service: "pgbg" };
    /// A throwaway service, so a test never touches pgbg's own items.
    #[cfg(test)]
    pub(crate) const TEST: Self = Self { service: "pgbg-test" };

    /// The password the item `account` holds.
    pub(crate) async fn read(&self, account: &KeychainItem) -> Result<Secret> {
        let output = tokio::process::Command::new(SECURITY)
            .args(["find-generic-password", "-a", account.as_str(), "-s", self.service, "-g"])
            .stdin(Stdio::null())
            .output()
            .await
            .context(RUNNING_SECURITY)?;
        if output.status.code() == Some(NOT_FOUND) {
            bail!("no keychain item `{account}`: `pgbg password set <connection>` saves one");
        }
        if !output.status.success() {
            return Err(failed("reading", account, output.status));
        }
        let dump = Zeroizing::new(String::from_utf8(output.stderr).map_err(|_password_bytes| not_utf8(account))?);
        from_dump(&dump).with_context(|| format!("reading the keychain item `{account}`"))
    }

    /// Saves `password` as the item `account`, over the one already there: safe to run twice.
    /// Written as hex through `security -i`, so it never shows in argv and is never read from the terminal.
    pub(crate) fn write(&self, account: &KeychainItem, password: &Secret) -> Result<()> {
        let line = Zeroizing::new(format!("add-generic-password -a {account} -s {} -U -X {}\n", self.service, *hex(password.expose())));
        ensure!(line.len() <= LINE_LIMIT_BYTES, "the password is too long for the keychain: {} bytes", password.expose().len());
        let mut child = Command::new(SECURITY)
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context(RUNNING_SECURITY)?;
        let written = child.stdin.take().context("security took no input")?.write_all(line.as_bytes());
        let status = child.wait().context(RUNNING_SECURITY)?;
        written.context("giving the password to /usr/bin/security")?;
        if !status.success() {
            return Err(failed("writing", account, status));
        }
        Ok(())
    }

    /// Deletes the item `account`; an item already gone is not an error, so it is safe to run twice.
    pub(crate) fn delete(&self, account: &KeychainItem) -> Result<()> {
        let status = Command::new(SECURITY)
            .args(["delete-generic-password", "-a", account.as_str(), "-s", self.service])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context(RUNNING_SECURITY)?;
        if status.success() || status.code() == Some(NOT_FOUND) {
            return Ok(());
        }
        Err(failed("deleting", account, status))
    }
}

fn failed(doing: &str, account: &KeychainItem, status: ExitStatus) -> anyhow::Error {
    anyhow!("{doing} the keychain item `{account}` failed: /usr/bin/security {status}")
}

fn not_utf8(account: &KeychainItem) -> anyhow::Error {
    anyhow!("the keychain item `{account}` is not UTF-8")
}

fn hex(text: &str) -> Zeroizing<String> {
    Zeroizing::new(text.bytes().map(|b| format!("{b:02x}")).collect())
}

fn from_hex(digits: &str) -> Option<Vec<u8>> {
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    (0..digits.len()).step_by(2).map(|at| digits.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok())).collect()
}

fn hex_password(encoded: &str) -> Result<String> {
    let digits = encoded.split_whitespace().next().unwrap_or_default();
    let bytes = from_hex(digits).context("security printed a password that is not hex")?;
    Ok(String::from_utf8(bytes)?)
}

fn quoted_password(printed: &str) -> Result<String> {
    let quoted = printed.strip_prefix('"').and_then(|text| text.strip_suffix('"'));
    quoted.map(str::to_owned).context("security printed no quoted password")
}

/// The password `security -g` prints: `password: "hunter2"`, or `password: 0x6869  "hi"` when it is not
/// printable ASCII, the quoted part then escaped and the hex the real bytes.
fn from_dump(dump: &str) -> Result<Secret> {
    let printed = dump.lines().find_map(|line| line.strip_prefix(PASSWORD_LINE)).context("security printed no password")?;
    let password = match printed.strip_prefix(HEX_PREFIX) {
        Some(encoded) => hex_password(encoded)?,
        None => quoted_password(printed)?,
    };
    Ok(Secret(password))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> KeychainItem {
        name.parse().unwrap()
    }

    #[test]
    fn an_item_name_with_a_space_or_a_quote_is_refused() {
        for name in ["pgbg/prod -s other", "pgbg/prod\"", "pgbg/prod'", "pgbg/prod\nlist-keychains"] {
            assert_eq!(name.parse::<KeychainItem>().unwrap_err().to_string(), NOT_AN_ITEM, "{name:?}");
        }
    }

    #[test]
    fn an_empty_item_name_is_refused() {
        assert_eq!("".parse::<KeychainItem>().unwrap_err().to_string(), NO_ITEM);
    }

    #[test]
    fn an_item_name_made_of_a_connection_name_is_one_word() {
        for name in ["pgbg/prod", "pgbg/shop-eu_1.ro", "shop:5432"] {
            assert_eq!(item(name).as_str(), name);
        }
    }

    #[test]
    fn a_printable_password_reads_from_between_its_quotes() {
        assert_eq!(from_dump("password: \"a\"b c\"\n").unwrap().expose(), "a\"b c");
    }

    #[test]
    fn a_password_security_prints_in_hex_reads_back_as_its_text() {
        assert_eq!(from_dump("password: 0xC3A92278  \"\\303\\251\"x\"\n").unwrap().expose(), "é\"x");
    }

    #[test]
    fn a_printable_password_that_starts_like_hex_stays_as_typed() {
        assert_eq!(from_dump("password: \"0xC3A9\"\n").unwrap().expose(), "0xC3A9");
    }

    #[test]
    fn a_dump_without_a_password_line_is_an_error() {
        assert!(from_dump("keychain: \"/Users/nina/login.keychain-db\"\n").is_err());
    }

    #[test]
    fn a_password_too_long_for_one_line_of_security_is_refused_before_it_runs() {
        let err = Keychain::PGBG.write(&item("pgbg/prod"), &Secret("x".repeat(2100))).unwrap_err().to_string();
        assert!(err.contains("too long") && !err.contains("78"), "{err}");
    }

    #[test]
    fn hex_encodes_bytes() {
        assert_eq!(*hex("A{\"} é"), "417b227d20c3a9");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "touches the real login keychain"]
    async fn a_written_password_reads_back_a_second_write_replaces_it_and_a_delete_removes_it() {
        let keychain = Keychain::TEST;
        keychain.write(&item("probe"), &Secret("hunter2 \"one\"".into())).unwrap();
        assert_eq!(keychain.read(&item("probe")).await.unwrap().expose(), "hunter2 \"one\"");
        keychain.write(&item("probe"), &Secret("é\ttwo".into())).unwrap();
        assert_eq!(keychain.read(&item("probe")).await.unwrap().expose(), "é\ttwo");
        keychain.delete(&item("probe")).unwrap();
        keychain.delete(&item("probe")).unwrap();
        assert!(keychain.read(&item("probe")).await.unwrap_err().to_string().contains("no keychain item `probe`"));
    }
}
