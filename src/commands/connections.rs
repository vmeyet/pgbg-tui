use crate::config;
use crate::connection::{Connection, WritePolicy};
use crate::render;
use anyhow::Result;
use std::path::Path;

/// Prints the connections of the config: a plain table, or JSON with `json`.
pub fn run(json: bool) -> Result<()> {
    let path = config::path();
    let config = super::load(&path)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&config.connections)?);
        return Ok(());
    }
    print!("{}", text(&config.connections, &path));
    Ok(())
}

fn text(connections: &[Connection], path: &Path) -> String {
    if connections.is_empty() {
        return format!("no connections in {}\n", path.display());
    }
    render::table(&connections.iter().map(row).collect::<Vec<_>>())
}

fn row(connection: &Connection) -> Vec<String> {
    vec![connection.name().to_string(), connection.tier().to_string(), connection.target().to_string(), notes(connection)]
}

/// `tunnel · password from keychain · writes allowed`, each part only when it applies.
fn notes(connection: &Connection) -> String {
    let tunnel = connection.tunnel().map(|_| "tunnel".to_owned());
    let password = connection.password().map(|source| format!("password from {}", source.kind()));
    let writes = (connection.writes() == WritePolicy::Allowed).then(|| "writes allowed".to_owned());
    [tunnel, password, writes].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const SHOP: &str = r#"
[connections.local]
tier = "local"
url = "postgres://nina@localhost:5432/shop"
writes = "allowed"

[connections.prod]
tier = "production"
url = "postgres://nina@localhost/shop?sslmode=require"
password = { keychain = "pgbg/prod" }
tunnel = { command = ["ssh", "-N", "-L", "{port}:localhost:5432", "db.example.com"] }
"#;

    fn connections() -> Vec<Connection> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, SHOP).unwrap();
        Config::read(&path).unwrap().config.connections
    }

    #[test]
    fn each_connection_prints_its_name_tier_target_and_how_it_is_reached() {
        assert_eq!(
            text(&connections(), Path::new("config.toml")),
            "local  local       postgres://nina@localhost:5432/shop             writes allowed\n\
             prod   production  postgres://nina@localhost/shop?sslmode=require  tunnel · password from keychain\n"
        );
    }

    #[test]
    fn no_connection_says_which_file_was_read() {
        assert_eq!(text(&[], Path::new("/tmp/shop.toml")), "no connections in /tmp/shop.toml\n");
    }

    #[test]
    fn json_mirrors_the_config() {
        let json = serde_json::to_value(connections()).unwrap();
        assert_eq!(json[0]["writes"], "allowed");
        assert_eq!(json[1]["name"], "prod");
        assert_eq!(json[1]["tier"], "production");
        assert_eq!(json[1]["writes"], "read-only");
        assert_eq!(json[1]["password"]["keychain"], "pgbg/prod");
        assert_eq!(json[1]["tunnel"]["command"][3], "{port}:localhost:5432");
        assert!(json[0].get("tunnel").is_none());
    }
}
