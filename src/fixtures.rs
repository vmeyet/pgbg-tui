//! Invented fixtures the tests share: the connection `shop`, the test database, tunnels to it, and results of `orders`.
use crate::connection::{Connection, Entry, WritePolicy};
use crate::db::model::{Column, ColumnKind, Extent, Index, Page, ResultSet, Schema, SchemaColumn, SchemaNode, Table, TableKind, Value};
use rustix::process::{Pid, test_kill_process};
use std::path::Path;
use std::time::Duration;

/// The environment variable naming the throwaway database the `#[ignore]`d tests run against.
const DATABASE_URL: &str = "PGBG_TEST_DATABASE_URL";

/// A forwarder from `127.0.0.1:argv[1]` to `argv[2]:argv[3]`, for tunnels in tests.
pub(crate) const FORWARD: &str = "
import socket, sys, threading
def pipe(source, sink):
    while data := source.recv(65536):
        sink.sendall(data)
    sink.shutdown(socket.SHUT_WR)
server = socket.create_server(('127.0.0.1', int(sys.argv[1])))
while True:
    client, _ = server.accept()
    upstream = socket.create_connection((sys.argv[2], int(sys.argv[3])))
    for source, sink in ((client, upstream), (upstream, client)):
        threading.Thread(target=pipe, args=(source, sink), daemon=True).start()
";

/// How long a test waits on a child process: to write its pid, or to be gone.
const PROCESS_WAIT: Duration = Duration::from_secs(5);
const PROCESS_POLL: Duration = Duration::from_millis(20);

/// A shell line that starts a `sleep` it does not wait on, as `gcloud` starts helpers, writes that `sleep`'s pid to
/// `pid_file`, then waits for it.
pub(crate) fn leaving_a_sleep(pid_file: &Path) -> Vec<String> {
    let line = "sleep 60 & echo $! > \"$1\"; wait";
    ["sh", "-c", line, "sh"].into_iter().map(str::to_owned).chain([pid_file.display().to_string()]).collect()
}

/// Drops `work` once the command it runs wrote its `sleep`'s pid to `pid_file`, then says whether that `sleep` went too.
pub(crate) async fn sleep_goes_when_dropped<T>(work: impl Future<Output = T>, pid_file: &Path) -> bool {
    let written = async {
        loop {
            if let Some(pid) = std::fs::read_to_string(pid_file).ok().and_then(|pid| pid.trim().parse().ok()).and_then(Pid::from_raw) {
                return pid;
            }
            tokio::time::sleep(PROCESS_POLL).await;
        }
    };
    let pid = tokio::select! {
        _ = work => panic!("the command ended on its own"),
        pid = tokio::time::timeout(PROCESS_WAIT, written) => pid.unwrap(),
    };
    let gone = async {
        while test_kill_process(pid).is_ok() {
            tokio::time::sleep(PROCESS_POLL).await;
        }
    };
    tokio::time::timeout(PROCESS_WAIT, gone).await.is_ok()
}

/// The connection named `name` that the TOML table `entry` describes.
pub(crate) fn named(name: &str, entry: &str) -> Connection {
    let entry: Entry = toml::from_str(entry).unwrap();
    Connection::new(name, entry).unwrap()
}

/// The connection `shop` that the TOML table `entry` describes.
pub(crate) fn connection(entry: &str) -> Connection {
    named("shop", entry)
}

pub(crate) fn test_database_url() -> String {
    std::env::var(DATABASE_URL).unwrap()
}

/// The connection `name` on the test database, a local one.
pub(crate) fn test_database_as(name: &str) -> Connection {
    named(name, &format!("tier = \"local\"\nurl = \"{}\"\n", test_database_url()))
}

/// `shop` on the test database, of `tier`, with the keys of `extra` too.
pub(crate) fn test_database(tier: &str, extra: &str) -> Connection {
    connection(&format!("tier = \"{tier}\"\nurl = \"{}\"\n{extra}", test_database_url()))
}

/// `shop` on the test database logged in as `user`, a local one, with the keys of `extra` too.
pub(crate) fn test_database_by(user: &str, extra: &str) -> Connection {
    let mut url = url::Url::parse(&test_database_url()).unwrap();
    url.set_username(user).unwrap();
    connection(&format!("tier = \"local\"\nurl = \"{url}\"\n{extra}"))
}

