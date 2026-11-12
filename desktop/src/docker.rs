//! Finding and driving the `docker` CLI from an app that Finder started without the user's PATH.
//! Docker Desktop, OrbStack, Colima (via Homebrew), and Rancher Desktop all provide the CLI on a
//! Mac; Docker Desktop and Rancher Desktop on Windows. The app only needs the daemon running and
//! the images present, cedar does the rest. When the daemon isn't running, the app it comes with
//! is started.

use crate::i18n::tf;
use crate::proc;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// A docker CLI and the `PATH` to run it with. The CLI itself execs credential helpers
/// (`docker-credential-desktop`, `docker-credential-osxkeychain`, `docker-credential-wincred`)
/// from `PATH`, even for a public pull, so the directories those live in have to be on it.
#[derive(Clone, Debug)]
pub struct DockerCli {
    pub bin: PathBuf,
    pub path_env: String,
}

/// Directories the CLI is usually installed in, in the order to try.
#[cfg(not(windows))]
fn candidate_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs = vec![
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/Applications/Docker.app/Contents/Resources/bin"),
    ];
    if let Some(home) = &home {
        dirs.push(home.join(".orbstack/bin"));
        dirs.push(home.join(".rd/bin"));
        dirs.push(home.join(".docker/bin"));
    }
    dirs
}

/// Docker Desktop's CLI and credential helpers, then Rancher Desktop's (per-user install first).
#[cfg(windows)]
fn candidate_dirs() -> Vec<PathBuf> {
    let env_dir = |var: &str| std::env::var_os(var).map(PathBuf::from);
    let mut dirs = Vec::new();
    if let Some(pf) = env_dir("ProgramFiles") {
        dirs.push(pf.join(r"Docker\Docker\resources\bin"));
    }
    if let Some(local) = env_dir("LOCALAPPDATA") {
        dirs.push(local.join(r"Programs\Rancher Desktop\resources\resources\win32\bin"));
    }
    if let Some(pf) = env_dir("ProgramFiles") {
        dirs.push(pf.join(r"Rancher Desktop\resources\resources\win32\bin"));
    }
    dirs
}

/// The first `docker` found in the usual places, then on this process's own `PATH`.
pub fn find() -> Option<DockerCli> {
    let mut dirs = candidate_dirs();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.iter()
        .find_map(|d| proc::executable_in(d, "docker"))
        .map(|bin| DockerCli {
            path_env: path_env(Some(&bin)),
            bin,
        })
}

/// A `PATH` with the CLI's own directory first, then the usual places, then the system default.
/// On Windows the app inherits the user's `PATH` (it is not started from Finder), and that goes
/// before the system directories.
pub fn path_env(bin: Option<&Path>) -> String {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = bin.and_then(Path::parent) {
        dirs.push(dir.to_path_buf());
    }
    dirs.extend(candidate_dirs());
    #[cfg(not(windows))]
    for d in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        dirs.push(PathBuf::from(d));
    }
    #[cfg(windows)]
    {
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        let root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        dirs.push(root.join("System32"));
        dirs.push(root);
    }
    let mut seen = Vec::new();
    for d in dirs {
        if !seen.contains(&d) {
            seen.push(d);
        }
    }
    // A directory with the separator in its name can't be on PATH; leave such a one out.
    seen.retain(|d| std::env::join_paths([d]).is_ok());
    std::env::join_paths(seen)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Docker Desktop's own program on Windows, if it is installed.
#[cfg(windows)]
pub fn desktop_app() -> Option<PathBuf> {
    let app =
        PathBuf::from(std::env::var_os("ProgramFiles")?).join(r"Docker\Docker\Docker Desktop.exe");
    app.is_file().then_some(app)
}

/// Only Linux asks (see `provider_app`); a Mac finds the app from the CLI instead.
#[cfg(not(windows))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn desktop_app() -> Option<PathBuf> {
    None
}

