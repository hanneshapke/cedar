//! Update checks for a self-hosted cedar-chat (and the cedar next to it): every few hours the
//! latest GitHub Release is looked up, and the page's banner (`static/update.js`) shows it when
//! it is newer than this build. On Linux, where both servers run under systemd as an unprivileged
//! user, an admin's **Update** drops a request file into `CHAT_UPDATE_DIR`; a root-owned
//! `cedar-update` unit (scripts/cedar-update.sh) picks it up, downloads and checks the release,
//! installs it and restarts both services, writing its progress to `status.json` there.
//!
//! The desktop app (macOS, Windows) runs this server with `CHAT_UPDATE_CHECK=off` and updates
//! itself instead (desktop/src/update.rs).

use crate::api::AppState;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::header::{ETAG, IF_NONE_MATCH, USER_AGENT};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The latest published release of this repository.
pub const DEFAULT_URL: &str = "https://api.github.com/repos/hanneshapke/cedar/releases/latest";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The first check waits a little, so a restart loop doesn't hammer GitHub.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// "Check now" more often than this answers with the last result.
const MIN_INTERVAL: Duration = Duration::from_secs(60);
/// An install the helper reports as running for longer than this is taken to have died.
const STALE_INSTALL: Duration = Duration::from_secs(15 * 60);
/// The file the `cedar-update.path` unit watches, and the one its helper reports in.
pub const REQUEST_FILE: &str = "request";
pub const STATUS_FILE: &str = "status.json";

/// A release newer or not, as the page shows it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes_url: String,
    pub published_at: Option<String>,
    /// The tarball for this machine and `checksums.txt`, when the release has them.
    pub tarball_url: Option<String>,
    pub checksums_url: Option<String>,
}

#[derive(Default)]
struct Checked {
    latest: Option<Release>,
    etag: Option<String>,
    at: Option<DateTime<Utc>>,
    /// When the last check was tried, successful or not (for "Check now").
    tried: Option<SystemTime>,
    error: Option<String>,
}

/// What the background check learned; kept in [`AppState`].
#[derive(Default)]
pub struct Updates {
    checked: Mutex<Checked>,
}

/// The part of GitHub's release object read here.
#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// `1.2.3` or `v1.2.3`; the release workflow allows nothing else.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let v = (next()?, next()?, next()?);
    parts.next().is_none().then_some(v)
}

/// Whether `candidate` is a later version than `current`; anything unreadable is not.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(v)) => c > v,
        _ => false,
    }
}

/// The Linux tarball's name for a CPU (`std::env::consts::ARCH`), as the release workflow names it.
pub fn linux_tarball(version: &str, arch: &str) -> Option<String> {
    let arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    Some(format!("Cedar-{version}-linux-{arch}.tar.gz"))
}

fn release_from(gh: GithubRelease) -> Result<Release> {
    if gh.draft || gh.prerelease {
        bail!("the latest release is a draft or pre-release");
    }
    let (major, minor, patch) = parse_version(&gh.tag_name).ok_or_else(|| anyhow!("unreadable release tag {:?}", gh.tag_name))?;
    let version = format!("{major}.{minor}.{patch}");
    let url = |name: &str| gh.assets.iter().find(|a| a.name == name).map(|a| a.browser_download_url.clone());
    let tarball_url = if cfg!(target_os = "linux") { linux_tarball(&version, std::env::consts::ARCH).and_then(|n| url(&n)) } else { None };
    Ok(Release { checksums_url: url("checksums.txt"), tarball_url, version, notes_url: gh.html_url, published_at: gh.published_at })
}

/// Looks the latest release up once (unless `force` is false and it was looked up a minute ago).
pub async fn check(state: &AppState, force: bool) -> Result<()> {
    let etag = {
        let mut checked = state.updates.checked.lock().unwrap();
        let recent = checked.tried.and_then(|t| t.elapsed().ok()).is_some_and(|e| e < MIN_INTERVAL);
        if recent && !force {
            return Ok(());
        }
        checked.tried = Some(SystemTime::now());
        checked.etag.clone()
    };
    let result = fetch(state, etag).await;
    let mut checked = state.updates.checked.lock().unwrap();
    match result {
        Ok(None) => {
            checked.at = Some(Utc::now());
            checked.error = None;
            Ok(())
        }
        Ok(Some((release, etag))) => {
            checked.latest = Some(release);
            checked.etag = etag;
            checked.at = Some(Utc::now());
            checked.error = None;
            Ok(())
        }
        Err(e) => {
            checked.error = Some(format!("{e:#}"));
            Err(e)
        }
    }
}

