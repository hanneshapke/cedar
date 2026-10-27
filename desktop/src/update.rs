//! Updating the app itself. A minute after the start and then every six hours the app asks the
//! latest GitHub Release for its `latest.json` (tauri-plugin-updater), and when that names a newer
//! version the setup page, the chat page (through the window, see window.rs) and the tray menu
//! say so. **Update** downloads the release's DMG app bundle or MSI, checks its signature
//! against the key the release was built with, stops the servers, and installs: on a Mac the
//! app bundle is replaced and the app restarts; on Windows the MSI runs (asking for administrator
//! rights, like the first install) and starts Cedar again when it is done.
//!
//! Builds without `CEDAR_UPDATER_PUBKEY` (anything but the release workflow's) don't check.

use crate::app::{self, AppState};
use crate::i18n::{t, tf};
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

/// The minisign public key the release workflow signs updates with, baked in at build time.
const PUBKEY: Option<&str> = option_env!("CEDAR_UPDATER_PUBKEY");
const FIRST_CHECK: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// What the pages and the tray show. `state` is `idle`, `checking`, `downloading`, `installing`
/// or `failed`; the chat's banner (update.js) uses the same words.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct UpdateStatus {
    /// Whether this build checks at all (a release build, with checks turned on).
    pub enabled: bool,
    pub current: &'static str,
    /// The newer version a check found.
    pub version: Option<String>,
    pub notes_url: Option<String>,
    /// The user dismissed the banner for this version.
    pub dismissed: bool,
    pub state: &'static str,
    pub detail: Option<String>,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            current: crate::config::VERSION,
            version: None,
            notes_url: None,
            dismissed: false,
            state: "idle",
            detail: None,
        }
    }
}

/// Where a release's notes are.
fn notes_url(version: &str) -> String {
    format!("https://github.com/hanneshapke/cedar/releases/tag/v{version}")
}

/// "12 of 140 MB", for the banner while the download runs.
fn progress(got: u64, total: Option<u64>) -> String {
    let mb = |b: u64| (b / 1_000_000).to_string();
    match total {
        Some(total) => tf("update.downloading_of", &[("got", &mb(got)), ("total", &mb(total))]),
        None => tf("update.downloading_mb", &[("got", &mb(got))]),
    }
}

/// The chat banner's view of it (update.js `fromApp`).
pub fn for_chat(s: &UpdateStatus) -> serde_json::Value {
    serde_json::json!({
        "available": s.version.is_some(),
        "current": s.current,
        "version": s.version,
        "notes_url": s.notes_url,
        "dismissed": s.dismissed,
        "install": { "state": s.state, "detail": s.detail },
    })
}

fn updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater> {
    let key = PUBKEY.filter(|k| !k.trim().is_empty()).ok_or_else(|| anyhow!("this build doesn't update itself"))?;
    // A release before this one's signature carries its version, so an old, genuine release
    // can't be passed off as a newer one (see require_signed_version in tauri.conf.json).
    app.updater_builder()
        .pubkey(key.trim())
        .timeout(Duration::from_secs(60))
        .build()
        .context("setting up the updater")
}

fn set(app: &AppHandle, change: impl FnOnce(&mut UpdateStatus)) {
    let state = app.state::<AppState>();
    let status = state.update_status(change);
    crate::tray::show_update(app, &status);
    crate::window::push_update(app, &status);
}

