//! Isolated execution environments for a run's tools.
//!
//! Every sandbox is an [OpenShell](https://github.com/NVIDIA/OpenShell) sandbox, driven through
//! the `openshell` command (see [`crate::openshell`]): a container on the gateway's Docker driver
//! that runs as a non-root user without capabilities, with Landlock on its files and every
//! connection it opens checked against its policy (see [`crate::policy`]). A conversation's files
//! live in a Docker volume that OpenShell mounts at `/workspace`, so they outlive the sandbox and
//! personal tool calls (each a sandbox of its own) can work on them too.
//!
//! Every file and command operation goes through the sandbox, never the host filesystem. Reading
//! host-side from the volume would let the agent point a symlink outside its workspace; the files
//! API reads a finished conversation's volume through a locked-down, read-only container.

use crate::config::SandboxConfig;
use crate::openshell::{self, Denial, OpenShell, PolicyFile};
use crate::policy::{self, Endpoint, WORKSPACE};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use uuid::Uuid;

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
        bail!("personal tools need an OpenShell sandbox")
    }
    /// The connections and requests the sandbox's policy refused since `since`, oldest first.
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
    /// Install a personal tool's `pip` requirements into its own environment (see
    /// [`tool_env_volume`]), in a sandbox that can reach PyPI and nothing else. Returns pip's
    /// output.
    async fn setup_tool(&self, _name: &str, _requirements: &[String]) -> Result<String> {
        bail!("installing a tool's packages needs an OpenShell sandbox")
    }
    /// Delete a personal tool's environment. Succeeds if it doesn't exist.
    async fn remove_tool_env(&self, _name: &str) -> Result<()> {
        Ok(())
    }
}

/// One call of a personal tool. It runs in a fresh sandbox that sees the run's workspace, a copy
/// of the tool's own folder (read-only, at [`TOOL_MOUNT`]) and the tool's own environment (at
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
    /// The hosts the manifest declares; the call's sandbox can reach these (from Python) and
    /// nothing else.
    pub hosts: &'a [String],
}

/// Where a personal tool's folder is mounted in its sandbox.
pub const TOOL_MOUNT: &str = "/opt/cedar/tool";
/// Where a personal tool's environment (its home, and its `pip install --user` packages) is.
pub const TOOL_ENV_MOUNT: &str = "/opt/cedar/env";
const TOOL_LABEL: &str = "cedar.tool";
const TOOL_ENV_PREFIX: &str = "cedar-tool-env-";
const TOOL_CODE_PREFIX: &str = "cedar-tool-code-";
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
// Names, labels, and the Docker volumes behind workspaces
// ---------------------------------------------------------------------------------------------

/// What kind of sandbox cedar made (`run`, `tool`, `setup`), so a restart can sweep them.
const KIND_LABEL: &str = "cedar.kind";
const RUN_LABEL: &str = "cedar.run";
const WORKSPACE_LABEL: &str = "cedar.workspace";
const VOLUME_PREFIX: &str = "cedar-ws-";
/// The labels OpenShell's Docker driver wants on a volume before it attaches it to a sandbox.
const ATTACHABLE: &str = "openshell.ai/sandbox-attachable";
const ATTACHABLE_WORKSPACE: &str = "openshell.ai/sandbox-attachable-workspace";
pub(crate) const DOCKER_CALL_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DOCKER_CREATE_TIMEOUT: Duration = Duration::from_secs(120);
/// Extra time beyond a command's own limit before the host gives up on it: the in-sandbox wrapper
/// needs a moment to stop it, and the `openshell` call itself takes one.
const EXEC_BACKSTOP: Duration = Duration::from_secs(15);
/// Room for the wrapper's last line beyond the output it keeps.
const TRAILER_ROOM: usize = 256;
/// The most a single write sends through stdin: OpenShell takes at most 4 MiB per call.
const WRITE_CHUNK: usize = 3 * 1024 * 1024;
/// How long to wait for a sandbox to come (back) up.
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// Runs commands for cedar inside a sandbox: see `sandbox/exec_wrapper.py`.
const EXEC_WRAPPER: &str = include_str!("../sandbox/exec_wrapper.py");
/// The wrapper's last line on stderr: `\x1ecedar-exit:<code>:<stdout bytes>:<stderr bytes>`.
const TRAILER: &[u8] = b"\x1ecedar-exit:";

/// The name of `run_id`'s sandbox. OpenShell takes at most 19 characters, so a name is a kind
/// (`cr` for a run, `ct` for a tool call, `cs` for a tool's setup) and the last 16 hex digits of
/// an id: its random part for the version 4 and 7 ids cedar makes. The full ids are in labels.
pub fn run_sandbox_name(run_id: Uuid) -> String {
    short_name("cr", run_id)
}

fn short_name(kind: &str, id: Uuid) -> String {
    let hex = id.simple().to_string();
    format!("{kind}-{}", &hex[hex.len() - 16..])
}

fn volume_name(workspace_id: Uuid) -> String {
    format!("{VOLUME_PREFIX}{workspace_id}")
}

