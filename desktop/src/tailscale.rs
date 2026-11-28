//! Sharing the chat with an iPhone, iPad or Android phone over Tailscale: `tailscale serve` proxies
//! `https://<computer>.<tailnet>.ts.net` to the chat on 127.0.0.1, so only devices on the tailnet
//! can reach it. What Tailscale must be ready for, and where its CLI is, is shared with the chat's
//! own Settings page (`cedar_pairing::tailscale`); running the CLI is this app's, and so is
//! starting a Tailscale that isn't running when the app starts with sharing on. A chat on another
//! computer (remote.rs) is reached over the tailnet too: when it doesn't answer and Tailscale
//! isn't connected here, the setup page offers to start it ([`TailscaleCli::connect`]).

use crate::proc;
use anyhow::{anyhow, bail, Context, Result};
use cedar_pairing::tailscale::{check as read_status, failure, terminal_env, Check, State};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

pub use cedar_pairing::tailscale::MISSING;

#[derive(Clone, Debug)]
pub struct TailscaleCli {
    pub bin: PathBuf,
}

pub fn find() -> Option<TailscaleCli> {
    cedar_pairing::tailscale::find().map(|bin| TailscaleCli { bin })
}

/// Tailscale's own app: opened after installing it so the user can log in, and started when the
/// CLI finds it not running.
#[cfg(target_os = "macos")]
pub fn app() -> Option<PathBuf> {
    Some(PathBuf::from("/Applications/Tailscale.app"))
}

#[cfg(windows)]
pub fn app() -> Option<PathBuf> {
    std::env::var_os("ProgramFiles")
        .map(|pf| PathBuf::from(pf).join(r"Tailscale\tailscale-ipn.exe"))
}

/// On Linux Tailscale is a system service (`tailscaled`), which takes root to start.
#[cfg(not(any(target_os = "macos", windows)))]
pub fn app() -> Option<PathBuf> {
    None
}

/// Starts Tailscale's app, behind the other windows on a Mac. Whether there was one to start.
fn start_app() -> bool {
    let Some(app) = app().filter(|a| a.exists()) else {
        return false;
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut cmd = proc::std_command("/usr/bin/open");
        cmd.arg("-g").arg(&app);
        cmd
    };
    #[cfg(not(target_os = "macos"))]
    let mut cmd = proc::std_command(&app);
    let started = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match started {
        Ok(_) => true,
        Err(e) => {
            eprintln!("starting {}: {e:#}", app.display());
            false
        }
    }
}

/// How long [`TailscaleCli::url_started`] waits for a Tailscale that was just started, or is
/// still connecting, and how often it asks.
#[derive(Clone, Copy)]
struct Patience {
    /// For Tailscale to answer and connect, from the first look.
    settle: Duration,
    /// For an app that was just started to stop reporting "logged out", which it does until it
    /// has read its own state.
    login: Duration,
    poll: Duration,
}

const PATIENCE: Patience = Patience {
    settle: Duration::from_secs(45),
    login: Duration::from_secs(15),
    poll: Duration::from_secs(1),
};

fn address(check: &Check, https_port: u16) -> Result<String> {
    check
        .url(https_port)
        .ok_or_else(|| anyhow!("{}", check.detail))
}

/// Whether this computer is on its tailnet. Reaching another computer there takes no more:
/// HTTPS Certificates (and this computer's own name) are for serving the chat from here.
fn connected(state: State) -> bool {
    matches!(state, State::Ready | State::NoHttps | State::NoMagicDns)
}

/// Whether `address` is on a tailnet, by its name (`https://spark.tail1234.ts.net`).
pub fn on_tailnet(address: &str) -> bool {
    url::Url::parse(address).is_ok_and(|u| {
        u.host_str().is_some_and(|h| h.to_ascii_lowercase().ends_with(".ts.net"))
    })
}

