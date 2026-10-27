//! A tunnel: a child process pgbg owns that forwards a free `127.0.0.1` port to a database this machine cannot reach straight.
use crate::connection::TunnelSpec;
use crate::db::model::{LocalPort, Route};
use crate::groups::{self, OWN_GROUP, keep_last_words, with_last_words};
use crate::program::Program;
use anyhow::{Context, Result, anyhow, bail};
use rustix::io::Errno;
use rustix::process::Pid;
use std::fmt;
use std::net::Ipv4Addr;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};

/// How long a tunnel has to answer on its port: an IAP or SSH handshake can take seconds.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const READY_POLL: Duration = Duration::from_millis(50);
/// How long `before` may take: starting a stopped bastion can take a minute.
const BEFORE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a stopped tunnel's port may keep answering while the processes of its group go.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(1);
const HANDED_OFF: &str = "the tunnel ended cleanly before its port answered, so it likely handed the forward to another ssh \
                          (a `ControlMaster`) or went to the background: add `-o ControlPath=none` to an ssh command, and drop `-f`";
/// How long a dead tunnel's stderr is waited for, in case a process outside its group still holds it.
const LAST_WORDS_TIMEOUT: Duration = Duration::from_secs(1);

/// A running tunnel; dropping it kills its whole process group.
#[derive(Debug)]
pub(crate) struct Tunnel {
    port: LocalPort,
    program: Program,
    child: Child,
    /// Taken once the group is killed, so a stop never signals a reused id.
    group: Option<Pid>,
    last_words: Option<JoinHandle<Vec<String>>>,
}

/// How a tunnel ended: its exit and the last lines it wrote to stderr.
#[derive(Debug)]
pub(crate) struct Died {
    program: Program,
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
            if let Err(left) = tunnel.stop().await {
                tracing::warn!("{left:#}");
            }
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

    /// Whether the tunnel's own process still runs.
    /// Once it is seen ended, its group is killed at once, while its id cannot yet be reused.
    pub(crate) fn is_running(&mut self) -> bool {
        let is_running = matches!(self.child.try_wait(), Ok(None));
        if !is_running {
            self.kill_group();
        }
        is_running
    }

    /// Kills the tunnel's whole group and reaps it, then fails if its port still answers.
    /// A port that still answers means a process outside the group holds the forward, as an ssh `ControlMaster` does.
    /// Safe to call twice.
    pub(crate) async fn stop(&mut self) -> Result<()> {
        self.kill_group();
        if let Err(err) = self.child.wait().await {
            tracing::warn!("the tunnel `{}` could not be reaped: {err}", self.program);
        }
        if !goes_silent(self.port, SILENCE_TIMEOUT).await {
            bail!(
                "the tunnel `{}` stopped, but port {} still answers: another process holds the forward, such as an ssh `ControlMaster`; \
                 add `-o ControlPath=none` to the command",
                self.program,
                self.port
            );
        }
        Ok(())
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
        Ok(Self { port, program: Program::new(program), child, group, last_words })
    }

