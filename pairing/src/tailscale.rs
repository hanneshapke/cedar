//! What Tailscale must be ready for before the chat can be served to the iOS app. The app only
//! connects over HTTPS with a certificate it trusts, and the chat listens on loopback, so
//! `tailscale serve` does the reaching: it gets a Let's Encrypt certificate for this computer's
//! tailnet name and proxies `https://<computer>.<tailnet>.ts.net` to the chat. That needs the
//! CLI, a logged-in and connected Tailscale, MagicDNS (the name) and HTTPS Certificates turned on
//! for the tailnet. Tailscale itself is the user's: nothing here installs it or logs in.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where the CLI is, in the order to try, then every directory on `PATH`. On a Mac: the one
/// Tailscale's own menu installs ("Install CLI"), Homebrew's, then the binary inside the app (Mac
/// App Store or standalone download), which acts as the CLI when it is given arguments.
#[cfg(target_os = "macos")]
fn known() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/local/bin/tailscale"),
        PathBuf::from("/opt/homebrew/bin/tailscale"),
        PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
    ]
}

/// The Windows installer puts `tailscale.exe` next to the GUI (and on the system `PATH`).
#[cfg(windows)]
fn known() -> Vec<PathBuf> {
    std::env::var_os("ProgramFiles")
        .map(|pf| PathBuf::from(pf).join("Tailscale").join("tailscale.exe"))
        .into_iter()
        .collect()
}

/// The Linux packages install it in `/usr/bin`; a snap or a manual install elsewhere is on `PATH`.
#[cfg(not(any(target_os = "macos", windows)))]
fn known() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/bin/tailscale"), PathBuf::from("/usr/local/bin/tailscale")]
}

pub fn candidates() -> Vec<PathBuf> {
    let mut all = known();
    if let Some(path) = std::env::var_os("PATH") {
        let exe = format!("tailscale{}", std::env::consts::EXE_SUFFIX);
        all.extend(std::env::split_paths(&path).map(|d| d.join(&exe)));
    }
    all
}

/// The tailscale CLI, if it is installed.
pub fn find() -> Option<PathBuf> {
    candidates().into_iter().find(|p| is_executable(p))
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

#[cfg(target_os = "macos")]
pub const MISSING: &str =
    "Tailscale is not installed. Get it from https://tailscale.com/download/mac \
    or the Mac App Store, log in, and try again";

#[cfg(windows)]
pub const MISSING: &str =
    "Tailscale is not installed. Get it from https://tailscale.com/download/windows, \
    log in, and try again";

#[cfg(not(any(target_os = "macos", windows)))]
pub const MISSING: &str =
    "Tailscale is not installed. Get it from https://tailscale.com/download/linux, \
    log in with `sudo tailscale up`, and try again";

/// How far Tailscale is from serving the chat, first problem first.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// No CLI was found.
    Missing,
    /// The CLI is there but didn't answer (tailscaled isn't running, say).
    Unavailable,
    LoggedOut,
    Stopped,
    Connecting,
    NoMagicDns,
    NoHttps,
    Ready,
}

/// What `tailscale status --json` says, for a page to show.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub state: State,
    /// This computer's tailnet name (`my-mac.tail1234.ts.net`), once it has one.
    pub name: Option<String>,
    /// What to do next, or that it is ready.
    pub detail: String,
}

impl Check {
    /// A check that didn't get as far as a status: no CLI, or it failed.
    pub fn without_status(state: State, detail: impl Into<String>) -> Self {
        Self { state, name: None, detail: detail.into() }
    }

    pub fn ready(&self) -> bool {
        self.state == State::Ready
    }

    /// The chat's address on the tailnet once `tailscale serve` serves it on `https_port`.
    pub fn url(&self, https_port: u16) -> Option<String> {
        let name = self.name.as_deref().filter(|_| self.ready())?;
        Some(if https_port == 443 {
            format!("https://{name}")
        } else {
            format!("https://{name}:{https_port}")
        })
    }
}

/// The parts of `tailscale status --json` this needs.
#[derive(Deserialize, Debug, Default)]
#[serde(default, rename_all = "PascalCase")]
struct StatusJson {
    backend_state: String,
    #[serde(rename = "Self")]
    me: Option<SelfJson>,
    /// Empty unless HTTPS Certificates are turned on for the tailnet.
    cert_domains: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default, rename_all = "PascalCase")]
