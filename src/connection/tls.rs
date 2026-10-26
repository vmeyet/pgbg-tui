use super::{Target, TunnelSpec};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

const VERIFIED_SSL_MODES: [&str; 3] = ["require", "verify-ca", "verify-full"];
const TLS_MISSING: &str =
    "the host is not local and `url` has no `sslmode=require`: add it, or set `tls = \"off\"` to send everything in clear on purpose";
const TLS_MISSING_THROUGH_TUNNEL: &str = "the connection goes through a tunnel and `url` has no `sslmode=require`: add it, so TLS checks the server at the tunnel's end, or set `tls = \"off\"` to send everything in clear on purpose";
const TLS_CONTRADICTED: &str = "`tls = \"off\"` contradicts the `sslmode` of `url`: keep one";

/// Whether a session talks TLS, with the server's certificate checked, or in clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Tls {
    On,
    Off,
}

/// TLS when `url` asks for it or `setting` says `on`; clear only when `setting` says `off`, or to a local host
/// reached straight: through a tunnel, whatever answers on its local port could be another process.
pub(super) fn tls(target: &Target, setting: Option<Tls>, tunnel: Option<&TunnelSpec>) -> Result<Tls> {
    let asks_tls = target.ssl_mode().is_some_and(|mode| VERIFIED_SSL_MODES.contains(&mode.as_str()));
    match setting {
        Some(Tls::Off) if asks_tls => bail!(TLS_CONTRADICTED),
        Some(setting) => Ok(setting),
        None if asks_tls => Ok(Tls::On),
        None if tunnel.is_some() => bail!(TLS_MISSING_THROUGH_TUNNEL),
        None if target.is_local() => Ok(Tls::Off),
        None => bail!(TLS_MISSING),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tls_of(url: &str, setting: Option<Tls>) -> Result<Tls> {
        tls(&Target::parse(url)?, setting, None)
    }

    fn tunnelled_tls_of(url: &str, setting: Option<Tls>) -> Result<Tls> {
        let tunnel: TunnelSpec = toml::from_str("command = [\"ssh\", \"-N\", \"-L\", \"{port}:localhost:5432\", \"db.example.com\"]")?;
        tls(&Target::parse(url)?, setting, Some(&tunnel))
    }

    #[test]
    fn a_local_host_behind_a_tunnel_without_sslmode_is_refused() {
        let err = tunnelled_tls_of("postgres://nina@localhost/shop", None).unwrap_err().to_string();
        assert!(err.contains("tunnel") && err.contains("sslmode=require"), "{err}");
    }

    #[test]
    fn a_local_host_behind_a_tunnel_may_talk_in_clear_on_purpose() {
        assert_eq!(tunnelled_tls_of("postgres://nina@localhost/shop", Some(Tls::Off)).unwrap(), Tls::Off);
    }

    #[test]
    fn a_host_behind_a_tunnel_with_sslmode_require_turns_tls_on() {
        assert_eq!(tunnelled_tls_of("postgres://nina@localhost/shop?sslmode=require", None).unwrap(), Tls::On);
    }

    #[test]
    fn sslmode_require_or_stronger_turns_tls_on() {
        assert_eq!(tls_of("postgres://nina@db.example.com/shop?sslmode=require", None).unwrap(), Tls::On);
        assert_eq!(tls_of("postgres://nina@db.example.com/shop?sslmode=verify-full", None).unwrap(), Tls::On);
    }

    #[test]
    fn a_local_host_without_sslmode_talks_in_clear() {
        assert_eq!(tls_of("postgres://nina@localhost:5432/shop", None).unwrap(), Tls::Off);
        assert_eq!(tls_of("postgres://nina@127.0.0.1/shop", None).unwrap(), Tls::Off);
        assert_eq!(tls_of("postgres://nina@[::1]/shop", None).unwrap(), Tls::Off);
        assert_eq!(tls_of("postgres:///shop?host=/var/run/postgresql", None).unwrap(), Tls::Off);
    }

    #[test]
    fn a_remote_host_without_sslmode_is_refused() {
        assert!(tls_of("postgres://nina@db.example.com/shop", None).unwrap_err().to_string().contains("sslmode=require"));
        assert!(tls_of("postgres://nina@db.example.com/shop?sslmode=prefer", None).is_err());
    }

    #[test]
    fn a_remote_host_named_in_the_query_is_not_local() {
        assert!(tls_of("postgres:///shop?host=db.example.com", None).is_err());
    }

    #[test]
    fn tls_off_on_purpose_talks_in_clear_to_a_remote_host() {
        assert_eq!(tls_of("postgres://nina@db.example.com/shop", Some(Tls::Off)).unwrap(), Tls::Off);
    }

    #[test]
    fn tls_off_beside_sslmode_require_is_refused() {
        assert!(tls_of("postgres://nina@db.example.com/shop?sslmode=require", Some(Tls::Off)).is_err());
    }
}
