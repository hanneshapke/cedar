//! A desktop per run, for the `computer` tool: in a container, or the user's own.
//!
//! The desktop (a virtual screen with a window manager and a few applications, the
//! `cedar-computer` image, see `computer/`) runs in its own container, like the run's browser
//! (see [`crate::browser`]): it is started on the run's first `computer` call and removed with
//! the run, and each call is a `docker exec` of `cedar-computer call <json>`. Unlike the browser
//! it mounts the conversation's workspace, so a file the agent wrote can be opened in an
//! application and what an application saves is there for the sandbox's tools.
//!
//! With `COMPUTER_RUNTIME=native` there is no container: the "desktop" is the screen, mouse and
//! keyboard of the computer cedar runs on (see [`NativeComputerFactory`] and [`crate::native`]),
//! which one run at a time may use, and only with the user's OK (see [`ComputerSession::native`]).
//! Which of the two a run gets is `COMPUTER_RUNTIME`, unless an admin chose otherwise with the
//! `computer_runtime` setting (Settings → Advanced in the chat): [`Computers`] reads it as each
//! run starts.

use crate::config::{ComputerConfig, ComputerRuntime};
use crate::store::Store;
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
    /// The size of its screen, in the units positions are given in.
    fn screen(&self) -> (u32, u32);
}

/// Why a computer can't be started, in words meant for the model and the user (a permission that
/// is missing, a screen another conversation is using). Any other failure to start is an
/// operator's matter and reaches them as "could not be started".
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ComputerUnavailable(pub String);

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
    /// The user's own computer rather than a sandbox: then a run must be attended to use it,
    /// and its calls wait for the user's OK.
    native: bool,
    /// The user allowed this run's calls on their own computer for the rest of the run.
    allowed: AtomicBool,
    /// The saved actions the user allowed this run to carry out on their own computer, each
    /// with how many calls that still covers.
    actions: std::sync::Mutex<std::collections::HashMap<String, u32>>,
}

impl ComputerSession {
    pub fn new(factory: Arc<dyn ComputerFactory>, run_id: Uuid, workspace_id: Uuid) -> Self {
        Self {
            factory,
            run_id,
            workspace_id,
            computer: tokio::sync::Mutex::new(None),
            started: AtomicBool::new(false),
            native: false,
            allowed: AtomicBool::new(false),
            actions: Default::default(),
        }
    }

    /// The session when `factory` hands out the user's own computer.
    pub fn on_the_users_computer(mut self) -> Self {
        self.native = true;
        self
    }

    /// Whether this is the user's own computer rather than a sandbox.
    pub fn native(&self) -> bool {
        self.native
    }

    /// Whether the user allowed the rest of this run's calls on their own computer.
    pub fn allowed(&self) -> bool {
        self.allowed.load(Ordering::SeqCst)
    }

    /// Remember that the user allowed the rest of this run's calls.
    pub fn allow(&self) {
        self.allowed.store(true, Ordering::SeqCst);
    }

    /// Remember that the user allowed the saved action `name`, for `calls` calls.
    pub fn allow_action(&self, name: &str, calls: u32) {
        self.actions.lock().unwrap().insert(name.to_string(), calls);
    }

    /// Take one call of what the user allowed for the saved action `name`; `false` when it was
    /// not allowed or is used up, and the user has to be asked.
    pub fn spend_action(&self, name: &str) -> bool {
        match self.actions.lock().unwrap().get_mut(name) {
            Some(left) if *left > 0 => {
                *left -= 1;
                true
            }
            _ => false,
        }
    }

    /// The run's desktop, started first if this is its first use.
    async fn computer(&self) -> Result<Arc<dyn Computer>> {
        let mut slot = self.computer.lock().await;
        if let Some(computer) = slot.as_ref() {
            return Ok(computer.clone());
        }
        self.started.store(true, Ordering::SeqCst);
        match self.factory.create(self.run_id, self.workspace_id).await {
            Ok(computer) => {
                *slot = Some(computer.clone());
                Ok(computer)
            }
            Err(e) if e.is::<ComputerUnavailable>() => Err(e),
            Err(e) => {
                // The detail (daemon socket paths, image names, ...) is for operators.
                tracing::error!(run_id = %self.run_id, error = %format!("{e:#}"), "could not start the computer");
                bail!("the computer could not be started");
            }
        }
    }