struct SelfJson {
    #[serde(rename = "DNSName")]
    dns_name: String,
}

/// Reads `tailscale status --json`. Checked before serving, because `tailscale serve` waits
/// forever for HTTPS to be turned on rather than failing.
pub fn check(status: &str) -> Result<Check> {
    let s: StatusJson = serde_json::from_str(status).context("reading `tailscale status --json`")?;
    let name = s
        .me
        .map(|m| m.dns_name.trim_end_matches('.').to_string())
        .filter(|n| !n.is_empty());
    let (state, detail) = match s.backend_state.as_str() {
        "Running" if name.is_none() => (
            State::NoMagicDns,
            "this computer has no tailnet name. Turn on MagicDNS in the Tailscale admin console (DNS page)".to_string(),
        ),
        "Running" if s.cert_domains.unwrap_or_default().is_empty() => (
            State::NoHttps,
            "HTTPS is not enabled for the tailnet. Turn on HTTPS Certificates in the Tailscale admin \
             console (DNS page, https://login.tailscale.com/admin/dns), then try again"
                .to_string(),
        ),
        "Running" => (State::Ready, "Tailscale is connected, with MagicDNS and HTTPS Certificates on".to_string()),
        "NeedsLogin" | "NoState" | "" => (
            State::LoggedOut,
            "Tailscale is not logged in. Log in from the Tailscale app, then try again".to_string(),
        ),
        "Stopped" => (
            State::Stopped,
            "Tailscale is turned off. Connect it from the Tailscale app, then try again".to_string(),
        ),
        other => (State::Connecting, format!("Tailscale is not connected yet ({other}). Try again in a moment")),
    };
    Ok(Check { state, name, detail })
}

/// The address the chat gets on the tailnet, or why it can't get one.
pub fn https_url(status: &str, https_port: u16) -> Result<String> {
    let c = check(status)?;
    match c.url(https_port) {
        Some(url) => Ok(url),
        None => bail!("{}", c.detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READY: &str = r#"{"BackendState": "Running", "Self": {"DNSName": "my-mac.tail1234.ts.net."},
        "CertDomains": ["my-mac.tail1234.ts.net"]}"#;

    #[test]
    fn the_url_drops_the_trailing_dot_and_a_default_port() {
        assert_eq!(https_url(READY, 443).unwrap(), "https://my-mac.tail1234.ts.net");
        assert_eq!(https_url(READY, 8443).unwrap(), "https://my-mac.tail1234.ts.net:8443");
        let c = check(READY).unwrap();
        assert_eq!(c.state, State::Ready);
        assert_eq!(c.name.as_deref(), Some("my-mac.tail1234.ts.net"));
    }

    #[test]
    fn not_ready_says_what_to_do() {
        let logged_out = r#"{"BackendState": "NeedsLogin", "Self": {"DNSName": ""}}"#;
        assert!(format!("{:#}", https_url(logged_out, 443).unwrap_err()).contains("not logged in"));
        assert_eq!(check(logged_out).unwrap().state, State::LoggedOut);
        let no_https = r#"{"BackendState": "Running", "Self": {"DNSName": "m.t.ts.net."}, "CertDomains": null}"#;
        assert!(format!("{:#}", https_url(no_https, 443).unwrap_err()).contains("HTTPS Certificates"));
        let c = check(no_https).unwrap();
        assert_eq!((c.state, c.name.as_deref(), c.url(443)), (State::NoHttps, Some("m.t.ts.net"), None));
        let no_name = r#"{"BackendState": "Running", "Self": {"DNSName": ""}, "CertDomains": ["x"]}"#;
        assert!(format!("{:#}", https_url(no_name, 443).unwrap_err()).contains("MagicDNS"));
        let stopped = r#"{"BackendState": "Stopped"}"#;
        assert_eq!(check(stopped).unwrap().state, State::Stopped);
        assert_eq!(check(r#"{"BackendState": "Starting"}"#).unwrap().state, State::Connecting);
        assert!(check("not json").is_err());
    }

    #[test]
    fn the_cli_is_looked_for_on_path_too() {
        let all = candidates();
        assert!(all.len() > known().len() || std::env::var_os("PATH").is_none());
        assert!(all.iter().all(|p| p.file_stem().is_some_and(|n| n.eq_ignore_ascii_case("tailscale"))));
    }
}
