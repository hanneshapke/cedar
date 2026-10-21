//! Isolated execution environments for a run's tools.
//!
//! Every file and command operation goes through the sandbox (`docker exec`), never the host
//! filesystem. Reading host-side from a bind mount would let the agent point a symlink outside
//! its workspace.

use crate::config::SandboxConfig;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
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
    /// Run `command` with `sh -c`, killing it after `timeout`. Exit code 124 means it timed out.
    async fn exec(&self, command: &str, timeout: Duration, max_output: usize)
        -> Result<ExecOutput>;
    async fn read_file(&self, path: &str, max_bytes: usize) -> Result<FileContent>;
    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()>;
    /// Run one call of a personal tool in a container of its own (see [`ToolRun`]). Exit code
    /// 124 means it timed out.
    async fn run_tool(
        &self,
        _call: &ToolRun<'_>,
        _timeout: Duration,
        _max_output: usize,
    ) -> Result<ExecOutput> {
        bail!("personal tools need the Docker sandbox")
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
    /// [`tool_env_volume`]), with the network personal tools get. Returns pip's output.
    async fn setup_tool(&self, _name: &str, _requirements: &[String]) -> Result<String> {
        bail!("installing a tool's packages needs the Docker sandbox")
    }
    /// Delete a personal tool's environment. Succeeds if it doesn't exist.
    async fn remove_tool_env(&self, _name: &str) -> Result<()> {
        Ok(())
    }
}

/// One call of a personal tool. It runs in a fresh container that sees the run's workspace, the
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
    /// Environment variables for the script: the secrets its manifest declares.
    pub env: &'a [(String, String)],
    /// The manifest declares hosts it reaches, so the call gets `tools_network` instead of none.
    pub network: bool,
}

/// Where a personal tool's folder is mounted in its container.
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
// Docker
// ---------------------------------------------------------------------------------------------

const LABEL: &str = "cedar.run";
const WORKSPACE_LABEL: &str = "cedar.workspace";
const VOLUME_PREFIX: &str = "cedar-ws-";
pub(crate) const DOCKER_CALL_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DOCKER_CREATE_TIMEOUT: Duration = Duration::from_secs(120);
/// Extra time beyond the in-container `timeout` before the host gives up on `docker exec`.
const EXEC_BACKSTOP: Duration = Duration::from_secs(10);
const WRITE_SCRIPT: &str = r#"mkdir -p -- "$(dirname -- "$1")" && cat > "$1""#;

fn container_name(run_id: Uuid) -> String {
    format!("cedar-run-{run_id}")
}

fn volume_name(workspace_id: Uuid) -> String {
    format!("{VOLUME_PREFIX}{workspace_id}")
}