    async fn wait_until_ready(&mut self, ready_timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + ready_timeout;
        loop {
            if answers(self.port).await && self.is_running() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("the tunnel `{}` did not answer on port {} within {}s", self.program, self.port, ready_timeout.as_secs_f32());
            }
            tokio::select! {
                status = self.child.wait() => return Err(self.died_before_ready(status.ok()).await),
                () = sleep(READY_POLL) => {}
            }
        }
    }

    async fn died_before_ready(&mut self, status: Option<ExitStatus>) -> anyhow::Error {
        let is_clean_exit = status.is_some_and(|status| status.success());
        let died = anyhow::Error::new(self.died(status).await);
        if is_clean_exit { died.context(HANDED_OFF) } else { died }
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
        match groups::kill(group) {
            Ok(()) | Err(Errno::SRCH) => {}
            Err(err) => tracing::warn!("the tunnel `{}` could not be killed: {err}", self.program),
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
    let mut command = Command::new(program);
    let running = groups::output(command.args(args).stdin(Stdio::null()));
    let output = timeout(BEFORE_TIMEOUT, running)
        .await
        .map_err(|_elapsed| anyhow!("`tunnel.before` (`{program}`) did not end within {}s", BEFORE_TIMEOUT.as_secs()))?
        .with_context(|| format!("running `tunnel.before` (`{program}`)"))?;
    if !output.status.success() {
        let last_words = keep_last_words(output.stderr.as_slice()).await;
        bail!("`tunnel.before` (`{program}`) failed: {}", with_last_words(&output.status.to_string(), &last_words));
    }
    Ok(())
}

async fn answers(port: LocalPort) -> bool {
    TcpStream::connect((Ipv4Addr::LOCALHOST, port.get())).await.is_ok()
}

/// Whether nothing answers on `port` any more, within `wait`.
async fn goes_silent(port: LocalPort, wait: Duration) -> bool {
    let silence = async {
        while answers(port).await {
            sleep(READY_POLL).await;
        }
    };
    timeout(wait, silence).await.is_ok()
}

async fn free_port() -> Result<LocalPort> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.context("finding a free port for the tunnel")?;
    Ok(LocalPort::new(listener.local_addr()?.port()))
}

impl fmt::Display for Died {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let how = self.status.map_or_else(|| "it ended".to_owned(), |status| status.to_string());
        write!(f, "the tunnel `{}` died, {}", self.program, with_last_words(&how, &self.last_words))
    }
}

impl std::error::Error for Died {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::Session;
    use crate::db::guard;
    use crate::db::model::{Params, Statement, Value};
    use crate::fixtures::{self, FORWARD, TUNNEL_PORTS};
    use rustix::process::{Signal, kill_process, test_kill_process_group};

    const LISTEN: &str = "import socket, sys, time; server = socket.create_server(('127.0.0.1', int(sys.argv[1]))); time.sleep(60)";
    /// A forward that leaves the tunnel's group, as an ssh `ControlMaster` holds one, and goes away on its own soon after.
    const DETACH: &str = "
import os, socket, sys, time
if os.fork() == 0:
    os.setsid()
    server = socket.create_server(('127.0.0.1', int(sys.argv[1])))
    time.sleep(3)
    sys.exit(0)
time.sleep(60)
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

    fn group_of(tunnel: &Tunnel) -> Pid {
        tunnel.group.unwrap()
    }

