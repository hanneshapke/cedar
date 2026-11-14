//! Runs llama.cpp's `llama-server` as a third child process, when the app installed it as the
//! model server (see llamasetup.rs), and keeps it running the way supervisor.rs keeps cedar and
//! cedar-chat: a crash is restarted with backoff until it looks like a loop.
//!
//! It is not part of that pair: a restart of the two servers leaves the model in the GPU's memory,
//! and loading it again takes a while. llama-server has no "exit when stdin closes" either, so
//! one that outlives the app (a crash, Task Manager) is found by its process id on the next start.

use crate::i18n;
use crate::paths::Paths;
use crate::proc;
use crate::supervisor::{describe, open_log, wait_healthy, ServiceState, FAST_EXIT, MAX_FAST_CRASHES, STOP_GRACE};
use anyhow::{anyhow, bail, Context, Result};
use cedar_llama::catalog::Model;
use cedar_llama::server;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::Child;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// How long the model gets to load. The largest is 63 GB, read from the disk into the GPU.
const LOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const NAME: &str = "llama-server";

/// What to run: the server of an installed build with a downloaded model.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub exe: PathBuf,
    pub model: &'static Model,
    /// Its weights and its vision projector on the disk.
    pub files: cedar_llama::install::ModelFiles,
    pub port: u16,
    pub api_key: String,
}

pub struct LlamaServer {
    paths: Paths,
    state: Arc<Mutex<ServiceState>>,
    current: tokio::sync::Mutex<Option<Handle>>,
}

struct Handle {
    spec: Spec,
    stop: CancellationToken,
    finished: tokio::task::JoinHandle<()>,
}