/// The app to start when the daemon isn't answering. On Windows that is Docker Desktop. On a Mac
/// it is the app `cli` comes from (Docker Desktop, OrbStack, Rancher Desktop), so the one the user
/// chose is started; a CLI outside an app (Colima's, from Homebrew) has none.
pub fn provider_app(cli: &DockerCli) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        app_bundle(&std::fs::canonicalize(&cli.bin).ok()?)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cli;
        desktop_app()
    }
}

/// The outermost `.app` bundle `path` is in: `/usr/local/bin/docker` links into
/// `/Applications/Docker.app/Contents/Resources/bin`, `~/.orbstack/bin/docker` into
/// `/Applications/OrbStack.app`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_bundle(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
        .last()
        .map(Path::to_path_buf)
}

/// Starts a GUI program (on a Mac, an app bundle) and leaves it running after the app quits.
pub fn launch(program: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut cmd = proc::std_command("/usr/bin/open");
        cmd.arg(program);
        cmd
    };
    #[cfg(not(target_os = "macos"))]
    let mut cmd = proc::std_command(program);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("running {}", program.display()))?;
    Ok(())
}

impl DockerCli {
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = proc::command(&self.bin);
        cmd.args(args)
            .env("PATH", &self.path_env)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }

    async fn output(&self, args: &[&str], timeout: Duration) -> Result<String> {
        let out = tokio::time::timeout(timeout, self.command(args).output())
            .await
            .map_err(|_| anyhow!("`docker {}` did not answer within {:?}", args.join(" "), timeout))?
            .with_context(|| format!("running {}", self.bin.display()))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            bail!(
                "`docker {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )
        }
    }

    /// The daemon's version, or why it can't be reached.
    pub async fn server_version(&self) -> Result<String> {
        self.output(
            &["info", "--format", "{{.ServerVersion}}"],
            Duration::from_secs(20),
        )
        .await
    }

    /// [`Self::server_version`], asked again until the daemon answers or `within` has passed.
    pub async fn wait_until_answering(&self, within: Duration) -> Result<String> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            match self.server_version().await {
                Ok(version) => return Ok(version),
                Err(e) if tokio::time::Instant::now() >= deadline => return Err(e),
                Err(_) => tokio::time::sleep(Duration::from_secs(3)).await,
            }
        }
    }

    pub async fn image_present(&self, image: &str) -> bool {
        self.output(
            &["image", "inspect", "--format", "{{.Id}}", image],
            Duration::from_secs(20),
        )
        .await
        .is_ok()
    }

    /// `docker pull`, reporting each progress line as it comes, with how far the pull is by then.
    /// Large images take minutes.
    pub async fn pull(
        &self,
        image: &str,
        mut on_progress: impl FnMut(String, PullProgress),
    ) -> Result<()> {
        let mut child = self
            .command(&["pull", image])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("running {}", self.bin.display()))?;
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let mut lines = BufReader::new(stdout).lines();
        let errors = tokio::spawn(async move {
            let mut text = String::new();
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                text.push_str(&line);
                text.push('\n');
            }
            text
        });
        let mut layers = PullLayers::default();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45 * 60);
        loop {
            let next = tokio::time::timeout_at(deadline, lines.next_line()).await;
            match next {
                Ok(Ok(Some(line))) => {
                    let line = line.trim();
                    if !line.is_empty() {
                        layers.read(line);
                        on_progress(line.to_string(), layers.progress());
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(e)) => return Err(e).context("reading docker pull output"),
                Err(_) => {
                    let _ = child.kill().await;
                    bail!(tf("images.pull_slow", &[("image", image)]));
                }
            }
        }
        let status = child.wait().await?;
        if status.success() {
            Ok(())
        } else {
            let stderr = errors.await.unwrap_or_default();
            let stderr = stderr.trim();
            if stderr.contains("denied") || stderr.contains("not found") || stderr.contains("manifest unknown") {
                bail!(tf("images.unpublished", &[("image", image), ("stderr", stderr)]));
            }
            bail!(tf("images.pull_failed", &[("image", image), ("stderr", stderr)]))
        }
    }
}