/// The environment every sandbox gets: OpenShell passes the image's `PATH` but not the rest of
/// its `ENV`, so what the sandbox image sets for pip and matplotlib is set again here.
fn workspace_env() -> Vec<(String, String)> {
    [
        ("PYTHONUSERBASE", "/workspace/.local"),
        ("PATH", "/workspace/.local/bin:/usr/local/bin:/usr/bin:/bin"),
        ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
        ("MPLBACKEND", "Agg"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// The environment of a personal tool's sandbox: its own home and package directory.
fn tool_env() -> Vec<(String, String)> {
    [
        ("HOME", TOOL_ENV_MOUNT.to_string()),
        ("PYTHONUSERBASE", format!("{TOOL_ENV_MOUNT}/.local")),
        (
            "PATH",
            format!("{TOOL_ENV_MOUNT}/.local/bin:/usr/local/bin:/usr/bin:/bin"),
        ),
        ("PIP_DISABLE_PIP_VERSION_CHECK", "1".into()),
        ("MPLBACKEND", "Agg".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// A volume mount in OpenShell's Docker driver config.
fn mount(volume: &str, target: &str, read_only: bool) -> Value {
    json!({ "type": "volume", "source": volume, "target": target, "read_only": read_only })
}

/// Arguments for `openshell sandbox create`: a detached sandbox of `cfg.image` that idles until
/// cedar runs commands in it, restarted if something in it ends that idle process.
#[allow(clippy::too_many_arguments)]
pub fn create_args(
    cfg: &SandboxConfig,
    name: &str,
    labels: &[(&str, String)],
    policy_file: &str,
    mounts: &[Value],
    env: &[(String, String)],
    env_from: &[String],
    memory: &str,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "sandbox",
        "create",
        "--name",
        name,
        "--from",
        &cfg.image,
        "--cpu",
        &cfg.cpus,
        "--memory",
        memory,
        "--policy",
        policy_file,
        "--no-auto-providers",
        "--no-credential-warnings",
        "--no-tty",
        "--restart-policy",
        "always",
        "--detach",
        "-o",
        "json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (k, v) in labels {
        args.extend(["--label".into(), format!("{k}={v}")]);
    }
    for (k, v) in env {
        args.extend(["--env".into(), format!("{k}={v}")]);
    }
    // Secrets go by name; the `openshell` process has the values in its own environment.
    for k in env_from {
        args.extend(["--env-from".into(), k.clone()]);
    }
    if !mounts.is_empty() {
        args.extend([
            "--driver-config-json".into(),
            json!({ "docker": { "mounts": mounts } }).to_string(),
        ]);
    }
    args.extend(["--".into(), "sleep".into(), "infinity".into()]);
    args
}

/// Arguments for `openshell sandbox exec` of `argv` in `/workspace`, with OpenShell's own limit
/// a little past cedar's (the wrapper enforces cedar's).
pub fn exec_args(name: &str, limit: Duration, argv: &[String]) -> Vec<String> {
    let mut args: Vec<String> = [
        "sandbox",
        "exec",
        "-n",
        name,
        "--no-tty",
        "--no-login-shell",
        "--workdir",
        WORKSPACE,
        "--timeout",
        &limit.as_secs().max(1).to_string(),
        "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(argv.iter().cloned());
    args
}

/// The wrapper's command line: `sh -c command` (`mode` `sh`) or a program and its arguments
/// (`run`), stopped after `timeout`, with `max_output` bytes of each stream kept.
fn wrapped(timeout: Duration, max_output: usize, mode: &str, rest: &[&str]) -> Vec<String> {
    let mut argv: Vec<String> = [
        "python3",
        "-I",
        "-S",
        "-c",
        EXEC_WRAPPER,
        &timeout.as_secs().max(1).to_string(),
        &max_output.to_string(),
        mode,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    argv.extend(rest.iter().map(|s| s.to_string()));
    argv
}

/// Split the wrapper's report off its stderr: the exit code (`None` for a signal) and the real
/// sizes of both streams. `None` when there is no report, i.e. the command never ran.
fn take_trailer(stderr: &mut Vec<u8>) -> Option<(Option<i32>, usize, usize)> {
    let at = stderr.windows(TRAILER.len()).rposition(|w| w == TRAILER)?;
    let line = String::from_utf8_lossy(&stderr[at + TRAILER.len()..]).into_owned();
    let mut parts = line.trim().split(':');
    let code = match parts.next()? {
        "signal" => None,
        n => Some(n.parse().ok()?),
    };
    let out_total = parts.next()?.parse().ok()?;
    let err_total = parts.next()?.parse().ok()?;
    // The wrapper puts a line break before its report.
    let mut cut = at;
    if cut > 0 && stderr[cut - 1] == b'\n' {
        cut -= 1;
    }
    stderr.truncate(cut);
    Some((code, out_total, err_total))
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
    out.extend(locked_down(cfg, "64"));
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

/// The restrictions cedar's own helper containers (files API, volume copies) run under.
fn locked_down(cfg: &SandboxConfig, pids: &str) -> Vec<String> {
    [
        "--network",
        "none",
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

/// Labels on a Docker volume, or `None` if there is no such volume.
async fn volume_labels(docker: &str, volume: &str) -> Result<Option<Value>> {
    let args = ["volume", "inspect", "--format", "{{json .Labels}}", volume].map(String::from);
    let out = docker_output(docker, &args, DOCKER_CALL_TIMEOUT).await?;
    if out.status.success() {
        let labels: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| Value::Object(Default::default()));
        return Ok(Some(labels));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("no such volume") || stderr.contains("No such volume") {
        return Ok(None);
    }
    bail!(
        "`docker volume inspect` failed ({}): {}",
        out.status,
        stderr.trim()
    )
}

/// Create a volume OpenShell will attach (in workspace `ows`), with `label` as cedar's own label.
async fn create_volume(docker: &str, volume: &str, label: &str, ows: &str) -> Result<()> {
    let args = [
        "volume",
        "create",
        "--label",
        label,
        "--label",
        &format!("{ATTACHABLE}=true"),
        "--label",
        &format!("{ATTACHABLE_WORKSPACE}={ows}"),
        volume,
    ]
    .map(String::from);
    docker_checked(docker, &args, DOCKER_CALL_TIMEOUT).await?;
    Ok(())
}

/// Make sure `volume` exists with the labels OpenShell needs to attach it. Returns whether it was
/// created just now.
///
/// Docker can't add labels to a volume, so one made before cedar ran on OpenShell (a workspace or
/// a tool's environment) is moved into a labelled one of the same name: copied aside, recreated,
/// and copied back. Nothing is removed before its copy is made.
async fn ensure_volume(
    docker: &str,
    image: &str,
    volume: &str,
    label: &str,
    ows: &str,
) -> Result<bool> {
    let Some(labels) = volume_labels(docker, volume).await? else {
        create_volume(docker, volume, label, ows).await?;
        return Ok(true);
    };
    if labels[ATTACHABLE] == "true" && labels[ATTACHABLE_WORKSPACE] == ows {
        return Ok(false);
    }
    tracing::info!(%volume, "relabelling a volume so OpenShell can attach it");
    let aside = format!("{volume}-relabel");
    create_volume(docker, &aside, label, ows).await?;
    copy_volume(docker, image, volume, &aside).await?;
    docker_checked(
        docker,
        &["volume", "rm", volume].map(String::from),
        DOCKER_CALL_TIMEOUT,
    )
    .await
    .context("the volume is still in use; it is relabelled once nothing mounts it")?;
    create_volume(docker, volume, label, ows).await?;
    copy_volume(docker, image, &aside, volume).await?;
    docker_checked(
        docker,
        &["volume", "rm", "-f", &aside].map(String::from),
        DOCKER_CALL_TIMEOUT,
    )
    .await?;
    Ok(false)
}

/// Copy one volume's files into another, keeping owners and modes. Root is needed to keep the
/// owners; nothing but `cp` runs, offline and without other capabilities.
async fn copy_volume(docker: &str, image: &str, from: &str, to: &str) -> Result<()> {
    let args = [
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
        "--cap-add",
        "FOWNER",
        "--cap-add",
        "DAC_OVERRIDE",
        "-v",
        &format!("{from}:/from:ro"),
        "-v",
        &format!("{to}:/to"),
        image,
        "cp",
        "-a",
        "/from/.",
        "/to/",
    ]
    .map(String::from);
    docker_checked(docker, &args, DOCKER_CREATE_TIMEOUT)
        .await
        .with_context(|| format!("copying volume {from} to {to}"))?;
    Ok(())
}

/// Seconds since the epoch, as OpenShell's log stamps them.
fn epoch_seconds(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------------------------
// OpenShell sandboxes
// ---------------------------------------------------------------------------------------------

/// What every sandbox cedar makes needs to reach OpenShell and Docker.
#[derive(Clone)]
struct Shared {
    shell: OpenShell,
    docker: String,
    cfg: SandboxConfig,
    /// `SANDBOX_MEMORY` as OpenShell takes it.
    memory: String,
}

impl Shared {
    fn new(cfg: SandboxConfig) -> Self {
        // Checked when the configuration was read.
        let memory = openshell::memory_quantity(&cfg.memory).unwrap_or_else(|_| "512Mi".into());
        Self {
            shell: OpenShell::new(cfg.openshell_bin.clone(), cfg.openshell_workspace.clone()),
            docker: cfg.docker_bin.clone(),
            cfg,
            memory,
        }
    }

    /// Run `argv` in sandbox `name` through the wrapper, with `stdin` (or nothing) as its input,
    /// keeping `max_output` bytes of each stream. A command that couldn't be run at all (the
    /// sandbox isn't there or isn't ready) is an `Err`; one that ran is an [`ExecOutput`].
    async fn run(
        &self,
        name: &str,
        argv: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
        max_output: usize,
        secrets: &[(String, String)],
    ) -> Result<std::result::Result<ExecOutput, String>> {
        let args = exec_args(name, timeout + EXEC_BACKSTOP, argv);
        let mut cmd = self.shell.command();
        cmd.args(&args);
        for (k, v) in secrets {
            cmd.env(k, v);
        }
        if stdin.is_some() {
            cmd.stdin(Stdio::piped());
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to run `{}`", self.shell.bin))?;
        let input = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let data = stdin.unwrap_or_default();
        let write = async move {
            if let Some(mut pipe) = input {
                // A command that never reads its input closes the pipe early; that is not an error.
                let _ = pipe.write_all(data).await;
                drop(pipe);
            }
        };
        let finished = tokio::time::timeout(timeout + EXEC_BACKSTOP * 2, async {
            tokio::join!(
                write,
                read_capped(stdout, max_output + TRAILER_ROOM),
                read_capped(stderr, max_output + TRAILER_ROOM),
                child.wait()
            )
        })
        .await;
        let Ok(((), out, err, status)) = finished else {
            drop(child);
            return Err(
                SandboxFatal("a command did not terminate after its timeout".into()).into(),
            );
        };
        let (mut stdout, _) = out?;
        let (mut stderr, _) = err?;
        let status = status?;
        match take_trailer(&mut stderr) {
            Some((exit_code, stdout_total, stderr_total)) => {
                stdout.truncate(max_output);
                stderr.truncate(max_output);
                Ok(Ok(ExecOutput {
                    stdout,
                    stderr,
                    stdout_total,
                    stderr_total,
                    exit_code,
                }))
            }
            // The wrapper never reported: `openshell` couldn't run it.
            None => Ok(Err(format!(
                "{} ({status})",
                String::from_utf8_lossy(&stderr).trim()
            ))),
        }
    }

    /// Wait until sandbox `name` is ready; `Err` if it is gone or doesn't come back.
    async fn wait_ready(&self, name: &str) -> Result<()> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            match self.shell.sandbox(name).await? {
                None => bail!("sandbox {name} is gone"),
                Some(s) if openshell::phase(&s) == "Ready" => return Ok(()),
                Some(s) if tokio::time::Instant::now() >= deadline => {
                    bail!("sandbox {name} is {}", openshell::phase(&s))
                }
                Some(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
    }

    /// Delete sandbox `name` and wait (a little) until it is gone, so the volumes it mounted can
    /// be removed.
    async fn delete(&self, name: &str) {
        if let Err(e) = self.shell.delete(&[name.to_string()]).await {
            tracing::warn!(error = %format!("{e:#}"), sandbox = %name, "removing sandbox failed");
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while tokio::time::Instant::now() < deadline {
            match self.shell.sandbox(name).await {
                Ok(None) => return,
                Ok(Some(_)) => tokio::time::sleep(Duration::from_millis(200)).await,
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), sandbox = %name, "checking a removed sandbox failed");
                    return;
                }
            }
        }
    }

    /// Create a sandbox with `policy` and wait for it; on failure nothing is left behind.
    #[allow(clippy::too_many_arguments)]
    async fn create(
        &self,
        name: &str,
        labels: &[(&str, String)],
        policy: &Value,
        mounts: &[Value],
        env: &[(String, String)],
        secrets: &[(String, String)],
    ) -> Result<()> {
        let file = PolicyFile::write(policy)?;
        let env_from: Vec<String> = secrets.iter().map(|(k, _)| k.clone()).collect();
        let args = create_args(
            &self.cfg,
            name,
            labels,
            &file.path_string(),
            mounts,
            env,
            &env_from,
            &self.memory,
        );
        let mut cmd = self.shell.command();
        cmd.args(&args);
        for (k, v) in secrets {
            cmd.env(k, v);
        }
        let child = cmd
            .spawn()
            .with_context(|| format!("failed to run `{}`", self.shell.bin))?;
        let out = tokio::time::timeout(openshell::CREATE_TIMEOUT, child.wait_with_output()).await;
        let failure = match out {
            Ok(Ok(out)) if out.status.success() => return Ok(()),
            Ok(Ok(out)) => anyhow!(
                "`openshell sandbox create` failed ({}): {}",
                out.status,
                openshell::error_text(&out)
            ),
            Ok(Err(e)) => e.into(),
            Err(_) => anyhow!("`openshell sandbox create` timed out"),
        };
        self.delete(name).await;
        Err(failure)
    }

    /// The volume with a copy of a tool's folder, made if this version of the folder has none.
    /// The folder is bind-mounted into a plain container only to copy it, read-only and offline;
    /// the tool's sandbox gets the copy, read-only.
    async fn tool_code_volume(&self, name: &str, dir: &std::path::Path) -> Result<String> {
        let digest = folder_digest(dir)?;
        let volume = format!("{TOOL_CODE_PREFIX}{name}-{digest}");
        let label = format!("{TOOL_LABEL}={name}");
        if volume_labels(&self.docker, &volume).await?.is_some() {
            return Ok(volume);
        }
        create_volume(&self.docker, &volume, &label, &self.cfg.openshell_workspace).await?;
        let args = [
            "run",
            "--rm",
            "--network",
            "none",
            "--user",
            "0:0",
            "--cap-drop",
            "ALL",
            "--cap-add",
            "DAC_OVERRIDE",
            "--cap-add",
            "FOWNER",
            "--mount",
            &bind_mount_ro(dir, "/from"),
            "-v",
            &format!("{volume}:/to"),
            &self.cfg.image,
            "sh",
            "-c",
            "cp -R /from/. /to/ && chmod -R a+rX,go-w /to",
        ]
        .map(String::from);
        if let Err(e) = docker_checked(&self.docker, &args, DOCKER_CREATE_TIMEOUT).await {
            let rm = ["volume", "rm", "-f", &volume].map(String::from);
            let _ = docker_checked(&self.docker, &rm, DOCKER_CALL_TIMEOUT).await;
            return Err(e.context(format!("copying the folder of {name}")));
        }
        // Older copies of the folder are no longer used.
        let ls = ["volume", "ls", "-q", "--filter", &format!("label={label}")].map(String::from);
        if let Ok(names) = docker_checked(&self.docker, &ls, DOCKER_CALL_TIMEOUT).await {
            for old in names.split_whitespace() {
                if old.starts_with(&format!("{TOOL_CODE_PREFIX}{name}-")) && old != volume {
                    let rm = ["volume", "rm", old].map(String::from);
                    let _ = docker_checked(&self.docker, &rm, DOCKER_CALL_TIMEOUT).await;
                }
            }
        }
        Ok(volume)
    }

    /// The tool's environment volume, created (and handed to the sandbox user) if it is new.
    /// With `fix_owner`, an existing one is handed over too: one left root's would fail every
    /// `pip install` with "Permission denied".
    async fn tool_env(&self, name: &str, fix_owner: bool) -> Result<String> {
        let volume = tool_env_volume(name);
        let created = ensure_volume(
            &self.docker,
            &self.cfg.image,
            &volume,
            &format!("{TOOL_LABEL}={name}"),
            &self.cfg.openshell_workspace,
        )
        .await?;
        if created || fix_owner {
            chown_tool_env(&self.docker, &self.cfg, &volume).await?;
        }
        Ok(volume)
    }
}

/// A short digest of a folder's file names, sizes and modification times: a new one means the
/// tool changed and its sandbox needs a fresh copy.
fn folder_digest(dir: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .collect::<std::io::Result<_>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let meta = entry.metadata()?;
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .display()
                .to_string();
            if meta.is_dir() {
                walk(&path, base, out)?;
            } else {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                out.push(format!("{rel}\0{}\0{modified}", meta.len()));
            }
        }
        Ok(())
    }
    let mut lines = Vec::new();
    walk(dir, dir, &mut lines)?;
    let digest = Sha256::digest(lines.join("\n").as_bytes());
    Ok(hex::encode(digest)[..12].to_string())
}

pub struct OpenShellSandbox {
    ctx: Shared,
    name: String,
    run_id: Uuid,
    workspace_id: Uuid,
    /// The policy it runs under now: the base policy plus what has been allowed since.
    policy: tokio::sync::Mutex<Value>,
    /// What has been allowed, so allowing it again changes nothing.
    allowed: std::sync::Mutex<BTreeSet<Endpoint>>,
}

impl OpenShellSandbox {
    /// Run `argv` (already wrapped), making sure the sandbox is up: if it isn't (stopped, or
    /// restarting because something ended its idle process), wait for it once and try again.
    async fn run_here(
        &self,
        argv: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
        max_output: usize,
        secrets: &[(String, String)],
    ) -> Result<ExecOutput> {
        let first = self
            .ctx
            .run(&self.name, argv, stdin, timeout, max_output, secrets)
            .await;
        let why = match first {
            Ok(Ok(out)) => return Ok(out),
            Ok(Err(why)) => why,
            Err(e) => {
                if e.is::<SandboxFatal>() {
                    self.destroy().await;
                }
                return Err(e);
            }
        };
        if let Err(e) = self.ctx.wait_ready(&self.name).await {
            return Err(SandboxFatal(format!("{e:#} ({why})")).into());
        }
        match self
            .ctx
            .run(&self.name, argv, stdin, timeout, max_output, secrets)
            .await?
        {
            Ok(out) => Ok(out),
            Err(why) => Err(SandboxFatal(why).into()),
        }
    }
}

#[async_trait]
impl Sandbox for OpenShellSandbox {
    async fn exec(
        &self,
        command: &str,
        timeout: Duration,
        max_output: usize,
    ) -> Result<ExecOutput> {
        let argv = wrapped(timeout, max_output, "sh", &[command]);
        self.run_here(&argv, None, timeout, max_output, &[]).await
    }

    async fn read_file(&self, path: &str, max_bytes: usize) -> Result<FileContent> {
        check_path(path)?;
        let limit = (max_bytes + 1).to_string();
        let argv = wrapped(
            Duration::from_secs(30),
            max_bytes + 1,
            "run",
            &["head", "-c", &limit, "--", path],
        );
        let out = self
            .run_here(&argv, None, Duration::from_secs(30), max_bytes + 1, &[])
            .await?;
        if out.exit_code != Some(0) {
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let mut bytes = out.stdout;
        let truncated = bytes.len() > max_bytes;
        bytes.truncate(max_bytes);
        Ok(FileContent { bytes, truncated })
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        check_path(path)?;
        // OpenShell takes at most 4 MiB of input per command, so a larger file goes in parts,
        // into a temporary file that replaces `path` once it is whole.
        const FIRST: &str = r#"mkdir -p -- "$(dirname -- "$1")" && cat > "$1""#;
        const MORE: &str = r#"cat >> "$1""#;
        const DONE: &str = r#"mv -f -- "$1.cedar-part" "$1""#;
        let chunks: Vec<&[u8]> = if content.is_empty() {
            vec![&[][..]]
        } else {
            content.chunks(WRITE_CHUNK).collect()
        };
        let whole = chunks.len() == 1;
        let target = if whole {
            path.to_string()
        } else {
            format!("{path}.cedar-part")
        };
        let timeout = Duration::from_secs(60);
        for (i, chunk) in chunks.iter().enumerate() {
            let script = if i == 0 { FIRST } else { MORE };
            let argv = wrapped(timeout, 4096, "run", &["sh", "-c", script, "sh", &target]);
            let out = self
                .run_here(&argv, Some(chunk), timeout, 4096, &[])
                .await?;
            if out.exit_code != Some(0) {
                bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
            }
        }
        if !whole {
            let argv = wrapped(timeout, 4096, "run", &["sh", "-c", DONE, "sh", path]);
            let out = self.run_here(&argv, None, timeout, 4096, &[]).await?;
            if out.exit_code != Some(0) {
                bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
            }
        }
        Ok(())
    }

    async fn run_tool(
        &self,
        call: &ToolRun<'_>,
        timeout: Duration,
        max_output: usize,
    ) -> Result<ExecOutput> {
        let env_volume = self.ctx.tool_env(call.name, false).await?;
        let code_volume = self.ctx.tool_code_volume(call.name, call.dir).await?;
        let name = short_name("ct", Uuid::new_v4());
        let labels = [
            (KIND_LABEL, "tool".to_string()),
            (RUN_LABEL, self.run_id.to_string()),
            (TOOL_LABEL, call.name.to_string()),
        ];
        let policy = policy::for_tool(TOOL_MOUNT, TOOL_ENV_MOUNT, call.hosts);
        let mounts = [
            mount(&volume_name(self.workspace_id), WORKSPACE, false),
            mount(&env_volume, TOOL_ENV_MOUNT, false),
            mount(&code_volume, TOOL_MOUNT, true),
        ];
        self.ctx
            .create(&name, &labels, &policy, &mounts, &tool_env(), call.env)
            .await
            .with_context(|| format!("starting the sandbox for {}", call.name))?;
        let script = format!("{TOOL_MOUNT}/{}", call.entry);
        let argv = wrapped(timeout, max_output, "run", &["python3", "--", &script]);
        let out = self
            .ctx
            .run(&name, &argv, Some(call.input), timeout, max_output, &[])
            .await;
        // Not waited for: nothing else uses its volumes in a way that would mind.
        if let Err(e) = self.ctx.shell.delete(std::slice::from_ref(&name)).await {
            tracing::warn!(error = %format!("{e:#}"), sandbox = %name, "removing a tool's sandbox failed");
        }
        match out? {
            Ok(out) => Ok(out),
            Err(why) => bail!("{} could not be run: {why}", call.name),
        }
    }

    async fn denials(&self, since: SystemTime) -> Result<Vec<Denial>> {
        let ago = SystemTime::now().duration_since(since).unwrap_or_default();
        let log = self.ctx.shell.sandbox_log(&self.name, ago, 500).await?;
        let from = epoch_seconds(since) - 1.0;
        Ok(openshell::parse_denials(&log)
            .into_iter()
            .filter(|d| d.at == 0.0 || d.at >= from)
            .collect())
    }

    async fn allow(&self, endpoints: &[Endpoint]) -> Result<()> {
        let new: Vec<&Endpoint> = {
            let allowed = self.allowed.lock().unwrap();
            endpoints.iter().filter(|e| !allowed.contains(*e)).collect()
        };
        if new.is_empty() {
            return Ok(());
        }
        let mut policy = self.policy.lock().await;
        let mut next = policy.clone();
        for endpoint in &new {
            endpoint.check()?;
            policy::add_endpoint(&mut next, endpoint);
        }
        self.ctx.shell.set_policy(&self.name, &next).await?;
        *policy = next;
        self.allowed
            .lock()
            .unwrap()
            .extend(new.into_iter().cloned());
        Ok(())
    }

    async fn pause(&self) {
        // OpenShell's Docker driver can't freeze a sandbox, so it is stopped: its processes end,
        // its files (the workspace, and even /tmp) stay, and it starts again in a couple of
        // seconds.
        let args = openshell::strings(&["sandbox", "stop", &self.name]);
        if let Err(e) = self.ctx.shell.checked(&args, openshell::CALL_TIMEOUT).await {
            tracing::warn!(error = %format!("{e:#}"), sandbox = %self.name, "stopping an idle sandbox failed");
        }
    }

    async fn resume(&self) -> Result<()> {
        let sandbox = self
            .ctx
            .shell
            .sandbox(&self.name)
            .await?
            .ok_or_else(|| anyhow!("sandbox {} is gone", self.name))?;
        match openshell::phase(&sandbox) {
            "Ready" | "Provisioning" | "Starting" => {}
            "Stopped" | "Stopping" | "Completed" => {
                let args = openshell::strings(&["sandbox", "start", &self.name]);
                self.ctx
                    .shell
                    .checked(&args, openshell::CREATE_TIMEOUT)
                    .await?;
            }
            other => bail!("sandbox {} is {other}", self.name),
        }
        self.ctx.wait_ready(&self.name).await
    }

    async fn destroy(&self) {
        // Only the sandbox: the workspace volume outlives the run so it can be continued.
        self.ctx.delete(&self.name).await;
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

pub struct OpenShellSandboxFactory {
    ctx: Shared,
}

impl OpenShellSandboxFactory {
    pub fn new(cfg: SandboxConfig) -> Self {
        Self {
            ctx: Shared::new(cfg),
        }
    }

    /// [`WorkspaceMissing`] if the workspace's volume doesn't exist.
    async fn require_workspace(&self, workspace_id: Uuid) -> Result<()> {
        match volume_labels(&self.ctx.docker, &volume_name(workspace_id)).await? {
            Some(_) => Ok(()),
            None => Err(WorkspaceMissing.into()),
        }
    }

    async fn run_in_workspace(
        &self,
        workspace_id: Uuid,
        script: &str,
        args: &[&str],
    ) -> Result<std::process::Output> {
        self.require_workspace(workspace_id).await?;
        let args = workspace_run_args(&self.ctx.cfg, workspace_id, script, args);
        docker_output(&self.ctx.docker, &args, DOCKER_CREATE_TIMEOUT).await
    }
}

#[async_trait]
impl SandboxFactory for OpenShellSandboxFactory {
    async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Sandbox>> {
        let ctx = &self.ctx;
        let volume = volume_name(workspace_id);
        ensure_volume(
            &ctx.docker,
            &ctx.cfg.image,
            &volume,
            &format!("{WORKSPACE_LABEL}={workspace_id}"),
            &ctx.cfg.openshell_workspace,
        )
        .await
        .context("preparing the workspace volume")?;
        let policy = ctx.cfg.policy.for_workspace(&[]);
        let name = run_sandbox_name(run_id);
        let labels = [
            (KIND_LABEL, "run".to_string()),
            (RUN_LABEL, run_id.to_string()),
            (WORKSPACE_LABEL, workspace_id.to_string()),
        ];
        ctx.create(
            &name,
            &labels,
            &policy,
            &[mount(&volume, WORKSPACE, false)],
            &workspace_env(),
            &[],
        )
        .await
        .context("creating the sandbox")?;
        Ok(Arc::new(OpenShellSandbox {
            ctx: ctx.clone(),
            name,
            run_id,
            workspace_id,
            policy: tokio::sync::Mutex::new(policy),
            allowed: Default::default(),
        }))
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        let mut names = Vec::new();
        for kind in ["run", "tool", "setup"] {
            names.extend(
                self.ctx
                    .shell
                    .names_with_label(&format!("{KIND_LABEL}={kind}"))
                    .await?,
            );
        }
        for name in &names {
            self.ctx.delete(name).await;
        }
        // Containers from before cedar ran its sandboxes on OpenShell.
        let docker = &self.ctx.docker;
        let ps = ["ps", "-aq", "--filter", &format!("label={RUN_LABEL}")].map(String::from);
        let old = docker_checked(docker, &ps, DOCKER_CALL_TIMEOUT).await?;
        let old: Vec<String> = old.split_whitespace().map(String::from).collect();
        if !old.is_empty() {
            let mut unpause = vec!["unpause".to_string()];
            unpause.extend(old.iter().cloned());
            let _ = docker_output(docker, &unpause, DOCKER_CALL_TIMEOUT).await;
            let mut rm = vec!["rm".to_string(), "-f".into()];
            rm.extend(old.iter().cloned());
            docker_checked(docker, &rm, DOCKER_CALL_TIMEOUT).await?;
        }
        Ok(names.len() + old.len())
    }

    async fn remove_workspace(&self, workspace_id: Uuid) -> Result<()> {
        // A sandbox that was just removed can hold on to the volume for a moment.
        let args = ["volume", "rm", "-f", &volume_name(workspace_id)].map(String::from);
        let mut last = None;
        for attempt in 0..10 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            match docker_checked(&self.ctx.docker, &args, DOCKER_CALL_TIMEOUT).await {
                Ok(_) => return Ok(()),
                Err(e) if e.to_string().contains("in use") => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.expect("at least one attempt"))
    }

    async fn list_workspaces(&self) -> Result<Vec<Uuid>> {
        let ls = [
            "volume",
            "ls",
            "-q",
            "--filter",
            &format!("label={WORKSPACE_LABEL}"),
        ]
        .map(String::from);
        let names = docker_checked(&self.ctx.docker, &ls, DOCKER_CALL_TIMEOUT).await?;
        Ok(names
            .split_whitespace()
            .filter_map(|n| n.strip_prefix(VOLUME_PREFIX)?.parse().ok())
            .collect())
    }

    async fn list_workspace_files(&self, workspace_id: Uuid) -> Result<WorkspaceListing> {
        let out = self
            .run_in_workspace(workspace_id, &list_script(), &[])
            .await?;
        if !out.status.success() {
            bail!(
                "listing workspace {workspace_id} failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(parse_listing(&out.stdout))
    }

    async fn read_workspace_file(
        &self,
        workspace_id: Uuid,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<FileContent>> {
        check_workspace_path(path)?;
        let limit = (max_bytes + 1).to_string();
        let out = self
            .run_in_workspace(workspace_id, READ_WORKSPACE_SCRIPT, &[path, &limit])
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

    async fn health(&self) -> Result<()> {
        let docker = &self.ctx.docker;
        docker_checked(
            docker,
            &["info", "--format", "{{.ServerVersion}}"].map(String::from),
            DOCKER_CALL_TIMEOUT,
        )
        .await
        .context("docker daemon is not reachable")?;
        let image = &self.ctx.cfg.image;
        let inspect = ["image", "inspect", "--format", "{{.Id}}", image].map(String::from);
        docker_checked(docker, &inspect, DOCKER_CALL_TIMEOUT)
            .await
            .with_context(|| {
                format!("sandbox image {image} is missing; build it with `docker build -t {image} sandbox/`")
            })?;
        self.ctx.shell.status().await
    }

    async fn setup_tool(&self, name: &str, requirements: &[String]) -> Result<String> {
        if requirements.is_empty() {
            return Ok(String::new());
        }
        let ctx = &self.ctx;
        let env_volume = ctx.tool_env(name, true).await?;
        let sandbox = short_name("cs", Uuid::new_v4());
        let labels = [
            (KIND_LABEL, "setup".to_string()),
            (TOOL_LABEL, name.to_string()),
        ];
        let policy = policy::for_tool_setup(TOOL_ENV_MOUNT);
        ctx.create(
            &sandbox,
            &labels,
            &policy,
            &[mount(&env_volume, TOOL_ENV_MOUNT, false)],
            &tool_env(),
            &[],
        )
        .await
        .context("starting the sandbox that installs the tool's packages")?;
        let mut argv: Vec<&str> = vec![
            "python3",
            "-m",
            "pip",
            "install",
            "--user",
            "--no-warn-script-location",
            "--",
        ];
        argv.extend(requirements.iter().map(String::as_str));
        let argv = wrapped(SETUP_TIMEOUT, 64 * 1024, "run", &argv);
        let out = ctx
            .run(&sandbox, &argv, None, SETUP_TIMEOUT, 64 * 1024, &[])
            .await;
        ctx.delete(&sandbox).await;
        let out = match out? {
            Ok(out) => out,
            Err(why) => bail!("pip could not be run: {why}"),
        };
        let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
        if out.exit_code != Some(0) {
            bail!(
                "pip install failed ({}): {}",
                out.exit_code
                    .map_or("killed".to_string(), |c| format!("exit status {c}")),
                text(&out.stderr)
            );
        }
        Ok(text(&out.stdout))
    }

    async fn remove_tool_env(&self, name: &str) -> Result<()> {
        let docker = &self.ctx.docker;
        let ls = [
            "volume",
            "ls",
            "-q",
            "--filter",
            &format!("label={TOOL_LABEL}={name}"),
        ]
        .map(String::from);
        let mut volumes: Vec<String> = docker_checked(docker, &ls, DOCKER_CALL_TIMEOUT)
            .await?
            .split_whitespace()
            .map(String::from)
            .collect();
        let env = tool_env_volume(name);
        if !volumes.contains(&env) {
            volumes.push(env);
        }
        let mut args = vec!["volume".to_string(), "rm".into(), "-f".into()];
        args.extend(volumes);
        docker_checked(docker, &args, DOCKER_CALL_TIMEOUT).await?;
        Ok(())
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
            memory: "256m".into(),
            cpus: "2".into(),
            tools_dir: None,
        }
    }

    #[test]
    fn create_args_make_a_detached_restarting_sandbox_with_its_volume() {
        let ws = Uuid::max();
        let mounts = [mount(&volume_name(ws), WORKSPACE, false)];
        let labels = [
            (KIND_LABEL, "run".to_string()),
            (RUN_LABEL, "r1".to_string()),
        ];
        let args = create_args(
            &cfg(),
            "cedar-run-x",
            &labels,
            "/tmp/p.yaml",
            &mounts,
            &workspace_env(),
            &[],
            "256Mi",
        );
        let joined = args.join(" ");
        for want in [
            "sandbox create --name cedar-run-x --from img:1",
            "--cpu 2 --memory 256Mi",
            "--policy /tmp/p.yaml",
            "--no-auto-providers",
            "--no-tty",
            "--restart-policy always",
            "--detach -o json",
            "--label cedar.kind=run --label cedar.run=r1",
            "--env PYTHONUSERBASE=/workspace/.local",
            "--env MPLBACKEND=Agg",
        ] {
            assert!(joined.contains(want), "{want:?} in {joined}");
        }
        let config_at = args
            .iter()
            .position(|a| a == "--driver-config-json")
            .unwrap();
        let config: Value = serde_json::from_str(&args[config_at + 1]).unwrap();
        assert_eq!(
            config["docker"]["mounts"][0],
            json!({ "type": "volume", "source": format!("cedar-ws-{ws}"), "target": "/workspace", "read_only": false })
        );
        // The idle command comes last, after `--`, so nothing in it is read as a flag.
        assert_eq!(&args[args.len() - 3..], ["--", "sleep", "infinity"]);
        assert!(!joined.contains("LLM_API_KEY"));
    }

    #[test]
    fn sandbox_names_fit_openshells_limit_and_differ_by_their_random_part() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let name = run_sandbox_name(a);
        assert_eq!(name.len(), 19, "{name}");
        assert!(name.starts_with("cr-"));
        assert_ne!(
            name,
            run_sandbox_name(b),
            "same millisecond, different names"
        );
        assert_eq!(
            run_sandbox_name(a),
            name,
            "the same run always has the same name"
        );
        assert!(short_name("ct", Uuid::new_v4()).starts_with("ct-"));
    }

    #[test]
    fn secrets_go_to_the_sandbox_by_name_only() {
        let args = create_args(
            &cfg(),
            "cedar-tool-x",
            &[],
            "/tmp/p.yaml",
            &[],
            &tool_env(),
            &["TANDEM_PASSWORD".to_string()],
            "256Mi",
        );
        let joined = args.join(" ");
        assert!(joined.contains("--env-from TANDEM_PASSWORD "), "{joined}");
        assert!(joined.contains("--env HOME=/opt/cedar/env"));
        assert!(joined.contains("--env PYTHONUSERBASE=/opt/cedar/env/.local"));
        assert!(!joined.contains("--driver-config-json"));
    }

    #[test]
    fn exec_args_run_in_the_workspace_with_the_command_as_argv() {
        let cmd = "echo 'hi'; rm -rf / # $(whoami)";
        let argv = wrapped(Duration::from_secs(30), 1000, "sh", &[cmd]);
        let args = exec_args("cedar-run-x", Duration::from_secs(45), &argv);
        assert_eq!(
            &args[..11],
            [
                "sandbox",
                "exec",
                "-n",
                "cedar-run-x",
                "--no-tty",
                "--no-login-shell",
                "--workdir",
                "/workspace",
                "--timeout",
                "45",
                "--"
            ]
        );
        assert_eq!(&args[11..15], ["python3", "-I", "-S", "-c"]);
        assert_eq!(args[15], EXEC_WRAPPER);
        assert_eq!(&args[16..], ["30", "1000", "sh", cmd]);
    }

    #[test]
    fn the_wrappers_report_is_split_off_stderr() {
        let mut err = b"boom\n\n\x1ecedar-exit:3:12:5\n".to_vec();
        assert_eq!(take_trailer(&mut err), Some((Some(3), 12, 5)));
        assert_eq!(err, b"boom\n");
        let mut err = b"\n\x1ecedar-exit:signal:0:0\n".to_vec();
        assert_eq!(take_trailer(&mut err), Some((None, 0, 0)));
        assert!(err.is_empty());
        // Output that only looks like a report earlier on doesn't count; the last one does.
        let mut err = b"\x1ecedar-exit:9:9:9 fake\n\n\x1ecedar-exit:0:1:2\n".to_vec();
        assert_eq!(take_trailer(&mut err), Some((Some(0), 1, 2)));
        let mut err = b"Error: sandbox 'x' is not ready (phase: Error)".to_vec();
        assert_eq!(take_trailer(&mut err), None);
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

    #[test]
    fn a_folders_digest_changes_with_its_files() {
        let dir = std::env::temp_dir().join(format!("cedar-digest-{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("tool.py"), "print(1)").unwrap();
        let a = folder_digest(&dir).unwrap();
        assert_eq!(a.len(), 12);
        assert_eq!(folder_digest(&dir).unwrap(), a);
        std::fs::write(dir.join("sub/helper.py"), "x = 2").unwrap();
        assert_ne!(folder_digest(&dir).unwrap(), a);
        std::fs::remove_dir_all(&dir).unwrap();
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
