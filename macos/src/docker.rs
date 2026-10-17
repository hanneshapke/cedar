//! Finding and driving the `docker` CLI from an app that Finder started without the user's PATH.
//! Docker Desktop, OrbStack, Colima (via Homebrew), and Rancher Desktop all provide the CLI; the
//! app only needs the daemon running and the images present, cedar does the rest.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// A docker CLI and the `PATH` to run it with. The CLI itself execs credential helpers
/// (`docker-credential-desktop`, `docker-credential-osxkeychain`) from `PATH`, even for a public
/// pull, so the directories those live in have to be on it.
#[derive(Clone, Debug)]
pub struct DockerCli {
    pub bin: PathBuf,
    pub path_env: String,
}

/// Directories the CLI is usually installed in, in the order to try.
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

/// The first `docker` found in the usual places, then on this process's own `PATH`.
pub fn find() -> Option<DockerCli> {
    let mut dirs = candidate_dirs();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.iter()
        .map(|d| d.join("docker"))
        .find(|p| is_executable(p))
        .map(|bin| DockerCli {
            path_env: path_env(Some(&bin)),
            bin,
        })
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// A `PATH` with the CLI's own directory first, then the usual places, then the system default.
pub fn path_env(bin: Option<&Path>) -> String {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = bin.and_then(Path::parent) {
        dirs.push(dir.to_path_buf());
    }
    dirs.extend(candidate_dirs());
    for d in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        dirs.push(PathBuf::from(d));
    }
    let mut seen = Vec::new();
    for d in dirs {
        if !seen.contains(&d) {
            seen.push(d);
        }
    }
    std::env::join_paths(seen)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/usr/bin:/bin".into())
}

impl DockerCli {
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.bin);
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

    pub async fn image_present(&self, image: &str) -> bool {
        self.output(
            &["image", "inspect", "--format", "{{.Id}}", image],
            Duration::from_secs(20),
        )
        .await
        .is_ok()
    }

    /// `docker pull`, reporting each progress line as it comes. Large images take minutes.
    pub async fn pull(&self, image: &str, mut on_progress: impl FnMut(String)) -> Result<()> {
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45 * 60);
        loop {
            let next = tokio::time::timeout_at(deadline, lines.next_line()).await;
            match next {
                Ok(Ok(Some(line))) => {
                    let line = line.trim();
                    if !line.is_empty() {
                        on_progress(line.to_string());
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(e)) => return Err(e).context("reading docker pull output"),
                Err(_) => {
                    let _ = child.kill().await;
                    bail!("pulling {image} took longer than 45 minutes; check the connection and try again");
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
                bail!(
                    "{image} could not be pulled ({stderr}). Either this version's images have not been \
                     published yet or the package is private on GitHub; see macos/README.md"
                );
            }
            bail!("pulling {image} failed: {stderr}")
        }
    }
}

#[cfg(test)]
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
}