impl LlamaServer {
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            state: Arc::new(Mutex::new(ServiceState::Stopped)),
            current: tokio::sync::Mutex::new(None),
        }
    }

    pub fn state(&self) -> ServiceState {
        self.state.lock().unwrap().clone()
    }

    /// Starts the server and returns once the model is loaded, or with the reason it isn't. One
    /// that already runs with the same build, model and port is left as it is.
    pub async fn start(&self, spec: Spec) -> Result<()> {
        let mut current = self.current.lock().await;
        if let Some(h) = current.as_ref() {
            if h.spec == spec && self.state() == ServiceState::Running {
                return Ok(());
            }
        }
        if let Some(h) = current.take() {
            h.stop.cancel();
            let _ = h.finished.await;
        }
        kill_stale(&self.paths.llama_pid()).await;
        let stop = CancellationToken::new();
        let (tx, rx) = oneshot::channel();
        let runner = Runner {
            paths: self.paths.clone(),
            spec: spec.clone(),
            state: self.state.clone(),
            stop: stop.clone(),
        };
        let finished = tokio::spawn(runner.run(tx));
        *current = Some(Handle { spec, stop, finished });
        drop(current);
        rx.await.unwrap_or_else(|_| Err(anyhow!("{NAME} stopped before the model was loaded")))
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
    paths: Paths,
    spec: Spec,
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
            let ended = self.run_once(&mut first).await;
            let _ = std::fs::remove_file(self.paths.llama_pid());
            let Err(e) = ended else {
                self.set(ServiceState::Stopped);
                return;
            };
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
                self.set(ServiceState::Crashed(i18n::tf(
                    "services.gave_up",
                    &[("detail", &detail), ("count", &fast_crashes.to_string())],
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

    /// One life of the server. `Ok` means a deliberate stop; `Err` carries why it ended otherwise.
    async fn run_once(&self, first: &mut Option<oneshot::Sender<Result<()>>>) -> Result<()> {
        let spec = &self.spec;
        if std::net::TcpListener::bind(("127.0.0.1", spec.port)).is_err() {
            bail!(i18n::tf(
                "services.port_in_use",
                &[
                    ("port", &spec.port.to_string()),
                    ("what", NAME),
                    ("config", &self.paths.config().display().to_string()),
                ],
            ));
        }
        self.set(ServiceState::Starting(i18n::tf("llama.loading", &[("model", spec.model.label)])));
        let log = self.paths.llama_log();
        let out = open_log(&log)?;
        let err = out.try_clone()?;
        let mut child = proc::command(&spec.exe)
            // Its libraries sit beside it.
            .current_dir(spec.exe.parent().unwrap_or(Path::new(".")))
            .args(server::args(spec.model, &spec.files, spec.port))
            .env(server::API_KEY_ENV, &spec.api_key)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {}", spec.exe.display()))?;
        if let Some(pid) = child.id() {
            let _ = std::fs::write(self.paths.llama_pid(), pid.to_string());
        }

        let health = format!("http://127.0.0.1:{}{}", spec.port, server::HEALTH_PATH);
        let loaded = tokio::select! {
            _ = self.stop.cancelled() => {
                stop_child(&mut child).await;
                return Ok(());
            }
            loaded = wait_healthy(&mut child, &health, NAME, &log, LOAD_TIMEOUT) => loaded,
        };
        if let Err(e) = loaded {
            stop_child(&mut child).await;
            return Err(e);
        }
        self.set(ServiceState::Running);
        if let Some(tx) = first.take() {
            let _ = tx.send(Ok(()));
        }

        tokio::select! {
            _ = self.stop.cancelled() => {
                stop_child(&mut child).await;
                Ok(())
            }
            status = child.wait() => {
                Err(anyhow!("{NAME} exited ({}); see {}", describe(status), log.display()))
            }
        }
    }
}

/// SIGTERM, a grace period, then a kill. Windows has no SIGTERM and the server keeps nothing that
/// a kill would lose, so there it is the kill at once.
async fn stop_child(child: &mut Child) {
    let Some(pid) = child.id() else { return };
    #[cfg(unix)]
    {
        // SAFETY: a plain signal to a process id we own; the worst case is ESRCH if it just exited.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_ok() {
            return;
        }
    }
    #[cfg(not(unix))]
    let _ = (pid, STOP_GRACE);
    let _ = child.kill().await;
}

/// Whether what the system says about a process id (`ps -o comm=` or `tasklist`) names a
/// llama-server. The id in an old pid file may long belong to another program.
fn is_llama_server(listing: &str) -> bool {
    listing.to_ascii_lowercase().contains(NAME)
}

/// Ends a llama-server that an earlier run of the app started and did not stop (the app crashed
/// or was killed), so that it does not keep the port and the GPU's memory.
async fn kill_stale(pid_file: &Path) {
    let Ok(text) = std::fs::read_to_string(pid_file) else {
        return;
    };
    let _ = std::fs::remove_file(pid_file);
    let Ok(pid) = text.trim().parse::<u32>() else {
        return;
    };
    let pid = pid.to_string();
    let (list, kill): (&[&str], &[&str]) = if cfg!(windows) {
        (&["tasklist", "/FI", "PID eq {pid}", "/FO", "CSV", "/NH"], &["taskkill", "/PID", "{pid}", "/F"])
    } else {
        (&["ps", "-p", "{pid}", "-o", "comm="], &["kill", "-9", "{pid}"])
    };
    let run = |words: &[&str]| {
        let words: Vec<String> = words.iter().map(|w| w.replace("{pid}", &pid)).collect();
        let mut command = proc::command(&words[0]);
        command.args(&words[1..]).stdin(Stdio::null()).stderr(Stdio::null());
        command
    };
    let listing = match tokio::time::timeout(Duration::from_secs(10), run(list).output()).await {
        Ok(Ok(out)) => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => return,
    };
    if is_llama_server(&listing) {
        let _ = tokio::time::timeout(Duration::from_secs(10), run(kill).output()).await;
        // The port is free a moment after the process is gone.
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_listing_names_the_server() {
        assert!(is_llama_server("llama-server\n"));
        assert!(is_llama_server("\"llama-server.exe\",\"4242\",\"Console\",\"1\",\"9,812,344 K\"\r\n"));
        assert!(!is_llama_server("INFO: No tasks are running which match the specified criteria.\r\n"));
        assert!(!is_llama_server("Finder\n"));
        assert!(!is_llama_server(""));
    }

    /// The whole life of the server, with a stand-in for llama-server that answers `/health` on
    /// the port it is given: it is up once that answers, a second start with the same spec leaves
    /// it alone, and a stop ends it and clears the pid file. Skipped where there is no python3.
    #[cfg(unix)]
    #[tokio::test]
    async fn starts_waits_for_health_and_stops() {
        use std::os::unix::fs::PermissionsExt;
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cedar-llamaserver-life-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("llama-server");
        let script = "#!/usr/bin/env python3\n\
            import sys, os, http.server\n\
            assert os.environ['LLAMA_API_KEY'] == 'secret'\n\
            port = int(sys.argv[sys.argv.index('--port') + 1])\n\
            class H(http.server.BaseHTTPRequestHandler):\n\
            \x20   def do_GET(self):\n\
            \x20       self.send_response(200 if self.path == '/health' else 404)\n\
            \x20       self.end_headers()\n\
            \x20   def log_message(self, *a): pass\n\
            http.server.HTTPServer(('127.0.0.1', port), H).serve_forever()\n";
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A port that is free right now.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let paths = Paths { data: dir.clone(), logs: dir.clone(), tools: dir.clone() };
        let server = LlamaServer::new(paths.clone());
        let spec = Spec {
            exe,
            model: &cedar_llama::catalog::MODELS[0],
            files: cedar_llama::install::ModelFiles { weights: dir.join("model.gguf"), mmproj: dir.join("mmproj.gguf") },
            port,
            api_key: "secret".into(),
        };

        server.start(spec.clone()).await.unwrap();
        assert_eq!(server.state(), ServiceState::Running);
        let pid = std::fs::read_to_string(paths.llama_pid()).unwrap();
        // The same again: the running one stays.
        server.start(spec.clone()).await.unwrap();
        assert_eq!(std::fs::read_to_string(paths.llama_pid()).unwrap(), pid);

        server.stop().await;
        assert_eq!(server.state(), ServiceState::Stopped);
        assert!(!paths.llama_pid().exists());
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(), "the port is still taken");

        // A server that exits at once is reported with where to look.
        std::fs::write(&spec.exe, "#!/bin/sh\nexit 3\n").unwrap();
        let failed = server.start(spec).await.unwrap_err().to_string();
        assert!(failed.contains("exit code 3") && failed.contains("llama-server.log"), "{failed}");
        assert!(matches!(server.state(), ServiceState::Crashed(_)));
    }

    /// A pid file left by a crash names a process that is something else by now: it is left alone,
    /// and the file is cleared either way.
    #[cfg(unix)]
    #[tokio::test]
    async fn only_a_llama_server_is_ended() {
        let dir = std::env::temp_dir().join(format!("cedar-llamaserver-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("llama-server.pid");
        let mut other = tokio::process::Command::new("sleep").arg("30").kill_on_drop(true).spawn().unwrap();
        std::fs::write(&pid_file, other.id().unwrap().to_string()).unwrap();
        kill_stale(&pid_file).await;
        assert!(!pid_file.exists());
        assert!(other.try_wait().unwrap().is_none(), "a process that is not llama-server was ended");

        // One whose name is llama-server is.
        let fake = dir.join("llama-server");
        std::fs::copy("/bin/sleep", &fake).unwrap();
        let mut stale = tokio::process::Command::new(&fake).arg("30").kill_on_drop(true).spawn().unwrap();
        std::fs::write(&pid_file, stale.id().unwrap().to_string()).unwrap();
        kill_stale(&pid_file).await;
        let status = tokio::time::timeout(Duration::from_secs(5), stale.wait()).await;
        assert!(status.is_ok(), "the stale llama-server still runs");

        std::fs::write(&pid_file, "not a number").unwrap();
        kill_stale(&pid_file).await;
        assert!(!pid_file.exists());
    }
}