/// How far a `docker pull` is, in layers. Without a terminal `docker pull` prints no byte counts,
/// only a line when a layer changes state, so layers are the finest measure there is.
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct PullProgress {
    /// The layers the image has; 0 until docker has listed them.
    pub layers: usize,
    /// Of those, the ones downloaded (or already on disk).
    pub downloaded: usize,
    /// Of those, the ones unpacked too, which is the second half of a layer's work.
    pub unpacked: usize,
}

/// Follows the layers through the lines of a `docker pull`: `<id>: Pulling fs layer` for each
/// one first, then `<id>: Download complete` and `<id>: Pull complete` as they come in.
#[derive(Default)]
struct PullLayers {
    /// Each layer by its id, with how many of its two steps (download, unpack) are done.
    steps: HashMap<String, u8>,
}

impl PullLayers {
    fn read(&mut self, line: &str) {
        let Some((id, status)) = line.split_once(": ") else {
            return;
        };
        let steps = match status.trim() {
            "Pulling fs layer" | "Waiting" => 0,
            "Download complete" => 1,
            "Pull complete" | "Already exists" => 2,
            _ => return,
        };
        let layer = self.steps.entry(id.to_string()).or_insert(0);
        *layer = (*layer).max(steps);
    }

    fn progress(&self) -> PullProgress {
        PullProgress {
            layers: self.steps.len(),
            downloaded: self.steps.values().filter(|s| **s >= 1).count(),
            unpacked: self.steps.values().filter(|s| **s >= 2).count(),
        }
    }
}

#[cfg(test)]
mod pull_tests {
    use super::*;

    #[test]
    fn layers_are_counted_from_the_pull_output() {
        let mut layers = PullLayers::default();
        assert_eq!(layers.progress(), PullProgress::default());
        for line in [
            "0.4.2: Pulling from hanneshapke/cedar-sandbox",
            "aaaaaaaaaaaa: Already exists",
            "bbbbbbbbbbbb: Pulling fs layer",
            "cccccccccccc: Pulling fs layer",
            "dddddddddddd: Pulling fs layer",
            "dddddddddddd: Waiting",
            "bbbbbbbbbbbb: Verifying Checksum",
            "bbbbbbbbbbbb: Download complete",
            "cccccccccccc: Download complete",
            "bbbbbbbbbbbb: Pull complete",
        ] {
            layers.read(line);
        }
        assert_eq!(layers.progress(), PullProgress { layers: 4, downloaded: 3, unpacked: 2 });
        // A late "Waiting" does not take a finished layer back.
        layers.read("bbbbbbbbbbbb: Waiting");
        assert_eq!(layers.progress().unpacked, 2);
        for line in [
            "cccccccccccc: Pull complete",
            "dddddddddddd: Pull complete",
            "Digest: sha256:0123",
            "Status: Downloaded newer image for ghcr.io/hanneshapke/cedar-sandbox:0.4.2",
        ] {
            layers.read(line);
        }
        assert_eq!(layers.progress(), PullProgress { layers: 4, downloaded: 4, unpacked: 4 });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn path_env_puts_the_cli_first_and_dedups() {
        let p = path_env(Some(Path::new("/opt/homebrew/bin/docker")));
        let dirs: Vec<&str> = p.split(':').collect();
        assert_eq!(dirs[0], "/opt/homebrew/bin");
        assert_eq!(dirs.iter().filter(|d| **d == "/opt/homebrew/bin").count(), 1);
        assert!(dirs.contains(&"/usr/bin"));
    }

    #[test]
    fn app_bundles() {
        assert_eq!(
            app_bundle(Path::new("/Applications/Docker.app/Contents/Resources/bin/docker")),
            Some(PathBuf::from("/Applications/Docker.app"))
        );
        assert_eq!(
            app_bundle(Path::new(
                "/Applications/OrbStack.app/Contents/MacOS/Helper.app/Contents/xbin/docker"
            )),
            Some(PathBuf::from("/Applications/OrbStack.app"))
        );
        assert_eq!(
            app_bundle(Path::new("/opt/homebrew/Cellar/docker/27.3.1/bin/docker")),
            None
        );
    }
}
