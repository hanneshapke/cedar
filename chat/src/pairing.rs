//! Settings -> Phone: what the Cedar iOS and Android apps need to connect to this chat. The apps
//! connect over HTTPS only, so the address is `CHAT_PUBLIC_URL` when that is an `https://` one:
//! the tailnet address the desktop app and `scripts/cedar-stack.sh` start the chat with when they
//! serve it with `tailscale serve`, or the user's own. The QR code and the reading of `tailscale status` are
//! shared with the desktop app's setup page (`cedar_pairing`).

use crate::api::AppState;
use cedar_pairing::tailscale::{self, Check, State};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

/// How long `tailscale status` may take before the page says it didn't answer.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize, Debug)]
pub struct Pairing {
    /// `linux`, `macos` or `windows`, for the page's wording.
    pub platform: &'static str,
    /// The HTTPS address the app connects to; `None` while the chat isn't served over HTTPS.
    pub server: Option<String>,
    /// `server` is this computer's name on the tailnet.
    pub on_tailnet: bool,
    /// `CHAT_ACCESS_TOKEN`, which whoever asks has already presented.
    pub access_token: Option<String>,
    /// What the QR code carries, and the code as SVG markup, with `server`.
    pub link: Option<String>,
    pub qr_svg: Option<String>,
    pub tailscale: Check,
}

/// What `tailscale status --json` says right now.
pub async fn tailscale_check(cli: Option<&str>) -> Check {
    let Some(bin) = cli.map(PathBuf::from).or_else(tailscale::find) else {
        return Check::without_status(State::Missing, tailscale::MISSING);
    };
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(["status", "--json"]).stdin(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    for var in tailscale::LAUNCH_ENV {
        cmd.env_remove(var);
    }
    // CREATE_NO_WINDOW: under the desktop app on Windows, no console flashes up.
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = match tokio::time::timeout(STATUS_TIMEOUT, cmd.output()).await {
        Err(_) => return Check::without_status(State::Unavailable, format!("`tailscale status` did not answer within {STATUS_TIMEOUT:?}")),
        Ok(Err(e)) => return Check::without_status(State::Unavailable, format!("running {}: {e}", bin.display())),
        Ok(Ok(out)) => out,
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    // A logged-out Tailscale still prints its status, with a failing exit code.
    match tailscale::check(&stdout) {
        Ok(c) => c,
        Err(_) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let msg = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
            Check::without_status(State::Unavailable, format!("`tailscale status` failed: {}", tailscale::failure(msg)))
        }
    }
}

/// The host of an `https://` address, without a port.
fn host(url: &str) -> &str {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or(rest);
    authority.rsplit_once(':').map_or(authority, |(h, _)| h)
}

pub async fn pairing(state: &AppState) -> Pairing {
    let cfg = &state.cfg;
    let ts = tailscale_check(cfg.tailscale_cli.as_deref()).await;
    let server = cfg.public_url.starts_with("https://").then(|| cfg.public_url.clone());
    let on_tailnet = server.as_deref().is_some_and(|s| ts.name.as_deref().is_some_and(|n| host(s).eq_ignore_ascii_case(n)));
    let link = server.as_deref().map(|s| cedar_pairing::connect::link(s, cfg.access_token.as_deref().unwrap_or_default()));
    let qr_svg = link.as_deref().and_then(|l| cedar_pairing::connect::qr_svg(l).ok());
    Pairing { platform: std::env::consts::OS, server, on_tailnet, access_token: cfg.access_token.clone(), link, qr_svg, tailscale: ts }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_drops_the_scheme_port_and_path() {
        assert_eq!(host("https://m.t.ts.net"), "m.t.ts.net");
        assert_eq!(host("https://m.t.ts.net:8443"), "m.t.ts.net");
        assert_eq!(host("https://m.t.ts.net/chat"), "m.t.ts.net");
    }

    #[tokio::test]
    async fn a_missing_cli_is_reported() {
        let c = tailscale_check(Some("/nonexistent/tailscale")).await;
        assert_eq!(c.state, State::Unavailable);
        assert!(c.name.is_none());
    }
}
