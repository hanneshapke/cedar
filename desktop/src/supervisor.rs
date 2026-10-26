//! Runs cedar and cedar-chat as child processes and keeps them running.
//!
//! Both binaries sit next to the app's own executable (Tauri's `externalBin`): in `Contents/MacOS/`
//! on a Mac, in the install directory on Windows. They get their configuration as environment
//! variables, write their logs to files, and hold a stdin pipe whose other end is this process:
//! with `CEDAR_EXIT_ON_STDIN_EOF=1` they exit when the app goes away, however it goes (Quit, a
//! crash, Task Manager). A deliberate stop is a SIGTERM and a grace period, like systemd; Windows
//! has no SIGTERM, so there it is closing that pipe, which the servers treat the same way.

use crate::config::Config;
use crate::docker::DockerCli;
use crate::paths::Paths;
use crate::proc;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::{Child, ChildStdin};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ServiceState {
    Stopped,
    Starting(String),
    Running,
    Crashed(String),
}

/// How long a server gets to answer `/healthz` after it was started.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(45);
/// cedar gives active runs 10 s to record their state on a stop; wait a little longer than that.
const STOP_GRACE: Duration = Duration::from_secs(15);
const MAX_LOG_BYTES: u64 = 20 * 1024 * 1024;
/// An exit this soon after a start counts as a crash loop.
const FAST_EXIT: Duration = Duration::from_secs(10);
const MAX_FAST_CRASHES: u32 = 3;

pub struct Supervisor {
    exe_dir: PathBuf,
    paths: Paths,
    state: Arc<Mutex<ServiceState>>,
    current: tokio::sync::Mutex<Option<Handle>>,
}

struct Handle {
    stop: CancellationToken,
    finished: tokio::task::JoinHandle<()>,
}

/// A running server and the stdin pipe it holds. tokio's `Child::wait` closes a stdin handle it
/// finds in the child, which with CEDAR_EXIT_ON_STDIN_EOF would stop the server as soon as it is
/// up, so the pipe is kept out here until [`terminate`] closes it.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Supervisor {
    pub fn new(exe_dir: PathBuf, paths: Paths) -> Self {
        Self {
            exe_dir,
            paths,
            state: Arc::new(Mutex::new(ServiceState::Stopped)),
            current: tokio::sync::Mutex::new(None),
        }
    }

    pub fn state(&self) -> ServiceState {
        self.state.lock().unwrap().clone()
    }

    /// Starts both servers, stopping a running pair first, and returns once they answer or with
    /// the reason they didn't. Supervision goes on in the background: a crash is restarted with
    /// backoff until it looks like a loop. `public_url` is the chat's address for other devices
    /// (its tailnet address while it is shared), for the links it sends.
    pub async fn start(&self, cfg: Config, docker: DockerCli, public_url: Option<String>) -> Result<()> {
        let mut current = self.current.lock().await;
        if let Some(h) = current.take() {
            h.stop.cancel();
            let _ = h.finished.await;
        }
        let stop = CancellationToken::new();
        let (tx, rx) = oneshot::channel();
        let runner = Runner {
            exe_dir: self.exe_dir.clone(),
            paths: self.paths.clone(),
            cfg,
            docker,
            public_url,
            state: self.state.clone(),
            stop: stop.clone(),
        };
        let finished = tokio::spawn(runner.run(tx));
        *current = Some(Handle { stop, finished });
        drop(current);
        rx.await
            .unwrap_or_else(|_| Err(anyhow!("the services stopped before they came up")))
    }

    pub async fn stop(&self) {
        let mut current = self.current.lock().await;
        if let Some(h) = current.take() {
            h.stop.cancel();
            let _ = h.finished.await;
        }
        *self.state.lock().unwrap() = ServiceState::Stopped;
    }
}

struct Runner {
    exe_dir: PathBuf,
    paths: Paths,
    cfg: Config,
    docker: DockerCli,
    public_url: Option<String>,
    state: Arc<Mutex<ServiceState>>,
    stop: CancellationToken,
}

impl Runner {
    fn set(&self, s: ServiceState) {
        *self.state.lock().unwrap() = s;
    }