    /// Whether every process of `group` is gone within `STOP_TIMEOUT`: a port could be taken by another test's tunnel.
    async fn until_gone(group: Pid) -> bool {
        timeout(STOP_TIMEOUT, async {
            while test_kill_process_group(group).is_ok() {
                sleep(READY_POLL).await;
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_started_tunnel_answers_on_the_port_it_picked() {
        let _ports = TUNNEL_PORTS.lock().await;
        let tunnel = Tunnel::start(&listener()).await.unwrap();
        assert!(answers(tunnel.port).await);
        assert_eq!(tunnel.route(), Route::Tunnel(tunnel.port));
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_tunnel_that_dies_before_answering_says_how_with_its_last_words() {
        let _ports = TUNNEL_PORTS.lock().await;
        let dying = spec(&["sh", "-c", "echo 'bastion.example.com: connection refused' >&2; exit 3", "{port}"], None);
        let err = Tunnel::start(&dying).await.unwrap_err().to_string();
        assert!(err.contains("`sh` died") && err.contains('3') && err.contains("connection refused"), "{err}");
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn a_port_that_answers_once_the_tunnel_died_is_not_taken_for_it() {
        let other = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = LocalPort::new(other.local_addr().unwrap().port());
        let mut tunnel = Tunnel::spawn(&["sh".into(), "-c".into(), "exit 3".into()], port).unwrap();
        tunnel.child.wait().await.unwrap();
        let err = tunnel.wait_until_ready(READY_TIMEOUT).await.unwrap_err().to_string();
        assert!(err.contains("`sh` died"), "{err}");
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_tunnel_that_never_answers_is_given_up() {
        let _ports = TUNNEL_PORTS.lock().await;
        let silent = spec(&["sh", "-c", "sleep 30", "{port}"], None);
        let err = Tunnel::start_within(&silent, Duration::from_millis(200)).await.unwrap_err().to_string();
        assert!(err.contains("did not answer"), "{err}");
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn stopping_a_tunnel_kills_the_processes_it_started_too() {
        let _ports = TUNNEL_PORTS.lock().await;
        let forking = spec(&["sh", "-c", &format!("python3 -c \"{LISTEN}\" \"$0\"; true"), "{port}"], None);
        let mut tunnel = Tunnel::start(&forking).await.unwrap();
        let group = group_of(&tunnel);
        let stopped = tunnel.stop().await;
        assert!(until_gone(group).await, "{stopped:?}");
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_tunnel_that_ends_cleanly_before_answering_says_it_likely_handed_the_forward_off() {
        let _ports = TUNNEL_PORTS.lock().await;
        let handing_off = spec(&["sh", "-c", "exit 0", "{port}"], None);
        let err = format!("{:#}", Tunnel::start(&handing_off).await.unwrap_err());
        assert!(err.contains("handed the forward") && err.contains("ControlPath=none"), "{err}");
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn stopping_a_tunnel_whose_port_still_answers_says_so() {
        let _ports = TUNNEL_PORTS.lock().await;
        let detaching = spec(&["python3", "-c", DETACH, "{port}"], None);
        let mut tunnel = Tunnel::start(&detaching).await.unwrap();
        let err = tunnel.stop().await.unwrap_err().to_string();
        assert!(err.contains("still answers"), "{err}");
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn stopping_a_tunnel_whose_port_went_silent_is_fine() {
        let _ports = TUNNEL_PORTS.lock().await;
        let mut tunnel = Tunnel::start(&listener()).await.unwrap();
        assert!(tunnel.stop().await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    #[ignore = "starts a tunnel"]
    async fn a_before_command_that_never_ends_is_given_up() {
        let stuck = spec(&["python3", "-c", LISTEN, "{port}"], Some(&["sleep", "600"]));
        let err = Tunnel::start(&stuck).await.unwrap_err().to_string();
        assert!(err.contains("`tunnel.before`") && err.contains("did not end within"), "{err}");
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn stopping_a_tunnel_twice_is_safe() {
        let _ports = TUNNEL_PORTS.lock().await;
        let mut tunnel = Tunnel::start(&listener()).await.unwrap();
        let group = group_of(&tunnel);
        let first = tunnel.stop().await;
        let second = tunnel.stop().await;
        assert!(until_gone(group).await, "{first:?} {second:?}");
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn dropping_a_tunnel_kills_it() {
        let _ports = TUNNEL_PORTS.lock().await;
        let tunnel = Tunnel::start(&listener()).await.unwrap();
        let group = group_of(&tunnel);
        drop(tunnel);
        assert!(until_gone(group).await);
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
    #[ignore = "starts a tunnel"]
    async fn a_tunnel_seen_ended_takes_the_processes_it_started_with_it() {
        let _ports = TUNNEL_PORTS.lock().await;
        let forking = spec(&["sh", "-c", &format!("python3 -c \"{LISTEN}\" \"$0\"; true"), "{port}"], None);
        let mut tunnel = Tunnel::start(&forking).await.unwrap();
        let group = group_of(&tunnel);
        kill_process(group, Signal::KILL).unwrap();
        assert!(until_ended(&mut tunnel).await);
        assert!(until_gone(group).await);
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn last_words_wait_cut_short_are_still_there_on_the_next_wait() {
        let _ports = TUNNEL_PORTS.lock().await;
        let talking =
            spec(&["sh", "-c", &format!("echo 'bastion.example.com: no route' >&2; exec python3 -c \"{LISTEN}\" \"$0\""), "{port}"], None);
        let mut tunnel = Tunnel::start(&talking).await.unwrap();
        assert!(timeout(Duration::from_millis(50), tunnel.last_words()).await.is_err());
        tunnel.kill_group();
        assert_eq!(tunnel.last_words().await, ["bastion.example.com: no route"]);
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_tunnel_that_ends_on_its_own_says_how() {
        let _ports = TUNNEL_PORTS.lock().await;
        let mut tunnel = Tunnel::start(&listener()).await.unwrap();
        assert!(tunnel.is_running());
        kill_process(group_of(&tunnel), Signal::KILL).unwrap();
        assert!(tunnel.ended().await.to_string().contains("`python3` died"));
        assert!(!tunnel.is_running());
    }

    async fn group_of_a_tunnel_left_by_a_panicking_pgbg() -> Pid {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([PANIC_TEST, "--exact", "--include-ignored", "--nocapture"])
            .env(PANICKING_CHILD, "1")
            .output()
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let group = stdout.lines().find_map(|line| line.strip_prefix("group=")).unwrap().parse().unwrap();
        Pid::from_raw(group).unwrap()
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_panic_kills_every_live_tunnel_even_where_no_drop_runs() {
        let _ports = TUNNEL_PORTS.lock().await;
        if std::env::var_os(PANICKING_CHILD).is_some() {
            let leaked = Box::leak(Box::new(Tunnel::start(&listener()).await.unwrap()));
            println!("group={}", group_of(leaked).as_raw_nonzero());
            let on_the_main_thread = std::thread::Builder::new().name("main".into());
            _ = on_the_main_thread.spawn(|| panic!("pgbg panicked with a tunnel open")).unwrap().join();
            return;
        }
        assert!(until_gone(group_of_a_tunnel_left_by_a_panicking_pgbg().await).await);
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn a_before_command_dropped_halfway_takes_what_it_started_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let before = fixtures::leaving_a_sleep(&pid_file);
        let starting = spec(&["python3", "-c", LISTEN, "{port}"], Some(&before.iter().map(String::as_str).collect::<Vec<_>>()));
        assert!(fixtures::sleep_goes_when_dropped(Tunnel::start(&starting), &pid_file).await);
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn the_before_command_runs_first() {
        let _ports = TUNNEL_PORTS.lock().await;
        let folder = tempfile::tempdir().unwrap();
        let woken = folder.path().join("woken").display().to_string();
        let waking =
            spec(&["sh", "-c", &format!("test -f {woken} && exec python3 -c \"{LISTEN}\" \"$0\""), "{port}"], Some(&["touch", &woken]));
        assert!(Tunnel::start(&waking).await.is_ok());
    }

    #[tokio::test]
    #[ignore = "starts a tunnel"]
    async fn a_failing_before_command_stops_the_start_and_says_so() {
        let failing = spec(&["python3", "-c", LISTEN, "{port}"], Some(&["sh", "-c", "echo 'bastion is gone' >&2; exit 1"]));
        let err = Tunnel::start(&failing).await.unwrap_err().to_string();
        assert!(err.contains("`tunnel.before`") && err.contains("bastion is gone"), "{err}");
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn a_session_through_a_tunnel_reaches_the_database() {
        let _ports = TUNNEL_PORTS.lock().await;
        let database = url::Url::parse(&fixtures::test_database_url()).unwrap();
        let (host, port) = (database.host_str().unwrap().to_owned(), database.port().unwrap_or(5432).to_string());
        let mut unreachable = database.clone();
        unreachable.set_port(Some(1)).unwrap();
        let connection = fixtures::connection(&format!("tier = \"local\"\nurl = \"{unreachable}\"\n"));
        let tunnel = Tunnel::start(&spec(&["python3", "-c", FORWARD, "{port}", &host, &port], None)).await.unwrap();
        let mut session = Session::open(&connection, tunnel.route()).await.unwrap();
        let result =
            session.run(&guard::admit(Statement::new("SELECT 7"), Params::default(), WritePolicy::ReadOnly).unwrap()).await.unwrap();
        assert_eq!(result.rows(), [[Value::Int(7)]]);
    }
}