/// The release, or `None` when it is unchanged since the `etag` (GitHub doesn't count those
/// against the rate limit).
async fn fetch(state: &AppState, etag: Option<String>) -> Result<Option<(Release, Option<String>)>> {
    let mut req = state
        .http
        .get(&state.cfg.update_url)
        .header(USER_AGENT, format!("cedar-chat/{VERSION}"))
        .header("accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(30));
    if let Some(etag) = etag {
        req = req.header(IF_NONE_MATCH, etag);
    }
    let resp = req.send().await.context("looking up the latest release")?;
    if resp.status() == StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail!("looking up the latest release: {}", resp.status());
    }
    let etag = resp.headers().get(ETAG).and_then(|v| v.to_str().ok()).map(String::from);
    let gh: GithubRelease = resp.json().await.context("reading the latest release")?;
    Ok(Some((release_from(gh)?, etag)))
}

/// Checks a minute after the start and then every six hours, until `stop`; nothing when
/// `CHAT_UPDATE_CHECK=off`.
pub fn start(state: AppState, stop: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        if !state.cfg.update_check {
            return;
        }
        let mut wait = FIRST_CHECK;
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
            if let Err(e) = check(&state, true).await {
                tracing::warn!(error = %format!("{e:#}"), "update check failed");
            }
            wait = EVERY;
        }
    })
}

/// The helper's progress, from `status.json`, or `requested` while the request waits for it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Install {
    /// `idle`, `requested`, `downloading`, `verifying`, `installing`, `restarting`, `done`, `failed`.
    pub state: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

impl Install {
    fn idle() -> Self {
        Install { state: "idle".into(), version: None, detail: None }
    }

    fn running(&self) -> bool {
        !matches!(self.state.as_str(), "idle" | "done" | "failed")
    }
}

fn modified_ago(path: &Path) -> Option<Duration> {
    std::fs::metadata(path).ok()?.modified().ok()?.elapsed().ok()
}

fn read_install(dir: &Path) -> Install {
    let request = dir.join(REQUEST_FILE);
    if request.exists() && modified_ago(&request).is_none_or(|age| age < STALE_INSTALL) {
        let version = std::fs::read_to_string(&request)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["version"].as_str().map(String::from));
        return Install { state: "requested".into(), version, detail: None };
    }
    let path = dir.join(STATUS_FILE);
    let Some(mut install) = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Install>(&s).ok()) else {
        return Install::idle();
    };
    if install.running() && modified_ago(&path).is_some_and(|age| age > STALE_INSTALL) {
        install.state = "failed".into();
        install.detail = Some("the update stopped without finishing; see `journalctl -u cedar-update`".into());
    }
    install
}

/// `GET /api/update`.
#[derive(Serialize, Debug)]
pub struct Status {
    pub enabled: bool,
    pub current: &'static str,
    pub latest: Option<Release>,
    pub available: bool,
    pub checked_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    /// An admin may start the install from the page (`CHAT_UPDATE_DIR` and the admin token are set).
    pub can_install: bool,
    pub install: Install,
    /// What to type to update by hand, on Linux.
    pub commands: Option<String>,
}

pub fn status(state: &AppState, admin_enabled: bool) -> Status {
    let checked = state.updates.checked.lock().unwrap();
    let latest = checked.latest.clone();
    let available = latest.as_ref().is_some_and(|r| is_newer(&r.version, VERSION));
    let dir = state.cfg.update_dir.as_deref();
    Status {
        enabled: state.cfg.update_check,
        current: VERSION,
        commands: latest.as_ref().filter(|_| available).and_then(|r| commands(r, dir.is_some())),
        available,
        latest,
        checked_at: checked.at,
        error: checked.error.clone(),
        can_install: admin_enabled && dir.is_some(),
        install: dir.map(read_install).unwrap_or_else(Install::idle),
    }
}

/// The manual steps: the helper if it is set up, otherwise DEPLOYMENT.md's by hand.
fn commands(release: &Release, helper: bool) -> Option<String> {
    if helper {
        return Some(format!("sudo cedar-update {}", release.version));
    }
    let (tarball, checksums) = (release.tarball_url.as_deref()?, release.checksums_url.as_deref()?);
    let name = tarball.rsplit('/').next()?;
    let dir = name.strip_suffix(".tar.gz")?;
    Some(format!(
        "curl -fLO {tarball}\ncurl -fLO {checksums}\nsha256sum -c --ignore-missing checksums.txt\ntar -xzf {name}\n\
         sudo install -m 0755 {dir}/cedar {dir}/cedar-chat /usr/local/bin/\nsudo systemctl restart cedar cedar-chat"
    ))
}

