//! The `sbx` sandbox backend (`SANDBOX_BACKEND=sbx`): each run's tools run in a Docker Sandbox,
//! a microVM with its own kernel and its own Docker engine, driven by the `sbx` CLI.
//!
//! How it maps onto the Docker backend (see [`crate::sandbox`]):
//!
//! - A run gets the microVM `cedar-run-<run id>`, created from `SANDBOX_IMAGE` as the template.
//!   It is removed when the run ends.
//! - A workspace is a directory under `SBX_DATA_DIR/workspaces`, mounted into the run's microVM.
//!   `sbx` mounts it at its host path, so `/workspace` is made a link to it. It outlives the run,
//!   like the Docker backend's volume, so a continuation picks up its files.
//! - Files of a workspace whose run has finished are read through `cedar-files`, a microVM that
//!   sees every workspace read-only and has no network, never from the host: a symlink the agent
//!   left there must not reach outside its workspace.
//! - Network: `SANDBOX_NETWORK=none` denies every host for the run (a deny rule, which no allow
//!   rule can reopen). `bridge` (or `**`) allows every host; anything else is read as a
//!   comma-separated allow-list of hosts.
//! - A personal tool runs in a microVM of its own per run (`cedar-tool-...`), which sees the
//!   run's workspace, the tool's folder read-only and the tool's own environment, and may reach
//!   only the hosts its manifest declares. Those allow rules are added on top of the global
//!   policy (`sbx policy`), so they are an exact allow-list only when that policy denies by
//!   default (`sbx policy init deny-all`).
//! - The browser gets a microVM of its own (`cedar-browser-<run id>`), from `BROWSER_IMAGE`.
//!
//! cedar shells out to `sbx` as it does to `docker`; `sbx exec` takes `docker exec`'s flags.

use crate::browser::{Browser, BrowserFactory, DockerBrowser};
use crate::config::{BrowserConfig, SandboxConfig, SbxConfig};
use crate::sandbox::{
    check_path, check_workspace_path, docker_output, head_command, list_script, parse_listing,
    read_via, run_capped, tool_env_args, write_via_stdin, ExecOutput, FileContent, Sandbox,
    SandboxFactory, SandboxFatal, ToolRun, WorkspaceListing, WorkspaceMissing, EXEC_BACKSTOP,
    READ_WORKSPACE_SCRIPT, SETUP_TIMEOUT, TOOL_ENV_MOUNT, TOOL_MOUNT, WRITE_SCRIPT,
};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

const RUN_PREFIX: &str = "cedar-run-";
const TOOL_PREFIX: &str = "cedar-tool-";
const SETUP_PREFIX: &str = "cedar-setup-";
const BROWSER_PREFIX: &str = "cedar-browser-";
/// The microVM that reads the files of finished runs' workspaces.
const FILES_SANDBOX: &str = "cedar-files";
/// What `sbx create` is given as the agent: no agent, just the template image.
const AGENT: &str = "shell";
/// Where the agent works, as in the Docker backend's image.
const WORKSPACE: &str = "/workspace";
/// The hosts `pip` needs to install a personal tool's requirements.
const PIP_HOSTS: &[&str] = &["pypi.org", "files.pythonhosted.org"];

const SBX_CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Booting a microVM, and pulling its template the first time.
const SBX_CREATE_TIMEOUT: Duration = Duration::from_secs(600);

/// Makes each `$1` reachable at `$2` (a link, replacing an empty directory there). Runs as root:
/// the links live outside the agent's home. They are how a sandbox finds its mounts, which `sbx`
/// puts at their host paths, at the fixed paths the image and the tools expect.
const LINK_SCRIPT: &str = r#"set -e
while [ $# -ge 2 ]; do
  mkdir -p -- "$(dirname -- "$2")"
  rmdir -- "$2" 2>/dev/null || true
  ln -sfn -- "$1" "$2"
  [ -L "$2" ] || { echo "could not link $2" >&2; exit 1; }
  shift 2
done"#;

/// /workspace is a link to the mount, so commands get `PWD` too: `pwd` then says /workspace rather
/// than the host path behind it.
const PWD: &str = "PWD=/workspace";

/// Starts the browser's driver in the background; `cedar-browser call` waits for its socket.
const START_BROWSER: &str = "setsid cedar-browser serve >/tmp/cedar-browser.log 2>&1 </dev/null &";

/// Outbound network access for a microVM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Egress {
    /// No host at all.
    None,
    /// Every host.
    All,
    /// These hosts (on top of whatever the global `sbx policy` allows).
    Hosts(Vec<String>),
}

