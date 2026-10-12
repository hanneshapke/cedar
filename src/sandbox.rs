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

#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Run `command` with `sh -c`, killing it after `timeout`. Exit code 124 means it timed out.
    async fn exec(&self, command: &str, timeout: Duration, max_output: usize)
        -> Result<ExecOutput>;
    async fn read_file(&self, path: &str, max_bytes: usize) -> Result<FileContent>;
    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()>;
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
    /// Is the backing runtime reachable and usable? (for `/readyz`)
    async fn health(&self) -> Result<()>;
}

// ---------------------------------------------------------------------------------------------
// Docker
// ---------------------------------------------------------------------------------------------

const LABEL: &str = "cedar.run";
const WORKSPACE_LABEL: &str = "cedar.workspace";
const VOLUME_PREFIX: &str = "cedar-ws-";
const DOCKER_CALL_TIMEOUT: Duration = Duration::from_secs(30);
const DOCKER_CREATE_TIMEOUT: Duration = Duration::from_secs(120);
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
        &cfg.image,
        "sleep",
        "infinity",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.shrink_to_fit();
    args
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

async fn docker_output(
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
async fn docker_checked(bin: &str, args: &[String], timeout: Duration) -> Result<String> {
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
            docker: "docker".into(),
            cfg,
        }
    }

    fn label_filter() -> String {
        format!("label={LABEL}")
    }
}

#[async_trait]
impl SandboxFactory for DockerSandboxFactory {
    async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Sandbox>> {
        let sandbox = DockerSandbox {
            docker: self.docker.clone(),
            container: container_name(run_id),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SandboxConfig {
        SandboxConfig {
            image: "img:1".into(),
            network: "none".into(),
            memory: "256m".into(),
            cpus: "2".into(),
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
}
