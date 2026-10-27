//! A named database target from the config, parsed once: code above it never checks it again.
use crate::secret::SecretSource;
use anyhow::{Result, bail};
use password::PasswordEntry;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
pub(crate) use target::Target;
pub(crate) use timeout::{StatementTimeout, TimeoutChoice};
pub(crate) use tls::Tls;
pub(crate) use tunnel::TunnelSpec;

mod password;
mod target;
mod timeout;
mod tls;
mod tunnel;

const PLAIN_PASSWORD: &str = "`password` holds a plain password: pgbg never keeps one in the config; use `{ keychain = \"...\" }`, `{ env = \"...\" }` or `{ command = [...] }`";

/// A connection's table in the config, before it is checked.
#[derive(Deserialize)]
pub(crate) struct Entry {
    tier: Tier,
    url: String,
    #[serde(default)]
    password: Option<PasswordEntry>,
    #[serde(default)]
    tunnel: Option<TunnelSpec>,
    #[serde(default)]
    writes: WritePolicy,
    #[serde(default)]
    tls: Option<Tls>,
    #[serde(default)]
    statement_timeout: Option<String>,
    #[serde(default)]
    schema: SchemaReading,
}

/// A named database target: where it is, how to reach it, how to log in, and whether it takes writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Connection {
    name: ConnectionName,
    tier: Tier,
    url: Target,
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel: Option<TunnelSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<SecretSource>,
    writes: WritePolicy,
    tls: Tls,
    /// The `tls` key as the config says it, which `tls` was read from.
    #[serde(skip)]
    tls_setting: Option<Tls>,
    statement_timeout: TimeoutChoice,
    schema: SchemaReading,
}

/// The name a connection has under `[connections.<name>]`, typed on the command line.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ConnectionName(String);

/// Whether the TUI reads a connection's schema, through a second login, the first time the sidebar or completion needs it.
/// `schema = "off"` saves that login where a pooler or `CONNECTION LIMIT` has no slot to spare.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SchemaReading {
    #[default]
    On,
    Off,
}

/// The kind of database a connection reaches, which sets its colour and its default timeout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Local,
    Staging,
    Production,
}

/// Read only unless the connection says `writes = "allowed"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum WritePolicy {
    #[default]
    ReadOnly,
    Allowed,
}

impl Connection {
    /// The connection `[connections.<name>]` describes, or why it is refused.
    pub(crate) fn new(name: &str, entry: Entry) -> Result<Self> {
        let Entry { tier, url, password, tunnel, writes, tls, statement_timeout, schema } = entry;
        let url = Target::parse(&url)?;
        let statement_timeout = statement_timeout.as_deref().map(StatementTimeout::parse).transpose()?;
        Ok(Self {
            name: ConnectionName::new(name)?,
            tier,
            tls: tls::tls(&url, tls, tunnel.as_ref())?,
            tls_setting: tls,
            url,
            tunnel,
            password: password.map(secret_source).transpose()?,
            writes,
            statement_timeout: statement_timeout.map_or_else(|| TimeoutChoice::Tier(tier.statement_timeout()), TimeoutChoice::Own),
            schema,
        })
    }

    pub(crate) fn name(&self) -> &ConnectionName {
        &self.name
    }

    pub(crate) fn tier(&self) -> Tier {
        self.tier
    }

    pub(crate) fn target(&self) -> &Target {
        &self.url
    }

    pub(crate) fn tunnel(&self) -> Option<&TunnelSpec> {
        self.tunnel.as_ref()
    }

    pub(crate) fn password(&self) -> Option<&SecretSource> {
        self.password.as_ref()
    }

    pub(crate) fn writes(&self) -> WritePolicy {
        self.writes
    }

    pub(crate) fn tls(&self) -> Tls {
        self.tls
    }

    /// The `tls` key the config has, if any.
    pub(crate) fn tls_setting(&self) -> Option<Tls> {
        self.tls_setting
    }

    /// The connection's own `statement_timeout`, else its tier's.
    pub(crate) fn statement_timeout(&self) -> TimeoutChoice {
        self.statement_timeout
    }

    pub(crate) fn schema(&self) -> SchemaReading {
        self.schema
    }
}

impl Tier {
    /// The tier's default timeout: none on a local database, and the shortest on production, where a slow statement costs the most.
    fn statement_timeout(self) -> StatementTimeout {
        match self {
            Self::Local => StatementTimeout::NONE,
            Self::Staging => StatementTimeout::STAGING,
            Self::Production => StatementTimeout::PRODUCTION,
        }
    }
}

fn secret_source(entry: PasswordEntry) -> Result<SecretSource> {
    match entry {
        PasswordEntry::Plain => bail!(PLAIN_PASSWORD),
        PasswordEntry::Source(source) => Ok(source),
    }
}

impl ConnectionName {
    fn new(name: &str) -> Result<Self> {
        let is_typable = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !is_typable {
            bail!("`{name}` is not a connection name: use letters, digits, `-`, `_` and `.`");
        }
        Ok(Self(name.to_owned()))
    }
}