/// `POST /api/update`: asks the `cedar-update` unit to install the latest release.
pub fn request_install(state: &AppState) -> Result<Install, RequestError> {
    let dir = state.cfg.update_dir.as_deref().ok_or(RequestError::NotSetUp)?;
    let version = {
        let checked = state.updates.checked.lock().unwrap();
        match &checked.latest {
            Some(r) if is_newer(&r.version, VERSION) => r.version.clone(),
            _ => return Err(RequestError::NothingNewer),
        }
    };
    if read_install(dir).running() {
        return Err(RequestError::Running);
    }
    // Written whole and renamed, so the unit never reads half a file.
    let tmp = dir.join(format!(".{REQUEST_FILE}.tmp"));
    let body = serde_json::json!({ "version": version, "requested_at": Utc::now() }).to_string();
    std::fs::write(&tmp, body)
        .and_then(|()| std::fs::rename(&tmp, dir.join(REQUEST_FILE)))
        .map_err(|e| RequestError::Io(format!("writing the update request to {}: {e}", dir.display())))?;
    Ok(read_install(dir))
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("updates are installed by hand here (CHAT_UPDATE_DIR is not set; see DEPLOYMENT.md)")]
    NotSetUp,
    #[error("no newer release is known; check for updates first")]
    NothingNewer,
    #[error("an update is already being installed")]
    Running,
    #[error("{0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.1.7"), Some((0, 1, 7)));
        assert_eq!(parse_version("10.0.2"), Some((10, 0, 2)));
        assert_eq!(parse_version("0.1"), None);
        assert_eq!(parse_version("0.1.7-beta"), None);
        assert_eq!(parse_version("0.1.7.1"), None);
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.7", "0.1.7"));
        assert!(!is_newer("0.1.6", "0.1.7"));
        assert!(!is_newer("garbage", "0.1.7"));
    }

    #[test]
    fn tarballs() {
        assert_eq!(linux_tarball("0.1.8", "x86_64").as_deref(), Some("Cedar-0.1.8-linux-x64.tar.gz"));
        assert_eq!(linux_tarball("0.1.8", "aarch64").as_deref(), Some("Cedar-0.1.8-linux-arm64.tar.gz"));
        assert_eq!(linux_tarball("0.1.8", "riscv64"), None);
    }

    #[test]
    fn drafts_and_odd_tags_are_not_releases() {
        let gh = |tag: &str, prerelease: bool| GithubRelease {
            tag_name: tag.into(),
            html_url: "https://example/r".into(),
            published_at: None,
            draft: false,
            prerelease,
            assets: vec![GithubAsset { name: "checksums.txt".into(), browser_download_url: "https://example/c".into() }],
        };
        let r = release_from(gh("v0.2.0", false)).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.checksums_url.as_deref(), Some("https://example/c"));
        assert!(release_from(gh("v0.2.0", true)).is_err());
        assert!(release_from(gh("nightly", false)).is_err());
    }

    #[test]
    fn manual_commands() {
        let r = Release {
            version: "0.2.0".into(),
            notes_url: String::new(),
            published_at: None,
            tarball_url: Some("https://x/v0.2.0/Cedar-0.2.0-linux-x64.tar.gz".into()),
            checksums_url: Some("https://x/v0.2.0/checksums.txt".into()),
        };
        assert_eq!(commands(&r, true).as_deref(), Some("sudo cedar-update 0.2.0"));
        let by_hand = commands(&r, false).unwrap();
        assert!(by_hand.contains("sha256sum -c --ignore-missing checksums.txt"));
        assert!(by_hand.contains("Cedar-0.2.0-linux-x64/cedar-chat /usr/local/bin/"));
        assert_eq!(commands(&Release { tarball_url: None, ..r }, false), None);
    }

    #[test]
    fn install_progress_is_read_from_the_helper() {
        let dir = std::env::temp_dir().join(format!("cedar-update-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_install(&dir).state, "idle");
        std::fs::write(dir.join(STATUS_FILE), r#"{"state":"downloading","version":"0.2.0"}"#).unwrap();
        let install = read_install(&dir);
        assert_eq!((install.state.as_str(), install.version.as_deref()), ("downloading", Some("0.2.0")));
        assert!(install.running());
        std::fs::write(dir.join(REQUEST_FILE), r#"{"version":"0.2.1"}"#).unwrap();
        assert_eq!(read_install(&dir).state, "requested");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
