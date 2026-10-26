//! End-to-end runs of the `pgbg` binary against invented configs.
#![cfg(test)]
use rustix::process::{Pid, Signal, kill_process};
use std::net::{Ipv4Addr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

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
    pgbg_rewriting(config, args).0
}

/// The binary on `config`, and the config's text once it ran.
fn pgbg_rewriting(config: &str, args: &[&str]) -> (Output, String) {
    let dir = tempfile::tempdir().unwrap();
    let output = pgbg_in(dir.path(), config, args);
    (output, std::fs::read_to_string(dir.path().join("config.toml")).unwrap_or_default())
}

/// The binary on `config`, written in `dir`, which also holds its data folder, so no run writes the real history.
fn pgbg_in(dir: &Path, config: &str, args: &[&str]) -> Output {
    let path = dir.join("config.toml");
    std::fs::write(&path, config).unwrap();
    let command = Command::new(env!("CARGO_BIN_EXE_pgbg"))
        .args(args)
        .env("PGBG_CONFIG", &path)
        .env("PGBG_LOG", "off")
        .env("PGBG_DATA_DIR", dir)
        .output();
    command.unwrap()
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
fn an_error_prints_the_control_characters_it_quotes_escaped() {
    let output = pgbg("[connections.\"shop\\u001b[2J\"]\ntier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n", &["connections"]);
    let err = stderr(&output);
    assert!(err.contains("shop\\u{1b}[2J") && !err.contains("shop\u{1b}"), "{err:?}");
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
fn run_adds_the_statement_to_the_history_of_its_connection() {
    let dir = tempfile::tempdir().unwrap();
    pgbg_in(dir.path(), SHOP, &["run", "local", "DELETE FROM orders"]);
    let output = pgbg_in(dir.path(), SHOP, &["history", "local", "--json"]);
    let history: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!((&history[0]["statement"], &history[0]["outcome"]["kind"]), (&"DELETE FROM orders".into(), &"refused".into()));
}

#[test]
fn history_of_a_connection_never_run_prints_nothing() {
    let output = pgbg(SHOP, &["history", "local"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
}

#[test]
fn run_on_an_unknown_connection_says_where_to_find_them() {
    let output = pgbg(SHOP, &["run", "shop", "SELECT 1"]);
    assert!(stderr(&output).contains("no connection `shop`") && stderr(&output).contains("pgbg connections"), "{}", stderr(&output));
}

#[test]
fn run_through_a_tunnel_that_dies_says_why_before_connecting() {
    let config = "[connections.shop]\ntier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n\
                  tunnel = { command = [\"sh\", \"-c\", \"echo 'bastion.example.com: no route' >&2; exit 255\", \"{port}\"] }\n";
    let output = pgbg(config, &["run", "shop", "SELECT 1"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("tunnel `sh` died") && stderr(&output).contains("no route"), "{}", stderr(&output));
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
    assert_eq!(json["is_cut"], false);
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

#[test]
fn password_set_refuses_a_connection_whose_password_comes_from_elsewhere_before_reading_one() {
    let output = pgbg(
        "[connections.staging]\ntier = \"staging\"\nurl = \"postgres://nina@localhost/shop\"\npassword = { env = \"STAGING_PGPASSWORD\" }\n",
        &["password", "set", "staging"],
    );
    assert!(!output.status.success());
    assert!(stderr(&output).contains("not the keychain"), "{}", stderr(&output));
}

#[test]
fn password_set_on_an_unknown_connection_says_where_to_find_them() {
    let output = pgbg(SHOP, &["password", "set", "nope"]);
    assert!(stderr(&output).contains("no connection `nope`"), "{}", stderr(&output));
}

#[test]
fn connection_add_writes_a_connection_the_list_then_shows() {
    let args = [
        "connection",
        "add",
        "staging",
        "--tier",
        "staging",
        "--url",
        "postgres://nina@localhost:5433/shop?sslmode=require",
        "--tunnel",
        "ssh -N -o 'ProxyCommand ssh -W %h:%p jump' -L {port}:localhost:5432 db.example.com",
        "--password-env",
        "STAGING_PGPASSWORD",
    ];
    let (output, config) = pgbg_rewriting(SHOP, &args);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(config.starts_with(SHOP), "{config}");
    assert!(config.contains("tunnel = { command = [\"ssh\", \"-N\", \"-o\", \"ProxyCommand ssh -W %h:%p jump\", \"-L\", \"{port}:localhost:5432\", \"db.example.com\"] }"), "{config}");
    assert!(config.ends_with("password = { env = \"STAGING_PGPASSWORD\" }\n"), "{config}");
    assert!(stdout(&pgbg(&config, &["connections"])).contains("staging     postgres://nina@localhost:5433/shop?sslmode=require"));
}

#[test]
fn connection_add_refuses_a_password_in_the_url_and_leaves_the_file() {
    let (output, config) =
        pgbg_rewriting(SHOP, &["connection", "add", "staging", "--tier", "staging", "--url", "postgres://nina:hunter2@localhost/shop"]);
    assert!(!output.status.success());
    assert!(!stderr(&output).contains("hunter2") && stderr(&output).contains("`--url`"), "{}", stderr(&output));
    assert_eq!(config, SHOP);
}

#[test]
fn connection_remove_with_keep_password_drops_a_connection_that_does_not_load() {
    let broken = format!("{SHOP}\n[connections.old]\ntier = \"staging\"\nurl = \"postgres://nina@db.example.com/shop\"\n");
    let (output, config) = pgbg_rewriting(&broken, &["connection", "remove", "old", "--keep-password"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(config, SHOP);
}

#[test]
fn connection_remove_of_a_config_that_does_not_load_says_how_to_keep_going() {
    let broken = format!("{SHOP}\n[connections.old]\ntier = \"staging\"\nurl = \"postgres://nina@db.example.com/shop\"\n");
    let output = pgbg(&broken, &["connection", "remove", "old"]);
    assert!(stderr(&output).contains("--keep-password"), "{}", stderr(&output));
}

#[test]
fn connection_remove_with_keep_password_drops_only_the_table() {
    let (output, config) = pgbg_rewriting(SHOP, &["connection", "remove", "prod", "--keep-password"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!stdout(&output).contains("keychain"), "{}", stdout(&output));
    assert_eq!(config, SHOP.split("\n[connections.prod]").next().unwrap());
}

/// A tunnel that writes its port to `argv[2]` and takes every link without a word, so `pgbg run` waits on it.
const SILENT_TUNNEL: &str = "
import socket, sys
server = socket.create_server(('127.0.0.1', int(sys.argv[1])))
open(sys.argv[2], 'w').write(sys.argv[1])
links = []
while True:
    links.append(server.accept())
";
const TUNNEL_WAIT: Duration = Duration::from_secs(10);

fn answers(port: u16) -> bool {
    TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok()
}

fn within(wait: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + wait;
    while !done() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// `pgbg run` through `SILENT_TUNNEL`, and the port of that tunnel once it answers.
fn waiting_on_a_tunnel(dir: &Path) -> (Child, u16) {
    let written = dir.join("port");
    let config = format!(
        "[connections.shop]\ntier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n\
         tunnel = {{ command = [\"python3\", \"-c\", {SILENT_TUNNEL:?}, \"{{port}}\", {:?}] }}\n",
        written.display().to_string()
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, config).unwrap();
    let pgbg = Command::new(env!("CARGO_BIN_EXE_pgbg"))
        .args(["run", "shop", "SELECT 1"])
        .env("PGBG_CONFIG", &path)
        .env("PGBG_LOG", "off")
        .env("PGBG_DATA_DIR", dir)
        .spawn()
        .unwrap();
    assert!(within(TUNNEL_WAIT, || std::fs::read_to_string(&written).is_ok_and(|port| !port.is_empty())));
    let port = std::fs::read_to_string(&written).unwrap().parse().unwrap();
    assert!(within(TUNNEL_WAIT, || answers(port)));
    (pgbg, port)
}

/// Whether the tunnel of a `pgbg run` that `signal` stopped went with it, and pgbg said which signal it was.
fn a_tunnel_goes_when_pgbg_is_stopped_by(signal: Signal) {
    let dir = tempfile::tempdir().unwrap();
    let (mut pgbg, port) = waiting_on_a_tunnel(dir.path());
    kill_process(Pid::from_child(&pgbg), signal).unwrap();
    let status = pgbg.wait().unwrap();
    assert_eq!(status.code(), Some(128 + signal.as_raw()));
    assert!(within(TUNNEL_WAIT, || !answers(port)), "the tunnel on port {port} outlived pgbg");
}

#[test]
#[ignore = "starts a tunnel"]
fn ctrl_c_on_pgbg_run_stops_its_tunnel_too() {
    a_tunnel_goes_when_pgbg_is_stopped_by(Signal::INT);
}

#[test]
#[ignore = "starts a tunnel"]
fn a_kill_of_pgbg_run_stops_its_tunnel_too() {
    a_tunnel_goes_when_pgbg_is_stopped_by(Signal::TERM);
}

#[test]
#[ignore = "starts a tunnel"]
fn closing_the_terminal_of_pgbg_run_stops_its_tunnel_too() {
    a_tunnel_goes_when_pgbg_is_stopped_by(Signal::HUP);
}