    async fn run(self, first: oneshot::Sender<Result<()>>) {
        let mut first = Some(first);
        let mut fast_crashes = 0u32;
        let mut backoff = Duration::from_secs(1);
        loop {
            let started = Instant::now();
            match self.run_once(&mut first).await {
                Ok(()) => {
                    self.set(ServiceState::Stopped);
                    return;
                }
                Err(e) => {
                    let detail = format!("{e:#}");
                    self.set(ServiceState::Crashed(detail.clone()));
                    if let Some(tx) = first.take() {
                        // The very first start failed: report it and leave it to the user.
                        let _ = tx.send(Err(anyhow!(detail)));
                        return;
                    }
                    if started.elapsed() < FAST_EXIT {
                        fast_crashes += 1;
                    } else {
                        fast_crashes = 0;
                        backoff = Duration::from_secs(1);
                    }
                    if fast_crashes >= MAX_FAST_CRASHES {
                        self.set(ServiceState::Crashed(format!(
                            "{detail}. Gave up after {fast_crashes} crashes in a row; use Restart Services once the cause is fixed"
                        )));
                        return;
                    }
                    tokio::select! {
                        _ = self.stop.cancelled() => {
                            self.set(ServiceState::Stopped);
                            return;
                        }
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    /// One life of the pair: start both, report, then wait for a stop or an exit.
    /// `Ok` means a deliberate stop; `Err` carries why it ended otherwise.
    async fn run_once(&self, first: &mut Option<oneshot::Sender<Result<()>>>) -> Result<()> {
        for (port, what) in [(self.cfg.cedar_port, "cedar"), (self.cfg.chat_port, "the chat")] {
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
                bail!(
                    "port {port} is already in use, so {what} can't listen on it; change the port in {}",
                    self.paths.config().display()
                );
            }
        }

        self.set(ServiceState::Starting("starting cedar".into()));
        let cedar_log = self.paths.cedar_log();
        let mut cedar = self.spawn("cedar", self.cedar_env(), &cedar_log)?;
        if let Err(e) = wait_healthy(&mut cedar.child, &self.cfg.cedar_url(), "cedar", &cedar_log).await {
            terminate(&mut cedar).await;
            return Err(e);
        }

        self.set(ServiceState::Starting("starting the chat".into()));
        let chat_log = self.paths.chat_log();
        let mut chat = match self.spawn("cedar-chat", self.chat_env(), &chat_log) {
            Ok(c) => c,
            Err(e) => {
                terminate(&mut cedar).await;
                return Err(e);
            }
        };
        if let Err(e) = wait_healthy(&mut chat.child, &self.cfg.chat_url(), "cedar-chat", &chat_log).await {
            terminate(&mut chat).await;
            terminate(&mut cedar).await;
            return Err(e);
        }

        self.set(ServiceState::Running);
        if let Some(tx) = first.take() {
            let _ = tx.send(Ok(()));
        }

        tokio::select! {
            _ = self.stop.cancelled() => {
                // Both at once, so a quit waits for one grace period at most, not two.
                tokio::join!(terminate(&mut chat), terminate(&mut cedar));
                Ok(())
            }
            status = cedar.child.wait() => {
                terminate(&mut chat).await;
                Err(anyhow!("cedar exited ({}); see {}", describe(status), cedar_log.display()))
            }
            status = chat.child.wait() => {
                terminate(&mut cedar).await;
                Err(anyhow!("cedar-chat exited ({}); see {}", describe(status), chat_log.display()))
            }
        }
    }

    fn spawn(&self, name: &str, env: Vec<(String, String)>, log: &Path) -> Result<Server> {
        let bin = self
            .exe_dir
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if !bin.is_file() {
            bail!("{} is missing from the app bundle", bin.display());
        }
        let out = open_log(log)?;
        let err = out.try_clone()?;
        let mut child = proc::command(&bin)
            .current_dir(&self.paths.data)
            .envs(env)
            .stdin(Stdio::piped()) // held, never written: closing it tells the server to exit
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {}", bin.display()))?;
        let stdin = child.stdin.take();
        Ok(Server { child, stdin })
    }

    fn cedar_env(&self) -> Vec<(String, String)> {
        let cfg = &self.cfg;
        let s = &cfg.secrets;
        let mut env: Vec<(String, String)> = [
            ("CEDAR_API_KEYS", s.cedar_api_key.clone()),
            // The chat's key is the admin key, so Settings can save a Brave key or connect Google.
            ("CEDAR_ADMIN_KEYS", s.cedar_api_key.clone()),
            ("CEDAR_SECRET_KEY", s.cedar_secret_key.clone()),
            ("BIND_ADDR", format!("127.0.0.1:{}", cfg.cedar_port)),
            ("DATABASE_URL", sqlite_url(&self.paths.cedar_db())),
            // The model server is not configuration: the app registers it through cedar's admin
            // API once cedar is up (see models::register), and cedar keeps it in its database.
            ("SANDBOX_IMAGE", cfg.sandbox_image()),
            ("BROWSER", if cfg.browser { "on" } else { "off" }.to_string()),
            ("BROWSER_IMAGE", cfg.browser_image()),
            ("DOCKER_BIN", self.docker.bin.to_string_lossy().into_owned()),
            ("PATH", self.docker.path_env.clone()),
            ("CEDAR_EXIT_ON_STDIN_EOF", "1".to_string()),
            ("RUST_LOG", "info,sqlx=warn".to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        // The user's own entries win (later duplicates override earlier ones in `envs`).
        env.extend(cfg.cedar_env.iter().map(|(k, v)| (k.clone(), v.clone())));
        env
    }

    fn chat_env(&self) -> Vec<(String, String)> {
        let cfg = &self.cfg;
        let mut env: Vec<(String, String)> = [
            ("CEDAR_URL", cfg.cedar_url()),
            ("CEDAR_API_KEY", cfg.secrets.cedar_api_key.clone()),
            ("CHAT_BIND_ADDR", format!("127.0.0.1:{}", cfg.chat_port)),
            ("CHAT_DATABASE_URL", sqlite_url(&self.paths.chat_db())),
            ("CHAT_ACCESS_TOKEN", cfg.secrets.chat_access_token.clone()),
            // Lets Settings manage cedar's models and integrations; the window seeds the token.
            ("CHAT_ADMIN_TOKEN", cfg.secrets.chat_admin_token.clone()),
            ("PATH", self.docker.path_env.clone()),
            ("CEDAR_EXIT_ON_STDIN_EOF", "1".to_string()),
            // The app updates itself, servers and all (update.rs); the chat's banner shows its state.
            ("CHAT_UPDATE_CHECK", "off".to_string()),
            ("RUST_LOG", "info,sqlx=warn".to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        // Links in notifications and "Connect Google" point here; a CHAT_PUBLIC_URL of the user's
        // own in `chat_env` still wins.
        if let Some(url) = &self.public_url {
            env.push(("CHAT_PUBLIC_URL".to_string(), url.clone()));
        }
        env.extend(cfg.chat_env.iter().map(|(k, v)| (k.clone(), v.clone())));
        env
    }
}

/// Polls `/healthz` until it answers, the child exits, or the timeout passes.
async fn wait_healthy(child: &mut Child, url: &str, name: &str, log: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            bail!(
                "{name} exited ({}) right after starting; see {}",
                describe(Ok(status)),
                log.display()
            );
        }
        let ok = client
            .get(format!("{url}/healthz"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!(
                "{name} did not answer on {url} within {}s; see {}",
                HEALTH_TIMEOUT.as_secs(),
                log.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// SIGTERM and closing its stdin (only the latter on Windows, which has no SIGTERM; the servers
/// stop the same way on either), a grace period, then a kill. Nothing to do if it already exited.
async fn terminate(server: &mut Server) {
    let Some(pid) = server.child.id() else { return };
    #[cfg(unix)]
    // SAFETY: a plain signal to a process id we own; the worst case is ESRCH if it just exited.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
    server.stdin.take();
    if tokio::time::timeout(STOP_GRACE, server.child.wait()).await.is_err() {
        let _ = server.child.kill().await;
    }
}

fn describe(status: std::io::Result<std::process::ExitStatus>) -> String {
    match status {
        Ok(s) => match s.code() {
            Some(code) => format!("exit code {code}"),
            None => "killed by a signal".to_string(),
        },
        Err(e) => format!("wait failed: {e}"),
    }
}

/// Opens the log for appending, moving a large one aside first.
fn open_log(path: &Path) -> Result<std::fs::File> {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_LOG_BYTES {
            let _ = std::fs::rename(path, path.with_extension("log.1"));
        }
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}

/// A `sqlite://` URL for an absolute path. sqlx percent-decodes the path, so the characters that
/// would be read as URL syntax are encoded; a space is fine as it is.
fn sqlite_url(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut out = String::with_capacity(raw.len() + 9);
    out.push_str("sqlite://");
    for c in raw.chars() {
        match c {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3F"),
            '#' => out.push_str("%23"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug behind "stdin closed; the launcher is gone" half a second after every start:
    /// `Child::wait` drops the stdin handle unless it was taken out of the child first.
    #[cfg(unix)]
    #[tokio::test]
    async fn waiting_does_not_close_a_stdin_handle_we_hold() {
        let mut child = tokio::process::Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let held = child.stdin.take();
        let waited = tokio::time::timeout(Duration::from_millis(500), child.wait()).await;
        assert!(waited.is_err(), "cat exited, so its stdin was closed under it");
        drop(held);
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("cat exits once its stdin is closed")
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn sqlite_urls_keep_spaces_and_escape_url_syntax() {
        assert_eq!(
            sqlite_url(Path::new("/Users/me/Library/Application Support/x/cedar.db")),
            "sqlite:///Users/me/Library/Application Support/x/cedar.db"
        );
        assert_eq!(sqlite_url(Path::new("/tmp/a#b?c%.db")), "sqlite:///tmp/a%23b%3Fc%25.db");
    }

    #[test]
    fn service_state_serializes_for_the_page() {
        let s = serde_json::to_value(ServiceState::Crashed("boom".into())).unwrap();
        assert_eq!(s["state"], "crashed");
        assert_eq!(s["detail"], "boom");
        assert_eq!(serde_json::to_value(ServiceState::Running).unwrap()["state"], "running");
    }
}
