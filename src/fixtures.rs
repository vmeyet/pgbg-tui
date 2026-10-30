//! Invented fixtures the tests share: the connection `shop`, the test database, tunnels to it, and results of `orders`.
use crate::connection::{Connection, Entry, WritePolicy};
use crate::db::model::{
    Catalog, Column, ColumnKind, ColumnName, ColumnNumber, Extent, ForeignKey, Index, Origin, Page, Params, ResultSet, Route, Schema,
    SchemaColumn, SchemaNode, Statement, Table, TableKind, TableOid, TypeName, Value,
};
use crate::db::{Session, guard};
use crate::release::Commit;
use rustix::process::{Pid, test_kill_process};
use std::path::Path;
use std::time::Duration;

/// The environment variable naming the throwaway database the `#[ignore]`d tests run against.
const DATABASE_URL: &str = "PGBG_TEST_DATABASE_URL";

/// A forwarder from `127.0.0.1:argv[1]` to `argv[2]:argv[3]`, for tunnels in tests; `argv[5]`, when given, holds each
/// chunk that many seconds on its way.
pub(crate) const FORWARD: &str = "
import socket, sys, threading, time
hop = float(sys.argv[5]) if len(sys.argv) > 5 else 0
def pipe(source, sink):
    while data := source.recv(65536):
        time.sleep(hop)
        sink.sendall(data)
    sink.shutdown(socket.SHUT_WR)
server = socket.create_server(('127.0.0.1', int(sys.argv[1])))
while True:
    client, _ = server.accept()
    upstream = socket.create_connection((sys.argv[2], int(sys.argv[3])))
    for source, sink in ((client, upstream), (upstream, client)):
        threading.Thread(target=pipe, args=(source, sink), daemon=True).start()
";

/// Held for the whole of every test that starts a tunnel: between picking a free port and binding it, or while a stopped
/// tunnel's port is checked for silence, another test's tunnel could take that port.
pub(crate) static TUNNEL_PORTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Shared by the tests that make or drop a relation, and held alone by each schema read: the server fails a read that
/// meets a relation dropped since its snapshot began.
static CATALOG: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Held while a test makes or drops relations; never while it holds [`reading_the_catalog`].
pub(crate) async fn changing_the_catalog() -> tokio::sync::RwLockReadGuard<'static, ()> {
    CATALOG.read().await
}

/// Held while a test reads the schema, so no relation is dropped under the read; after `TUNNEL_PORTS` when both are.
pub(crate) async fn reading_the_catalog() -> tokio::sync::RwLockWriteGuard<'static, ()> {
    CATALOG.write().await
}

/// How long a test waits on a child process: to write its pid, or to be gone.
const PROCESS_WAIT: Duration = Duration::from_secs(5);
const PROCESS_POLL: Duration = Duration::from_millis(20);
/// How long the slow forwarder holds each chunk each way: a round trip of about 30 ms, as to a distant server.
const SLOW_HOP: Duration = Duration::from_millis(15);

/// A shell line that starts a `sleep` it does not wait on, as `gcloud` starts helpers, writes that `sleep`'s pid to
/// `pid_file`, then waits for it.
pub(crate) fn leaving_a_sleep(pid_file: &Path) -> Vec<String> {
    let line = "sleep 60 & echo $! > \"$1\"; wait";
    ["sh", "-c", line, "sh"].into_iter().map(str::to_owned).chain([pid_file.display().to_string()]).collect()
}

/// Drops `work` once the command it runs wrote its `sleep`'s pid to `pid_file`, then says whether that `sleep` went too.
pub(crate) async fn sleep_goes_when_dropped<T>(work: impl Future<Output = T>, pid_file: &Path) -> bool {
    let pid = tokio::select! {
        _ = work => panic!("the command ended on its own"),
        pid = written_pid(pid_file) => pid,
    };
    is_gone(pid).await
}

/// The pid a command wrote to `pid_file`, once it did.
pub(crate) async fn written_pid(pid_file: &Path) -> Pid {
    let written = async {
        loop {
            if let Some(pid) = std::fs::read_to_string(pid_file).ok().and_then(|pid| pid.trim().parse().ok()).and_then(Pid::from_raw) {
                return pid;
            }
            tokio::time::sleep(PROCESS_POLL).await;
        }
    };
    tokio::time::timeout(PROCESS_WAIT, written).await.unwrap()
}

