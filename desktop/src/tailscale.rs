//! Sharing the chat with an iPhone or iPad over Tailscale. The iOS app only connects over HTTPS
//! with a certificate it trusts, and both servers listen on loopback, so `tailscale serve` does
//! the reaching: it gets a Let's Encrypt certificate for this computer's tailnet name and proxies
//! `https://<computer>.<tailnet>.ts.net` to the chat on 127.0.0.1. Only devices on the tailnet
//! can reach it. Tailscale itself is the user's: the app finds its CLI, it never installs or logs in.

use crate::proc;
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

/// Where the CLI is, in the order to try: the one Tailscale's own menu installs ("Install CLI"),
/// Homebrew's, then the binary inside the app (Mac App Store or standalone download), which acts
/// as the CLI when it is given arguments.
#[cfg(not(windows))]
fn candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/local/bin/tailscale"),
        PathBuf::from("/opt/homebrew/bin/tailscale"),
        PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
    ]
}

/// The Windows installer puts `tailscale.exe` next to the GUI and on the system `PATH`.
#[cfg(windows)]
fn candidates() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("ProgramFiles")
        .map(|pf| PathBuf::from(pf).join("Tailscale"))
        .into_iter()
        .collect();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.iter()
        .filter_map(|d| proc::executable_in(d, "tailscale"))
        .collect()
}

#[cfg(not(windows))]
pub const MISSING: &str =
    "Tailscale is not installed. Get it from https://tailscale.com/download/mac \
    or the Mac App Store, log in, and try again";

#[cfg(windows)]
pub const MISSING: &str =
    "Tailscale is not installed. Get it from https://tailscale.com/download/windows, \
    log in, and try again";

#[derive(Clone, Debug)]
pub struct TailscaleCli {
    pub bin: PathBuf,
}

pub fn find() -> Option<TailscaleCli> {
    candidates()
        .into_iter()
        .find(|p| proc::is_executable(p))
        .map(|bin| TailscaleCli { bin })
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

/// The address the chat gets on the tailnet, or why it can't get one. Checked before serving,
/// because `tailscale serve` waits forever for HTTPS to be turned on rather than failing.
fn https_url(status: &str, https_port: u16) -> Result<String> {
    let s: StatusJson =
        serde_json::from_str(status).context("reading `tailscale status --json`")?;
    match s.backend_state.as_str() {
        "Running" => {}
        "NeedsLogin" | "NoState" | "" => {
            bail!("Tailscale is not logged in. Log in from the Tailscale app, then try again")
        }
        "Stopped" => {
            bail!("Tailscale is turned off. Connect it from the Tailscale app, then try again")
        }
        other => bail!("Tailscale is not connected yet ({other}). Try again in a moment"),
    }
    let name =
        s.me.map(|m| m.dns_name.trim_end_matches('.').to_string())
            .unwrap_or_default();
    if name.is_empty() {
        bail!("this computer has no tailnet name. Turn on MagicDNS in the Tailscale admin console (DNS page)");
    }
    if s.cert_domains.unwrap_or_default().is_empty() {
        bail!(
            "HTTPS is not enabled for the tailnet. Turn on HTTPS Certificates in the Tailscale admin \
             console (DNS page, https://login.tailscale.com/admin/dns), then try again"
        );
    }
    Ok(if https_port == 443 {
        format!("https://{name}")
    } else {
        format!("https://{name}:{https_port}")
    })
}

impl TailscaleCli {
    async fn output(&self, args: &[&str], timeout: Duration) -> Result<String> {
        let out = tokio::time::timeout(
            timeout,
            proc::command(&self.bin)
                .args(args)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "`tailscale {}` did not answer within {:?}",
                args.join(" "),
                timeout
            )
        })?
        .with_context(|| format!("running {}", self.bin.display()))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let msg = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            bail!("`tailscale {}` failed: {}", args.join(" "), msg.trim())
        }
    }

    /// The chat's HTTPS address on the tailnet, if Tailscale is ready to serve it.
    pub async fn url(&self, https_port: u16) -> Result<String> {
        let status = self
            .output(&["status", "--json"], Duration::from_secs(10))
            .await?;
        https_url(&status, https_port)
    }

    /// Serves the chat on the tailnet. `--bg` keeps the setting in Tailscale until [`Self::unserve`];
    /// the app turns it off when it quits and serves again on the next start.
    pub async fn serve(&self, https_port: u16, chat_port: u16) -> Result<()> {
        let https = format!("--https={https_port}");
        let target = format!("http://127.0.0.1:{chat_port}");
        self.output(&["serve", "--bg", &https, &target], Duration::from_secs(30))
            .await
            .map(|_| ())
    }

    pub async fn unserve(&self, https_port: u16) -> Result<()> {
        let https = format!("--https={https_port}");
        self.output(&["serve", &https, "off"], Duration::from_secs(10))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READY: &str = r#"{"BackendState": "Running", "Self": {"DNSName": "my-mac.tail1234.ts.net."},
        "CertDomains": ["my-mac.tail1234.ts.net"]}"#;

    #[test]
    fn the_url_drops_the_trailing_dot_and_a_default_port() {
        assert_eq!(
            https_url(READY, 443).unwrap(),
            "https://my-mac.tail1234.ts.net"
        );
        assert_eq!(
            https_url(READY, 8443).unwrap(),
            "https://my-mac.tail1234.ts.net:8443"
        );
    }

    #[test]
    fn not_ready_says_what_to_do() {
        let logged_out = r#"{"BackendState": "NeedsLogin", "Self": {"DNSName": ""}}"#;
        assert!(format!("{:#}", https_url(logged_out, 443).unwrap_err()).contains("not logged in"));
        let no_https = r#"{"BackendState": "Running", "Self": {"DNSName": "m.t.ts.net."}, "CertDomains": null}"#;
        assert!(
            format!("{:#}", https_url(no_https, 443).unwrap_err()).contains("HTTPS Certificates")
        );
        let no_name =
            r#"{"BackendState": "Running", "Self": {"DNSName": ""}, "CertDomains": ["x"]}"#;
        assert!(format!("{:#}", https_url(no_name, 443).unwrap_err()).contains("MagicDNS"));
    }
}