/// Arguments for `docker run` that start the (idle) sandbox container.
pub fn run_args(cfg: &SandboxConfig, run_id: Uuid, workspace_id: Uuid) -> Vec<String> {
    let mut args: Vec<String> = [
        "run",
        "-d",
        "--init", // reap zombies left by killed commands; the pids limit is finite
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
        "256",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "1000:1000",
        "-w",
        "/workspace",
        "-v",
        &format!("{}:/workspace", volume_name(workspace_id)),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // Personal tools are not mounted here: each call runs in a container of its own that sees
    // only that tool (see `tool_run_args`), so the agent and other tools can't read its files.
    args.extend([cfg.image.clone(), "sleep".into(), "infinity".into()]);
    args
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
/// read-only, under the same restrictions as a sandbox. Used to look at a workspace whose run
/// has finished (and whose container is therefore gone).
pub fn workspace_run_args(
    cfg: &SandboxConfig,
    workspace_id: Uuid,
    script: &str,
    args: &[&str],
) -> Vec<String> {
    let mut out: Vec<String> = [
        "run",
        "--rm",
        "--network",
        "none",
        "--memory",
        &cfg.memory,
        "--memory-swap",
        &cfg.memory,
        "--cpus",
        &cfg.cpus,
        "--pids-limit",
        "64",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "1000:1000",
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
    .map(|s| s.to_string())
    .collect();
    out.extend(args.iter().map(|s| s.to_string()));
    out
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

/// Arguments for `docker exec` that run `command` under an in-container `timeout`.
/// `-k 2` escalates to SIGKILL if SIGTERM is ignored; exit code 124 means the command timed out.
pub fn exec_args(container: &str, timeout_secs: u64, command: &str) -> Vec<String> {
    [
        "exec",
        container,
        "timeout",
        "-k",
        "2",
        &timeout_secs.to_string(),
        "sh",
        "-c",
        command,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The restrictions every sandbox container runs under, as `docker run` arguments.
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

/// The environment a personal tool's container gets: its own home and package directory. Secret
/// names follow as `-e NAME` only, so their values come from the `docker` process's environment
/// and never appear in its arguments (which other users of the host can see).
fn tool_env_args(env_names: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = [
        format!("HOME={TOOL_ENV_MOUNT}"),
        format!("PYTHONUSERBASE={TOOL_ENV_MOUNT}/.local"),
        format!("PATH={TOOL_ENV_MOUNT}/.local/bin:/usr/local/bin:/usr/bin:/bin"),
        "PIP_DISABLE_PIP_VERSION_CHECK=1".into(),
    ]
    .into_iter()
    .flat_map(|e| ["-e".to_string(), e])
    .collect();
    for name in env_names {
        args.push("-e".into());
        args.push(name.to_string());
    }
    args
}

/// The network a personal tool's call gets: `tools_network` if its manifest declares hosts,
/// else none.
fn tool_network(cfg: &SandboxConfig, wants: bool) -> &str {
    if wants {
        &cfg.tools_network
    } else {
        "none"
    }
}

/// Arguments for the `docker run` of one personal tool call (see [`ToolRun`]): the run's
/// workspace, the tool's folder read-only, the tool's environment, the declared secrets by name,
/// and the script under an in-container `timeout`, reading its input from stdin.
pub fn tool_run_args(
    cfg: &SandboxConfig,
    run_id: Uuid,
    workspace_id: Uuid,
    container: &str,
    call: &ToolRun<'_>,
    timeout_secs: u64,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "run",
        "--rm",
        "-i",
        "--init",
        "--name",
        container,
        // So `cleanup_orphans` removes a call left behind by a crash, like the run's sandbox.
        "--label",
        &format!("{LABEL}={run_id}"),
        "--label",
        &format!("{TOOL_LABEL}={}", call.name),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(locked_down(cfg, tool_network(cfg, call.network), "128"));
    args.extend([
        "-w".to_string(),
        "/workspace".into(),
        "-v".into(),
        format!("{}:/workspace", volume_name(workspace_id)),
        "--mount".into(),
        bind_mount_ro(call.dir, TOOL_MOUNT),
        "-v".into(),
        format!("{}:{TOOL_ENV_MOUNT}", tool_env_volume(call.name)),
    ]);
    let names: Vec<&str> = call.env.iter().map(|(k, _)| k.as_str()).collect();
    args.extend(tool_env_args(&names));
    args.extend([
        cfg.image.clone(),
        "timeout".into(),
        "-k".into(),
        "2".into(),
        timeout_secs.to_string(),
        "python3".into(),
        "--".into(),
        format!("{TOOL_MOUNT}/{}", call.entry),
    ]);
    args
}

/// Arguments for installing a tool's requirements into its environment, as the agent's user.
/// Requirements were checked when the manifest was read (no options, URLs or paths).
pub fn tool_setup_args(cfg: &SandboxConfig, name: &str, requirements: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["run", "--rm", "--label", &format!("{TOOL_LABEL}={name}")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.extend(locked_down(cfg, &cfg.tools_network, "128"));
    args.extend([
        "-v".to_string(),
        format!("{}:{TOOL_ENV_MOUNT}", tool_env_volume(name)),
        "-w".into(),
        TOOL_ENV_MOUNT.into(),
    ]);
    args.extend(tool_env_args(&[]));
    args.extend(
        [
            &cfg.image,
            "python3",
            "-m",
            "pip",
            "install",
            "--user",
            "--no-warn-script-location",
            "--",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args.extend(requirements.iter().cloned());
    args
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

pub struct DockerSandbox {
    docker: String,
    container: String,
    cfg: SandboxConfig,
    run_id: Uuid,
    workspace_id: Uuid,
}

#[async_trait]
impl Sandbox for DockerSandbox {
    async fn exec(
        &self,
        command: &str,
        timeout: Duration,
        max_output: usize,
    ) -> Result<ExecOutput> {
        let args = exec_args(&self.container, timeout.as_secs().max(1), command);
        let mut child = Command::new(&self.docker)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to run `{}`", self.docker))?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let finished = tokio::time::timeout(timeout + EXEC_BACKSTOP, async {
            tokio::join!(
                read_capped(stdout, max_output),
                read_capped(stderr, max_output),
                child.wait()
            )
        })
        .await;

        match finished {
            Ok((out, err, status)) => {
                let (stdout, stdout_total) = out?;
                let (stderr, stderr_total) = err?;
                Ok(ExecOutput {
                    stdout,
                    stderr,
                    stdout_total,
                    stderr_total,
                    exit_code: status?.code(),
                })
            }
            Err(_) => {
                // The in-container timeout should have fired long ago, so the container is wedged.
                drop(child);
                self.destroy().await;
                Err(SandboxFatal("command did not terminate after its timeout".into()).into())
            }
        }
    }

    async fn read_file(&self, path: &str, max_bytes: usize) -> Result<FileContent> {
        check_path(path)?;
        let args: Vec<String> = [
            "exec",
            &self.container,
            "head",
            "-c",
            &(max_bytes + 1).to_string(),
            "--",
            path,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let out = docker_output(&self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
        if !out.status.success() {
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let mut bytes = out.stdout;
        let truncated = bytes.len() > max_bytes;
        bytes.truncate(max_bytes);
        Ok(FileContent { bytes, truncated })
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        check_path(path)?;
        let mut child = Command::new(&self.docker)
            .args([
                "exec",
                "-i",
                &self.container,
                "sh",
                "-c",
                WRITE_SCRIPT,
                "sh",
                path,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to run `{}`", self.docker))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let write = async move {
            let res = stdin.write_all(content).await;
            drop(stdin); // EOF for `cat`
            res
        };
        let (written, out) = tokio::time::timeout(DOCKER_CALL_TIMEOUT, async {
            tokio::join!(write, child.wait_with_output())
        })
        .await
        .map_err(|_| anyhow!("writing {path} timed out"))?;
        let out = out?;
        if !out.status.success() {
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        // A broken pipe with a zero exit would mean `cat` quit early; surface it.
        written.context("writing file contents")?;
        Ok(())
    }

    async fn run_tool(
        &self,
        call: &ToolRun<'_>,
        timeout: Duration,
        max_output: usize,
    ) -> Result<ExecOutput> {
        ensure_tool_env(&self.docker, &self.cfg, call.name).await?;
        let container = format!("cedar-tool-{}", Uuid::new_v4());
        let args = tool_run_args(
            &self.cfg,
            self.run_id,
            self.workspace_id,
            &container,
            call,
            timeout.as_secs().max(1),
        );
        let mut cmd = Command::new(&self.docker);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Passed by name in the arguments; docker takes the values from its own environment.
        for (k, v) in call.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to run `{}`", self.docker))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let input = call.input;
        let write = async move {
            // A script that never reads its input closes the pipe early; that is not an error.
            let _ = stdin.write_all(input).await;
            drop(stdin);
        };
        // Starting a container takes a moment on top of the script's own limit.
        let finished = tokio::time::timeout(timeout + EXEC_BACKSTOP + DOCKER_CALL_TIMEOUT, async {
            tokio::join!(
                write,
                read_capped(stdout, max_output),
                read_capped(stderr, max_output),
                child.wait()
            )
        })
        .await;
        match finished {
            Ok(((), out, err, status)) => {
                let (stdout, stdout_total) = out?;
                let (stderr, stderr_total) = err?;
                Ok(ExecOutput {
                    stdout,
                    stderr,
                    stdout_total,
                    stderr_total,
                    exit_code: status?.code(),
                })
            }
            Err(_) => {
                drop(child);
                let rm = vec!["rm".to_string(), "-f".into(), container.clone()];
                if let Err(e) = docker_checked(&self.docker, &rm, DOCKER_CALL_TIMEOUT).await {
                    tracing::warn!(error = %e, %container, "removing a stuck tool container failed");
                }
                bail!("{} did not finish after its timeout", call.name)
            }
        }
    }

    async fn destroy(&self) {
        // Only the container: the workspace volume outlives the run so it can be continued.
        let args = vec!["rm".to_string(), "-f".into(), self.container.clone()];
        if let Err(e) = docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await {
            tracing::warn!(error = %e, container = %self.container, "removing sandbox container failed");
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

pub struct DockerSandboxFactory {
    docker: String,
    cfg: SandboxConfig,
}

impl DockerSandboxFactory {
    pub fn new(cfg: SandboxConfig) -> Self {
        Self {
            docker: cfg.docker_bin.clone(),
            cfg,
        }
    }

    fn label_filter() -> String {
        format!("label={LABEL}")
    }

    /// `docker run` would silently create a missing volume, so check first. [`WorkspaceMissing`]
    /// if it doesn't exist; other docker failures are returned as they are.
    async fn require_workspace(&self, workspace_id: Uuid) -> Result<()> {
        let args = [
            "volume",
            "inspect",
            "--format",
            "{{.Name}}",
            &volume_name(workspace_id),
        ]
        .map(String::from);
        let out = docker_output(&self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
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

    async fn run_in_workspace(
        &self,
        workspace_id: Uuid,
        script: &str,
        args: &[&str],
    ) -> Result<std::process::Output> {
        self.require_workspace(workspace_id).await?;
        let args = workspace_run_args(&self.cfg, workspace_id, script, args);
        docker_output(&self.docker, &args, DOCKER_CREATE_TIMEOUT).await
    }
}

#[async_trait]
impl SandboxFactory for DockerSandboxFactory {
    async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Sandbox>> {
        let sandbox = DockerSandbox {
            docker: self.docker.clone(),
            container: container_name(run_id),
            cfg: self.cfg.clone(),
            run_id,
            workspace_id,
        };
        let started: Result<()> = async {
            // Creating a volume that already exists is a no-op, which is what a continuation wants.
            let volume_args = [
                "volume",
                "create",
                "--label",
                &format!("{WORKSPACE_LABEL}={workspace_id}"),
                &volume_name(workspace_id),
            ]
            .map(String::from);
            docker_checked(&self.docker, &volume_args, DOCKER_CALL_TIMEOUT).await?;
            docker_checked(
                &self.docker,
                &run_args(&self.cfg, run_id, workspace_id),
                DOCKER_CREATE_TIMEOUT,
            )
            .await?;
            Ok(())
        }
        .await;
        if let Err(e) = started {
            sandbox.destroy().await;
            return Err(e.context("creating sandbox container"));
        }
        Ok(Arc::new(sandbox))
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        let ps = ["ps", "-aq", "--filter", &Self::label_filter()].map(String::from);
        let ids = docker_checked(&self.docker, &ps, DOCKER_CALL_TIMEOUT).await?;
        let ids: Vec<String> = ids.split_whitespace().map(String::from).collect();
        if !ids.is_empty() {
            let mut args = vec!["rm".to_string(), "-f".into()];
            args.extend(ids.iter().cloned());
            docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
        }

        // Volumes labelled per run come from before workspaces outlived their run.
        let ls = ["volume", "ls", "-q", "--filter", &Self::label_filter()].map(String::from);
        let volumes = docker_checked(&self.docker, &ls, DOCKER_CALL_TIMEOUT).await?;
        let volumes: Vec<String> = volumes.split_whitespace().map(String::from).collect();
        if !volumes.is_empty() {
            let mut args = vec!["volume".to_string(), "rm".into(), "-f".into()];
            args.extend(volumes);
            docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
        }
        Ok(ids.len())
    }

    async fn remove_workspace(&self, workspace_id: Uuid) -> Result<()> {
        let args = ["volume", "rm", "-f", &volume_name(workspace_id)].map(String::from);
        docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
        Ok(())
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
        let names = docker_checked(&self.docker, &ls, DOCKER_CALL_TIMEOUT).await?;
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
        docker_checked(
            &self.docker,
            &["info", "--format", "{{.ServerVersion}}"].map(String::from),
            DOCKER_CALL_TIMEOUT,
        )
        .await
        .context("docker daemon is not reachable")?;
        let inspect =
            ["image", "inspect", "--format", "{{.Id}}", &self.cfg.image].map(String::from);
        docker_checked(&self.docker, &inspect, DOCKER_CALL_TIMEOUT)
            .await
            .with_context(|| {
                format!(
                    "sandbox image {} is missing; build it with `docker build -t {} sandbox/`",
                    self.cfg.image, self.cfg.image
                )
            })?;
        Ok(())
    }

    async fn setup_tool(&self, name: &str, requirements: &[String]) -> Result<String> {
        if requirements.is_empty() {
            return Ok(String::new());
        }
        ensure_tool_env(&self.docker, &self.cfg, name).await?;
        let args = tool_setup_args(&self.cfg, name, requirements);
        let out = docker_output(&self.docker, &args, SETUP_TIMEOUT).await?;
        let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
        if !out.status.success() {
            bail!("pip install failed ({}): {}", out.status, text(&out.stderr));
        }
        Ok(text(&out.stdout))
    }

    async fn remove_tool_env(&self, name: &str) -> Result<()> {
        let args = ["volume", "rm", "-f", &tool_env_volume(name)].map(String::from);
        docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
        Ok(())
    }
}

/// Create a personal tool's environment volume if it doesn't exist yet, owned by the agent's user
/// (a new volume is root's, unless the image already has the directory).
async fn ensure_tool_env(docker: &str, cfg: &SandboxConfig, name: &str) -> Result<()> {
    let volume = tool_env_volume(name);
    let inspect = ["volume", "inspect", "--format", "{{.Name}}", &volume].map(String::from);
    if docker_output(docker, &inspect, DOCKER_CALL_TIMEOUT)
        .await?
        .status
        .success()
    {
        return Ok(());
    }
    let create = [
        "volume",
        "create",
        "--label",
        &format!("{TOOL_LABEL}={name}"),
        &volume,
    ]
    .map(String::from);
    docker_checked(docker, &create, DOCKER_CALL_TIMEOUT).await?;
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

    fn cfg() -> SandboxConfig {
        SandboxConfig {
            docker_bin: "docker".into(),
            image: "img:1".into(),
            network: "none".into(),
            memory: "256m".into(),
            cpus: "2".into(),
            tools_dir: None,
            tools_network: "bridge".into(),
        }
    }

    #[test]
    fn run_args_lock_the_container_down() {
        let id = Uuid::nil();
        let ws = Uuid::max();
        let args = run_args(&cfg(), id, ws);
        let joined = args.join(" ");
        assert!(joined.contains("--network none"));
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--security-opt no-new-privileges"));
        assert!(joined.contains("--user 1000:1000"));
        assert!(joined.contains("--memory 256m --memory-swap 256m"));
        assert!(joined.contains(&format!("--name cedar-run-{id}")));
        assert!(
            joined.contains(&format!("-v cedar-ws-{ws}:/workspace")),
            "the workspace, not the run, names the volume"
        );
        // Image and the idle command come last, so nothing after the image is parsed as a docker flag.
        assert_eq!(&args[args.len() - 3..], ["img:1", "sleep", "infinity"]);
        assert!(!joined.contains("LLM_API_KEY"));
        assert!(!joined.contains("/opt/cedar/tools"));

        // Personal tools are never mounted into the run's sandbox, only into their own calls.
        let with_tools = SandboxConfig {
            tools_dir: Some("/home/me/.cedar/tools".into()),
            ..cfg()
        };
        let args = run_args(&with_tools, id, ws);
        assert!(!args.join(" ").contains("/home/me/.cedar/tools"));
        assert_eq!(&args[args.len() - 3..], ["img:1", "sleep", "infinity"]);
    }

    #[test]
    fn a_tool_folder_mounts_from_a_windows_path_or_one_with_a_comma() {
        // `-v C:\tools\t:/opt/cedar/tool:ro` would split at the drive letter's colon.
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
    fn a_tool_call_sees_only_its_own_folder_environment_and_secrets() {
        let (run, ws) = (Uuid::nil(), Uuid::max());
        let env = vec![("TANDEM_PASSWORD".to_string(), "hunter2".to_string())];
        let call = ToolRun {
            name: "tandem_glucose",
            dir: std::path::Path::new("/home/me/tools/tandem_glucose"),
            entry: "tool.py",
            input: b"{}",
            env: &env,
            network: false,
        };
        let args = tool_run_args(&cfg(), run, ws, "cedar-tool-x", &call, 30);
        let joined = args.join(" ");
        assert!(
            joined.contains("--network none"),
            "no hosts declared: {joined}"
        );
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--user 1000:1000"));
        assert!(joined.contains(&format!("--label cedar.run={run}")));
        assert!(joined.contains(&format!("-v cedar-ws-{ws}:/workspace ")));
        assert!(joined.contains(
            "--mount type=bind,src=/home/me/tools/tandem_glucose,dst=/opt/cedar/tool,readonly"
        ));
        assert!(joined.contains("-v cedar-tool-env-tandem_glucose:/opt/cedar/env"));
        assert!(joined.contains("-e PYTHONUSERBASE=/opt/cedar/env/.local"));
        // The secret goes by name; its value is in docker's environment, not its arguments.
        assert!(joined.contains("-e TANDEM_PASSWORD "));
        assert!(!joined.contains("hunter2"));
        assert!(
            !joined.contains("/home/me/tools "),
            "not the whole tools directory"
        );
        assert_eq!(
            &args[args.len() - 8..],
            [
                "img:1",
                "timeout",
                "-k",
                "2",
                "30",
                "python3",
                "--",
                "/opt/cedar/tool/tool.py"
            ]
        );

        let networked = ToolRun {
            network: true,
            ..call
        };
        let joined = tool_run_args(&cfg(), run, ws, "c", &networked, 30).join(" ");
        assert!(joined.contains("--network bridge"), "{joined}");
    }

    #[test]
    fn tool_setup_installs_into_the_tools_own_environment() {
        let args = tool_setup_args(&cfg(), "t", &["tconnectsync>=3.0".to_string()]);
        let joined = args.join(" ");
        assert!(joined.contains("--network bridge"));
        assert!(joined.contains("-v cedar-tool-env-t:/opt/cedar/env"));
        assert!(joined.ends_with(
            "img:1 python3 -m pip install --user --no-warn-script-location -- tconnectsync>=3.0"
        ));
    }

    #[test]
    fn exec_args_keep_the_command_as_a_single_argv_element() {
        let cmd = "echo 'hi'; rm -rf / # $(whoami)";
        let args = exec_args("c1", 30, cmd);
        assert_eq!(
            args,
            ["exec", "c1", "timeout", "-k", "2", "30", "sh", "-c", cmd]
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
