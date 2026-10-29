//! Isolated execution environments for a run's tools.
//!
//! Two runtimes run them (see [`Runtime`]), and an admin picks one with the `sandbox_runtime`
//! setting (Settings → Advanced in the chat):
//!
//! - **Docker** (the default, [`docker`]): a locked-down container per conversation, with no
//!   network unless the sandbox's policy reaches something, and then all of `SANDBOX_NETWORK`.
//! - **OpenShell** (opt-in, not on Windows, [`openshell`]): an NVIDIA OpenShell sandbox per
//!   conversation, whose policy decides which program may reach which host (see
//!   [`crate::policy`]).
//!
//! Either way, a conversation's files live in a Docker volume mounted at `/workspace`, so they
//! outlive the sandbox and a conversation can move from one runtime to the other. Every file and
//! command operation goes through the sandbox, never the host filesystem. Reading host-side from
//! the volume would let the agent point a symlink outside its workspace; the files API reads a
//! finished conversation's volume through a locked-down, read-only container.

pub mod docker;
pub mod openshell;

use crate::config::SandboxConfig;
use crate::openshell::Denial;
use crate::policy::Endpoint;
use crate::store::Store;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use uuid::Uuid;

pub use docker::DockerSandboxFactory;
pub use openshell::{run_sandbox_name, OpenShellSandboxFactory};

/// The sandbox itself is broken (e.g. a command wouldn't die). Unlike an ordinary tool error,
/// this is not something the model can recover from, so the run fails.
#[derive(Debug, thiserror::Error)]
#[error("sandbox failure: {0}")]
pub struct SandboxFatal(pub String);

#[derive(Debug, Default)]
pub struct ExecOutput {
    /// At most `max_output` bytes of each stream; the `*_total` fields are the real sizes.
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_total: usize,
    pub stderr_total: usize,
    /// `None` if the process was killed by a signal.
    pub exit_code: Option<i32>,
}

#[derive(Debug)]
pub struct FileContent {
    pub bytes: Vec<u8>,
    /// The file was longer than the requested limit.
    pub truncated: bool,
}

/// A regular file in a workspace, as reported by [`SandboxFactory::list_workspace_files`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkspaceFile {
    /// Relative to the workspace root, with `/` separators.
    pub path: String,
    pub size: u64,
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct WorkspaceListing {
    /// Sorted by path.
    pub files: Vec<WorkspaceFile>,
    /// There were more than [`MAX_LISTED_FILES`] files; the listing stops there.
    pub truncated: bool,
}

/// Most files a listing reports.
pub const MAX_LISTED_FILES: usize = 1000;

/// Directories a listing skips: tool caches and dependency trees, which would otherwise crowd
/// out the files the agent actually produced. `.local` is where pip installs packages.
pub const UNLISTED_DIRS: &[&str] = &[
    ".git",
    ".cache",
    ".local",
    ".venv",
    "venv",
    "node_modules",
    "__pycache__",
];

/// The workspace's files are gone (expired, deleted, or never created).
#[derive(Debug, thiserror::Error)]
#[error("workspace does not exist")]
pub struct WorkspaceMissing;

/// Check a workspace-relative path for use with [`SandboxFactory::read_workspace_file`]:
/// non-empty, relative, and without `.` / `..` components or NUL bytes.
pub fn check_workspace_path(path: &str) -> Result<()> {
    check_path(path)?;
    if path.starts_with('/') {
        bail!("path must be relative to the workspace");
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        bail!("path must not contain empty, `.` or `..` components");
    }
    Ok(())
}

