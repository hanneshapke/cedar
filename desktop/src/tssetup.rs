//! Installing Tailscale from the setup page, for sharing the chat with the phone apps on a
//! computer that doesn't have it yet: download Tailscale's own installer, check it is signed by
//! Tailscale, install it with the same one permission prompt as the Docker setup (winsetup.rs,
//! macsetup.rs), and open Tailscale. Logging in, and turning on MagicDNS and HTTPS Certificates
//! for the tailnet, stay the user's; the page says what is left.
//!
//! On a Mac it is the standalone app (`Tailscale-latest-macos.pkg`, the variant Tailscale
//! recommends), installed with `installer`. On Windows it is the MSI for the PC's processor, from
//! Tailscale's package list, installed with `msiexec /quiet`. Progress and errors go to
//! `tailscale-setup.log` in the logs folder.

#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use crate::i18n::{t, tf};
use crate::installer::last_error;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// Tailscale's standalone Mac app, a universal package, as linked from its macOS install guide.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MAC_PKG: &str = "https://pkgs.tailscale.com/stable/Tailscale-latest-macos.pkg";

/// Tailscale's stable packages, and the list of the current Windows ones.
#[cfg_attr(not(windows), allow(dead_code))]
const PKGS: &str = "https://pkgs.tailscale.com/stable/";
#[cfg_attr(not(windows), allow(dead_code))]
const WINDOWS_INDEX: &str = "https://pkgs.tailscale.com/stable/?mode=json&os=windows";

/// Tailscale Inc's Apple Developer team, on the Developer ID certificate that signs its package.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const TAILSCALE_TEAM: &str = "W5364U7YZB";

/// The parts of the Windows package list this needs: the MSI for each processor.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(serde::Deserialize, Debug, Default)]
#[serde(default)]
struct WindowsIndex {
    #[serde(rename = "MSIs")]
    msis: HashMap<String, String>,
}

/// The address of the MSI for the PC's processor, from the package list `index`. The list may
/// name the file or give its whole address.
#[cfg_attr(not(windows), allow(dead_code))]
fn msi_url(index: &str, arm64: bool) -> Result<String> {
    let index: WindowsIndex =
        serde_json::from_str(index).context("reading Tailscale's list of packages")?;
    let arch = if arm64 { "arm64" } else { "amd64" };
    let file = index
        .msis
        .get(arch)
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .ok_or_else(|| anyhow!("Tailscale's list of packages has no {arch} installer"))?;
    Ok(if file.starts_with("https://") {
        file.to_string()
    } else {
        format!("{PKGS}{}", file.trim_start_matches('/'))
    })
}

/// How the elevated script ended.
#[derive(Debug, PartialEq)]
enum Outcome {
    Done,
    Declined,
    Failed(String),
}

/// `declined` is whether the permission prompt was answered No (each platform says so its own
/// way).
fn outcome(code: Option<i32>, log: &str, stderr: &str, declined: bool) -> Outcome {
    if code == Some(0) {
        return Outcome::Done;
    }
    if declined {
        return Outcome::Declined;
    }
    Outcome::Failed(
        last_error(log)
            .or_else(|| Some(stderr.trim().to_string()).filter(|s| !s.is_empty()))
            .unwrap_or_else(|| {
                let code = code.map_or(t("setup.none"), |c| c.to_string());
                tf("setup.tailscale_stopped", &[("code", &code)])
            }),
    )
}

/// The Mac's part that runs as root, for `/bin/sh`. `@@LOG@@`, `@@PKG@@` and `@@TEAM@@` are filled
/// in by [`mac_script`].
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MAC_SCRIPT: &str = r#"
log=@@LOG@@
pkg=@@PKG@@
note() { printf '%s\n' "$1" >> "$log"; }
fail() { note "ERROR: $1"; exit 1; }
# A folder only root can write to, so nothing else can swap the package after it is checked.
dir=$(/usr/bin/mktemp -d /private/var/tmp/cedar-tailscale.XXXXXX) || fail 'could not make a folder for the Tailscale installer'
trap '/bin/rm -rf "$dir"' EXIT

