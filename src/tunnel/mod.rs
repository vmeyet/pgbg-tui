//! A tunnel: a child process pgbg owns that forwards a free `127.0.0.1` port to a database out of reach.
use crate::connection::TunnelSpec;
use crate::db::model::{LocalPort, Route};
use anyhow::{Context, Result, anyhow, bail};
use rustix::process::Pid;
use std::collections::VecDeque;
use std::fmt;
use std::net::Ipv4Addr;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};

/// How long a tunnel has to answer on its port: an IAP or SSH handshake can take seconds.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const READY_POLL: Duration = Duration::from_millis(50);
/// How long a dead tunnel's stderr is waited for, in case a process outside its group still holds it.
const LAST_WORDS_TIMEOUT: Duration = Duration::from_secs(1);
const LAST_WORDS_LINES: usize = 5;
const LAST_WORDS_LINE_CHARS: usize = 200;
const OWN_GROUP: i32 = 0;

mod groups;

/// A running tunnel; dropping it kills its whole process group.
#[derive(Debug)]
pub(crate) struct Tunnel {
    port: LocalPort,
    program: String,
    child: Child,
    /// Taken once the group is killed, so a stop never signals a reused id.
    group: Option<Pid>,
    last_words: Option<JoinHandle<Vec<String>>>,
}

/// How a tunnel ended: its exit and the last lines it wrote to stderr.
#[derive(Debug)]
pub(crate) struct Died {
    program: String,
    status: Option<ExitStatus>,
    last_words: Vec<String>,
}

impl Tunnel {
    /// Runs `before` once when set, then the command on a free port, and waits until that port answers.
    pub(crate) async fn start(spec: &TunnelSpec) -> Result<Self> {
        Self::start_within(spec, READY_TIMEOUT).await
    }

    async fn start_within(spec: &TunnelSpec, ready_timeout: Duration) -> Result<Self> {
        if let Some(before) = spec.before() {
            run_before(before).await?;
        }
        let port = free_port().await?;
        let mut tunnel = Self::spawn(&spec.command_on(port), port)?;
        if let Err(err) = tunnel.wait_until_ready(ready_timeout).await {
            tunnel.stop().await;
            return Err(err);
        }
        Ok(tunnel)
    }

    /// The route a session takes through this tunnel.
    pub(crate) fn route(&self) -> Route {
        Route::Tunnel(self.port)
    }

    pub(crate) fn port(&self) -> LocalPort {
        self.port
    }

    /// Waits until the tunnel ends on its own, then says how; its group is killed on the way.
    pub(crate) async fn ended(&mut self) -> Died {
        let status = self.child.wait().await.ok();
        self.died(status).await
    }

    /// Whether the tunnel's own process still runs; once it is seen ended, its group is killed at once, while its id
    /// cannot yet be reused.
    pub(crate) fn is_running(&mut self) -> bool {
        let is_running = matches!(self.child.try_wait(), Ok(None));
        if !is_running {
            self.kill_group();
        }
        is_running
    }

    /// Kills the tunnel's whole group and reaps it; safe to call twice.
    pub(crate) async fn stop(&mut self) {
        self.kill_group();
        if let Err(err) = self.child.wait().await {
            tracing::warn!("the tunnel `{}` could not be reaped: {err}", self.program);
        }
    }

