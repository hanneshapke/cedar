//! What setting Docker up from the setup page takes on both Windows (winsetup.rs) and the Mac
//! (macsetup.rs): downloading Docker Desktop with its progress on the page, and following the part
//! that runs with administrator rights through the `STEP:` and `ERROR:` lines it writes to its log.

#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use crate::app::{AppState, SetupStatus};
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const STEP: &str = "STEP: ";
pub const ERROR: &str = "ERROR: ";

/// The text of the last log line that starts with `tag`. PowerShell may start the file with a
/// byte order mark.
pub fn last_tagged(log: &str, tag: &str) -> Option<String> {
    log.lines()
        .rev()
        .map(|l| l.trim_start_matches('\u{feff}').trim())
        .find_map(|l| l.strip_prefix(tag))
        .map(str::to_string)
}

pub fn progress(got: u64, total: Option<u64>) -> String {
    let mb = |b: u64| b / 1_000_000;
    match total {
        Some(total) => format!(
            "Downloading Docker Desktop: {} of {} MB",
            mb(got),
            mb(total)
        ),
        None => format!("Downloading Docker Desktop: {} MB", mb(got)),
    }
}

/// Docker Desktop's installer from `url`, saved as `name` in the user's temp folder. One
/// downloaded by an earlier try that failed is used again if it is complete.
pub async fn download(state: &AppState, url: &str, name: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("cedar-docker-setup");
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating {}", dir.display()))?;
    let dest = dir.join(name);

    state.set_docker_setup(SetupStatus::Running(progress(0, None)));
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()?;
    let resp = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .context("downloading Docker Desktop; check the internet connection")?;
    let total = resp.content_length();
    if let (Some(total), Ok(meta)) = (total, tokio::fs::metadata(&dest).await) {
        if meta.len() == total {
            return Ok(dest);
        }
    }

    let part = dir.join(format!("{name}.part"));
    let mut file = tokio::fs::File::create(&part)
        .await
        .with_context(|| format!("creating {}", part.display()))?;
    let mut stream = resp.bytes_stream();
    let mut got = 0u64;
    let mut shown = 0u64;
    loop {
        let chunk = match tokio::time::timeout(Duration::from_secs(60), stream.next()).await {
            Err(_) => {
                bail!("the Docker Desktop download stalled; check the connection and try again")
            }
            Ok(None) => break,
            Ok(Some(chunk)) => chunk.context("downloading Docker Desktop")?,
        };
        file.write_all(&chunk).await?;
        got += chunk.len() as u64;
        if got - shown >= 5_000_000 {
            shown = got;
            state.set_docker_setup(SetupStatus::Running(progress(got, total)));
        }
    }
    file.flush().await?;
    drop(file);
    if total.is_some_and(|t| t != got) {
        bail!("the Docker Desktop download was cut off; try again");
    }
    tokio::fs::rename(&part, &dest).await?;
    Ok(dest)
}

/// Waits for `child`, the elevated part, showing the last `STEP:` line in `log` on the page while
/// it runs. Its exit code and what it wrote to stderr (which must be piped).
pub async fn follow(
    state: &AppState,
    mut child: tokio::process::Child,
    log: &Path,
) -> Result<(Option<i32>, String)> {
    let mut stderr = child.stderr.take().expect("piped");
    let errors = tokio::spawn(async move {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text).await;
        text
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60 * 60);
    let status = loop {
        tokio::select! {
            status = child.wait() => break status?,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                let step = std::fs::read_to_string(log).ok().and_then(|t| last_tagged(&t, STEP));
                if let Some(step) = step {
                    state.set_docker_setup(SetupStatus::Running(step));
                }
                if tokio::time::Instant::now() > deadline {
                    let _ = child.kill().await;
                    bail!("setting up Docker took more than an hour; docker-setup.log in the logs folder says how far it got");
                }
            }
        }
    };
    Ok((status.code(), errors.await.unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_lines() {
        let log = "\u{feff}STEP: Turning on WSL\nERROR: first\nSTEP: Installing\nERROR: the installer stopped\n";
        assert_eq!(last_tagged(log, STEP).as_deref(), Some("Installing"));
        assert_eq!(
            last_tagged(log, ERROR).as_deref(),
            Some("the installer stopped")
        );
        assert_eq!(
            last_tagged("\u{feff}STEP: one\n", STEP).as_deref(),
            Some("one")
        );
        assert_eq!(last_tagged("installer output\n", STEP), None);
    }

    #[test]
    fn download_progress() {
        assert_eq!(
            progress(123_456_789, Some(600_000_000)),
            "Downloading Docker Desktop: 123 of 600 MB"
        );
        assert_eq!(progress(0, None), "Downloading Docker Desktop: 0 MB");
    }
}