impl TailscaleCli {
    async fn output(&self, args: &[&str], timeout: Duration) -> Result<String> {
        let mut cmd = proc::command(&self.bin);
        cmd.args(args).stdin(Stdio::null()).kill_on_drop(true);
        if let Some((k, v)) = terminal_env() {
            cmd.env(k, v);
        }
        let out = tokio::time::timeout(timeout, cmd.output())
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
            bail!(
                "`tailscale {}` failed: {}",
                args.join(" "),
                failure(msg.trim())
            )
        }
    }

    /// What Tailscale says about itself, or why it didn't answer (it isn't running, say).
    async fn check(&self) -> Result<Check> {
        let status = self
            .output(&["status", "--json"], Duration::from_secs(10))
            .await?;
        // The Mac app's binary exits 0 with its "GUI failed to start" on stdout, not JSON.
        if !status.starts_with('{') {
            bail!("`tailscale status` failed: {}", failure(&status));
        }
        read_status(&status)
    }

    /// The chat's HTTPS address on the tailnet, if Tailscale is ready to serve it.
    pub async fn url(&self, https_port: u16) -> Result<String> {
        address(&self.check().await?, https_port)
    }

    /// [`Self::url`], after starting a Tailscale that isn't running: its app when the CLI gets no
    /// answer, `tailscale up` when it is turned off. One that is still connecting (both apps start
    /// at login, say) is waited for. Logging in stays the user's.
    pub async fn url_started(&self, https_port: u16) -> Result<String> {
        self.url_after(https_port, start_app, PATIENCE).await
    }

    async fn url_after(
        &self,
        https_port: u16,
        start_app: impl FnMut() -> bool,
        patience: Patience,
    ) -> Result<String> {
        address(&self.check_after(start_app, patience).await?, https_port)
    }

    /// Whether [`Self::connect`] has something to try: Tailscale doesn't answer and its app is
    /// there to start, or it is turned off. One that is logged out is the user's to log in.
    pub async fn can_connect(&self) -> bool {
        self.can_connect_with(app().is_some_and(|a| a.exists())).await
    }

    async fn can_connect_with(&self, has_app: bool) -> bool {
        match self.check().await.map(|c| c.state) {
            Err(_) => has_app,
            Ok(state) => state == State::Stopped,
        }
    }

    /// Puts this computer on its tailnet, starting Tailscale as [`Self::url_started`] does. An
    /// error says what is left for the user (logging in, say).
    pub async fn connect(&self) -> Result<()> {
        self.connect_after(start_app, PATIENCE).await
    }

    async fn connect_after(
        &self,
        start_app: impl FnMut() -> bool,
        patience: Patience,
    ) -> Result<()> {
        let check = self.check_after(start_app, patience).await?;
        if !connected(check.state) {
            bail!("{}", check.detail);
        }
        Ok(())
    }

    /// What Tailscale says once one that wasn't running was started and has settled.
    async fn check_after(
        &self,
        mut start_app: impl FnMut() -> bool,
        patience: Patience,
    ) -> Result<Check> {
        let began = Instant::now();
        let mut app_started: Option<Instant> = None;
        let (mut tried_app, mut tried_up) = (false, false);
        loop {
            let check = self.check().await;
            let wait = match check.as_ref().map(|c| c.state) {
                Err(_) => {
                    if !tried_app {
                        tried_app = true;
                        app_started = start_app().then(Instant::now);
                    }
                    app_started.is_some()
                }
                Ok(State::Stopped) if !tried_up => {
                    tried_up = true;
                    match self.output(&["up"], Duration::from_secs(20)).await {
                        Ok(_) => true,
                        Err(e) => {
                            eprintln!("{e:#}");
                            false
                        }
                    }
                }
                Ok(State::LoggedOut) => app_started.is_some_and(|t| t.elapsed() < patience.login),
                Ok(State::Connecting) => true,
                Ok(_) => false,
            };
            if !wait || began.elapsed() >= patience.settle {
                return check;
            }
            tokio::time::sleep(patience.poll).await;
        }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const QUICK: Patience = Patience {
        settle: Duration::from_secs(20),
        login: Duration::from_secs(5),
        poll: Duration::from_millis(20),
    };

    /// A `tailscale` that answers `status` by what is in its folder: nothing until `running`
    /// exists, then "logged out" until `loaded` does (if `slow_to_load`), turned off until `up`
    /// has been run, then ready.
    fn cli(name: &str, slow_to_load: bool) -> (TailscaleCli, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cedar-ts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let loading = if slow_to_load {
            r#"[ -e "$dir/loaded" ] || { : > "$dir/loaded"; echo '{"BackendState": "NoState"}'; exit 0; }"#
        } else {
            ""
        };
        let script = format!(
            r#"#!/bin/sh
dir=$(dirname "$0")
echo "$1" >> "$dir/calls"
case "$1" in
status)
    [ -e "$dir/running" ] || {{ echo "failed to connect to local Tailscale service" >&2; exit 1; }}
    {loading}
    if [ -e "$dir/up" ]; then
        echo '{{"BackendState": "Running", "Self": {{"DNSName": "my-mac.tail1234.ts.net."}}, "CertDomains": ["my-mac.tail1234.ts.net"]}}'
    else
        echo '{{"BackendState": "Stopped"}}'
    fi ;;
up) : > "$dir/up" ;;
esac
"#
        );
        let bin = dir.join("tailscale");
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (TailscaleCli { bin }, dir)
    }

    fn calls(dir: &std::path::Path, what: &str) -> usize {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap()
            .lines()
            .filter(|l| *l == what)
            .count()
    }

    #[tokio::test]
    async fn a_tailscale_that_is_not_running_is_started_and_connected() {
        let (cli, dir) = cli("start", true);
        let mut starts = 0;
        let url = cli
            .url_after(
                443,
                || {
                    starts += 1;
                    std::fs::write(dir.join("running"), "").unwrap();
                    true
                },
                QUICK,
            )
            .await
            .unwrap();
        assert_eq!(url, "https://my-mac.tail1234.ts.net");
        assert_eq!((starts, calls(&dir, "up")), (1, 1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn one_that_is_ready_is_left_alone() {
        let (cli, dir) = cli("ready", false);
        std::fs::write(dir.join("running"), "").unwrap();
        std::fs::write(dir.join("up"), "").unwrap();
        let url = cli.url_after(8443, || panic!("it runs"), QUICK).await.unwrap();
        assert_eq!(url, "https://my-mac.tail1234.ts.net:8443");
        assert_eq!((calls(&dir, "status"), calls(&dir, "up")), (1, 0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn nothing_to_start_fails_at_once_with_what_the_cli_said() {
        let (cli, dir) = cli("none", false);
        let mut starts = 0;
        let err = cli
            .url_after(
                443,
                || {
                    starts += 1;
                    false
                },
                QUICK,
            )
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("failed to connect"), "{err:#}");
        assert_eq!((starts, calls(&dir, "status")), (1, 1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn connecting_starts_tailscale_and_needs_no_https() {
        let (cli, dir) = cli("connect", true);
        // Not running: worth offering only with an app to start.
        assert!(cli.can_connect_with(true).await);
        assert!(!cli.can_connect_with(false).await);
        cli.connect_after(
            || {
                std::fs::write(dir.join("running"), "").unwrap();
                true
            },
            QUICK,
        )
        .await
        .unwrap();
        assert_eq!(calls(&dir, "up"), 1);
        // Connected: nothing left to offer.
        assert!(!cli.can_connect_with(true).await);
        assert!(connected(State::NoHttps) && connected(State::NoMagicDns));
        assert!(!connected(State::LoggedOut) && !connected(State::Connecting));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn one_that_is_turned_off_can_be_connected_and_logged_out_cannot() {
        let (cli, dir) = cli("off", true);
        std::fs::write(dir.join("running"), "").unwrap();
        // The first answer is "logged out" (see `cli`), the next "turned off".
        assert!(!cli.can_connect_with(true).await);
        assert!(cli.can_connect_with(false).await);
        let err = {
            std::fs::remove_file(dir.join("loaded")).unwrap();
            cli.connect_after(|| panic!("it runs"), QUICK).await.unwrap_err()
        };
        assert!(format!("{err:#}").contains("not logged in"), "{err:#}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_tailnet_address_is_told_by_its_name() {
        assert!(on_tailnet("https://spark-7535.tail71d958.ts.net"));
        assert!(on_tailnet("https://Spark.T.TS.NET:8443"));
        assert!(!on_tailnet("https://cedar.example.com"));
        assert!(!on_tailnet("https://ts.net.example.com"));
        assert!(!on_tailnet("spark"));
    }

    #[tokio::test]
    async fn logged_out_is_the_users_to_fix() {
        let (cli, dir) = cli("login", true);
        std::fs::write(dir.join("running"), "").unwrap();
        // Nothing was started, so "logged out" is believed the first time it is said.
        let err = cli.url_after(443, || panic!("it runs"), QUICK).await.unwrap_err();
        assert!(format!("{err:#}").contains("not logged in"), "{err:#}");
        assert_eq!(calls(&dir, "up"), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
