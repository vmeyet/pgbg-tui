//! End-to-end runs of the `pgbg` binary against invented configs.
#![cfg(test)]
use std::process::{Command, Output};

const SHOP: &str = r#"
[connections.local]
tier = "local"
url = "postgres://nina@localhost:5432/shop"

[connections.prod]
tier = "production"
url = "postgres://nina@localhost/shop?sslmode=require"
password = { keychain = "pgbg/prod" }
tunnel = { command = ["ssh", "-N", "-L", "{port}:localhost:5432", "db.example.com"] }
"#;

/// The binary on `config`, with the log off so no run writes to the real cache folder.
fn pgbg(config: &str, args: &[&str]) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, config).unwrap();
    Command::new(env!("CARGO_BIN_EXE_pgbg")).args(args).env("PGBG_CONFIG", &path).env("PGBG_LOG", "off").output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn version_prints_the_name_and_the_crate_version() {
    let output = pgbg("", &["--version"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), format!("pgbg {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn connections_lists_each_one_by_name() {
    let output = pgbg(SHOP, &["connections"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let lines: Vec<String> = stdout(&output).lines().map(str::to_owned).collect();
    assert!(lines[0].starts_with("local ") && lines[1].starts_with("prod "), "{lines:?}");
}

#[test]
fn connections_json_is_an_array_of_connections() {
    let output = pgbg(SHOP, &["connections", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json[1]["name"], "prod");
    assert_eq!(json[1]["tier"], "production");
}

#[test]
fn a_password_in_a_url_stops_pgbg_without_printing_it() {
    let output =
        pgbg("[connections.prod]\ntier = \"production\"\nurl = \"postgres://nina:hunter2@db.example.com/shop\"\n", &["connections"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("connection `prod`") && err.contains("password") && !err.contains("hunter2"), "{err}");
}

#[test]
fn an_unknown_key_warns_on_stderr_and_the_list_still_prints() {
    let output = pgbg(&format!("theme = \"nord\"\n{SHOP}"), &["connections"]);
    assert!(output.status.success());
    assert!(stderr(&output).contains("`theme` is not a setting"), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("local "));
}

#[test]
fn run_refuses_a_write_on_a_read_only_connection_before_connecting() {
    let output = pgbg(SHOP, &["run", "local", "DELETE FROM orders"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("`DELETE` is not a read"), "{}", stderr(&output));
}

#[test]
fn run_on_an_unknown_connection_says_where_to_find_them() {
    let output = pgbg(SHOP, &["run", "shop", "SELECT 1"]);
    assert!(stderr(&output).contains("no connection `shop`") && stderr(&output).contains("pgbg connections"), "{}", stderr(&output));
}

#[test]
#[ignore = "needs PGBG_TEST_DATABASE_URL"]
fn run_json_prints_the_columns_and_rows() {
    let url = std::env::var("PGBG_TEST_DATABASE_URL").unwrap();
    let output = pgbg(
        &format!("[connections.shop]\ntier = \"local\"\nurl = \"{url}\"\n"),
        &["run", "shop", "SELECT 7 AS id, 'nina' AS customer", "--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["columns"][1]["name"], "customer");
    assert_eq!(json["rows"][0], serde_json::json!([7, "nina"]));
}

#[test]
fn tui_on_an_unknown_connection_stops_before_taking_the_terminal() {
    let output = pgbg(SHOP, &["tui", "nope"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("no connection `nope`"), "{}", stderr(&output));
}

#[test]
fn pgbg_alone_without_connections_says_where_to_add_one() {
    let output = pgbg("", &[]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("[connections.<name>]"), "{}", stderr(&output));
}