impl Egress {
    /// Read a Docker-style network setting (`SANDBOX_NETWORK`, `BROWSER_NETWORK`): `none`;
    /// `bridge`, `host` or `**` for every host; or a comma-separated list of hosts.
    pub fn from_setting(network: &str) -> Self {
        match network.trim() {
            "" | "none" => Self::None,
            "bridge" | "host" | "**" => Self::All,
            hosts => Self::hosts(hosts.split(',')),
        }
    }

    fn hosts<S: AsRef<str>>(hosts: impl IntoIterator<Item = S>) -> Self {
        let hosts: Vec<String> = hosts
            .into_iter()
            .map(|h| h.as_ref().trim().to_string())
            .filter(|h| !h.is_empty())
            .collect();
        if hosts.is_empty() {
            Self::None
        } else {
            Self::Hosts(hosts)
        }
    }
}

/// `sbx` takes whole CPUs; Docker also takes fractions ("0.5"), which are rounded up.
fn whole_cpus(cpus: &str) -> String {
    match cpus.trim().parse::<f64>() {
        Ok(n) if n.is_finite() => (n.ceil() as i64).max(1).to_string(),
        _ => cpus.trim().to_string(),
    }
}

/// What a new microVM is made of.
pub struct Spec<'a> {
    pub name: &'a str,
    pub image: &'a str,
    pub memory: &'a str,
    pub cpus: &'a str,
    pub egress: &'a Egress,
    /// Host paths to mount, each optionally with a `:ro` suffix. They appear at the same path
    /// inside.
    pub mounts: &'a [String],
}