    fn spawn(command: &[String], port: LocalPort) -> Result<Self> {
        let (program, args) = command.split_first().ok_or_else(|| anyhow!("`tunnel.command` is empty"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(OWN_GROUP)
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting the tunnel `{program}`"))?;
        let group = child.id().and_then(|id| Pid::from_raw(id as i32));
        if let Some(group) = group {
            groups::register(group);
        }
        let last_words = child.stderr.take().map(|stderr| tokio::spawn(keep_last_words(stderr)));
        Ok(Self { port, program: program.clone(), child, group, last_words })
    }

    async fn wait_until_ready(&mut self, ready_timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + ready_timeout;
        loop {
            if TcpStream::connect((Ipv4Addr::LOCALHOST, self.port.get())).await.is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("the tunnel `{}` did not answer on port {} within {}s", self.program, self.port, ready_timeout.as_secs_f32());
            }
            tokio::select! {
                status = self.child.wait() => return Err(self.died(status.ok()).await.into()),
                () = sleep(READY_POLL) => {}
            }
        }
    }

    async fn died(&mut self, status: Option<ExitStatus>) -> Died {
        self.kill_group();
        let last_words = self.last_words().await;
        Died { program: self.program.clone(), status, last_words }
    }

    /// The last lines of stderr, waited for `LAST_WORDS_TIMEOUT` at most; a wait dropped halfway leaves them for the next.
    async fn last_words(&mut self) -> Vec<String> {
        let Some(reader) = self.last_words.as_mut() else { return vec![] };
        let read = timeout(LAST_WORDS_TIMEOUT, reader).await;
        self.last_words = None;
        read.ok().and_then(Result::ok).unwrap_or_default()
    }

    fn kill_group(&mut self) {
        let Some(group) = self.group.take() else { return };
        if let Err(err) = groups::kill(group) {
            tracing::warn!("the tunnel `{}` could not be killed: {err}", self.program);
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.kill_group();
    }
}

/// Runs `before` to its end; it must be safe to run twice, since each start runs it again.
async fn run_before(before: &[String]) -> Result<()> {
    let (program, args) = before.split_first().ok_or_else(|| anyhow!("`tunnel.before` is empty"))?;
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("running `tunnel.before` (`{program}`)"))?;
    if !output.status.success() {
        let last_words = last_lines(String::from_utf8_lossy(&output.stderr).lines());
        bail!("`tunnel.before` (`{program}`) failed: {}", with_last_words(&output.status.to_string(), &last_words));
    }
    Ok(())
}

async fn free_port() -> Result<LocalPort> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.context("finding a free port for the tunnel")?;
    Ok(LocalPort::new(listener.local_addr()?.port()))
}

/// Reads `stderr` to its end, so the tunnel never blocks on a full pipe, and keeps its last lines.
async fn keep_last_words(stderr: impl AsyncRead + Unpin) -> Vec<String> {
    let mut lines = BufReader::new(stderr).lines();
    let mut kept = VecDeque::with_capacity(LAST_WORDS_LINES);
    while let Ok(Some(line)) = lines.next_line().await {
        if kept.len() == LAST_WORDS_LINES {
            kept.pop_front();
        }
        kept.push_back(shortened(&line));
    }
    kept.into()
}

fn last_lines<'a>(lines: impl DoubleEndedIterator<Item = &'a str>) -> Vec<String> {
    let mut kept: Vec<String> = lines.rev().take(LAST_WORDS_LINES).map(shortened).collect();
    kept.reverse();
    kept
}

fn shortened(line: &str) -> String {
    line.chars().take(LAST_WORDS_LINE_CHARS).collect()
}

fn with_last_words(how: &str, last_words: &[String]) -> String {
    match last_words.join(" / ") {
        words if words.is_empty() => how.to_owned(),
        words => format!("{how}: {words}"),
    }
}

impl fmt::Display for Died {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let how = self.status.map_or_else(|| "it ended".to_owned(), |status| status.to_string());
        write!(f, "the tunnel `{}` died, {}", self.program, with_last_words(&how, &self.last_words))
    }
}