    /// The size of the desktop's screen, starting the desktop first if need be.
    pub async fn screen(&self) -> Result<(u32, u32)> {
        Ok(self.computer().await?.screen())
    }

    /// Send one request, starting the desktop first if this is the run's first call.
    pub async fn call(&self, request: &Value, timeout: Duration) -> Result<Value> {
        let computer = self.computer().await?;
        match computer.call(request, timeout).await {
            Err(e) if e.is::<ComputerLost>() => {
                self.computer.lock().await.take();
                self.factory.destroy(self.run_id).await;
                Err(if self.native() {
                    anyhow!("{e}; the next call tries again")
                } else {
                    anyhow!(
                        "{e}, so it was shut down; the next call starts a new one (its open \
                         windows and unsaved work are gone, files saved in the workspace are \
                         still there)"
                    )
                })
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
// Which computer a run gets
// ---------------------------------------------------------------------------------------------

/// Where the chosen runtime is saved in the settings table.
const RUNTIME_SETTING: &str = "computer_runtime";

/// The saved `computer_runtime`, or `None` when none was chosen (then `COMPUTER_RUNTIME` holds).
pub async fn saved_runtime(store: &Store) -> Result<Option<ComputerRuntime>> {
    Ok(store
        .setting(RUNTIME_SETTING)
        .await?
        .and_then(|v| v.as_str().and_then(ComputerRuntime::parse)))
}

/// Save `computer_runtime`, or go back to `COMPUTER_RUNTIME` with `None`. It applies from the
/// next run; a run that is going keeps the computer it has.
pub async fn set_runtime(store: &Store, runtime: Option<ComputerRuntime>) -> Result<()> {
    store
        .set_setting(RUNTIME_SETTING, runtime.map(|r| serde_json::json!(r.as_str())).as_ref())
        .await
}

/// The computers of both runtimes, and the choice between them for each new run.
pub struct Computers {
    docker: Arc<dyn ComputerFactory>,
    /// `None` when this cedar can't drive the computer it runs on (built without the
    /// `native-computer` feature).
    native: Option<Arc<dyn ComputerFactory>>,
    store: Store,
    /// `COMPUTER_RUNTIME`.
    default: ComputerRuntime,
}

impl Computers {
    pub fn new(
        docker: Arc<dyn ComputerFactory>,
        native: Option<Arc<dyn ComputerFactory>>,
        store: Store,
        default: ComputerRuntime,
    ) -> Self {
        Self { docker, native, store, default }
    }

    /// `COMPUTER_RUNTIME`: what runs get while no admin has chosen.
    pub fn default_runtime(&self) -> ComputerRuntime {
        self.default
    }

    /// Whether this cedar can drive the computer it runs on.
    pub fn native_supported(&self) -> bool {
        self.native.is_some()
    }

    fn factory(&self, runtime: ComputerRuntime) -> Option<&Arc<dyn ComputerFactory>> {
        match runtime {
            ComputerRuntime::Docker => Some(&self.docker),
            ComputerRuntime::Native => self.native.as_ref(),
        }
    }

    /// The runtime new runs get: the saved one, else `COMPUTER_RUNTIME`. A saved `native` that
    /// this cedar can't drive falls back to the container, the one that is a sandbox.
    pub async fn runtime(&self) -> ComputerRuntime {
        let chosen = match saved_runtime(&self.store).await {
            Ok(saved) => saved.unwrap_or(self.default),
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "could not read computer_runtime; using COMPUTER_RUNTIME");
                self.default
            }
        };
        if self.factory(chosen).is_some() {
            chosen
        } else {
            ComputerRuntime::Docker
        }
    }

    /// A run's computer on the runtime in force now; nothing starts before its first call.
    pub async fn session(&self, run_id: Uuid, workspace_id: Uuid) -> ComputerSession {
        match (self.runtime().await, &self.native) {
            (ComputerRuntime::Native, Some(native)) => {
                ComputerSession::new(native.clone(), run_id, workspace_id).on_the_users_computer()
            }
            _ => ComputerSession::new(self.docker.clone(), run_id, workspace_id),
        }
    }

    /// Whether runs could use `runtime` right now; the error says why not.
    pub async fn check(&self, runtime: ComputerRuntime) -> Result<()> {
        match self.factory(runtime) {
            Some(factory) => factory.health().await,
            None => bail!(
                "this cedar was built without the driver for its own computer (the \
                 `native-computer` feature); the desktop app's has it"
            ),
        }
    }

    /// Is the runtime in force usable? (for `/readyz`)
    pub async fn health(&self) -> Result<()> {
        self.check(self.runtime().await).await
    }

    /// Remove desktops left behind by a previous process. Returns how many were removed.
    pub async fn cleanup_orphans(&self) -> Result<usize> {
        self.docker.cleanup_orphans().await
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
    screen: (u32, u32),
}

#[async_trait]
impl Computer for DockerComputer {
    fn screen(&self) -> (u32, u32) {
        self.screen
    }

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
            screen: (self.cfg.width, self.cfg.height),
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

// ---------------------------------------------------------------------------------------------
// The computer cedar runs on
// ---------------------------------------------------------------------------------------------

/// How long the driver may take to say what the screen is like.
const INFO_TIMEOUT: Duration = Duration::from_secs(20);

/// The user's own screen, mouse and keyboard, through a driver process per call (`cedar computer
/// call <json>`, see [`crate::native`]). There is one screen, so one run at a time holds it.
pub struct NativeComputerFactory {
    /// The driver's command: a program and the arguments before `info` or `call`.
    driver: Vec<String>,
    /// The run that holds the screen.
    holder: Arc<std::sync::Mutex<Option<Uuid>>>,
}

impl NativeComputerFactory {
    /// With this cedar's own `computer` subcommand as the driver.
    pub fn new() -> Result<Self> {
        let exe = std::env::current_exe().context("finding cedar's own program")?;
        Ok(Self::with_driver(vec![exe.to_string_lossy().into_owned(), "computer".into()]))
    }

    pub fn with_driver(driver: Vec<String>) -> Self {
        Self { driver, holder: Arc::default() }
    }
}

/// Run the driver with `args`; its reply, or as the error what it printed to explain itself.
async fn drive(driver: &[String], args: &[&str], timeout: Duration) -> Result<Value> {
    let mut command = tokio::process::Command::new(&driver[0]);
    command
        .args(&driver[1..])
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| ComputerLost(format!("did not respond within {}s", timeout.as_secs())))?
        .context("starting the computer driver")?;
    if !out.status.success() {
        let message = String::from_utf8_lossy(&out.stderr).trim().to_string();
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

pub struct NativeComputer {
    driver: Vec<String>,
    screen: (u32, u32),
}

#[async_trait]
impl Computer for NativeComputer {
    fn screen(&self) -> (u32, u32) {
        self.screen
    }

    async fn call(&self, request: &Value, timeout: Duration) -> Result<Value> {
        drive(&self.driver, &["call", &request.to_string()], timeout).await
    }
}

#[async_trait]
impl ComputerFactory for NativeComputerFactory {
    async fn create(&self, run_id: Uuid, _workspace_id: Uuid) -> Result<Arc<dyn Computer>> {
        {
            let mut holder = self.holder.lock().unwrap();
            if holder.is_some_and(|other| other != run_id) {
                return Err(ComputerUnavailable(
                    "another conversation is using this computer right now; try again when it \
                     has finished"
                        .into(),
                )
                .into());
            }
            *holder = Some(run_id);
        }
        // What the driver says when the screen can't be used (a permission that is missing) is
        // for the user to act on.
        let info = match drive(&self.driver, &["info"], INFO_TIMEOUT).await {
            Ok(info) => info,
            Err(e) => {
                self.destroy(run_id).await;
                return Err(ComputerUnavailable(format!("{e:#}")).into());
            }
        };
        let side = |i: usize| info["screen"][i].as_u64().and_then(|n| u32::try_from(n).ok());
        match (side(0), side(1)) {
            (Some(width), Some(height)) if width >= 16 && height >= 16 => {
                Ok(Arc::new(NativeComputer { driver: self.driver.clone(), screen: (width, height) }))
            }
            _ => {
                self.destroy(run_id).await;
                bail!("the computer driver did not say how large the screen is: {info}")
            }
        }
    }

    /// Give the screen back. Nothing on it is closed: it is the user's.
    async fn destroy(&self, run_id: Uuid) {
        let mut holder = self.holder.lock().unwrap();
        if *holder == Some(run_id) {
            *holder = None;
        }
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        Ok(0)
    }

    /// The driver is there. Whether the screen may be used is only known when a run asks: on a
    /// Mac the check itself can put a permission dialog on the screen.
    async fn health(&self) -> Result<()> {
        if !std::path::Path::new(&self.driver[0]).exists() {
            bail!("the computer driver {} is missing", self.driver[0]);
        }
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
            runtime: crate::config::ComputerRuntime::Docker,
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
        fn screen(&self) -> (u32, u32) {
            (1280, 800)
        }

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

    /// A driver that answers `info` with a 1440x900 screen and echoes a call's request, or
    /// fails the way the real one does without a permission.
    #[cfg(unix)]
    fn fake_driver(dir: &std::path::Path, permitted: bool) -> Vec<String> {
        use std::os::unix::fs::PermissionsExt;
        let script = if permitted {
            "#!/bin/sh\ncase \"$1\" in\n  info) echo '{\"screen\": [1440, 900]}' ;;\n  call) echo \"{\\\"echo\\\": $2}\" ;;\nesac\n"
        } else {
            "#!/bin/sh\necho 'Cedar may not see the screen yet: allow it under Screen Recording' >&2\nexit 1\n"
        };
        let path = dir.join(if permitted { "driver" } else { "driver-denied" });
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        vec![path.to_string_lossy().into_owned()]
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_users_own_computer_is_driven_through_a_process_per_call() {
        let dir = std::env::temp_dir().join(format!("cedar-native-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let factory = Arc::new(NativeComputerFactory::with_driver(fake_driver(&dir, true)));
        factory.health().await.unwrap();
        let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
        let session =
            ComputerSession::new(factory.clone(), first, Uuid::new_v4()).on_the_users_computer();
        assert!(session.native() && !session.allowed());
        session.allow();
        assert!(session.allowed());

        assert_eq!(session.screen().await.unwrap(), (1440, 900), "what the driver reports");
        let request = json!({ "action": "type", "text": "a'b; $(rm -rf /)" });
        let reply = session.call(&request, T).await.unwrap();
        assert_eq!(reply["echo"], request, "the request is one argument, as it was");

        // There is one screen: a second run waits for the first to be done with it.
        let other = ComputerSession::new(factory.clone(), second, Uuid::new_v4());
        let err = other.call(&json!({ "action": "screenshot" }), T).await.unwrap_err();
        assert!(err.to_string().contains("another conversation is using this computer"), "{err}");
        session.destroy().await;
        other.call(&json!({ "action": "screenshot" }), T).await.unwrap();
        other.destroy().await;

        // What the driver says about a missing permission reaches the user as it is.
        let denied = Arc::new(NativeComputerFactory::with_driver(fake_driver(&dir, false)));
        let session = ComputerSession::new(denied.clone(), first, Uuid::new_v4());
        let err = session.screen().await.unwrap_err();
        assert!(err.to_string().contains("Screen Recording"), "{err}");
        // And the screen is free again for the next try.
        assert!(denied.holder.lock().unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
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