/// Arguments for `sbx create`. `--memory` is always given: `sbx`'s default is half the host's.
pub fn create_args(sbx: &SbxConfig, spec: &Spec<'_>) -> Vec<String> {
    let mut args: Vec<String> = [
        "create",
        "--name",
        spec.name,
        "--template",
        spec.image,
        "--pull",
        &sbx.pull,
        "--memory",
        spec.memory,
        "--cpus",
        &whole_cpus(spec.cpus),
        // The shared agent skills store is the host user's, not the agent's.
        "--skills",
        "off",
        "--no-progress",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if *spec.egress == Egress::None {
        // A deny rule wins over every allow rule, including the global policy's.
        args.extend(["--deny-network".to_string(), "**".into()]);
    }
    args.push(AGENT.into());
    args.extend(spec.mounts.iter().cloned());
    args
}

/// Arguments for `sbx policy allow network` that open `egress` to the sandbox `name`, if it
/// allows anything.
pub fn allow_args(name: &str, egress: &Egress) -> Option<Vec<String>> {
    let resources = match egress {
        Egress::None => return None,
        Egress::All => "**".to_string(),
        Egress::Hosts(hosts) => hosts.join(","),
    };
    Some(
        ["policy", "allow", "network", "--sandbox", name, &resources]
            .map(String::from)
            .to_vec(),
    )
}

/// Arguments for `sbx exec` that link each `(target, link)` pair (see [`LINK_SCRIPT`]).
pub fn link_args(name: &str, links: &[(&str, &str)]) -> Vec<String> {
    let mut args: Vec<String> = ["exec", "-u", "root", name, "sh", "-c", LINK_SCRIPT, "sh"]
        .map(String::from)
        .to_vec();
    for (target, link) in links {
        args.push(target.to_string());
        args.push(link.to_string());
    }
    args
}

/// Arguments for `sbx exec` that run `command` in /workspace under an in-sandbox `timeout`, as in
/// [`crate::sandbox::exec_args`]. Exit code 124 means the command timed out.
pub fn exec_args(name: &str, timeout_secs: u64, command: &str) -> Vec<String> {
    [
        "exec",
        "-w",
        WORKSPACE,
        "-e",
        PWD,
        name,
        "timeout",
        "-k",
        "2",
        &timeout_secs.to_string(),
        "sh",
        "-c",
        command,
    ]
    .map(String::from)
    .to_vec()
}

/// Arguments for `sbx exec` of one personal tool call in its sandbox `name`: its environment,
/// the declared secrets by name (`sbx` takes their values from its own environment, so they never
/// appear in arguments), and the script under an in-sandbox `timeout`, reading stdin.
pub fn tool_exec_args(name: &str, call: &ToolRun<'_>, timeout_secs: u64) -> Vec<String> {
    let mut args: Vec<String> = ["exec", "-i", "-w", WORKSPACE, "-e", PWD]
        .map(String::from)
        .to_vec();
    let names: Vec<&str> = call.env.iter().map(|(k, _)| k.as_str()).collect();
    args.extend(tool_env_args(&names));
    args.extend([
        name.to_string(),
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

/// Arguments for `sbx exec` that install a tool's requirements into its environment, as in
/// [`crate::sandbox::tool_setup_args`].
pub fn tool_setup_exec_args(name: &str, requirements: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["exec", "-w", TOOL_ENV_MOUNT].map(String::from).to_vec();
    args.extend(tool_env_args(&[]));
    args.extend(
        [
            name,
            "python3",
            "-m",
            "pip",
            "install",
            "--user",
            "--no-warn-script-location",
            "--",
        ]
        .map(String::from),
    );
    args.extend(requirements.iter().cloned());
    args
}

/// The names in `sbx ls --json`: an array of sandboxes (or, before v0.48, `{sandboxes: [...]}`),
/// each with a `name`.
fn parse_names(json: &str) -> Result<Vec<String>> {
    let value: serde_json::Value =
        serde_json::from_str(json.trim()).context("`sbx ls --json` printed no JSON")?;
    let rows = match &value {
        serde_json::Value::Array(rows) => rows,
        serde_json::Value::Object(o) => match o.get("sandboxes") {
            Some(serde_json::Value::Array(rows)) => rows,
            _ => bail!("`sbx ls --json` printed an unexpected object"),
        },
        serde_json::Value::Null => return Ok(Vec::new()),
        _ => bail!("`sbx ls --json` printed an unexpected value"),
    };
    Ok(rows
        .iter()
        .filter_map(|row| match row {
            serde_json::Value::String(name) => Some(name.clone()),
            row => row.get("name")?.as_str().map(String::from),
        })
        .collect())
}

/// Delete a directory the agent wrote to; fine if it isn't there. `remove_dir_all` removes the
/// symlinks in it, not what they point to. A directory the agent made unwritable (`chmod 000`)
/// would stop it, so on a permission error the directories get their owner's permissions back,
/// without following links, and it tries again.
fn remove_tree(dir: &Path) -> std::io::Result<()> {
    fn restore(dir: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                restore(&entry.path())?;
            }
        }
        Ok(())
    }
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            restore(dir)?;
            std::fs::remove_dir_all(dir)
        }
        other => other,
    }
}

fn mount_ro(path: &Path) -> String {
    format!("{}:ro", path.display())
}

