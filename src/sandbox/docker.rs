//! Docker sandboxes: the default runtime, and the only one on Windows.
//!
//! A conversation's sandbox is a container started with `docker run`: no capabilities, a fixed
//! non-root user, memory, CPU and process limits, and no network unless the sandbox's policy
//! reaches anything, in which case it gets all of `SANDBOX_NETWORK` (Docker can't limit a
//! container to some hosts). Each personal tool call runs in a container of its own that sees
//! only that tool's folder and environment, with `TOOLS_NETWORK` if its manifest declares hosts.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

const LABEL: &str = "cedar.run";
/// Extra time beyond the in-container `timeout` before the host gives up on `docker exec`.
const EXEC_BACKSTOP: Duration = Duration::from_secs(10);
const WRITE_SCRIPT: &str = r#"mkdir -p -- "$(dirname -- "$1")" && cat > "$1""#;

fn container_name(run_id: Uuid) -> String {
    format!("cedar-run-{run_id}")
}

/// The Docker network a conversation's container gets: `SANDBOX_NETWORK` when the sandbox's
/// policy reaches anything, else none.
fn network(cfg: &SandboxConfig) -> &str {
    cfg.docker_network()
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
        network(cfg),
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
    args.extend(locked_down(
        cfg,
        tool_network(cfg, !call.hosts.is_empty()),
        "128",
    ));
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

pub struct DockerSandbox {
    docker: String,
    container: String,
    cfg: SandboxConfig,
    run_id: Uuid,
    workspace_id: Uuid,
    /// `docker pause` succeeded and no `docker unpause` has since.
    paused: AtomicBool,
}

impl DockerSandbox {
    async fn unpause(&self) -> Result<()> {
        let args = vec!["unpause".to_string(), self.container.clone()];
        docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
        self.paused.store(false, Ordering::SeqCst);
        Ok(())
    }
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
        ensure_tool_env(&self.docker, &self.cfg, call.name, false).await?;
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

    async fn pause(&self) {
        let args = vec!["pause".to_string(), self.container.clone()];
        match docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await {
            Ok(_) => self.paused.store(true, Ordering::SeqCst),
            Err(e) => {
                tracing::warn!(error = %e, container = %self.container, "pausing sandbox container failed")
            }
        }
    }

    async fn resume(&self) -> Result<()> {
        let inspect = [
            "container",
            "inspect",
            "--format",
            "{{.State.Status}}",
            &self.container,
        ]
        .map(String::from);
        let status = docker_checked(&self.docker, &inspect, DOCKER_CALL_TIMEOUT).await?;
        match status.trim() {
            "running" => Ok(()),
            "paused" => self.unpause().await,
            other => bail!("sandbox container {} is {other}", self.container),
        }
    }

    async fn destroy(&self) {
        // Some Docker versions refuse to kill a paused container.
        if self.paused.load(Ordering::SeqCst) {
            if let Err(e) = self.unpause().await {
                tracing::warn!(error = %e, container = %self.container, "unpausing sandbox container failed");
            }
        }
        // Only the container: the workspace volume outlives the run so it can be continued.
        let args = vec!["rm".to_string(), "-f".into(), self.container.clone()];
        if let Err(e) = docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await {
            tracing::warn!(error = %e, container = %self.container, "removing sandbox container failed");
        }
    }
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

    fn workspaces(&self) -> Workspaces<'_> {
        Workspaces {
            docker: &self.docker,
            cfg: &self.cfg,
        }
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
            paused: AtomicBool::new(false),
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
        // Sandboxes kept paused for an idle conversation; unpaused first, as `destroy` does.
        let paused = [
            "ps",
            "-q",
            "--filter",
            &Self::label_filter(),
            "--filter",
            "status=paused",
        ]
        .map(String::from);
        let paused = docker_checked(&self.docker, &paused, DOCKER_CALL_TIMEOUT).await?;
        let paused: Vec<String> = paused.split_whitespace().map(String::from).collect();
        if !paused.is_empty() {
            let mut args = vec!["unpause".to_string()];
            args.extend(paused);
            if let Err(e) = docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await {
                tracing::warn!(error = %e, "unpausing leftover sandboxes failed");
            }
        }
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
        self.workspaces().remove(workspace_id).await
    }

    async fn list_workspaces(&self) -> Result<Vec<Uuid>> {
        self.workspaces().list().await
    }

    async fn list_workspace_files(&self, workspace_id: Uuid) -> Result<WorkspaceListing> {
        self.workspaces().files(workspace_id).await
    }

    async fn read_workspace_file(
        &self,
        workspace_id: Uuid,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<FileContent>> {
        self.workspaces().read(workspace_id, path, max_bytes).await
    }

    async fn health(&self) -> Result<()> {
        docker_health(&self.docker, &self.cfg.image).await
    }

    async fn setup_tool(&self, name: &str, requirements: &[String]) -> Result<String> {
        if requirements.is_empty() {
            return Ok(String::new());
        }
        ensure_tool_env(&self.docker, &self.cfg, name, true).await?;
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
/// (a new volume is root's, unless the image already has the directory). With `fix_owner`, an
/// existing volume is handed to the agent's user too: one left root's (by an install whose chown
/// failed, or by `docker run -v` creating it) would otherwise fail every `pip install` with
/// "Permission denied".
async fn ensure_tool_env(
    docker: &str,
    cfg: &SandboxConfig,
    name: &str,
    fix_owner: bool,
) -> Result<()> {
    let volume = tool_env_volume(name);
    let inspect = ["volume", "inspect", "--format", "{{.Name}}", &volume].map(String::from);
    if docker_output(docker, &inspect, DOCKER_CALL_TIMEOUT)
        .await?
        .status
        .success()
    {
        if fix_owner {
            chown_tool_env(docker, cfg, &volume).await?;
        }
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
    chown_tool_env(docker, cfg, &volume).await
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
    fn a_tool_call_sees_only_its_own_folder_environment_and_secrets() {
        let (run, ws) = (Uuid::nil(), Uuid::max());
        let env = vec![("TANDEM_PASSWORD".to_string(), "hunter2".to_string())];
        let call = ToolRun {
            name: "tandem_glucose",
            dir: std::path::Path::new("/home/me/tools/tandem_glucose"),
            entry: "tool.py",
            input: b"{}",
            env: &env,
            hosts: &[],
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

        let hosts = ["api.example.com".to_string()];
        let networked = ToolRun {
            hosts: &hosts,
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

    #[test]
    fn the_container_gets_the_network_only_when_the_policy_reaches_anything() {
        let net = |c: &SandboxConfig| {
            let args = run_args(c, Uuid::nil(), Uuid::nil());
            let at = args.iter().position(|a| a == "--network").unwrap();
            args[at + 1].clone()
        };
        assert_eq!(net(&cfg()), "none");
        let packages = SandboxConfig {
            policy: Policy::packages(),
            ..cfg()
        };
        assert_eq!(net(&packages), "bridge");
    }
}
