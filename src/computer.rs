//! A desktop per run, for the `computer` tool.
//!
//! The desktop (a virtual screen with a window manager and a few applications, the
//! `cedar-computer` image, see `computer/`) runs in its own container, like the run's browser
//! (see [`crate::browser`]): it is started on the run's first `computer` call and removed with
//! the run, and each call is a `docker exec` of `cedar-computer call <json>`. Unlike the browser
//! it mounts the conversation's workspace, so a file the agent wrote can be opened in an
//! application and what an application saves is there for the sandbox's tools.

use crate::config::ComputerConfig;
use crate::sandbox::{
    docker_checked, docker_output, volume_name, DOCKER_CALL_TIMEOUT, DOCKER_CREATE_TIMEOUT,
};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// A desktop that is running, for one run.
#[async_trait]
pub trait Computer: Send + Sync {
    /// Send one request (see `computer/driver.py`) and return the driver's reply. An `Err`
    /// carries the driver's explanation, meant for the model; [`ComputerLost`] means the desktop
    /// itself is stuck or gone.
    async fn call(&self, request: &Value, timeout: Duration) -> Result<Value>;
}

#[async_trait]
pub trait ComputerFactory: Send + Sync {
    /// Start a desktop for `run_id` that works on the files of `workspace_id`.
    async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Computer>>;
    /// Remove `run_id`'s desktop, including one whose `create` was interrupted. Best effort and
    /// idempotent: failures are logged, not returned.
    async fn destroy(&self, run_id: Uuid);
    /// Remove desktops left behind by a previous process. Returns how many were removed.
    async fn cleanup_orphans(&self) -> Result<usize>;
    /// Is the computer image available? (for `/readyz`)
    async fn health(&self) -> Result<()>;
}

/// The desktop hung or stopped, e.g. it ran out of memory. The session replaces it, since a call
/// that never returned may still be holding the pointer.
#[derive(Debug, thiserror::Error)]
#[error("the computer {0}")]
pub struct ComputerLost(pub String);

/// A run's desktop: started on first use, restarted if it stops responding, and destroyed with
/// the run.
pub struct ComputerSession {
    factory: Arc<dyn ComputerFactory>,
    run_id: Uuid,
    workspace_id: Uuid,
    computer: tokio::sync::Mutex<Option<Arc<dyn Computer>>>,
    /// A desktop was (or may have been) created, so `destroy` has something to remove.
    started: AtomicBool,
}

impl ComputerSession {
    pub fn new(factory: Arc<dyn ComputerFactory>, run_id: Uuid, workspace_id: Uuid) -> Self {
        Self {
            factory,
            run_id,
            workspace_id,
            computer: tokio::sync::Mutex::new(None),
            started: AtomicBool::new(false),
        }
    }

    /// Send one request, starting the desktop first if this is the run's first call.
    pub async fn call(&self, request: &Value, timeout: Duration) -> Result<Value> {
        let computer = {
            let mut slot = self.computer.lock().await;
            match slot.as_ref() {
                Some(computer) => computer.clone(),
                None => {
                    self.started.store(true, Ordering::SeqCst);
                    let created = self.factory.create(self.run_id, self.workspace_id).await;
                    let computer = match created {
                        Ok(computer) => computer,
                        Err(e) => {
                            // The detail (daemon socket paths, image names, ...) is for operators.
                            tracing::error!(run_id = %self.run_id, error = %format!("{e:#}"), "could not start the computer");
                            bail!("the computer could not be started");
                        }
                    };
                    *slot = Some(computer.clone());
                    computer
                }
            }
        };
        match computer.call(request, timeout).await {
            Err(e) if e.is::<ComputerLost>() => {
                self.computer.lock().await.take();
                self.factory.destroy(self.run_id).await;
                Err(anyhow!(
                    "{e}, so it was shut down; the next call starts a new one (its open windows \
                     and unsaved work are gone, files saved in the workspace are still there)"
                ))
            }
            other => other,
        }
    }