note 'STEP: Checking the Tailscale installer'
/bin/cp "$pkg" "$dir/Tailscale.pkg" || fail "could not copy $pkg"
signature=$(/usr/sbin/pkgutil --check-signature "$dir/Tailscale.pkg" 2>&1)
case "$signature" in
  *'signed by a developer certificate issued by Apple'*) ;;
  *) /bin/rm -f "$pkg"; fail 'the downloaded Tailscale installer is not signed with a Developer ID. It was deleted; click Try again to download it again.' ;;
esac
printf '%s\n' "$signature" | /usr/bin/grep -q 'Developer ID Installer: .*(@@TEAM@@)' || {
  /bin/rm -f "$pkg"
  fail 'the downloaded Tailscale installer is not signed by Tailscale. It was deleted; click Try again to download it again.'
}

note 'STEP: Installing Tailscale into Applications'
/usr/sbin/installer -pkg "$dir/Tailscale.pkg" -target / </dev/null >>"$log" 2>&1 || fail "the Tailscale installer stopped with code $?. Its output is in tailscale-setup.log in the logs folder."
exit 0
"#;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_script(log: &Path, pkg: &Path) -> String {
    use crate::macsetup::sh_quote;
    MAC_SCRIPT
        .replace("@@LOG@@", &sh_quote(&log.display().to_string()))
        .replace("@@PKG@@", &sh_quote(&pkg.display().to_string()))
        .replace("@@TEAM@@", TAILSCALE_TEAM)
}

/// The Windows part that needs administrator rights. `@@LOG@@` and `@@MSI@@` are filled in by
/// [`windows_script`].
#[cfg_attr(not(windows), allow(dead_code))]
const WINDOWS_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$log = @@LOG@@
$msi = @@MSI@@
function Note($line) { Add-Content -LiteralPath $log -Value $line -Encoding UTF8 }
function Fail($message) { Note "ERROR: $message"; exit 1 }
try {
  Note 'STEP: Checking the Tailscale installer'
  # A folder only administrators can write to, so nothing unelevated can swap the installer
  # after it is checked.
  $dir = Join-Path $env:SystemRoot ('Temp\cedar-tailscale-' + [guid]::NewGuid())
  New-Item -ItemType Directory -Path $dir | Out-Null
  & icacls.exe $dir /inheritance:r /grant:r '*S-1-5-32-544:(OI)(CI)F' '*S-1-5-18:(OI)(CI)F' | Out-Null
  if ($LASTEXITCODE -ne 0) { Fail "could not protect $dir (icacls exit $LASTEXITCODE)" }
  $file = Join-Path $dir 'tailscale-setup.msi'
  Copy-Item -LiteralPath $msi -Destination $file
  $signature = Get-AuthenticodeSignature -LiteralPath $file
  if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O="?Tailscale Inc') {
    Remove-Item -LiteralPath $dir, $msi -Recurse -Force -ErrorAction SilentlyContinue
    Fail "the downloaded Tailscale installer is not signed by Tailscale (signature: $($signature.Status)). It was deleted; click Try again to download it again."
  }
  Note 'STEP: Installing Tailscale'
  $msiexec = Join-Path $env:SystemRoot 'System32\msiexec.exe'
  $process = Start-Process -FilePath $msiexec -ArgumentList '/i', ('"' + $file + '"'), '/quiet', '/norestart' -Wait -PassThru
  Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
  # 3010: installed, and a restart finishes it; Tailscale runs before that.
  if ($process.ExitCode -ne 0 -and $process.ExitCode -ne 3010) { Fail "the Tailscale installer stopped with code $($process.ExitCode)." }
} catch {
  Fail $_.Exception.Message
}
exit 0
"#;

#[cfg_attr(not(windows), allow(dead_code))]
fn windows_script(log: &Path, msi: &Path) -> String {
    use crate::winsetup::ps_quote;
    WINDOWS_SCRIPT
        .replace("@@LOG@@", &ps_quote(&log.display().to_string()))
        .replace("@@MSI@@", &ps_quote(&msi.display().to_string()))
}