/// Whether the process `pid` is gone within `PROCESS_WAIT`.
pub(crate) async fn is_gone(pid: Pid) -> bool {
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

/// A session on `shop`, the test database, a local connection of policy `writes`.
pub(crate) async fn session(writes: WritePolicy) -> Session {
    let writes = if writes == WritePolicy::Allowed { "allowed" } else { "read-only" };
    Session::open(&test_database("local", &format!("writes = \"{writes}\"\n")), Route::Direct).await.unwrap()
}

/// `sql` run alone on a new session of policy `writes`; it may make or drop a relation, so no schema read runs meanwhile.
pub(crate) async fn run(writes: WritePolicy, sql: &str) -> anyhow::Result<ResultSet> {
    let admitted = guard::admit(Statement::new(sql), Params::default(), writes).unwrap();
    let _changing = changing_the_catalog().await;
    session(writes).await.run(&admitted).await
}

/// `sql` run on `session`, a connection that allows writes.
pub(crate) async fn try_on(session: &mut Session, sql: &str) -> anyhow::Result<ResultSet> {
    session.run(&guard::admit(Statement::new(sql), Params::default(), WritePolicy::Allowed).unwrap()).await
}

/// Like `try_on`, failing the test when the statement fails.
pub(crate) async fn run_on(session: &mut Session, sql: &str) -> ResultSet {
    try_on(session, sql).await.unwrap()
}

/// `shop` on the test database logged in as `user`, a local one, with the keys of `extra` too.
pub(crate) fn test_database_by(user: &str, extra: &str) -> Connection {
    let mut url = url::Url::parse(&test_database_url()).unwrap();
    url.set_username(user).unwrap();
    connection(&format!("tier = \"local\"\nurl = \"{url}\"\n{extra}"))
}

/// `shop` on the test database through a forwarder whose command line carries `marker`, so a test can find it.
pub(crate) fn tunnelled(marker: &str) -> Connection {
    through_forwarder("\"python3\", \"-c\"", marker, WritePolicy::ReadOnly, Duration::ZERO)
}

/// Like `tunnelled`, with each round trip taking about 30 ms.
pub(crate) fn tunnelled_slowly(marker: &str, writes: WritePolicy) -> Connection {
    through_forwarder("\"python3\", \"-c\"", marker, writes, SLOW_HOP)
}

/// Like `tunnelled`, but the forwarder starts only once the shell line `first` succeeds; when it fails, the tunnel dies.
pub(crate) fn tunnelled_after(marker: &str, first: &str, writes: WritePolicy) -> Connection {
    let script = format!("{first} || exit 1; exec python3 -c \"$0\" \"$@\"");
    through_forwarder(&format!("\"sh\", \"-c\", {script:?}"), marker, writes, Duration::ZERO)
}

/// `shop` on the test database through a tunnel that runs `program`, then the forwarder's code and arguments, each
/// chunk held `hop` on its way.
fn through_forwarder(program: &str, marker: &str, writes: WritePolicy, hop: Duration) -> Connection {
    let database = url::Url::parse(&test_database_url()).unwrap();
    let (host, port) = (database.host_str().unwrap().to_owned(), database.port().unwrap_or(5432));
    let command = format!("[{program}, {FORWARD:?}, \"{{port}}\", \"{host}\", \"{port}\", \"{marker}\", \"{}\"]", hop.as_secs_f64());
    let writes = if writes == WritePolicy::Allowed { "writes = \"allowed\"\n" } else { "" };
    connection(&format!("tier = \"local\"\nurl = \"{database}\"\ntls = \"off\"\n{writes}tunnel = {{ command = {command} }}\n"))
}

/// A computed column of a result: read from no table.
pub(crate) fn column(name: &str, kind: ColumnKind) -> Column {
    Column { name: ColumnName::new(name), kind, type_name: TypeName::new(""), origin: None }
}

/// A column of a result read as is from the column `name` of `table` in [`keyed_catalog`], and named so.
pub(crate) fn shop_column(table: &str, name: &str, kind: ColumnKind) -> Column {
    let catalog = keyed_catalog();
    let table = catalog.schema.schemas.iter().flat_map(|node| &node.tables).find(|found| found.name == table).unwrap();
    Column { origin: Some(Origin { table: table.oid, column: table.column_number(name).unwrap() }), ..column(name, kind) }
}

/// `count` rows of one integer each, `0` and on.
pub(crate) fn numbers(count: usize) -> Vec<Vec<Value>> {
    (0..count).map(|n| vec![Value::Int(n as i64)]).collect()
}

/// A page of `count` rows of `numbers`, as much of the result as `extent` says.
pub(crate) fn page(count: usize, extent: Extent) -> Page {
    Page { rows: numbers(count), extent }
}

/// Four orders: an id, a total and the customer, one of them `NULL`, one with a name past ASCII.
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

/// Two rows of one column of a type pgbg does not decode, `shape`.
pub(crate) fn undecoded() -> ResultSet {
    ResultSet::whole(vec![column("shape", ColumnKind::Other)], vec![vec![Value::Unsupported]; 2])
}

pub(crate) fn schema_column(name: &str, type_name: &str, default: Option<&str>) -> SchemaColumn {
    SchemaColumn {
        number: ColumnNumber::new(0),
        name: name.to_owned(),
        type_name: TypeName::new(type_name),
        is_nullable: default.is_some(),
        default: default.map(str::to_owned),
        comment: None,
    }
}

/// The table `name` of oid 0, its columns numbered in order from 1.
pub(crate) fn schema_table(name: &str, columns: Vec<SchemaColumn>) -> Table {
    let numbered = |(at, column): (usize, SchemaColumn)| SchemaColumn { number: ColumnNumber::new(at as i16 + 1), ..column };
    Table {
        oid: TableOid::new(0),
        name: name.to_owned(),
        kind: TableKind::Table,
        estimated_rows: None,
        columns: columns.into_iter().enumerate().map(numbered).collect(),
        indexes: vec![],
        foreign_keys: vec![],
        comment: None,
    }
}

/// A one-column foreign key, `column` pointing to `id` of `target_schema.target_table`.
pub(crate) fn key_to_id(column: &str, target_schema: &str, target_table: &str) -> ForeignKey {
    ForeignKey {
        columns: vec![column.to_owned()],
        target_schema: target_schema.to_owned(),
        target_table: target_table.to_owned(),
        target_columns: vec!["id".to_owned()],
    }
}

/// The catalog of `shop`: its schema, `public` its search path, whole.
pub(crate) fn shop_catalog() -> Catalog {
    Catalog { schema: shop_schema(), search_path: vec!["public".to_owned()], is_cut: false }
}

/// The schema of `shop`: `orders` and `customers` in `public`, `invoices` in `billing`; `orders` has its key, a
/// default and a row estimate; `orders.customer_id` points to `customers` and `invoices.order_id` to `orders`; their
/// oids are 1 for `invoices`, 2 for `orders` and 3 for `customers`.
pub(crate) fn shop_schema() -> Schema {
    let key = Index { name: "orders_pkey".to_owned(), columns: vec!["id".to_owned()], is_unique: true, is_primary: true };
    let orders_columns = vec![
        schema_column("id", "integer", None),
        schema_column("customer_id", "integer", None),
        schema_column("total", "numeric(10,2)", Some("0")),
    ];
    let orders = Table {
        oid: TableOid::new(2),
        estimated_rows: Some(1200),
        indexes: vec![key],
        foreign_keys: vec![key_to_id("customer_id", "public", "customers")],
        ..schema_table("orders", orders_columns)
    };
    let customers = Table {
        oid: TableOid::new(3),
        ..schema_table("customers", vec![schema_column("id", "integer", None), schema_column("name", "text", None)])
    };
    let invoices = Table {
        oid: TableOid::new(1),
        foreign_keys: vec![key_to_id("order_id", "public", "orders")],
        ..schema_table("invoices", vec![schema_column("id", "integer", None), schema_column("order_id", "integer", None)])
    };
    let public = SchemaNode { name: "public".to_owned(), tables: vec![orders, customers] };
    let billing = SchemaNode { name: "billing".to_owned(), tables: vec![invoices] };
    Schema { schemas: vec![billing, public] }
}

/// [`shop_catalog`] whose `customers.user_id` points to `users`, a table of `public` of oid 4, so a key leads two tables
/// deep, and whose `invoices.order_id` an index starts with.
pub(crate) fn keyed_catalog() -> Catalog {
    let mut catalog = shop_catalog();
    let public = &mut catalog.schema.schemas[1];
    let customers = &mut public.tables[1];
    customers.columns.push(SchemaColumn { number: ColumnNumber::new(3), ..schema_column("user_id", "integer", None) });
    customers.foreign_keys.push(key_to_id("user_id", "public", "users"));
    let users = schema_table("users", vec![schema_column("id", "integer", None), schema_column("email", "text", None)]);
    public.tables.push(Table { oid: TableOid::new(4), ..users });
    let order = Index { name: "invoices_order_id".to_owned(), columns: vec!["order_id".to_owned()], is_unique: false, is_primary: false };
    catalog.schema.schemas[0].tables[0].indexes.push(order);
    catalog
}

/// `SELECT * FROM orders` on [`keyed_catalog`]: order 7 of customer 3, and order 12 of no customer.
pub(crate) fn keyed_orders() -> ResultSet {
    let column = |name: &str, kind| shop_column("orders", name, kind);
    let columns = vec![column("id", ColumnKind::Int), column("customer_id", ColumnKind::Int), column("total", ColumnKind::Numeric)];
    let rows = vec![
        vec![Value::Int(7), Value::Int(3), Value::Numeric("12.50".into())],
        vec![Value::Int(12), Value::Null, Value::Numeric("1499.00".into())],
    ];
    ResultSet::whole(columns, rows)
}

/// The row of `customers` that order 7 points to: customer 3, Nina, user 5.
pub(crate) fn keyed_customer() -> ResultSet {
    let column = |name: &str, kind| shop_column("customers", name, kind);
    let columns = vec![column("id", ColumnKind::Int), column("name", ColumnKind::Text), column("user_id", ColumnKind::Int)];
    ResultSet::whole(columns, vec![vec![Value::Int(3), Value::Text("nina".into()), Value::Int(5)]])
}

/// The shortest of five runs of `step`, so a busy machine slows one run, not the measure.
#[cfg(not(debug_assertions))]
pub(crate) fn best_of<T>(mut step: impl FnMut() -> T) -> Duration {
    let mut timed = || {
        let started = std::time::Instant::now();
        std::hint::black_box(step());
        started.elapsed()
    };
    (0..5).map(|_| timed()).min().unwrap_or_default()
}

/// Ten thousand rows, the cap, of fifty columns: one of each kind the grid colours, and plain text.
#[cfg(not(debug_assertions))]
pub(crate) fn capped() -> ResultSet {
    let kinds = [
        ColumnKind::Int,
        ColumnKind::Text,
        ColumnKind::Numeric,
        ColumnKind::TimestampTz,
        ColumnKind::Bool,
        ColumnKind::Uuid,
        ColumnKind::Json,
        ColumnKind::Text,
        ColumnKind::Int,
        ColumnKind::Text,
    ];
    let columns = (0..50).map(|index| column(&format!("c{index}"), kinds[index % kinds.len()].clone())).collect();
    let statuses = ["paid", "open", "void"];
    let cells = |n: i64| {
        let at = chrono::DateTime::UNIX_EPOCH + chrono::Duration::hours(n);
        [
            Value::Int(n),
            Value::Text(format!("customer {n}").into()),
            Value::Numeric(format!("{n}.50").into()),
            Value::TimestampTz(at),
            Value::Bool(n % 2 == 0),
            Value::Uuid(uuid::Uuid::from_u128(u128::from(n.unsigned_abs()) << 64)),
            Value::Json(crate::db::model::JsonText::of(&serde_json::json!({"id": n, "status": "paid"}))),
            Value::Text(statuses[n.unsigned_abs() as usize % statuses.len()].into()),
            Value::Int(-n),
            Value::Null,
        ]
    };
    let rows = (0..10_000).map(|n| cells(n).into_iter().cycle().take(50).collect()).collect();
    ResultSet::new(columns, Page { rows, extent: Extent::Cut })
}

/// The commit a test binary runs, as `build.rs` would embed it.
pub(crate) const RUNNING: &str = "9731436a0e7c4d1b2f3a4b5c6d7e8f9a0b1c2d3e";

/// A commit past [`RUNNING`], as the repo's latest.
pub(crate) const NEWER: &str = "635cf1b0000000000000000000000000000000ff";

pub(crate) fn commit(hash: &str) -> Commit {
    Commit::parse(hash).unwrap()
}