/// Checks a minute after the start and every six hours, while checks are on.
pub fn start(app: AppHandle) {
    let enabled = PUBKEY.is_some_and(|k| !k.trim().is_empty());
    set(&app, |s| s.enabled = enabled && app.state::<AppState>().config().check_updates);
    if !enabled {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if app.state::<AppState>().config().check_updates {
                if let Err(e) = check(&app).await {
                    eprintln!("checking for updates: {e:#}");
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// Hides the banner for the release it shows.
pub fn dismiss(app: &AppHandle) -> Result<()> {
    let Some(version) = app.state::<AppState>().update().version else {
        return Ok(());
    };
    app.state::<AppState>().update_config(|c| c.dismissed_update = version)?;
    set(app, |s| s.dismissed = true);
    Ok(())
}

/// Turns the checks on or off; on checks at once.
pub fn set_enabled(app: &AppHandle, enabled: bool) -> Result<()> {
    app.state::<AppState>().update_config(|c| c.check_updates = enabled)?;
    let releases = PUBKEY.is_some_and(|k| !k.trim().is_empty());
    set(app, |s| s.enabled = enabled && releases);
    if enabled && releases {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = check(&app).await {
                eprintln!("checking for updates: {e:#}");
            }
        });
    }
    Ok(())
}

/// Looks for a newer release once; the result is in [`AppState::update`].
pub async fn check(app: &AppHandle) -> Result<()> {
    if matches!(app.state::<AppState>().update().state, "checking" | "downloading" | "installing") {
        return Ok(());
    }
    set(app, |s| {
        s.state = "checking";
        s.detail = None;
    });
    let found = async { updater(app)?.check().await.context("looking up the latest release") }.await;
    match found {
        Ok(update) => {
            let version = update.map(|u| u.version);
            let dismissed = app.state::<AppState>().config().dismissed_update;
            set(app, |s| {
                s.dismissed = version.as_deref().is_some_and(|v| v == dismissed);
                s.notes_url = version.as_deref().map(notes_url);
                s.version = version;
                s.state = "idle";
            });
            Ok(())
        }
        Err(e) => {
            set(app, |s| {
                s.state = "idle";
                s.detail = Some(format!("{e:#}"));
            });
            Err(e)
        }
    }
}

/// Downloads, checks and installs the newer release, then restarts into it. The servers are
/// stopped only once the download is in hand and checked.
pub async fn install(app: AppHandle) {
    let state = app.state::<AppState>();
    let current = state.update();
    if current.version.is_none() || matches!(current.state, "checking" | "downloading" | "installing") {
        return;
    }
    if let Err(e) = try_install(&app).await {
        eprintln!("installing the update: {e:#}");
        set(&app, |s| {
            s.state = "failed";
            s.detail = Some(format!("{e:#}"));
        });
        // The servers may have been stopped for it.
        tauri::async_runtime::spawn(app::bootstrap(app.clone()));
    }
}

async fn try_install(app: &AppHandle) -> Result<()> {
    set(app, |s| {
        s.state = "downloading";
        s.detail = Some(t("update.downloading"));
    });
    let update = updater(app)?
        .check()
        .await
        .context("looking up the latest release")?
        .ok_or_else(|| anyhow!("no newer release is published any more"))?;
    let mut got: u64 = 0;
    let mut shown = 0;
    let bytes = update
        .download(
            |chunk, total| {
                got += chunk as u64;
                // A step per megabyte is plenty for the banner.
                if got / 1_000_000 != shown {
                    shown = got / 1_000_000;
                    let text = progress(got, total);
                    set(app, |s| s.detail = Some(text));
                }
            },
            || {},
        )
        .await
        .context("downloading the update (its signature is checked too)")?;

    set(app, |s| {
        s.state = "installing";
        s.detail = Some(tf("update.installing", &[("version", &update.version)]));
    });
    let state = app.state::<AppState>();
    state.supervisor.stop().await;
    state.stop_share().await;
    // On Windows this starts the MSI and ends the app; the MSI opens Cedar again.
    update.install(bytes).context("installing the update")?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_in_megabytes() {
        assert_eq!(progress(12_500_000, Some(140_000_000)), "Downloading: 12 of 140 MB");
        assert_eq!(progress(3_000_000, None), "Downloading: 3 MB");
    }

    #[test]
    fn the_chat_gets_the_banner_fields() {
        let s = UpdateStatus {
            version: Some("0.2.0".into()),
            notes_url: Some(notes_url("0.2.0")),
            state: "downloading",
            detail: Some("Downloading: 1 MB".into()),
            ..UpdateStatus::default()
        };
        let v = for_chat(&s);
        assert_eq!(v["available"], true);
        assert_eq!(v["version"], "0.2.0");
        assert_eq!(v["notes_url"], "https://github.com/hanneshapke/cedar/releases/tag/v0.2.0");
        assert_eq!(v["install"]["state"], "downloading");
        assert_eq!(v["current"], crate::config::VERSION);
        assert_eq!(for_chat(&UpdateStatus::default())["available"], false);
    }
}
