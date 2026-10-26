//! The macOS keychain through `/usr/bin/security`, service `pgbg`, one item per keychain account.
//! Reads name the service and account in argv, which hold nothing secret; the password comes back on
//! a pipe. Writes go through `security -i`, so the password never shows in argv.
//! Items are owned by Apple's signed binary, so a rebuilt pgbg never triggers an access prompt.
//! An error gives `security`'s exit status only: what it prints may hold the password.
use super::Secret;
use anyhow::{Context, Result, anyhow, bail, ensure};
use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};

const SECURITY: &str = "/usr/bin/security";
const RUNNING_SECURITY: &str = "running /usr/bin/security for the macOS keychain";
const NOT_FOUND: i32 = 44;
/// `security -i` cuts a longer line and runs the rest as a command.
const LINE_LIMIT_BYTES: usize = 4096;
const PASSWORD_LINE: &str = "password: ";
const HEX_PREFIX: &str = "0x";

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
            .args(["find-generic-password", "-a", account, "-s", self.service, "-g"])
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
        let dump = String::from_utf8(output.stderr).map_err(|_password_bytes| not_utf8(account))?;
        from_dump(&dump).with_context(|| format!("reading the keychain item `{account}`"))
    }

    /// Saves `password` as the item `account`, over the one already there: safe to run twice.
    /// Written as hex through `security -i`, so it never shows in argv and is never read from the terminal.
    pub(crate) fn write(&self, account: &str, password: &Secret) -> Result<()> {
        let line = format!("add-generic-password -a {} -s {} -U -X {}\n", plain(account)?, self.service, hex(password.expose()));
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
    pub(crate) fn delete(&self, account: &str) -> Result<()> {
        let status = Command::new(SECURITY)
            .args(["delete-generic-password", "-a", account, "-s", self.service])
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

/// Whether `security -i`, which splits its line on spaces and quotes, reads `account` as one word:
/// nothing in it can end the account and start another option.
pub(crate) fn is_plain(account: &str) -> bool {
    !account.is_empty() && account.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '/'))
}

fn plain(account: &str) -> Result<&str> {
    ensure!(is_plain(account), "not a keychain item name: {account:?}");
    Ok(account)
}

fn failed(doing: &str, account: &str, status: ExitStatus) -> anyhow::Error {
    anyhow!("{doing} the keychain item `{account}` failed: /usr/bin/security {status}")
}

fn not_utf8(account: &str) -> anyhow::Error {
    anyhow!("the keychain item `{account}` is not UTF-8")
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
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
        let err = Keychain::PGBG.write("pgbg/prod", &Secret("x".repeat(2100))).unwrap_err().to_string();
        assert!(err.contains("too long") && !err.contains("78"), "{err}");
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
        keychain.write("probe", &Secret("é\ttwo".into())).unwrap();
        assert_eq!(keychain.read("probe").await.unwrap().expose(), "é\ttwo");
        keychain.delete("probe").unwrap();
        keychain.delete("probe").unwrap();
        assert!(keychain.read("probe").await.unwrap_err().to_string().contains("no keychain item `probe`"));
    }
}