#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Which runtime this sandbox runs on.
    fn runtime(&self) -> Runtime {
        Runtime::Docker
    }
    /// Run `command` with `sh -c` in `/workspace`, stopping it (and everything it started) after
    /// `timeout`. Exit code 124 means it timed out.
    async fn exec(&self, command: &str, timeout: Duration, max_output: usize)
        -> Result<ExecOutput>;
    async fn read_file(&self, path: &str, max_bytes: usize) -> Result<FileContent>;
    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()>;
    /// Run one call of a personal tool in a sandbox of its own (see [`ToolRun`]). Exit code 124
    /// means it timed out.
    async fn run_tool(
        &self,
        _call: &ToolRun<'_>,
        _timeout: Duration,
        _max_output: usize,
    ) -> Result<ExecOutput> {
        bail!("this sandbox can't run personal tools")
    }
    /// The connections and requests the sandbox's policy refused since `since`, oldest first.
    /// Only an OpenShell sandbox knows; a Docker one has no policy to refuse them.
    async fn denials(&self, _since: SystemTime) -> Result<Vec<Denial>> {
        Ok(vec![])
    }
    /// Let the sandbox make these connections from now on (approved by the user). Takes effect
    /// for the next connection; nothing restarts.
    async fn allow(&self, _endpoints: &[Endpoint]) -> Result<()> {
        bail!("this sandbox's policy can't be changed")
    }
    /// Stop the environment's processes while its conversation is idle, keeping it (and its
    /// files) for the next run. Best effort: failures are logged, not returned.
    async fn pause(&self) {}
    /// Make a kept environment usable again, starting it if it was stopped. Fails if it is gone,
    /// so the caller can start a new one instead.
    async fn resume(&self) -> Result<()> {
        Ok(())
    }
    /// Stop and remove the execution environment, keeping the workspace for a later continuation.
    /// Best effort and idempotent: failures are logged, not returned.
    async fn destroy(&self);
}

#[async_trait]
pub trait SandboxFactory: Send + Sync {
    /// Start a sandbox for `run_id` with `workspace_id` mounted at /workspace. The workspace is
    /// created if it doesn't exist; otherwise its files are picked up as they were left.
    async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Sandbox>>;
    /// Remove sandboxes left behind by a previous process. Returns how many were removed.
    /// Workspaces are not touched; see [`SandboxFactory::list_workspaces`].
    async fn cleanup_orphans(&self) -> Result<usize>;
    /// Delete a workspace and its files. Succeeds if it doesn't exist.
    async fn remove_workspace(&self, workspace_id: Uuid) -> Result<()>;
    /// Every workspace that currently exists.
    async fn list_workspaces(&self) -> Result<Vec<Uuid>>;
    /// The regular files in a workspace (see [`UNLISTED_DIRS`] and [`MAX_LISTED_FILES`]).
    /// Fails with [`WorkspaceMissing`] if the workspace doesn't exist.
    async fn list_workspace_files(&self, workspace_id: Uuid) -> Result<WorkspaceListing>;
    /// Read up to `max_bytes` of a regular file in a workspace. `path` must pass
    /// [`check_workspace_path`] and resolve inside the workspace. `Ok(None)` if there is no such
    /// file; [`WorkspaceMissing`] if there is no such workspace.
    async fn read_workspace_file(
        &self,
        workspace_id: Uuid,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<FileContent>>;
    /// Is the backing runtime reachable and usable? (for `/readyz`)
    async fn health(&self) -> Result<()>;
    /// The runtime the next sandbox starts on.
    async fn runtime(&self) -> Runtime {
        Runtime::Docker
    }
    /// Whether `runtime` can run sandboxes here: checked before an admin switches to it.
    async fn check_runtime(&self, _runtime: Runtime) -> Result<()> {
        Ok(())
    }
    /// Install a personal tool's `pip` requirements into its own environment (see
    /// [`tool_env_volume`]), in a sandbox that can reach PyPI and nothing else. Returns pip's
    /// output.
    async fn setup_tool(&self, _name: &str, _requirements: &[String]) -> Result<String> {
        bail!("this runtime can't install a tool's packages")
    }
    /// Delete a personal tool's environment. Succeeds if it doesn't exist.
    async fn remove_tool_env(&self, _name: &str) -> Result<()> {
        Ok(())
    }
}

/// One call of a personal tool. It runs in a fresh sandbox that sees the run's workspace, the
/// tool's own folder (read-only, at [`TOOL_MOUNT`]) and the tool's own environment (at
/// [`TOOL_ENV_MOUNT`], where its packages are installed), and nothing of any other tool.
pub struct ToolRun<'a> {
    pub name: &'a str,
    /// The tool's folder on the host.
    pub dir: &'a std::path::Path,
    /// The script, a file name in that folder.
    pub entry: &'a str,
    /// Handed to the script on stdin.
    pub input: &'a [u8],
    /// Environment variables for the script: the secrets its manifest declares, and the locale.
    pub env: &'a [(String, String)],
    /// The hosts the manifest declares. On OpenShell the call's sandbox can reach these (from
    /// Python) and nothing else; on Docker, declaring any gives it `TOOLS_NETWORK`.
    pub hosts: &'a [String],
}