/// Like [`crate::sandbox::docker_checked`], for `sbx`.
async fn sbx_checked(bin: &str, args: &[String], timeout: Duration) -> Result<String> {
    let out = docker_output(bin, args, timeout).await?;
    if !out.status.success() {
        bail!(
            "`sbx {}` failed ({}): {}",
            args.first().map(String::as_str).unwrap_or(""),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The names of every sandbox `sbx` knows.
async fn list_sandboxes(bin: &str) -> Result<Vec<String>> {
    let out = sbx_checked(bin, &["ls", "--json"].map(String::from), SBX_CALL_TIMEOUT).await?;
    parse_names(&out)
}

/// Remove sandboxes. Best effort: failures are logged.
async fn remove(bin: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let mut args = vec!["rm".to_string(), "--force".into()];
    args.extend(names.iter().cloned());
    if let Err(e) = sbx_checked(bin, &args, SBX_CALL_TIMEOUT).await {
        tracing::warn!(error = %e, sandboxes = ?names, "removing sandboxes failed");
    }
}

/// Create a microVM, open its network, and link its mounts. Removed again if any step fails.
async fn create(sbx: &SbxConfig, spec: &Spec<'_>, links: &[(&str, &str)]) -> Result<()> {
    let bin = &sbx.bin;
    let started: Result<()> = async {
        sbx_checked(bin, &create_args(sbx, spec), SBX_CREATE_TIMEOUT).await?;
        if let Some(args) = allow_args(spec.name, spec.egress) {
            sbx_checked(bin, &args, SBX_CALL_TIMEOUT).await?;
        }
        if !links.is_empty() {
            sbx_checked(bin, &link_args(spec.name, links), SBX_CALL_TIMEOUT).await?;
        }
        Ok(())
    }
    .await;
    if let Err(e) = started {
        remove(bin, &[spec.name.to_string()]).await;
        return Err(e.context(format!("creating sandbox {}", spec.name)));
    }
    Ok(())
}

/// Is `sbx` there and usable, and (if it won't pull it) is `image` loaded?
async fn check_sbx(sbx: &SbxConfig, image: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    if !Path::new("/dev/kvm").exists() {
        bail!(
            "KVM is not available (no /dev/kvm): the sbx backend needs hardware virtualisation; \
             use SANDBOX_BACKEND=docker on this host"
        );
    }
    sbx_checked(&sbx.bin, &["version".to_string()], SBX_CALL_TIMEOUT)
        .await
        .with_context(|| format!("`{}` is not installed or does not run (SBX_BIN)", sbx.bin))?;
    list_sandboxes(&sbx.bin)
        .await
        .context("the sbx daemon is not usable (has `sbx login` been run for this user?)")?;
    if sbx.pull == "never" {
        let templates = sbx_checked(
            &sbx.bin,
            &["template", "ls", "--json"].map(String::from),
            SBX_CALL_TIMEOUT,
        )
        .await?;
        if !templates.contains(image.trim_end_matches(":latest")) {
            bail!(
                "template {image} is missing and SBX_PULL=never; load it with \
                 `docker save -o template.tar {image} && sbx template load template.tar`"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Sandboxes
// ---------------------------------------------------------------------------------------------

pub struct SbxSandbox {
    sbx: SbxConfig,
    cfg: SandboxConfig,
    name: String,
    run_id: Uuid,
    workspace: PathBuf,
    tool_envs: PathBuf,
    /// The microVM each personal tool called in this run has, by tool name.
    tools: tokio::sync::Mutex<HashMap<String, String>>,
}

impl SbxSandbox {
    /// The microVM for `call`'s tool in this run, created on its first call.
    async fn tool_sandbox(&self, call: &ToolRun<'_>) -> Result<String> {
        let mut tools = self.tools.lock().await;
        if let Some(name) = tools.get(call.name) {
            return Ok(name.clone());
        }
        let name = format!("{TOOL_PREFIX}{}-{}", self.run_id.simple(), tools.len());
        let env = self.tool_envs.join(call.name);
        tokio::fs::create_dir_all(&env)
            .await
            .with_context(|| format!("creating {}", env.display()))?;
        let mounts = [
            self.workspace.display().to_string(),
            mount_ro(call.dir),
            env.display().to_string(),
        ];
        let egress = Egress::hosts(call.hosts);
        let spec = Spec {
            name: &name,
            image: &self.cfg.image,
            memory: &self.cfg.memory,
            cpus: &self.cfg.cpus,
            egress: &egress,
            mounts: &mounts,
        };
        let (workspace, dir, env) = (
            self.workspace.display().to_string(),
            call.dir.display().to_string(),
            env.display().to_string(),
        );
        let links = [
            (workspace.as_str(), WORKSPACE),
            (dir.as_str(), TOOL_MOUNT),
            (env.as_str(), TOOL_ENV_MOUNT),
        ];
        create(&self.sbx, &spec, &links).await?;
        tools.insert(call.name.to_string(), name.clone());
        Ok(name)
    }
}

#[async_trait]
impl Sandbox for SbxSandbox {
    async fn exec(
        &self,
        command: &str,
        timeout: Duration,
        max_output: usize,
    ) -> Result<ExecOutput> {
        let mut cmd = Command::new(&self.sbx.bin);
        cmd.args(exec_args(&self.name, timeout.as_secs().max(1), command));
        match run_capped(cmd, None, timeout + EXEC_BACKSTOP, max_output).await? {
            Some(out) => Ok(out),
            None => {
                // The in-sandbox timeout should have fired long ago, so the microVM is wedged.
                self.destroy().await;
                Err(SandboxFatal("command did not terminate after its timeout".into()).into())
            }
        }
    }

    async fn read_file(&self, path: &str, max_bytes: usize) -> Result<FileContent> {
        check_path(path)?;
        let mut args = ["exec", "-w", WORKSPACE, "-e", PWD, &self.name]
            .map(String::from)
            .to_vec();
        args.extend(head_command(path, max_bytes));
        read_via(&self.sbx.bin, &args, max_bytes).await
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        check_path(path)?;
        let args = [
            "exec",
            "-i",
            "-w",
            WORKSPACE,
            "-e",
            PWD,
            &self.name,
            "sh",
            "-c",
            WRITE_SCRIPT,
            "sh",
            path,
        ]
        .map(String::from);
        write_via_stdin(&self.sbx.bin, &args, path, content).await
    }

    async fn run_tool(
        &self,
        call: &ToolRun<'_>,
        timeout: Duration,
        max_output: usize,
    ) -> Result<ExecOutput> {
        let sandbox = self.tool_sandbox(call).await?;
        let mut cmd = Command::new(&self.sbx.bin);
        cmd.args(tool_exec_args(&sandbox, call, timeout.as_secs().max(1)));
        for (k, v) in call.env {
            cmd.env(k, v);
        }
        match run_capped(cmd, Some(call.input), timeout + EXEC_BACKSTOP, max_output).await? {
            Some(out) => Ok(out),
            None => {
                self.tools.lock().await.remove(call.name);
                remove(&self.sbx.bin, &[sandbox]).await;
                bail!("{} did not finish after its timeout", call.name)
            }
        }
    }

    async fn destroy(&self) {
        // The workspace directory outlives the run so it can be continued.
        let mut names = vec![self.name.clone()];
        names.extend(self.tools.lock().await.drain().map(|(_, name)| name));
        remove(&self.sbx.bin, &names).await;
    }
}

pub struct SbxSandboxFactory {
    sbx: SbxConfig,
    cfg: SandboxConfig,
    /// `cedar-files` exists (as far as this process knows).
    files_ready: tokio::sync::Mutex<bool>,
}

impl SbxSandboxFactory {
    pub fn new(cfg: SandboxConfig, sbx: SbxConfig) -> Self {
        Self {
            sbx,
            cfg,
            files_ready: tokio::sync::Mutex::new(false),
        }
    }

    fn workspaces(&self) -> PathBuf {
        self.sbx.data_dir.join("workspaces")
    }

    fn workspace(&self, workspace_id: Uuid) -> PathBuf {
        self.workspaces().join(workspace_id.to_string())
    }

    fn tool_env(&self, name: &str) -> PathBuf {
        self.sbx.data_dir.join("tool-envs").join(name)
    }

    /// Create `cedar-files` if it isn't there: every workspace, read-only, and no network.
    async fn ensure_files_sandbox(&self) -> Result<()> {
        let mut ready = self.files_ready.lock().await;
        if *ready {
            return Ok(());
        }
        if !list_sandboxes(&self.sbx.bin)
            .await?
            .iter()
            .any(|n| n == FILES_SANDBOX)
        {
            let root = self.workspaces();
            tokio::fs::create_dir_all(&root).await?;
            let mounts = [mount_ro(&root)];
            let spec = Spec {
                name: FILES_SANDBOX,
                image: &self.cfg.image,
                memory: "512m",
                cpus: "1",
                egress: &Egress::None,
                mounts: &mounts,
            };
            create(&self.sbx, &spec, &[]).await?;
        }
        *ready = true;
        Ok(())
    }

    /// Run `script` with `args` in `cedar-files`, after checking the workspace exists.
    async fn run_in_workspace(
        &self,
        workspace_id: Uuid,
        script: &str,
        args: &[&str],
    ) -> Result<std::process::Output> {
        if !tokio::fs::try_exists(self.workspace(workspace_id))
            .await
            .unwrap_or(false)
        {
            return Err(WorkspaceMissing.into());
        }
        self.ensure_files_sandbox().await?;
        let mut argv = ["exec", FILES_SANDBOX, "sh", "-c", script, "sh"]
            .map(String::from)
            .to_vec();
        argv.extend(args.iter().map(|s| s.to_string()));
        let out = docker_output(&self.sbx.bin, &argv, SBX_CREATE_TIMEOUT).await?;
        if !matches!(out.status.code(), Some(0) | Some(2)) {
            // Perhaps someone removed it; the next call checks again.
            *self.files_ready.lock().await = false;
        }
        Ok(out)
    }
}

#[async_trait]
impl SandboxFactory for SbxSandboxFactory {
    async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Sandbox>> {
        let workspace = self.workspace(workspace_id);
        // `sbx` refuses to mount a directory that isn't there. One that is (a continuation) is
        // picked up as it was left.
        tokio::fs::create_dir_all(&workspace)
            .await
            .with_context(|| format!("creating workspace {}", workspace.display()))?;
        let name = format!("{RUN_PREFIX}{run_id}");
        let egress = Egress::from_setting(&self.cfg.network);
        let mounts = [workspace.display().to_string()];
        let spec = Spec {
            name: &name,
            image: &self.cfg.image,
            memory: &self.cfg.memory,
            cpus: &self.cfg.cpus,
            egress: &egress,
            mounts: &mounts,
        };
        let target = workspace.display().to_string();
        create(&self.sbx, &spec, &[(&target, WORKSPACE)]).await?;
        Ok(Arc::new(SbxSandbox {
            sbx: self.sbx.clone(),
            cfg: self.cfg.clone(),
            name,
            run_id,
            workspace,
            tool_envs: self.sbx.data_dir.join("tool-envs"),
            tools: Default::default(),
        }))
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        let names = list_sandboxes(&self.sbx.bin).await?;
        // `cedar-files` too: it is recreated on demand, with the mounts of this configuration.
        let orphans: Vec<String> = names
            .into_iter()
            .filter(|n| {
                n.starts_with(RUN_PREFIX)
                    || n.starts_with(TOOL_PREFIX)
                    || n.starts_with(SETUP_PREFIX)
                    || n == FILES_SANDBOX
            })
            .collect();
        remove(&self.sbx.bin, &orphans).await;
        *self.files_ready.lock().await = false;
        Ok(orphans.iter().filter(|n| n.starts_with(RUN_PREFIX)).count())
    }

    async fn remove_workspace(&self, workspace_id: Uuid) -> Result<()> {
        let dir = self.workspace(workspace_id);
        tokio::task::spawn_blocking(move || remove_tree(&dir))
            .await?
            .with_context(|| format!("removing workspace {workspace_id}"))
    }

    async fn list_workspaces(&self) -> Result<Vec<Uuid>> {
        let mut dir = match tokio::fs::read_dir(self.workspaces()).await {
            Ok(dir) => dir,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut ids = Vec::new();
        while let Some(entry) = dir.next_entry().await? {
            if let Some(id) = entry.file_name().to_str().and_then(|n| n.parse().ok()) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    async fn list_workspace_files(&self, workspace_id: Uuid) -> Result<WorkspaceListing> {
        let root = self.workspace(workspace_id).display().to_string();
        let out = self
            .run_in_workspace(workspace_id, &list_script(), &[&root])
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
        let root = self.workspace(workspace_id).display().to_string();
        let limit = (max_bytes + 1).to_string();
        let out = self
            .run_in_workspace(workspace_id, READ_WORKSPACE_SCRIPT, &[path, &limit, &root])
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
        check_sbx(&self.sbx, &self.cfg.image).await
    }

    async fn setup_tool(&self, name: &str, requirements: &[String]) -> Result<String> {
        if requirements.is_empty() {
            return Ok(String::new());
        }
        let env = self.tool_env(name);
        tokio::fs::create_dir_all(&env)
            .await
            .with_context(|| format!("creating {}", env.display()))?;
        let sandbox = format!("{SETUP_PREFIX}{}", Uuid::new_v4().simple());
        let mounts = [env.display().to_string()];
        let egress = Egress::hosts(PIP_HOSTS);
        let spec = Spec {
            name: &sandbox,
            image: &self.cfg.image,
            memory: &self.cfg.memory,
            cpus: &self.cfg.cpus,
            egress: &egress,
            mounts: &mounts,
        };
        let target = env.display().to_string();
        create(&self.sbx, &spec, &[(&target, TOOL_ENV_MOUNT)]).await?;
        let out = docker_output(
            &self.sbx.bin,
            &tool_setup_exec_args(&sandbox, requirements),
            SETUP_TIMEOUT,
        )
        .await;
        remove(&self.sbx.bin, &[sandbox]).await;
        let out = out?;
        let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
        if !out.status.success() {
            bail!("pip install failed ({}): {}", out.status, text(&out.stderr));
        }
        Ok(text(&out.stdout))
    }

    async fn remove_tool_env(&self, name: &str) -> Result<()> {
        let dir = self.tool_env(name);
        Ok(tokio::task::spawn_blocking(move || remove_tree(&dir)).await??)
    }
}

// ---------------------------------------------------------------------------------------------
// Browser
// ---------------------------------------------------------------------------------------------

/// A run's browser in a microVM of its own. Calls are `sbx exec`s of `cedar-browser call`, as on
/// Docker.
pub struct SbxBrowserFactory {
    sbx: SbxConfig,
    cfg: BrowserConfig,
}

impl SbxBrowserFactory {
    pub fn new(cfg: BrowserConfig, sbx: SbxConfig) -> Self {
        Self { sbx, cfg }
    }
}

fn browser_name(run_id: Uuid) -> String {
    format!("{BROWSER_PREFIX}{run_id}")
}

#[async_trait]
impl BrowserFactory for SbxBrowserFactory {
    async fn create(&self, run_id: Uuid) -> Result<Arc<dyn Browser>> {
        let name = browser_name(run_id);
        let egress = Egress::from_setting(&self.cfg.network);
        let spec = Spec {
            name: &name,
            image: &self.cfg.image,
            memory: &self.cfg.memory,
            cpus: &self.cfg.cpus,
            egress: &egress,
            mounts: &[],
        };
        create(&self.sbx, &spec, &[]).await?;
        let start = ["exec", &name, "sh", "-c", START_BROWSER].map(String::from);
        if let Err(e) = sbx_checked(&self.sbx.bin, &start, SBX_CALL_TIMEOUT).await {
            self.destroy(run_id).await;
            return Err(e.context("starting the browser"));
        }
        Ok(Arc::new(DockerBrowser::new(&self.sbx.bin, &name)))
    }

    async fn destroy(&self, run_id: Uuid) {
        remove(&self.sbx.bin, &[browser_name(run_id)]).await;
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        let orphans: Vec<String> = list_sandboxes(&self.sbx.bin)
            .await?
            .into_iter()
            .filter(|n| n.starts_with(BROWSER_PREFIX))
            .collect();
        remove(&self.sbx.bin, &orphans).await;
        Ok(orphans.len())
    }

    async fn health(&self) -> Result<()> {
        check_sbx(&self.sbx, &self.cfg.image).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sbx() -> SbxConfig {
        SbxConfig {
            bin: "sbx".into(),
            data_dir: "/var/lib/cedar/sbx".into(),
            pull: "missing".into(),
        }
    }

    fn spec<'a>(egress: &'a Egress, mounts: &'a [String]) -> Spec<'a> {
        Spec {
            name: "cedar-run-x",
            image: "img:1",
            memory: "512m",
            cpus: "0.5",
            egress,
            mounts,
        }
    }

    #[test]
    fn create_args_lock_the_microvm_down() {
        let mounts = ["/var/lib/cedar/sbx/workspaces/w".to_string()];
        let args = create_args(&sbx(), &spec(&Egress::None, &mounts));
        let joined = args.join(" ");
        assert!(joined.starts_with("create --name cedar-run-x --template img:1 --pull missing"));
        assert!(
            joined.contains("--memory 512m"),
            "sbx's default is half the host: {joined}"
        );
        assert!(joined.contains("--cpus 1"), "rounded up: {joined}");
        assert!(joined.contains("--skills off"));
        assert!(joined.contains("--deny-network **"));
        // The agent, then the mounts, come last.
        assert_eq!(
            &args[args.len() - 2..],
            ["shell", "/var/lib/cedar/sbx/workspaces/w"]
        );
        assert!(allow_args("cedar-run-x", &Egress::None).is_none());

        // Allowing anything means no deny rule, which no allow rule could reopen.
        for egress in [Egress::All, Egress::Hosts(vec!["api.example.com".into()])] {
            let joined = create_args(&sbx(), &spec(&egress, &mounts)).join(" ");
            assert!(!joined.contains("--deny-network"), "{joined}");
        }
    }

    #[test]
    fn network_settings_become_policies() {
        assert_eq!(Egress::from_setting("none"), Egress::None);
        assert_eq!(Egress::from_setting(""), Egress::None);
        assert_eq!(Egress::from_setting("bridge"), Egress::All);
        assert_eq!(Egress::from_setting("**"), Egress::All);
        assert_eq!(
            Egress::from_setting("pypi.org, *.github.com,"),
            Egress::Hosts(vec!["pypi.org".into(), "*.github.com".into()])
        );
        assert_eq!(
            allow_args("s", &Egress::All).unwrap(),
            ["policy", "allow", "network", "--sandbox", "s", "**"]
        );
        assert_eq!(
            allow_args("s", &Egress::from_setting("a.com,b.com")).unwrap(),
            [
                "policy",
                "allow",
                "network",
                "--sandbox",
                "s",
                "a.com,b.com"
            ]
        );
        assert_eq!(Egress::hosts(Vec::<String>::new()), Egress::None);
    }

    #[test]
    fn cpus_are_whole() {
        assert_eq!(whole_cpus("0.5"), "1");
        assert_eq!(whole_cpus("2"), "2");
        assert_eq!(whole_cpus("1.5"), "2");
        assert_eq!(whole_cpus("0"), "1");
    }

    #[test]
    fn exec_args_keep_the_command_as_a_single_argv_element() {
        let cmd = "echo 'hi'; rm -rf / # $(whoami)";
        assert_eq!(
            exec_args("c1", 30, cmd),
            [
                "exec",
                "-w",
                "/workspace",
                "-e",
                "PWD=/workspace",
                "c1",
                "timeout",
                "-k",
                "2",
                "30",
                "sh",
                "-c",
                cmd
            ]
        );
    }

    #[test]
    fn links_run_as_root_with_paths_as_arguments() {
        let args = link_args("c1", &[("/data/ws/a b", "/workspace")]);
        assert_eq!(&args[..4], ["exec", "-u", "root", "c1"]);
        assert_eq!(
            &args[args.len() - 3..],
            ["sh", "/data/ws/a b", "/workspace"]
        );
    }

    #[test]
    fn a_tool_call_gets_its_environment_and_secrets_by_name() {
        let env = vec![("TANDEM_PASSWORD".to_string(), "hunter2".to_string())];
        let call = ToolRun {
            name: "tandem_glucose",
            dir: Path::new("/home/me/tools/tandem_glucose"),
            entry: "tool.py",
            input: b"{}",
            env: &env,
            hosts: &[],
        };
        let args = tool_exec_args("cedar-tool-x-0", &call, 30);
        let joined = args.join(" ");
        assert!(joined.starts_with("exec -i -w /workspace -e PWD=/workspace "));
        assert!(joined.contains("-e PYTHONUSERBASE=/opt/cedar/env/.local"));
        assert!(joined.contains("-e TANDEM_PASSWORD "));
        assert!(!joined.contains("hunter2"));
        assert_eq!(
            &args[args.len() - 8..],
            [
                "cedar-tool-x-0",
                "timeout",
                "-k",
                "2",
                "30",
                "python3",
                "--",
                "/opt/cedar/tool/tool.py"
            ]
        );

        let setup = tool_setup_exec_args("cedar-setup-x", &["tconnectsync>=3.0".to_string()]);
        assert!(setup.join(" ").ends_with(
            "cedar-setup-x python3 -m pip install --user --no-warn-script-location -- tconnectsync>=3.0"
        ));
    }

    #[test]
    fn sandbox_listings_are_parsed() {
        let current = r#"[{"name":"cedar-run-1","status":"running"},{"name":"mine"}]"#;
        assert_eq!(parse_names(current).unwrap(), ["cedar-run-1", "mine"]);
        let old = r#"{"sandboxes":[{"name":"cedar-files"}]}"#;
        assert_eq!(parse_names(old).unwrap(), ["cedar-files"]);
        assert!(parse_names("[]").unwrap().is_empty());
        assert!(parse_names("null").unwrap().is_empty());
        assert!(parse_names("NAME STATUS").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn workspaces_are_removed_even_when_the_agent_locked_a_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cedar-sbx-rm-{}", Uuid::new_v4()));
        let outside = dir.with_extension("outside");
        std::fs::create_dir_all(dir.join("locked/deeper")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(dir.join("locked/deeper/f"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        std::fs::set_permissions(dir.join("locked"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        remove_tree(&dir).unwrap();
        assert!(!dir.exists());
        assert!(outside.exists(), "a link's target is left alone");
        remove_tree(&dir).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn a_read_only_mount_is_marked() {
        assert_eq!(mount_ro(Path::new("/srv/ws")), "/srv/ws:ro");
    }
}