#[cfg(any(windows, target_os = "macos"))]
pub use platform::install;

#[cfg(any(windows, target_os = "macos"))]
mod platform {
    use super::*;
    use crate::app::{AppState, SetupStatus};
    use crate::{docker, installer};
    use std::path::PathBuf;
    use tauri::{AppHandle, Manager};

    /// Runs the whole installation; the page follows along in [`AppState::tailscale_setup`].
    pub async fn install(app: AppHandle) {
        let state = app.state::<AppState>();
        let next = run(&state)
            .await
            .unwrap_or_else(|e| SetupStatus::Failed(format!("{e:#}")));
        state.set_tailscale_setup(next);
    }

    async fn run(state: &AppState) -> Result<SetupStatus> {
        let step = |s: String| state.set_tailscale_setup(SetupStatus::Running(s));
        let downloaded = download(&step).await?;
        let log = state.paths.logs.join("tailscale-setup.log");

        #[cfg(target_os = "macos")]
        let (result, declined_text) = {
            step(crate::macsetup::waiting_for_password());
            let script = mac_script(&log, &downloaded);
            let (code, stderr, log_text) = crate::macsetup::run_as_admin(
                &state.paths.logs,
                "tailscale-setup",
                &script,
                &t("mac.prompt_tailscale"),
                &step,
            )
            .await?;
            let declined = crate::macsetup::declined(&stderr);
            (
                outcome(code, &log_text, &stderr, declined),
                t("mac.declined"),
            )
        };
        #[cfg(windows)]
        let (result, declined_text) = {
            use crate::winsetup::{EXIT_CHANGED, EXIT_DECLINED};
            step(crate::winsetup::waiting_for_permission());
            let script = windows_script(&log, &downloaded);
            let (code, stderr, log_text) = crate::winsetup::run_elevated_script(
                &state.paths.logs,
                "tailscale-setup",
                &script,
                &step,
            )
            .await?;
            let result = if code == Some(EXIT_CHANGED) {
                Outcome::Failed(t("setup.script_changed"))
            } else {
                outcome(code, &log_text, &stderr, code == Some(EXIT_DECLINED))
            };
            (
                result,
                t("win.declined_tailscale"),
            )
        };

        Ok(match result {
            Outcome::Done => {
                // Kept after a failure, so trying again doesn't download it again.
                if let Some(dir) = downloaded.parent() {
                    let _ = std::fs::remove_dir_all(dir);
                }
                if let Some(app) = tailscale_app().filter(|a| a.exists()) {
                    if let Err(e) = docker::launch(&app) {
                        eprintln!("opening Tailscale: {e:#}");
                    }
                }
                SetupStatus::Installed
            }
            Outcome::Declined => SetupStatus::Failed(declined_text),
            Outcome::Failed(why) => SetupStatus::Failed(why),
        })
    }

    #[cfg(target_os = "macos")]
    async fn download(step: &impl Fn(String)) -> Result<PathBuf> {
        installer::download("Tailscale", MAC_PKG, "Tailscale.pkg", step).await
    }