/// Where a personal tool's folder is mounted in its sandbox.
pub const TOOL_MOUNT: &str = "/opt/cedar/tool";
/// Where a personal tool's environment (its home, and its `pip install --user` packages) is.
pub const TOOL_ENV_MOUNT: &str = "/opt/cedar/env";
const TOOL_LABEL: &str = "cedar.tool";
const TOOL_ENV_PREFIX: &str = "cedar-tool-env-";
/// How long a tool's packages may take to install.
const SETUP_TIMEOUT: Duration = Duration::from_secs(600);

/// The volume that holds a personal tool's environment. It outlives runs, so packages installed
/// once are there for every later call, and it is shared by no other tool.
pub fn tool_env_volume(name: &str) -> String {
    format!("{TOOL_ENV_PREFIX}{name}")
}

/// A read-only `--mount` of a host directory. Not `-v <host>:<container>:ro`, which splits on
/// colons and so can't take a Windows path (`C:\Users\...`). `--mount` is read as CSV, so the
/// `src=` field is quoted when the path holds a comma or a quote.
fn bind_mount_ro(host: &std::path::Path, container: &str) -> String {
    let src = format!("src={}", host.display());
    let src = if src.contains([',', '"']) {
        format!("\"{}\"", src.replace('"', "\"\""))
    } else {
        src
    };
    format!("type=bind,{src},dst={container},readonly")
}
// ---------------------------------------------------------------------------------------------
// Workspaces: Docker volumes, whichever runtime mounts them
// ---------------------------------------------------------------------------------------------

const WORKSPACE_LABEL: &str = "cedar.workspace";
const VOLUME_PREFIX: &str = "cedar-ws-";
pub(crate) const DOCKER_CALL_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DOCKER_CREATE_TIMEOUT: Duration = Duration::from_secs(120);

fn volume_name(workspace_id: Uuid) -> String {
    format!("{VOLUME_PREFIX}{workspace_id}")
}

/// Lists regular files (symlinks excluded) as NUL-separated `size\tmtime\tpath` records, skipping
/// [`UNLISTED_DIRS`]. One record more than the cap is read so truncation can be detected.
fn list_script() -> String {
    let prune = UNLISTED_DIRS
        .iter()
        .map(|d| format!("-name '{d}'"))
        .collect::<Vec<_>>()
        .join(" -o ");
    format!(
        "find /workspace -mindepth 1 \\( {prune} \\) -prune -o -type f -printf '%s\\t%T@\\t%P\\0' \
         | head -z -n {}",
        MAX_LISTED_FILES + 1
    )
}