impl std::error::Error for Died {}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::connection::{Connection, Entry};
    use crate::db::Session;
    use crate::db::guard;
    use crate::db::model::{Statement, Value};
    use rustix::process::{Signal, kill_process_group};

    const LISTEN: &str = "import socket, sys, time; server = socket.create_server(('127.0.0.1', int(sys.argv[1]))); time.sleep(60)";
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
    const STOP_TIMEOUT: Duration = Duration::from_secs(5);
    const PANICKING_CHILD: &str = "PGBG_TEST_PANICKING_TUNNEL";
    const PANIC_TEST: &str = "tunnel::tests::a_panic_kills_every_live_tunnel_even_where_no_drop_runs";

    fn spec(command: &[&str], before: Option<&[&str]>) -> TunnelSpec {
        let list = |args: &[&str]| format!("[{}]", args.iter().map(|arg| format!("{arg:?}")).collect::<Vec<_>>().join(", "));
        let before = before.map_or_else(String::new, |before| format!(", before = {}", list(before)));
        let text = format!("tunnel = {{ command = {}{before} }}", list(command));
        toml::from_str::<std::collections::BTreeMap<String, TunnelSpec>>(&text).unwrap().remove("tunnel").unwrap()
    }

    fn listener() -> TunnelSpec {
        spec(&["python3", "-c", LISTEN, "{port}"], None)
    }

    fn port_of(tunnel: &Tunnel) -> u16 {
        tunnel.port.get()
    }

    async fn answers(port: u16) -> bool {
        TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.is_ok()
    }

    async fn until_silent(port: u16) -> bool {
        timeout(STOP_TIMEOUT, async {
            while answers(port).await {
                sleep(READY_POLL).await;
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn a_started_tunnel_answers_on_the_port_it_picked() {
        let tunnel = Tunnel::start(&listener()).await.unwrap();
        assert!(answers(port_of(&tunnel)).await);
        assert_eq!(tunnel.route(), Route::Tunnel(tunnel.port));
    }

    #[tokio::test]
    async fn a_tunnel_that_dies_before_answering_says_how_with_its_last_words() {
        let dying = spec(&["sh", "-c", "echo 'bastion.example.com: connection refused' >&2; exit 3", "{port}"], None);
        let err = Tunnel::start(&dying).await.unwrap_err().to_string();
        assert!(err.contains("`sh` died") && err.contains('3') && err.contains("connection refused"), "{err}");
    }

    #[tokio::test]
    async fn a_tunnel_that_never_answers_is_given_up() {
        let silent = spec(&["sh", "-c", "sleep 30", "{port}"], None);
        let err = Tunnel::start_within(&silent, Duration::from_millis(200)).await.unwrap_err().to_string();
        assert!(err.contains("did not answer"), "{err}");
    }

    #[tokio::test]
    async fn stopping_a_tunnel_kills_the_processes_it_started_too() {
        let forking = spec(&["sh", "-c", &format!("python3 -c \"{LISTEN}\" \"$0\"; true"), "{port}"], None);
        let mut tunnel = Tunnel::start(&forking).await.unwrap();
        tunnel.stop().await;
        assert!(until_silent(port_of(&tunnel)).await);
    }

    #[tokio::test]
    async fn stopping_a_tunnel_twice_is_safe() {
        let mut tunnel = Tunnel::start(&listener()).await.unwrap();
        tunnel.stop().await;
        tunnel.stop().await;
        assert!(until_silent(port_of(&tunnel)).await);
    }

    #[tokio::test]
    async fn dropping_a_tunnel_kills_it() {
        let tunnel = Tunnel::start(&listener()).await.unwrap();
        let port = port_of(&tunnel);
        drop(tunnel);
        assert!(until_silent(port).await);
    }

    async fn until_ended(tunnel: &mut Tunnel) -> bool {
        timeout(STOP_TIMEOUT, async {
            while tunnel.is_running() {
                sleep(READY_POLL).await;
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn a_tunnel_seen_ended_takes_the_processes_it_started_with_it() {
        let forking = spec(&["sh", "-c", &format!("python3 -c \"{LISTEN}\" \"$0\"; true"), "{port}"], None);
        let mut tunnel = Tunnel::start(&forking).await.unwrap();
        let leader = Pid::from_raw(tunnel.child.id().unwrap() as i32).unwrap();
        rustix::process::kill_process(leader, Signal::KILL).unwrap();
        assert!(until_ended(&mut tunnel).await);
        assert!(until_silent(port_of(&tunnel)).await);
    }

    #[tokio::test]
    async fn last_words_wait_cut_short_are_still_there_on_the_next_wait() {
        let talking =
            spec(&["sh", "-c", &format!("echo 'bastion.example.com: no route' >&2; exec python3 -c \"{LISTEN}\" \"$0\""), "{port}"], None);
        let mut tunnel = Tunnel::start(&talking).await.unwrap();
        assert!(timeout(Duration::from_millis(50), tunnel.last_words()).await.is_err());
        tunnel.kill_group();
        assert_eq!(tunnel.last_words().await, ["bastion.example.com: no route"]);
    }

    #[tokio::test]
    async fn a_tunnel_that_ends_on_its_own_says_how() {
        let mut tunnel = Tunnel::start(&listener()).await.unwrap();
        assert!(tunnel.is_running());
        kill_process_group(tunnel.group.unwrap(), Signal::KILL).unwrap();
        assert!(tunnel.ended().await.to_string().contains("`python3` died"));
        assert!(!tunnel.is_running());
    }

    async fn port_of_a_tunnel_left_by_a_panicking_pgbg() -> u16 {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([PANIC_TEST, "--exact", "--nocapture"])
            .env(PANICKING_CHILD, "1")
            .output()
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        stdout.lines().find_map(|line| line.strip_prefix("port=")).unwrap().parse().unwrap()
    }

    #[tokio::test]
    async fn a_panic_kills_every_live_tunnel_even_where_no_drop_runs() {
        if std::env::var_os(PANICKING_CHILD).is_some() {
            let leaked = Box::leak(Box::new(Tunnel::start(&listener()).await.unwrap()));
            println!("port={}", leaked.port);
            let on_the_main_thread = std::thread::Builder::new().name("main".into());
            _ = on_the_main_thread.spawn(|| panic!("pgbg panicked with a tunnel open")).unwrap().join();
            return;
        }
        assert!(until_silent(port_of_a_tunnel_left_by_a_panicking_pgbg().await).await);
    }

    #[tokio::test]
    async fn the_before_command_runs_first() {
        let folder = tempfile::tempdir().unwrap();
        let woken = folder.path().join("woken").display().to_string();
        let waking =
            spec(&["sh", "-c", &format!("test -f {woken} && exec python3 -c \"{LISTEN}\" \"$0\""), "{port}"], Some(&["touch", &woken]));
        assert!(Tunnel::start(&waking).await.is_ok());
    }

    #[tokio::test]
    async fn a_failing_before_command_stops_the_start_and_says_so() {
        let failing = spec(&["python3", "-c", LISTEN, "{port}"], Some(&["sh", "-c", "echo 'bastion is gone' >&2; exit 1"]));
        let err = Tunnel::start(&failing).await.unwrap_err().to_string();
        assert!(err.contains("`tunnel.before`") && err.contains("bastion is gone"), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_through_a_tunnel_reaches_the_database() {
        let database = url::Url::parse(&std::env::var("PGBG_TEST_DATABASE_URL").unwrap()).unwrap();
        let (host, port) = (database.host_str().unwrap().to_owned(), database.port().unwrap_or(5432).to_string());
        let mut unreachable = database.clone();
        unreachable.set_port(Some(1)).unwrap();
        let entry: Entry = toml::from_str(&format!("tier = \"local\"\nurl = \"{unreachable}\"\n")).unwrap();
        let connection = Connection::new("shop", entry).unwrap();
        let tunnel = Tunnel::start(&spec(&["python3", "-c", FORWARD, "{port}", &host, &port], None)).await.unwrap();
        let mut session = Session::open(&connection, tunnel.route()).await.unwrap();
        let result = session.run(&guard::admit(Statement::new("SELECT 7"), WritePolicy::ReadOnly).unwrap()).await.unwrap();
        assert_eq!(result.rows, [[Value::Int(7)]]);
    }
}