    #[cfg(windows)]
    async fn download(step: &impl Fn(String)) -> Result<PathBuf> {
        step(t("setup.checking_pc"));
        let arm64 = crate::winsetup::machine_is_arm64().await?;
        let index = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(60))
            .build()?
            .get(WINDOWS_INDEX)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .context("looking up the current Tailscale; check the internet connection")?
            .text()
            .await
            .context("looking up the current Tailscale")?;
        let url = msi_url(&index, arm64)?;
        installer::download("Tailscale", &url, "tailscale-setup.msi", step).await
    }

    /// Tailscale's own app, to open after installing it so the user can log in.
    #[cfg(target_os = "macos")]
    fn tailscale_app() -> Option<PathBuf> {
        Some(PathBuf::from("/Applications/Tailscale.app"))
    }

    #[cfg(windows)]
    fn tailscale_app() -> Option<PathBuf> {
        std::env::var_os("ProgramFiles")
            .map(|pf| PathBuf::from(pf).join(r"Tailscale\tailscale-ipn.exe"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msi_for_the_processor() {
        let index = r#"{"Version":"1.104.2","MSIs":{"amd64":"tailscale-setup-1.104.2-amd64.msi","arm64":"tailscale-setup-1.104.2-arm64.msi","x86":"tailscale-setup-1.104.2-x86.msi"},"Exes":["tailscale-setup-1.104.2.exe"]}"#;
        assert_eq!(
            msi_url(index, false).unwrap(),
            "https://pkgs.tailscale.com/stable/tailscale-setup-1.104.2-amd64.msi"
        );
        assert_eq!(
            msi_url(index, true).unwrap(),
            "https://pkgs.tailscale.com/stable/tailscale-setup-1.104.2-arm64.msi"
        );
        let whole = r#"{"MSIs":{"amd64":"https://pkgs.tailscale.com/stable/x.msi"}}"#;
        assert_eq!(
            msi_url(whole, false).unwrap(),
            "https://pkgs.tailscale.com/stable/x.msi"
        );
        assert!(msi_url(whole, true).unwrap_err().to_string().contains("arm64"));
        assert!(msi_url("not json", false).is_err());
    }

    #[test]
    fn outcomes() {
        assert_eq!(outcome(Some(0), "", "", false), Outcome::Done);
        assert_eq!(outcome(Some(1), "", "", true), Outcome::Declined);
        assert_eq!(
            outcome(Some(1), "STEP: Installing\nERROR: not signed\n", "x", false),
            Outcome::Failed("not signed".into())
        );
        assert!(matches!(outcome(None, "", "", false), Outcome::Failed(m) if m.contains("none")));
    }

    #[test]
    fn scripts_are_filled_in() {
        let mac = mac_script(
            Path::new("/Users/o'brien/Library/Logs/cedar/tailscale-setup.log"),
            Path::new("/var/folders/x/T/cedar-tailscale-setup/Tailscale.pkg"),
        );
        assert!(mac.contains(r"log='/Users/o'\''brien/Library/Logs/cedar/tailscale-setup.log'"));
        assert!(mac.contains(&format!("({TAILSCALE_TEAM})")));
        assert!(!mac.contains("@@"));

        let win = windows_script(
            Path::new(r"C:\Users\o'brien\logs\tailscale-setup.log"),
            Path::new(r"C:\Temp\tailscale-setup.msi"),
        );
        assert!(win.contains(r"$log = 'C:\Users\o''brien\logs\tailscale-setup.log'"));
        assert!(win.contains(r"$msi = 'C:\Temp\tailscale-setup.msi'"));
        assert!(!win.contains("@@"));
    }

    /// `/bin/sh` accepts the Mac script, and without the package it stops with an `ERROR:` line.
    #[cfg(unix)]
    #[test]
    fn sh_takes_the_mac_script() {
        let dir = std::env::temp_dir().join(format!("cedar-tssetup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("tailscale-setup.log");
        let script = mac_script(&log, &dir.join("missing.pkg"));
        let check = std::process::Command::new("/bin/sh")
            .args(["-n", "-c", &script])
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "{}",
            String::from_utf8_lossy(&check.stderr)
        );
        let run = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(1));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(crate::installer::last_tagged(&text, crate::installer::ERROR).is_some(), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// PowerShell's own parser accepts the Windows script.
    #[cfg(windows)]
    #[test]
    fn powershell_takes_the_script() {
        use crate::winsetup::ps_quote;
        let script = windows_script(
            Path::new(r"C:\Users\o'brien\logs\tailscale-setup.log"),
            Path::new(r"C:\Temp\tailscale-setup.msi"),
        );
        let check = format!(
            "$errors = $null; \
             [void][System.Management.Automation.Language.Parser]::ParseInput({}, [ref]$null, [ref]$errors); \
             if ($errors) {{ $errors | ForEach-Object {{ [Console]::Error.WriteLine($_.ToString()) }}; exit 1 }}",
            ps_quote(&script)
        );
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let out = std::process::Command::new(format!(
            r"{root}\System32\WindowsPowerShell\v1.0\powershell.exe"
        ))
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(crate::winsetup::encode(&check))
        .output()
        .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
