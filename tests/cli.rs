//! End-to-end runs of the `pgbg` binary against invented configs.
#![cfg(test)]
use rustix::process::{Pid, Signal, kill_process};
use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
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
fn run_refuses_a_param_without_a_value_before_connecting() {
    let output = pgbg(SHOP, &["run", "local", "SELECT * FROM orders WHERE day > {{since}}"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("`{{since}}` has no value"), "{}", stderr(&output));
}

#[test]
fn run_keeps_a_param_in_the_history_never_its_value() {
    let dir = tempfile::tempdir().unwrap();
    pgbg_in(dir.path(), SHOP, &["run", "local", "DELETE FROM orders WHERE owner = {{owner}}", "--param", "owner=nina"]);
    let history = stdout(&pgbg_in(dir.path(), SHOP, &["history", "local", "--json"]));
    assert!(history.contains("{{owner}}") && !history.contains("nina"), "{history}");
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
fn doctor_marks_a_refused_login_and_exits_with_a_failure() {
    let output = pgbg("[connections.shop]\ntier = \"local\"\nurl = \"postgres://nina@localhost:1/shop\"\n", &["doctor", "shop"]);
    assert!(!output.status.success());
    assert!(stdout(&output).lines().any(|line| line.starts_with("✗  login")), "{}", stdout(&output));
    assert!(stderr(&output).contains("1 check failed"), "{}", stderr(&output));
}

#[test]
fn doctor_on_an_unknown_connection_fails_the_config_check_in_json_too() {
    let output = pgbg(SHOP, &["doctor", "shop", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!((&json["checks"][0]["name"], &json["checks"][0]["verdict"]), (&"config".into(), &"fail".into()));
    assert!(!output.status.success());
}

#[test]
#[ignore = "needs PGBG_TEST_DATABASE_URL"]
fn doctor_on_a_reachable_database_finds_nothing_broken() {
    let url = std::env::var("PGBG_TEST_DATABASE_URL").unwrap();
    let output = pgbg(&format!("[connections.shop]\ntier = \"local\"\nurl = \"{url}\"\n"), &["doctor", "shop"]);
    assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("✓  login"), "{}", stdout(&output));
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

/// `pgbg run` on the test database with `stdin` piped in, and the first value of its JSON result.
fn first_value_on_the_test_database(args: &[&str], stdin: &str) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let url = std::env::var("PGBG_TEST_DATABASE_URL").unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, format!("[connections.shop]\ntier = \"local\"\nurl = \"{url}\"\n")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pgbg"))
        .args(["run", "shop", "--json"])
        .args(args)
        .env("PGBG_CONFIG", &path)
        .env("PGBG_LOG", "off")
        .env("PGBG_DATA_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["rows"][0][0].clone()
}

#[test]
#[ignore = "needs PGBG_TEST_DATABASE_URL"]
fn run_binds_a_param_given_on_the_command_line() {
    assert_eq!(first_value_on_the_test_database(&["SELECT {{total}}::int4 + 1", "--param", "total=41"], ""), 42);
}

#[test]
#[ignore = "needs PGBG_TEST_DATABASE_URL"]
fn run_binds_the_default_and_the_type_its_header_declares() {
    assert_eq!(first_value_on_the_test_database(&["--", "-- param: total int4 = 41\nSELECT {{total}} + 1"], ""), 42);
}

#[test]
#[ignore = "needs PGBG_TEST_DATABASE_URL"]
fn run_binds_a_param_read_from_stdin() {
    assert_eq!(first_value_on_the_test_database(&["SELECT {{owner}} || '!'", "--params-file", "-"], "owner=nina\n"), "nina!");
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
const RUN: &[&str] = &["run", "shop", "SELECT 1"];
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

/// pgbg with `args` on `shop`, through `SILENT_TUNNEL`, and the port of that tunnel once it answers.
fn waiting_on_a_tunnel(dir: &Path, args: &[&str]) -> (Child, u16) {
    let written = dir.join("port");
    let config = format!(
        "[connections.shop]\ntier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\ntls = \"off\"\n\
         tunnel = {{ command = [\"python3\", \"-c\", {SILENT_TUNNEL:?}, \"{{port}}\", {:?}] }}\n",
        written.display().to_string()
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, config).unwrap();
    let pgbg = Command::new(env!("CARGO_BIN_EXE_pgbg"))
        .args(args)
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

/// Whether the tunnel of pgbg with `args` that `signal` stopped went with it, and pgbg said which signal it was.
fn a_tunnel_goes_when_pgbg_is_stopped_by(args: &[&str], signal: Signal) {
    let dir = tempfile::tempdir().unwrap();
    let (mut pgbg, port) = waiting_on_a_tunnel(dir.path(), args);
    kill_process(Pid::from_child(&pgbg), signal).unwrap();
    let status = pgbg.wait().unwrap();
    assert_eq!(status.code(), Some(128 + signal.as_raw()));
    assert!(within(TUNNEL_WAIT, || !answers(port)), "the tunnel on port {port} outlived pgbg");
}

#[test]
#[ignore = "starts a tunnel"]
fn ctrl_c_on_pgbg_run_stops_its_tunnel_too() {
    a_tunnel_goes_when_pgbg_is_stopped_by(RUN, Signal::INT);
}

#[test]
#[ignore = "starts a tunnel"]
fn a_kill_of_pgbg_run_stops_its_tunnel_too() {
    a_tunnel_goes_when_pgbg_is_stopped_by(RUN, Signal::TERM);
}

#[test]
#[ignore = "starts a tunnel"]
fn closing_the_terminal_of_pgbg_run_stops_its_tunnel_too() {
    a_tunnel_goes_when_pgbg_is_stopped_by(RUN, Signal::HUP);
}

#[test]
#[ignore = "starts a tunnel"]
fn a_kill_of_pgbg_doctor_stops_its_tunnel_too() {
    a_tunnel_goes_when_pgbg_is_stopped_by(&["doctor", "shop"], Signal::TERM);
}

#[test]
#[ignore = "needs PGBG_TEST_DATABASE_URL"]
fn schema_json_lists_the_tables_the_filter_keeps() {
    let url = std::env::var("PGBG_TEST_DATABASE_URL").unwrap();
    let config = format!("[connections.shop]\ntier = \"local\"\nurl = \"{url}\"\nwrites = \"allowed\"\n");
    let created = pgbg(&config, &["run", "shop", "CREATE TABLE IF NOT EXISTS cli_schema_orders (id int PRIMARY KEY)"]);
    assert!(created.status.success(), "{}", stderr(&created));
    let output = pgbg(&config, &["schema", "shop", "cli_schema_ord", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schemas"][0]["tables"][0]["name"], "cli_schema_orders");
    assert_eq!(json["schemas"][0]["tables"][0]["indexes"][0]["is_primary"], true);
    assert_eq!(json["schemas"][0]["tables"].as_array().unwrap().len(), 1);
}

/// A proxy to `argv[1]:argv[2]` that writes its port to `argv[3]` and keeps the server's side of a link open once the
/// client hangs up, as a pooler or a server that cannot check does: only a cancel request then stops a statement.
const HIDING_HANG_UPS: &str = "
import socket, sys, threading
server = socket.create_server(('127.0.0.1', 0))
open(sys.argv[3], 'w').write(str(server.getsockname()[1]))
links = []
def pipe(source, sink, hides_hang_up):
    try:
        while data := source.recv(65536):
            sink.sendall(data)
    except OSError:
        pass
    if not hides_hang_up:
        sink.close()
while True:
    client, _ = server.accept()
    upstream = socket.create_connection((sys.argv[1], int(sys.argv[2])))
    links.append(upstream)
    threading.Thread(target=pipe, args=(client, upstream, True), daemon=True).start()
    threading.Thread(target=pipe, args=(upstream, client, False), daemon=True).start()
";
const SLOW_INSERT: &str = "INSERT INTO cli_cancelled_orders SELECT 1 FROM pg_sleep(3)";
const ACTIVE_INSERTS: &str =
    "SELECT count(*) FROM pg_stat_activity WHERE state = 'active' AND query LIKE 'INSERT INTO cli_cancelled_orders%'";
const STATEMENT_WAIT: Duration = Duration::from_secs(10);

/// `[connections.<name>]` on `url`, local and allowing writes.
fn writable(name: &str, url: &str) -> String {
    format!("[connections.{name}]\ntier = \"local\"\nurl = \"{url}\"\nwrites = \"allowed\"\n")
}

/// The single value `sql` returns on `shop`, as JSON.
fn value_of(dir: &Path, config: &str, sql: &str) -> serde_json::Value {
    let output = pgbg_in(dir, config, &["run", "shop", sql, "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["rows"][0][0].clone()
}

/// `HIDING_HANG_UPS` in front of `database`, and the URL through it.
fn hiding_hang_ups(dir: &Path, database: &url::Url) -> (Child, url::Url) {
    let written = dir.join("proxy-port");
    let (host, port) = (database.host_str().unwrap().to_owned(), database.port().unwrap_or(5432).to_string());
    let proxy = Command::new("python3").args(["-c", HIDING_HANG_UPS, &host, &port, &written.display().to_string()]).spawn().unwrap();
    assert!(within(TUNNEL_WAIT, || std::fs::read_to_string(&written).is_ok_and(|port| !port.is_empty())));
    let mut proxied = database.clone();
    proxied.set_host(Some("127.0.0.1")).unwrap();
    proxied.set_port(Some(std::fs::read_to_string(&written).unwrap().parse().unwrap())).unwrap();
    (proxy, proxied)
}

#[test]
#[ignore = "needs PGBG_TEST_DATABASE_URL"]
fn ctrl_c_on_pgbg_run_cancels_its_write_even_where_the_server_cannot_see_pgbg_go() {
    let database = url::Url::parse(&std::env::var("PGBG_TEST_DATABASE_URL").unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let direct = writable("shop", database.as_str());
    for sql in ["DROP TABLE IF EXISTS cli_cancelled_orders", "CREATE TABLE cli_cancelled_orders (id int)"] {
        let output = pgbg_in(dir.path(), &direct, &["run", "shop", sql]);
        assert!(output.status.success(), "{}", stderr(&output));
    }
    let (mut proxy, proxied) = hiding_hang_ups(dir.path(), &database);
    let config = dir.path().join("proxied.toml");
    std::fs::write(&config, writable("shop", proxied.as_str())).unwrap();
    let mut pgbg = Command::new(env!("CARGO_BIN_EXE_pgbg"))
        .args(["run", "shop", SLOW_INSERT])
        .env("PGBG_CONFIG", &config)
        .env("PGBG_LOG", "off")
        .env("PGBG_DATA_DIR", dir.path())
        .spawn()
        .unwrap();
    assert!(within(STATEMENT_WAIT, || value_of(dir.path(), &direct, ACTIVE_INSERTS) == 1));
    kill_process(Pid::from_child(&pgbg), Signal::INT).unwrap();
    assert_eq!(pgbg.wait().unwrap().code(), Some(128 + Signal::INT.as_raw()));
    assert!(within(STATEMENT_WAIT, || value_of(dir.path(), &direct, ACTIVE_INSERTS) == 0));
    proxy.kill().unwrap();
    proxy.wait().unwrap();
    assert_eq!(value_of(dir.path(), &direct, "SELECT count(*) FROM cli_cancelled_orders"), 0);
}
