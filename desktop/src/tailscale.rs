//! Sharing the chat with an iPhone, iPad or Android phone over Tailscale: `tailscale serve` proxies
//! `https://<computer>.<tailnet>.ts.net` to the chat on 127.0.0.1, so only devices on the tailnet
//! can reach it. What Tailscale must be ready for, and where its CLI is, is shared with the chat's
//! own Settings page (`cedar_pairing::tailscale`); running the CLI is this app's.

use crate::proc;
use anyhow::{anyhow, bail, Context, Result};
use cedar_pairing::tailscale::https_url;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

pub use cedar_pairing::tailscale::MISSING;

#[derive(Clone, Debug)]
pub struct TailscaleCli {
    pub bin: PathBuf,
}

pub fn find() -> Option<TailscaleCli> {
    cedar_pairing::tailscale::find().map(|bin| TailscaleCli { bin })
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