    /// Remove the desktop if one was started. Safe to call more than once.
    pub async fn destroy(&self) {
        if self.started.swap(false, Ordering::SeqCst) {
            self.computer.lock().await.take();
            self.factory.destroy(self.run_id).await;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Docker
// ---------------------------------------------------------------------------------------------

const LABEL: &str = "cedar.computer";
/// The most of a reply that is read; a screenshot is far smaller.
const MAX_REPLY_BYTES: usize = 32 * 1024 * 1024;
/// How long the desktop gets to come up after its container started, before the first call.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

fn container_name(run_id: Uuid) -> String {
    format!("cedar-computer-{run_id}")
}

/// Arguments for `docker run` that start a run's desktop. The image's command starts the screen
/// and the desktop on it.
pub fn run_args(cfg: &ComputerConfig, run_id: Uuid, workspace_id: Uuid) -> Vec<String> {
    [
        "run",
        "-d",
        "--init", // a desktop starts many processes; reap them
        "--name",
        &container_name(run_id),
        "--label",
        &format!("{LABEL}={run_id}"),
        "--network",
        &cfg.network,
        "--memory",
        &cfg.memory,
        "--memory-swap",
        &cfg.memory,
        "--cpus",
        &cfg.cpus,
        "--pids-limit",
        "1024",
        "--shm-size",
        "512m",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "1000:1000",
        "-e",
        &format!("CEDAR_SCREEN={}x{}", cfg.width, cfg.height),
        "-v",
        &format!("{}:/workspace", volume_name(workspace_id)),
        &cfg.image,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Arguments for `docker exec` that send `request` to the driver. The JSON is one argv element.
pub fn call_args(container: &str, request: &Value) -> Vec<String> {
    vec![
        "exec".into(),
        container.into(),
        "cedar-computer".into(),
        "call".into(),
        request.to_string(),
    ]
}

pub struct DockerComputer {
    docker: String,
    container: String,
}

#[async_trait]
impl Computer for DockerComputer {
    async fn call(&self, request: &Value, timeout: Duration) -> Result<Value> {
        let args = call_args(&self.container, request);
        // docker_output's own limit is only a backstop; this one is the call's.
        let call = docker_output(&self.docker, &args, timeout + DOCKER_CALL_TIMEOUT);
        let out = tokio::time::timeout(timeout, call)
            .await
            .map_err(|_| ComputerLost(format!("did not respond within {}s", timeout.as_secs())))??;
        if !out.status.success() {
            let message = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if message.contains("is not running") || message.contains("No such container") {
                return Err(ComputerLost("stopped (it may have run out of memory)".into()).into());
            }
            bail!(if message.is_empty() {
                format!("the computer call failed ({})", out.status)
            } else {
                message
            });
        }
        if out.stdout.len() > MAX_REPLY_BYTES {
            bail!("the screenshot is too large");
        }
        serde_json::from_slice(&out.stdout).context("the computer sent an unreadable reply")
    }
}

pub struct DockerComputerFactory {
    docker: String,
    cfg: ComputerConfig,
}

impl DockerComputerFactory {
    pub fn new(cfg: ComputerConfig) -> Self {
        Self {
            docker: cfg.docker_bin.clone(),
            cfg,
        }
    }

    /// Wait until the screen answers, so the model's first look isn't at a desktop half drawn.
    async fn wait_ready(&self, container: &str) -> Result<()> {
        // The screen is up when xdotool can ask for its size; the desktop then gets a moment
        // to draw its panel.
        let probe = "until xdotool getdisplaygeometry >/dev/null 2>&1; do sleep 0.2; done; sleep 2";
        let args = ["exec", container, "sh", "-c", probe].map(String::from);
        tokio::time::timeout(
            STARTUP_TIMEOUT,
            docker_checked(&self.docker, &args, STARTUP_TIMEOUT + DOCKER_CALL_TIMEOUT),
        )
        .await
        .map_err(|_| anyhow!("the screen did not start within {}s", STARTUP_TIMEOUT.as_secs()))??;
        Ok(())
    }
}

#[async_trait]
impl ComputerFactory for DockerComputerFactory {
    async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Computer>> {
        let container = container_name(run_id);
        let started = async {
            let args = run_args(&self.cfg, run_id, workspace_id);
            docker_checked(&self.docker, &args, DOCKER_CREATE_TIMEOUT).await?;
            self.wait_ready(&container).await
        };
        if let Err(e) = started.await {
            self.destroy(run_id).await;
            return Err(e.context("creating computer container"));
        }
        Ok(Arc::new(DockerComputer {
            docker: self.docker.clone(),
            container,
        }))
    }

    async fn destroy(&self, run_id: Uuid) {
        let container = container_name(run_id);
        let args = ["rm", "-f", &container].map(String::from);
        if let Err(e) = docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await {
            // Removing a container that never got created is fine.
            if !e.to_string().contains("No such container") {
                tracing::warn!(error = %e, %container, "removing computer container failed");
            }
        }
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        let ps = ["ps", "-aq", "--filter", &format!("label={LABEL}")].map(String::from);
        let ids = docker_checked(&self.docker, &ps, DOCKER_CALL_TIMEOUT).await?;
        let ids: Vec<String> = ids.split_whitespace().map(String::from).collect();
        if !ids.is_empty() {
            let mut args = vec!["rm".to_string(), "-f".into()];
            args.extend(ids.iter().cloned());
            docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
        }
        Ok(ids.len())
    }

    async fn health(&self) -> Result<()> {
        let inspect =
            ["image", "inspect", "--format", "{{.Id}}", &self.cfg.image].map(String::from);
        docker_checked(&self.docker, &inspect, DOCKER_CALL_TIMEOUT)
            .await
            .with_context(|| {
                format!(
                    "computer image {} is missing; build it with `docker build -t {} computer/`",
                    self.cfg.image, self.cfg.image
                )
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    fn cfg() -> ComputerConfig {
        ComputerConfig {
            enabled: true,
            docker_bin: "docker".into(),
            image: "computer:1".into(),
            network: "bridge".into(),
            memory: "2g".into(),
            cpus: "2".into(),
            width: 1280,
            height: 800,
        }
    }

    #[test]
    fn run_args_lock_the_container_down_and_mount_the_workspace() {
        let (run, ws) = (Uuid::nil(), Uuid::from_u128(7));
        let args = run_args(&cfg(), run, ws);
        let joined = args.join(" ");
        assert!(joined.contains(&format!("--name cedar-computer-{run}")));
        assert!(joined.contains(&format!("--label cedar.computer={run}")));
        assert!(joined.contains("--network bridge"));
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--security-opt no-new-privileges"));
        assert!(joined.contains("--user 1000:1000"));
        assert!(joined.contains("--memory 2g --memory-swap 2g"));
        assert!(joined.contains("-e CEDAR_SCREEN=1280x800"));
        assert!(
            joined.contains(&format!("-v cedar-ws-{ws}:/workspace ")),
            "the conversation's files, as in its sandbox: {joined}"
        );
        // The image comes last, so its own command (the screen and the desktop) runs.
        assert_eq!(args.last().unwrap(), "computer:1");
    }

    #[test]
    fn call_args_keep_the_request_as_a_single_argv_element() {
        let request = json!({ "action": "type", "text": "a'b; $(rm -rf /)" });
        let args = call_args("c1", &request);
        assert_eq!(args.len(), 5);
        assert_eq!(&args[..4], ["exec", "c1", "cedar-computer", "call"]);
        assert_eq!(serde_json::from_str::<Value>(&args[4]).unwrap(), request);
    }

    /// Replies `{"n": <call number>}`, or hangs when the request says so.
    #[derive(Default)]
    struct FakeComputer {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Computer for FakeComputer {
        async fn call(&self, request: &Value, timeout: Duration) -> Result<Value> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            match request["action"].as_str() {
                Some("hang") => Err(ComputerLost(format!(
                    "did not respond within {}s",
                    timeout.as_secs()
                ))
                .into()),
                Some("fail") => bail!("`x` must be a whole number from 0 to 1279"),
                _ => Ok(json!({ "n": n })),
            }
        }
    }

    #[derive(Default)]
    struct FakeFactory {
        created: Mutex<Vec<(Uuid, Uuid)>>,
        destroyed: Mutex<Vec<Uuid>>,
        fail_create: AtomicBool,
    }

    #[async_trait]
    impl ComputerFactory for FakeFactory {
        async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Computer>> {
            if self.fail_create.load(Ordering::SeqCst) {
                bail!("dial unix /var/run/docker.sock: no such file");
            }
            self.created.lock().unwrap().push((run_id, workspace_id));
            Ok(Arc::new(FakeComputer::default()))
        }
        async fn destroy(&self, run_id: Uuid) {
            self.destroyed.lock().unwrap().push(run_id);
        }
        async fn cleanup_orphans(&self) -> Result<usize> {
            Ok(0)
        }
        async fn health(&self) -> Result<()> {
            Ok(())
        }
    }

    const T: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn the_computer_starts_on_first_use_and_is_reused() {
        let factory = Arc::new(FakeFactory::default());
        let (run, ws) = (Uuid::new_v4(), Uuid::new_v4());
        let session = ComputerSession::new(factory.clone(), run, ws);
        session.destroy().await;
        assert!(
            factory.destroyed.lock().unwrap().is_empty(),
            "nothing to remove before first use"
        );

        let first = session.call(&json!({ "action": "screenshot" }), T).await;
        let second = session.call(&json!({ "action": "screenshot" }), T).await;
        assert_eq!(first.unwrap()["n"], 1);
        assert_eq!(second.unwrap()["n"], 2, "the same desktop answers");
        assert_eq!(*factory.created.lock().unwrap(), [(run, ws)]);

        session.destroy().await;
        session.destroy().await;
        assert_eq!(*factory.destroyed.lock().unwrap(), [run]);
    }

    #[tokio::test]
    async fn driver_errors_reach_the_model_and_keep_the_computer() {
        let factory = Arc::new(FakeFactory::default());
        let session = ComputerSession::new(factory.clone(), Uuid::new_v4(), Uuid::new_v4());
        let err = session
            .call(&json!({ "action": "fail" }), T)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "`x` must be a whole number from 0 to 1279");
        session
            .call(&json!({ "action": "screenshot" }), T)
            .await
            .unwrap();
        assert_eq!(factory.created.lock().unwrap().len(), 1);
        assert!(factory.destroyed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_computer_that_hangs_is_replaced() {
        let factory = Arc::new(FakeFactory::default());
        let run = Uuid::new_v4();
        let session = ComputerSession::new(factory.clone(), run, Uuid::new_v4());
        let err = session
            .call(&json!({ "action": "hang" }), T)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not respond within 5s"), "{err}");
        assert!(err.contains("the next call starts a new one"), "{err}");
        assert_eq!(*factory.destroyed.lock().unwrap(), [run]);

        let again = session.call(&json!({ "action": "screenshot" }), T).await;
        assert_eq!(again.unwrap()["n"], 1, "a new desktop answers");
        assert_eq!(factory.created.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_computer_that_cannot_start_says_so_without_the_details() {
        let factory = Arc::new(FakeFactory::default());
        factory.fail_create.store(true, Ordering::SeqCst);
        let session = ComputerSession::new(factory.clone(), Uuid::new_v4(), Uuid::new_v4());
        let err = session
            .call(&json!({ "action": "screenshot" }), T)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "the computer could not be started");
    }
}