/// `shop` on the test database through a forwarder whose command line carries `marker`, so a test can find it.
pub(crate) fn tunnelled(marker: &str) -> Connection {
    through_forwarder("\"python3\", \"-c\"", marker, WritePolicy::ReadOnly)
}

/// Like `tunnelled`, the forwarder started once the shell line `first` succeeds, a tunnel that died when it fails.
pub(crate) fn tunnelled_after(marker: &str, first: &str, writes: WritePolicy) -> Connection {
    let script = format!("{first} || exit 1; exec python3 -c \"$0\" \"$@\"");
    through_forwarder(&format!("\"sh\", \"-c\", {script:?}"), marker, writes)
}

/// `shop` on the test database through a tunnel that runs `program`, then the forwarder's code and arguments.
fn through_forwarder(program: &str, marker: &str, writes: WritePolicy) -> Connection {
    let database = url::Url::parse(&test_database_url()).unwrap();
    let (host, port) = (database.host_str().unwrap().to_owned(), database.port().unwrap_or(5432));
    let command = format!("[{program}, {FORWARD:?}, \"{{port}}\", \"{host}\", \"{port}\", \"{marker}\"]");
    let writes = if writes == WritePolicy::Allowed { "writes = \"allowed\"\n" } else { "" };
    connection(&format!("tier = \"local\"\nurl = \"{database}\"\ntls = \"off\"\n{writes}tunnel = {{ command = {command} }}\n"))
}

pub(crate) fn column(name: &str, kind: ColumnKind) -> Column {
    Column { name: name.to_owned(), kind, type_name: String::new() }
}

/// Four orders: an id, a total and the customer, one of them `NULL`, one with a name past ASCII.
/// `count` rows of one integer each, `0` and on.
pub(crate) fn numbers(count: usize) -> Vec<Vec<Value>> {
    (0..count).map(|n| vec![Value::Int(n as i64)]).collect()
}

/// A page of `count` rows of `numbers`, as much of the result as `extent` says.
pub(crate) fn page(count: usize, extent: Extent) -> Page {
    Page { rows: numbers(count), extent }
}

pub(crate) fn orders() -> ResultSet {
    let row = |id: i64, total: &str, customer: Value| vec![Value::Int(id), Value::Numeric(total.into()), customer];
    ResultSet::whole(
        vec![column("id", ColumnKind::Int), column("total", ColumnKind::Numeric), column("customer", ColumnKind::Text)],
        vec![
            row(7, "12.50", Value::Text("nina".into())),
            row(12, "1499.00", Value::Null),
            row(103, "3.20", Value::Text("Zoë Nakamura".into())),
            row(104, "0.99", Value::Text("noel".into())),
        ],
    )
}

fn schema_column(name: &str, type_name: &str, default: Option<&str>) -> SchemaColumn {
    SchemaColumn {
        name: name.to_owned(),
        type_name: type_name.to_owned(),
        is_nullable: default.is_some(),
        default: default.map(str::to_owned),
    }
}

fn schema_table(name: &str, columns: Vec<SchemaColumn>) -> Table {
    Table { name: name.to_owned(), kind: TableKind::Table, estimated_rows: None, columns, indexes: vec![] }
}

/// The schema of `shop`: `orders` and `customers` in `public`, `invoices` in `billing`; `orders` has its key, a
/// default and a row estimate.
pub(crate) fn shop_schema() -> Schema {
    let key = Index { name: "orders_pkey".to_owned(), columns: vec!["id".to_owned()], is_unique: true, is_primary: true };
    let orders_columns = vec![
        schema_column("id", "integer", None),
        schema_column("customer_id", "integer", None),
        schema_column("total", "numeric(10,2)", Some("0")),
    ];
    let orders = Table { estimated_rows: Some(1200), indexes: vec![key], ..schema_table("orders", orders_columns) };
    let customers = schema_table("customers", vec![schema_column("id", "integer", None), schema_column("name", "text", None)]);
    let invoices = schema_table("invoices", vec![schema_column("id", "integer", None), schema_column("order_id", "integer", None)]);
    let public = SchemaNode { name: "public".to_owned(), tables: vec![orders, customers] };
    let billing = SchemaNode { name: "billing".to_owned(), tables: vec![invoices] };
    Schema { schemas: vec![billing, public] }
}