/// Prints up to `$2` bytes of `$1` if it is a regular file inside /workspace (after resolving
/// symlinks); exits 2 if there is no such file.
const READ_WORKSPACE_SCRIPT: &str = r#"cd /workspace || exit 3
p=$(realpath -e -- "$1" 2>/dev/null) || exit 2
case $p in /workspace/*) ;; *) exit 2 ;; esac
[ -f "$p" ] || exit 2
exec head -c "$2" -- "$p""#;

/// Arguments for a one-off `docker run` that executes `script` with the workspace mounted
/// read-only, offline and unprivileged. Used to look at a workspace whose run has finished (and
/// whose sandbox may be stopped or gone); no code of the agent's runs here.
pub fn workspace_run_args(
    cfg: &SandboxConfig,
    workspace_id: Uuid,
    script: &str,
    args: &[&str],
) -> Vec<String> {
    let mut out: Vec<String> = ["run", "--rm"].iter().map(|s| s.to_string()).collect();
    out.extend(locked_down(cfg, "none", "64"));
    out.extend(
        [
            "-w",
            "/workspace",
            "-v",
            &format!("{}:/workspace:ro", volume_name(workspace_id)),
            &cfg.image,
            "sh",
            "-c",
            script,
            "sh",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    out.extend(args.iter().map(|s| s.to_string()));
    out
}

/// The restrictions every container cedar runs itself is under (Docker sandboxes, the files API),
/// as `docker run` arguments.
fn locked_down(cfg: &SandboxConfig, network: &str, pids: &str) -> Vec<String> {
    [
        "--network",
        network,
        "--memory",
        &cfg.memory,
        "--memory-swap",
        &cfg.memory,
        "--cpus",
        &cfg.cpus,
        "--pids-limit",
        pids,
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "1000:1000",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Parse the output of [`list_script`]. Unparseable records are skipped.
fn parse_listing(raw: &[u8]) -> WorkspaceListing {
    let mut files: Vec<WorkspaceFile> = raw
        .split(|&b| b == 0)
        .filter(|rec| !rec.is_empty())
        .filter_map(|rec| {
            let rec = String::from_utf8_lossy(rec);
            let mut parts = rec.splitn(3, '\t');
            let size = parts.next()?.parse().ok()?;
            let mtime: f64 = parts.next()?.parse().ok()?;
            let path = parts.next()?.to_string();
            let modified_at = chrono::DateTime::from_timestamp_millis((mtime * 1000.0) as i64)?;
            Some(WorkspaceFile {
                path,
                size,
                modified_at,
            })
        })
        .collect();
    let truncated = files.len() > MAX_LISTED_FILES;
    files.truncate(MAX_LISTED_FILES);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    WorkspaceListing { files, truncated }
}

pub(crate) async fn docker_output(
    bin: &str,
    args: &[String],
    timeout: Duration,
) -> Result<std::process::Output> {
    let child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to run `{bin}`"))?;
    tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            anyhow!(
                "`{bin} {}` timed out",
                args.first().map(String::as_str).unwrap_or("")
            )
        })?
        .map_err(Into::into)
}

/// Like [`docker_output`], but a non-zero exit becomes an error carrying docker's stderr.
pub(crate) async fn docker_checked(
    bin: &str,
    args: &[String],
    timeout: Duration,
) -> Result<String> {
    let out = docker_output(bin, args, timeout).await?;
    if !out.status.success() {
        bail!(
            "`docker {}` failed ({}): {}",
            args.first().map(String::as_str).unwrap_or(""),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Read a stream to the end, keeping only the first `cap` bytes but counting all of them.
/// Draining the rest (instead of stopping) keeps a chatty command from blocking on a full pipe.
async fn read_capped<R: AsyncRead + Unpin>(
    mut reader: R,
    cap: usize,
) -> std::io::Result<(Vec<u8>, usize)> {
    let mut kept = Vec::new();
    let mut total = 0;
    let mut chunk = [0u8; 8192];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok((kept, total));
        }
        total += n;
        if kept.len() < cap {
            kept.extend_from_slice(&chunk[..n.min(cap - kept.len())]);
        }
    }
}
fn check_path(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("path must not be empty");
    }
    if path.contains('\0') {
        bail!("path must not contain NUL bytes");
    }
    Ok(())
}

/// A conversation's workspace volumes, the same for either runtime.
struct Workspaces<'a> {
    docker: &'a str,
    cfg: &'a SandboxConfig,
}

impl Workspaces<'_> {
    /// `docker run` would silently create a missing volume, so check first. [`WorkspaceMissing`]
    /// if it doesn't exist; other docker failures are returned as they are.
    async fn require(&self, workspace_id: Uuid) -> Result<()> {
        let args = [
            "volume",
            "inspect",
            "--format",
            "{{.Name}}",
            &volume_name(workspace_id),
        ]
        .map(String::from);
        let out = docker_output(self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("no such volume") || stderr.contains("No such volume") {
            return Err(WorkspaceMissing.into());
        }
        bail!(
            "`docker volume inspect` failed ({}): {}",
            out.status,
            stderr.trim()
        );
    }

    async fn run_in(
        &self,
        workspace_id: Uuid,
        script: &str,
        args: &[&str],
    ) -> Result<std::process::Output> {
        self.require(workspace_id).await?;
        let args = workspace_run_args(self.cfg, workspace_id, script, args);
        docker_output(self.docker, &args, DOCKER_CREATE_TIMEOUT).await
    }

    async fn remove(&self, workspace_id: Uuid) -> Result<()> {
        // A sandbox that was just removed can hold on to the volume for a moment.
        let args = ["volume", "rm", "-f", &volume_name(workspace_id)].map(String::from);
        let mut last = None;
        for attempt in 0..10 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            match docker_checked(self.docker, &args, DOCKER_CALL_TIMEOUT).await {
                Ok(_) => return Ok(()),
                Err(e) if e.to_string().contains("in use") => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.expect("at least one attempt"))
    }

    async fn list(&self) -> Result<Vec<Uuid>> {
        let ls = [
            "volume",
            "ls",
            "-q",
            "--filter",
            &format!("label={WORKSPACE_LABEL}"),
        ]
        .map(String::from);
        let names = docker_checked(self.docker, &ls, DOCKER_CALL_TIMEOUT).await?;
        Ok(names
            .split_whitespace()
            .filter_map(|n| n.strip_prefix(VOLUME_PREFIX)?.parse().ok())
            .collect())
    }

    async fn files(&self, workspace_id: Uuid) -> Result<WorkspaceListing> {
        let out = self.run_in(workspace_id, &list_script(), &[]).await?;
        if !out.status.success() {
            bail!(
                "listing workspace {workspace_id} failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(parse_listing(&out.stdout))
    }

    async fn read(
        &self,
        workspace_id: Uuid,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<FileContent>> {
        check_workspace_path(path)?;
        let limit = (max_bytes + 1).to_string();
        let out = self
            .run_in(workspace_id, READ_WORKSPACE_SCRIPT, &[path, &limit])
            .await?;
        match out.status.code() {
            Some(0) => {
                let mut bytes = out.stdout;
                let truncated = bytes.len() > max_bytes;
                bytes.truncate(max_bytes);
                Ok(Some(FileContent { bytes, truncated }))
            }
            Some(2) => Ok(None),
            _ => bail!(
                "reading {path} from workspace {workspace_id} failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        }
    }
}

/// Make a tool's environment volume, and everything in it, the agent's user's. Recursive, since a
/// root-owned `.local` or `.cache` inside the volume fails `pip install` just like a root-owned
/// volume does.
async fn chown_tool_env(docker: &str, cfg: &SandboxConfig, volume: &str) -> Result<()> {
    let chown = [
        "run",
        "--rm",
        "--network",
        "none",
        "--user",
        "0:0",
        "--cap-drop",
        "ALL",
        "--cap-add",
        "CHOWN",
        "-v",
        &format!("{volume}:{TOOL_ENV_MOUNT}"),
        &cfg.image,
        "chown",
        "-R",
        "1000:1000",
        TOOL_ENV_MOUNT,
    ]
    .map(String::from);
    docker_checked(docker, &chown, DOCKER_CREATE_TIMEOUT)
        .await
        .context("preparing the tool's environment")?;
    Ok(())
}

/// Whether the Docker daemon answers and has the sandbox image.
async fn docker_health(docker: &str, image: &str) -> Result<()> {
    docker_checked(
        docker,
        &["info", "--format", "{{.ServerVersion}}"].map(String::from),
        DOCKER_CALL_TIMEOUT,
    )
    .await
    .context("docker daemon is not reachable")?;
    let inspect = ["image", "inspect", "--format", "{{.Id}}", image].map(String::from);
    docker_checked(docker, &inspect, DOCKER_CALL_TIMEOUT)
        .await
        .with_context(|| {
            format!("sandbox image {image} is missing; build it with `docker build -t {image} sandbox/`")
        })?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Runtimes
// ---------------------------------------------------------------------------------------------

/// What runs a conversation's sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// A locked-down Docker container: the default, and the only runtime on Windows.
    Docker,
    /// An NVIDIA OpenShell sandbox, with a network policy per program and host. Opt-in.
    OpenShell,
}

/// Whether OpenShell can run here at all: it has no Windows support.
pub const OPENSHELL_SUPPORTED: bool = cfg!(not(windows));

/// Where the chosen runtime is saved in the settings table.
const RUNTIME_SETTING: &str = "sandbox_runtime";

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Docker => "docker",
            Runtime::OpenShell => "openshell",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "docker" => Some(Runtime::Docker),
            "openshell" => Some(Runtime::OpenShell),
            _ => None,
        }
    }
}

/// The saved `sandbox_runtime`, or `None` when it is the default (Docker).
pub async fn saved_runtime(store: &Store) -> Result<Option<Runtime>> {
    Ok(store
        .setting(RUNTIME_SETTING)
        .await?
        .and_then(|v| v.as_str().and_then(Runtime::parse)))
}

/// The runtime new sandboxes start on: the saved one where it can run, else Docker.
pub async fn runtime(store: &Store) -> Result<Runtime> {
    Ok(match saved_runtime(store).await? {
        Some(Runtime::OpenShell) if OPENSHELL_SUPPORTED => Runtime::OpenShell,
        _ => Runtime::Docker,
    })
}

/// Whether `runtime` may be chosen on this platform; the error says why not.
pub fn check_platform(runtime: Runtime) -> Result<(), String> {
    if runtime == Runtime::OpenShell && !OPENSHELL_SUPPORTED {
        return Err("OpenShell doesn't run on Windows; sandboxes run on Docker here".into());
    }
    Ok(())
}

/// Save `sandbox_runtime`, or go back to the default (Docker) with `None`. It applies from the
/// next run: a conversation's kept sandbox on the other runtime is replaced then.
pub async fn set_runtime(store: &Store, runtime: Option<Runtime>) -> Result<()> {
    store
        .set_setting(RUNTIME_SETTING, runtime.map(|r| json!(r.as_str())).as_ref())
        .await
}

/// What code in a conversation's sandbox can reach. OpenShell enforces the policy host by host;
/// Docker can only give a container a whole network or none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    Nothing,
    /// OpenShell: these hosts, from the programs the policy names, and nothing else.
    Hosts(Vec<String>),
    /// Docker: this network, and through it whatever it reaches (usually the internet).
    Network(String),
}

impl Reach {
    pub fn of(runtime: Runtime, cfg: &SandboxConfig) -> Self {
        match runtime {
            _ if !cfg.policy.networked() => Reach::Nothing,
            Runtime::OpenShell => Reach::Hosts(cfg.policy.hosts()),
            Runtime::Docker => match cfg.docker_network() {
                "none" => Reach::Nothing,
                network => Reach::Network(network.to_string()),
            },
        }
    }

    /// Whether anything at all can be reached.
    pub fn any(&self) -> bool {
        *self != Reach::Nothing
    }
}

/// What `run.started` says about the sandbox: `{runtime, network, policy, hosts, memory, cpus}`.
/// `network` is `none` when nothing can be reached, else the Docker network or (on OpenShell) the
/// policy's name; `hosts` are the hosts an OpenShell policy reaches, `null` on Docker.
pub fn describe(runtime: Runtime, cfg: &SandboxConfig) -> Value {
    let reach = Reach::of(runtime, cfg);
    let (network, hosts) = match &reach {
        Reach::Nothing => ("none".to_string(), json!([])),
        Reach::Hosts(hosts) => (cfg.policy.name.clone(), json!(hosts)),
        Reach::Network(network) => (network.clone(), Value::Null),
    };
    json!({
        "runtime": runtime.as_str(),
        "network": network,
        "policy": cfg.policy.name,
        "hosts": hosts,
        "memory": cfg.memory,
        // A number when it is one ("1", "0.5"), as Docker takes it.
        "cpus": cfg.cpus.parse::<f64>().map_or_else(|_| json!(cfg.cpus), |n| json!(n)),
    })
}

/// The sandbox factory cedar runs with: Docker, or OpenShell when an admin has turned it on (the
/// `sandbox_runtime` setting, read for every new sandbox). Workspaces and tool environments are
/// the same Docker volumes for both, so either runtime picks up where the other left off.
pub struct Sandboxes {
    docker: DockerSandboxFactory,
    openshell: OpenShellSandboxFactory,
    store: Store,
}

impl Sandboxes {
    pub fn new(cfg: SandboxConfig, store: Store) -> Self {
        Self {
            docker: DockerSandboxFactory::new(cfg.clone()),
            openshell: OpenShellSandboxFactory::new(cfg),
            store,
        }
    }

    fn current(&self, runtime: Runtime) -> &dyn SandboxFactory {
        match runtime {
            Runtime::Docker => &self.docker,
            Runtime::OpenShell => &self.openshell,
        }
    }

    fn workspaces(&self) -> &dyn SandboxFactory {
        &self.docker
    }
}

#[async_trait]
impl SandboxFactory for Sandboxes {
    async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Sandbox>> {
        let runtime = SandboxFactory::runtime(self).await;
        self.current(runtime).create(run_id, workspace_id).await
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        let mut removed = self.docker.cleanup_orphans().await?;
        if OPENSHELL_SUPPORTED {
            match self.openshell.cleanup_orphans().await {
                Ok(n) => removed += n,
                // Without OpenShell (most installs) there is nothing of it to sweep.
                Err(e) if SandboxFactory::runtime(self).await == Runtime::Docker => {
                    tracing::debug!(error = %format!("{e:#}"), "no OpenShell sandboxes to sweep");
                }
                Err(e) => return Err(e),
            }
        }
        Ok(removed)
    }

    async fn remove_workspace(&self, workspace_id: Uuid) -> Result<()> {
        self.workspaces().remove_workspace(workspace_id).await
    }

    async fn list_workspaces(&self) -> Result<Vec<Uuid>> {
        self.workspaces().list_workspaces().await
    }

    async fn list_workspace_files(&self, workspace_id: Uuid) -> Result<WorkspaceListing> {
        self.workspaces().list_workspace_files(workspace_id).await
    }

    async fn read_workspace_file(
        &self,
        workspace_id: Uuid,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<FileContent>> {
        self.workspaces()
            .read_workspace_file(workspace_id, path, max_bytes)
            .await
    }

    async fn health(&self) -> Result<()> {
        let runtime = SandboxFactory::runtime(self).await;
        self.current(runtime).health().await
    }

    async fn runtime(&self) -> Runtime {
        runtime(&self.store).await.unwrap_or_else(|e| {
            tracing::warn!(error = %format!("{e:#}"), "could not read sandbox_runtime; using Docker");
            Runtime::Docker
        })
    }

    async fn check_runtime(&self, runtime: Runtime) -> Result<()> {
        check_platform(runtime).map_err(|e| anyhow!(e))?;
        self.current(runtime).health().await
    }

    async fn setup_tool(&self, name: &str, requirements: &[String]) -> Result<String> {
        let runtime = SandboxFactory::runtime(self).await;
        self.current(runtime).setup_tool(name, requirements).await
    }

    async fn remove_tool_env(&self, name: &str) -> Result<()> {
        // Docker commands only: the environment, and the copies of the tool's folder OpenShell
        // mounted, whichever runtime made them.
        self.openshell.remove_tool_env(name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Policy;

    fn cfg() -> SandboxConfig {
        SandboxConfig {
            openshell_bin: "openshell".into(),
            openshell_workspace: "default".into(),
            docker_bin: "docker".into(),
            image: "img:1".into(),
            policy: Policy::offline(),
            network: "bridge".into(),
            memory: "256m".into(),
            cpus: "2".into(),
            tools_dir: None,
            tools_network: "bridge".into(),
        }
    }

    #[test]
    fn the_runtime_is_docker_unless_openshell_is_chosen_where_it_runs() {
        assert_eq!(Runtime::parse("docker"), Some(Runtime::Docker));
        assert_eq!(Runtime::parse("openshell"), Some(Runtime::OpenShell));
        assert_eq!(Runtime::parse("podman"), None);
        assert!(check_platform(Runtime::Docker).is_ok());
        assert_eq!(
            check_platform(Runtime::OpenShell).is_ok(),
            OPENSHELL_SUPPORTED
        );
    }

    #[test]
    fn what_a_sandbox_reaches_depends_on_the_runtime() {
        let offline = cfg();
        assert_eq!(Reach::of(Runtime::Docker, &offline), Reach::Nothing);
        assert_eq!(Reach::of(Runtime::OpenShell, &offline), Reach::Nothing);
        let packages = SandboxConfig {
            policy: Policy::packages(),
            ..cfg()
        };
        // OpenShell reaches the policy's hosts; Docker can only give the whole network.
        assert!(
            matches!(Reach::of(Runtime::OpenShell, &packages), Reach::Hosts(h) if h.contains(&"pypi.org".to_string()))
        );
        assert_eq!(
            Reach::of(Runtime::Docker, &packages),
            Reach::Network("bridge".into())
        );
        let none = SandboxConfig {
            network: "none".into(),
            ..packages.clone()
        };
        assert_eq!(Reach::of(Runtime::Docker, &none), Reach::Nothing);

        let info = describe(Runtime::Docker, &packages);
        assert_eq!(info["runtime"], "docker");
        assert_eq!(info["network"], "bridge");
        assert_eq!(info["hosts"], Value::Null);
        assert_eq!(info["cpus"], 2.0);
        let info = describe(Runtime::OpenShell, &packages);
        assert_eq!(info["runtime"], "openshell");
        assert_eq!(info["network"], "packages");
        assert!(info["hosts"]
            .as_array()
            .unwrap()
            .contains(&json!("pypi.org")));
        assert_eq!(describe(Runtime::OpenShell, &offline)["network"], "none");
    }

    #[test]
    fn a_tool_folder_mounts_from_a_windows_path_or_one_with_a_comma() {
        // `-v C:\tools\t:/from:ro` would split at the drive letter's colon.
        assert_eq!(
            bind_mount_ro(std::path::Path::new(r"C:\Users\me\tools\t"), TOOL_MOUNT),
            r"type=bind,src=C:\Users\me\tools\t,dst=/opt/cedar/tool,readonly"
        );
        assert_eq!(
            bind_mount_ro(std::path::Path::new(r#"/srv/a,b "c""#), TOOL_MOUNT),
            r#"type=bind,"src=/srv/a,b ""c""",dst=/opt/cedar/tool,readonly"#
        );
    }

    #[tokio::test]
    async fn read_capped_counts_everything_but_keeps_only_the_cap() {
        let data = vec![b'x'; 20_000];
        let (kept, total) = read_capped(&data[..], 100).await.unwrap();
        assert_eq!((kept.len(), total), (100, 20_000));
        let (kept, total) = read_capped(&b"abc"[..], 100).await.unwrap();
        assert_eq!((kept.as_slice(), total), (&b"abc"[..], 3));
    }

    #[test]
    fn paths_are_validated() {
        assert!(check_path("a/b.txt").is_ok());
        assert!(check_path("").is_err());
        assert!(check_path("a\0b").is_err());
    }

    #[test]
    fn workspace_paths_must_stay_inside_the_workspace() {
        for ok in [
            "a.pdf",
            "deep/nested/file.txt",
            "it's a $(x) file",
            ".hidden",
        ] {
            assert!(check_workspace_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "a//b",
            "a/",
            "a\0b",
        ] {
            assert!(check_workspace_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn workspace_run_args_mount_read_only_and_lock_down() {
        let ws = Uuid::nil();
        let args = workspace_run_args(&cfg(), ws, "echo \"$1\"", &["x y", "2"]);
        let joined = args.join(" ");
        assert!(joined.starts_with("run --rm --network none"));
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--user 1000:1000"));
        assert!(joined.contains(&format!("-v cedar-ws-{ws}:/workspace:ro")));
        // Image, then the script as one argv element, then `$0` and the positional arguments.
        assert_eq!(
            &args[args.len() - 7..],
            ["img:1", "sh", "-c", "echo \"$1\"", "sh", "x y", "2"]
        );
    }

    #[test]
    fn list_script_prunes_noise_and_caps_the_listing() {
        let script = list_script();
        assert!(script.contains("-name '.git'"));
        assert!(script.contains("-name 'node_modules'"));
        assert!(script.contains("-type f"));
        assert!(script.ends_with(&format!("head -z -n {}", MAX_LISTED_FILES + 1)));
    }

    #[test]
    fn listings_are_parsed_sorted_and_truncated() {
        let raw = b"5828\t1700000000.5\tout/rust-intro.pdf\0\
                    12\t1700000001\ta.txt\0\
                    garbage\0\
                    3\t1700000002\tdir with space/b c.md\0";
        let listing = parse_listing(raw);
        assert!(!listing.truncated);
        let paths: Vec<_> = listing.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["a.txt", "dir with space/b c.md", "out/rust-intro.pdf"]
        );
        assert_eq!(listing.files[2].size, 5828);
        assert_eq!(
            listing.files[2].modified_at.timestamp_millis(),
            1_700_000_000_500
        );

        let many: Vec<u8> = (0..=MAX_LISTED_FILES)
            .flat_map(|i| format!("1\t1\tf{i:05}\0").into_bytes())
            .collect();
        let listing = parse_listing(&many);
        assert!(listing.truncated);
        assert_eq!(listing.files.len(), MAX_LISTED_FILES);
    }
}