impl FromStr for ConnectionName {
    type Err = anyhow::Error;

    fn from_str(name: &str) -> Result<Self> {
        Self::new(name)
    }
}

impl TryFrom<String> for ConnectionName {
    type Error = anyhow::Error;

    fn try_from(name: String) -> Result<Self> {
        Self::new(&name)
    }
}

impl From<ConnectionName> for String {
    fn from(name: ConnectionName) -> Self {
        name.0
    }
}

impl fmt::Display for ConnectionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Local => "local",
            Self::Staging => "staging",
            Self::Production => "production",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(name: &str, text: &str) -> Result<Connection> {
        Connection::new(name, toml::from_str(text)?)
    }

    fn shop(extra: &str) -> Result<Connection> {
        connection("shop", &format!("tier = \"staging\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\n{extra}"))
    }

    #[test]
    fn a_connection_keeps_its_name_tier_and_target_and_is_read_only_by_default() {
        let shop = shop("").unwrap();
        assert_eq!(shop.name().to_string(), "shop");
        assert_eq!(shop.tier(), Tier::Staging);
        assert_eq!(shop.target().to_string(), "postgres://nina@db.example.com/shop?sslmode=require");
        assert_eq!(shop.tls(), Tls::On);
        assert_eq!(shop.writes(), WritePolicy::ReadOnly);
        assert!(shop.tunnel().is_none() && shop.password().is_none());
    }

    #[test]
    fn a_staging_connection_may_allow_writes() {
        assert_eq!(shop("writes = \"allowed\"").unwrap().writes(), WritePolicy::Allowed);
    }

    #[test]
    fn a_production_connection_may_allow_writes() {
        let text = "tier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\nwrites = \"allowed\"\n";
        assert_eq!(connection("shop", text).unwrap().writes(), WritePolicy::Allowed);
    }

    #[test]
    fn a_password_source_is_kept_for_later() {
        let shop = shop("password = { keychain = \"pgbg/shop\" }").unwrap();
        assert_eq!(shop.password(), Some(&SecretSource::Keychain("pgbg/shop".parse().unwrap())));
    }

    #[test]
    fn a_plain_password_is_refused_without_repeating_it() {
        let err = shop("password = \"hunter2\"").unwrap_err().to_string();
        assert!(err.contains("plain password") && !err.contains("hunter2"), "{err}");
    }

    #[test]
    fn a_password_source_that_names_nothing_is_refused() {
        assert!(shop("password = { command = [] }").is_err());
    }

    #[test]
    fn an_unknown_password_source_is_refused() {
        let err = shop("password = { vault = \"shop\" }").unwrap_err().to_string();
        assert!(err.contains("vault") && err.contains("keychain"), "{err}");
    }

    #[test]
    fn a_connection_keeps_its_tunnel() {
        let shop = shop("tunnel = { command = [\"ssh\", \"-N\", \"-L\", \"{port}:localhost:5432\", \"db.example.com\"] }").unwrap();
        assert!(shop.tunnel().is_some());
    }

    #[test]
    fn a_name_the_command_line_cannot_type_is_refused() {
        assert!(connection("my shop", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n").is_err());
        assert!(connection("", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n").is_err());
        assert!(connection("shop-eu_1.ro", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n").is_ok());
    }

    #[test]
    fn each_tier_has_its_statement_timeout() {
        let local = connection("shop", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n").unwrap();
        let production = connection("shop", "tier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\n");
        assert_eq!(local.statement_timeout(), TimeoutChoice::Tier(StatementTimeout::NONE));
        assert_eq!(shop("").unwrap().statement_timeout(), TimeoutChoice::Tier(StatementTimeout::STAGING));
        assert_eq!(production.unwrap().statement_timeout(), TimeoutChoice::Tier(StatementTimeout::PRODUCTION));
    }

    #[test]
    fn a_connection_may_set_its_own_statement_timeout() {
        let own = TimeoutChoice::Own(StatementTimeout::parse("2min").unwrap());
        assert_eq!(shop("statement_timeout = \"2min\"").unwrap().statement_timeout(), own);
    }

    #[test]
    fn a_statement_timeout_that_does_not_parse_is_refused() {
        assert!(shop("statement_timeout = \"soon\"").unwrap_err().to_string().contains("statement_timeout"));
    }

    #[test]
    fn a_connection_reads_its_schema_unless_it_says_off() {
        assert_eq!((shop("").unwrap().schema(), shop("schema = \"off\"").unwrap().schema()), (SchemaReading::On, SchemaReading::Off));
    }

    #[test]
    fn an_unknown_schema_setting_is_refused() {
        assert!(shop("schema = \"lazy\"").is_err());
    }

    #[test]
    fn an_unknown_tier_is_refused() {
        assert!(connection("shop", "tier = \"qa\"\nurl = \"postgres://nina@localhost/shop\"\n").is_err());
    }
}
