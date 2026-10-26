//! Setting Docker up on Windows from one button on the setup page, for people who have never
//! installed it: turn on WSL 2, download and install Docker Desktop, let the user's account use it,
//! and come back after the restart or sign-out that needs. What can't be done from here (turning
//! on virtualization in the firmware, a Windows too old for WSL 2) is checked first and explained.
//!
//! The parts that need administrator rights run as one PowerShell script, elevated with a single
//! UAC prompt. It is saved as `docker-setup.ps1` in the logs folder, and the elevated command
//! holds its SHA-256: it reads the file once, checks the hash, and runs the text it checked, so
//! nothing unelevated can change the script between writing and running. The script checks the
//! installer's signature itself. It reports its progress in `docker-setup.log` next to it.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::dockersetup::{last_tagged, ERROR};
use std::path::Path;

/// `wsl --install` and the WSL 2 backend need Windows 10 version 2004 or newer.
const MIN_BUILD: u32 = 19041;

const INSTALLER_X64: &str =
    "https://desktop.docker.com/win/main/amd64/Docker%20Desktop%20Installer.exe";
const INSTALLER_ARM64: &str =
    "https://desktop.docker.com/win/main/arm64/Docker%20Desktop%20Installer.exe";

/// The elevated script's exit codes besides 0 (done, nothing more to do) and 1 (failed, the
/// reason on an `ERROR:` line in the log).
const EXIT_SIGN_OUT: i32 = 10;
const EXIT_RESTART: i32 = 11;
/// The script file no longer had the hash it was written with.
const EXIT_CHANGED: i32 = 2;
/// `ERROR_CANCELLED`: the user chose No in the UAC prompt.
const EXIT_DECLINED: i32 = 1223;

/// What the PC has, from [`MACHINE_SCRIPT`].
#[derive(serde::Deserialize, Debug, PartialEq)]
struct Machine {
    build: u32,
    /// A hypervisor already runs (Hyper-V, WSL 2), so virtualization is on whatever the firmware
    /// flag says: Windows reports that flag as off once its own hypervisor has taken it.
    hypervisor: bool,
    firmware: bool,
    arm64: bool,
}

const MACHINE_SCRIPT: &str = "$cs = Get-CimInstance Win32_ComputerSystem; \
    $cpu = @(Get-CimInstance Win32_Processor)[0]; \
    [pscustomobject]@{ build = [int][Environment]::OSVersion.Version.Build; \
    hypervisor = [bool]$cs.HypervisorPresent; firmware = [bool]$cpu.VirtualizationFirmwareEnabled; \
    arm64 = ($cpu.Architecture -eq 12) } | ConvertTo-Json -Compress";

/// Why this PC can't get WSL 2 from here, if it can't.
fn blocker(m: &Machine) -> Option<crate::app::SetupStatus> {
    use crate::app::SetupStatus;
    if m.build < MIN_BUILD {
        return Some(SetupStatus::Failed(format!(
            "WSL 2 needs Windows 10 version 2004 (build {MIN_BUILD}) or newer, and this PC runs \
             build {}. Update Windows in Settings → Windows Update, then try again.",
            m.build
        )));
    }
    if !m.hypervisor && !m.firmware {
        return Some(SetupStatus::NeedsVirtualization);
    }
    None
}

/// How the elevated script ended.
#[derive(Debug, PartialEq)]
enum Outcome {
    Done,
    SignOut,
    Restart,
    Declined,
    Failed(String),
}

fn outcome(code: Option<i32>, log: &str, stderr: &str) -> Outcome {
    match code {
        Some(0) => Outcome::Done,
        Some(EXIT_SIGN_OUT) => Outcome::SignOut,
        Some(EXIT_RESTART) => Outcome::Restart,
        Some(EXIT_DECLINED) => Outcome::Declined,
        Some(EXIT_CHANGED) => Outcome::Failed(
            "the setup script was changed before it could run, so it didn't; try again".into(),
        ),
        _ => Outcome::Failed(
            last_tagged(log, ERROR)
                .or_else(|| Some(stderr.trim().to_string()).filter(|s| !s.is_empty()))
                .unwrap_or_else(|| {
                    format!(
                        "setting up Docker stopped (exit code {}); docker-setup.log in the logs \
                         folder says how far it got",
                        code.map_or("none".to_string(), |c| c.to_string())
                    )
                }),
        ),
    }
}

/// A PowerShell string literal. Single-quoted strings expand nothing; a quote is doubled, and
/// PowerShell takes the typographic single quotes for quotes too.
fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// What `-EncodedCommand` takes: the script as UTF-16LE, base64-encoded.
fn encode(script: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The part that needs administrator rights. `@@LOG@@`, `@@INSTALLER@@` (empty when Docker
/// Desktop is installed already) and `@@USER@@` are filled in by [`elevated_script`].
const ELEVATED_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$log = @@LOG@@
$installer = @@INSTALLER@@
$user = @@USER@@
function Note($line) { Add-Content -LiteralPath $log -Value $line -Encoding UTF8 }
function Fail($message) { Note "ERROR: $message"; exit 1 }
$restart = $false
$signOut = $false
try {
  Note 'STEP: Turning on the Windows Subsystem for Linux'
  foreach ($feature in 'Microsoft-Windows-Subsystem-Linux', 'VirtualMachinePlatform') {
    $featureState = (Get-WindowsOptionalFeature -Online -FeatureName $feature).State
    if ($featureState -ne 'Enabled') {
      if ($featureState -ne 'EnablePending') {
        Enable-WindowsOptionalFeature -Online -FeatureName $feature -All -NoRestart -WarningAction SilentlyContinue | Out-Null
      }
      $restart = $true
    }
  }
  # Not fatal: before the restart that turns WSL on this may not work yet, and Docker Desktop
  # offers the update itself when it starts.
  Note 'STEP: Updating WSL'
  try { & wsl.exe --update --web-download *> $null } catch { }
  if ($LASTEXITCODE -ne 0) { try { & wsl.exe --update *> $null } catch { } }

  if ($installer) {
    Note 'STEP: Checking the Docker Desktop installer'
    # A folder only administrators can write to, so nothing unelevated can swap the installer
    # or put a DLL next to it before it runs.
    $dir = Join-Path $env:SystemRoot ('Temp\cedar-docker-' + [guid]::NewGuid())
    New-Item -ItemType Directory -Path $dir | Out-Null
    & icacls.exe $dir /inheritance:r /grant:r '*S-1-5-32-544:(OI)(CI)F' '*S-1-5-18:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "could not protect $dir (icacls exit $LASTEXITCODE)" }
    $exe = Join-Path $dir 'Docker Desktop Installer.exe'
    Copy-Item -LiteralPath $installer -Destination $exe
    $signature = Get-AuthenticodeSignature -LiteralPath $exe
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O="?Docker Inc') {
      Remove-Item -LiteralPath $dir, $installer -Recurse -Force -ErrorAction SilentlyContinue
      Fail "the downloaded Docker Desktop installer is not signed by Docker (signature: $($signature.Status)). It was deleted; click Try again to download it again."
    }
    Note 'STEP: Installing Docker Desktop. This takes a few minutes.'
    $process = Start-Process -FilePath $exe -ArgumentList 'install', '--quiet', '--accept-license', '--backend=wsl-2' -Wait -PassThru
    Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    if ($process.ExitCode -eq 3010) { $restart = $true }
    elseif ($process.ExitCode -ne 0) { Fail "the Docker Desktop installer stopped with code $($process.ExitCode). Its log is in C:\ProgramData\DockerDesktop." }
    # The installer puts the account it runs as in docker-users, and a Windows session picks up
    # a new group only when it starts.
    $signOut = $true
  }

  # When a standard user's UAC prompt was answered with an administrator's password, this
  # script runs as that administrator, and the user's own account still needs the group.
  if ($user) {
    Note 'STEP: Letting your account use Docker'
    try {
      Add-LocalGroupMember -Group 'docker-users' -Member $user
      $signOut = $true
    } catch {
      if ($_.FullyQualifiedErrorId -notlike 'MemberExists*' -and $_.FullyQualifiedErrorId -notlike 'GroupNotFound*') { throw }
    }
  }
} catch {
  Fail $_.Exception.Message
}
if ($restart) { exit 11 }
if ($signOut) { exit 10 }
exit 0
"#;

fn elevated_script(log: &Path, installer: Option<&Path>, user: &str) -> String {
    let installer = installer
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    ELEVATED_SCRIPT
        .replace("@@LOG@@", &ps_quote(&log.display().to_string()))
        .replace("@@INSTALLER@@", &ps_quote(&installer))
        .replace("@@USER@@", &ps_quote(user))
}

fn sha256_hex(text: &str) -> String {
    use sha2::Digest;
    hex::encode_upper(sha2::Sha256::digest(text.as_bytes()))
}

/// What runs elevated: reads the script at `path` once, and runs that text only if it still has
/// the hash it was written with. Short, so the elevated command line stays short.
fn stub(path: &Path, sha256: &str) -> String {
    format!(
        "$s = [IO.File]::ReadAllText({path}); \
         $h = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($s))).Replace('-', ''); \
         if ($h -ne '{sha256}') {{ exit {EXIT_CHANGED} }}; Invoke-Expression $s",
        path = ps_quote(&path.display().to_string()),
    )
}

/// Runs `encoded` elevated and exits with its exit code, or with [`EXIT_DECLINED`] when the
/// user says No. Only single quotes, so it passes through the command line as it is.
fn launcher(powershell: &Path, encoded: &str) -> String {
    format!(
        "try {{ $p = Start-Process -FilePath {ps} -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList '-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {encoded}'; \
         exit $p.ExitCode }} catch {{ $e = $_.Exception; while ($e) {{ \
         if ($e.NativeErrorCode -eq {EXIT_DECLINED}) {{ exit {EXIT_DECLINED} }}; $e = $e.InnerException }}; \
         if ($_.Exception.Message -match 'canceled by the user') {{ exit {EXIT_DECLINED} }}; \
         [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}",
        ps = ps_quote(&powershell.display().to_string()),
    )
}

#[cfg(windows)]
pub use windows::{install, restart};

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::app::{AppState, SetupStatus};
    use crate::{docker, dockersetup, proc};
    use anyhow::{bail, Context, Result};
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::Duration;
    use tauri::{AppHandle, Manager};

    fn system32(exe: &str) -> PathBuf {
        let root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        root.join("System32").join(exe)
    }

    pub(super) fn powershell() -> PathBuf {
        system32(r"WindowsPowerShell\v1.0\powershell.exe")
    }

    /// `DOMAIN\user` of the signed-in user, which the elevated script may not run as.
    fn current_user() -> String {
        match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
            (Ok(domain), Ok(user)) if !domain.is_empty() => format!("{domain}\\{user}"),
            (_, Ok(user)) => user,
            _ => String::new(),
        }
    }

    /// Runs the whole setup; the page follows along in [`AppState::docker_setup`]. Once nothing
    /// is left to do, the launch sequence runs again and starts Docker Desktop.
    pub async fn install(app: AppHandle) {
        let state = app.state::<AppState>();
        let next = run(&state)
            .await
            .unwrap_or_else(|e| SetupStatus::Failed(format!("{e:#}")));
        let done = next == SetupStatus::Idle;
        state.set_docker_setup(next);
        if done {
            crate::app::bootstrap(app.clone()).await;
        }
    }

    async fn run(state: &AppState) -> Result<SetupStatus> {
        state.set_docker_setup(SetupStatus::Running("Checking this PC".into()));
        let machine = machine().await?;
        if let Some(blocked) = blocker(&machine) {
            return Ok(blocked);
        }
        let installer = match docker::desktop_app() {
            Some(_) => None,
            None => Some(download_installer(state, machine.arm64).await?),
        };

        state.set_docker_setup(SetupStatus::Running(
            "Waiting for permission: Windows asks whether Cedar may make changes to this PC. \
             Choose Yes. If no prompt appears, click the flashing shield in the taskbar."
                .into(),
        ));
        let log = state.paths.logs.join("docker-setup.log");
        std::fs::write(&log, "").with_context(|| format!("creating {}", log.display()))?;
        let script = elevated_script(&log, installer.as_deref(), &current_user());
        let script_path = state.paths.logs.join("docker-setup.ps1");
        std::fs::write(&script_path, &script)
            .with_context(|| format!("writing {}", script_path.display()))?;
        let elevated = stub(&script_path, &sha256_hex(&script));
        let (code, stderr) = run_elevated(state, &elevated, &log).await?;
        let log_text = std::fs::read_to_string(&log).unwrap_or_default();

        let result = outcome(code, &log_text, &stderr);
        // Kept after a failure, so trying again doesn't download it again.
        if matches!(result, Outcome::Done | Outcome::SignOut | Outcome::Restart) {
            if let Some(dir) = installer.as_deref().and_then(Path::parent) {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        Ok(match result {
            Outcome::Done => SetupStatus::Idle,
            Outcome::SignOut => {
                open_again_after_sign_in()?;
                SetupStatus::NeedsSignOut
            }
            Outcome::Restart => {
                open_again_after_sign_in()?;
                SetupStatus::NeedsRestart
            }
            Outcome::Declined => SetupStatus::Failed(
                "Windows did not get permission to make changes, so nothing was installed. Click \
                 Set up Docker again and choose Yes; on a work PC, an administrator may have to \
                 enter their password."
                    .into(),
            ),
            Outcome::Failed(why) => SetupStatus::Failed(why),
        })
    }

    async fn machine() -> Result<Machine> {
        let ps = powershell();
        let out = tokio::time::timeout(
            Duration::from_secs(60),
            proc::command(&ps)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                ])
                .arg(MACHINE_SCRIPT)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("checking this PC took longer than a minute")?
        .with_context(|| format!("running {}", ps.display()))?;
        if !out.status.success() {
            bail!(
                "checking this PC failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let text = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(text.trim())
            .with_context(|| format!("reading what this PC has: {}", text.trim()))
    }

    async fn download_installer(state: &AppState, arm64: bool) -> Result<PathBuf> {
        let url = if arm64 {
            INSTALLER_ARM64
        } else {
            INSTALLER_X64
        };
        dockersetup::download(state, url, "Docker Desktop Installer.exe").await
    }

    /// The exit code of the elevated `command`, and what went wrong if it never ran. The
    /// `STEP:` lines in `log` are shown on the page while it runs.
    async fn run_elevated(
        state: &AppState,
        command: &str,
        log: &Path,
    ) -> Result<(Option<i32>, String)> {
        let ps = powershell();
        let child = proc::command(&ps)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
            ])
            .arg(launcher(&ps, &encode(command)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("running {}", ps.display()))?;
        dockersetup::follow(state, child, log).await
    }

    /// Starts Cedar once at the next sign-in, so the setup carries on by itself after the
    /// restart or sign-out. `RunOnce` removes the entry when it runs.
    fn open_again_after_sign_in() -> Result<()> {
        let exe = std::env::current_exe()?;
        let status = proc::std_command(system32("reg.exe"))
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce",
                "/v",
                "Cedar",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\"", exe.display()),
                "/f",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("running reg.exe")?;
        if !status.success() {
            bail!("could not set Cedar to open after the restart (reg.exe {status})");
        }
        Ok(())
    }

    /// Restarts the PC, or signs out, to finish the setup.
    pub fn restart(sign_out: bool) -> Result<()> {
        let args: &[&str] = if sign_out {
            &["/l"]
        } else {
            &["/r", "/t", "0"]
        };
        proc::std_command(system32("shutdown.exe"))
            .args(args)
            .spawn()
            .context("running shutdown.exe")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SetupStatus;

    fn machine(build: u32, hypervisor: bool, firmware: bool) -> Machine {
        Machine {
            build,
            hypervisor,
            firmware,
            arm64: false,
        }
    }

    #[test]
    fn machine_report_parses() {
        let m: Machine = serde_json::from_str(
            r#"{"build":22631,"hypervisor":false,"firmware":true,"arm64":false}"#,
        )
        .unwrap();
        assert_eq!(m, machine(22631, false, true));
    }

    #[test]
    fn blockers() {
        assert!(matches!(
            blocker(&machine(18363, true, true)),
            Some(SetupStatus::Failed(_))
        ));
        assert_eq!(
            blocker(&machine(22631, false, false)),
            Some(SetupStatus::NeedsVirtualization)
        );
        // Windows reports the firmware flag as off once its own hypervisor runs.
        assert_eq!(blocker(&machine(22631, true, false)), None);
        assert_eq!(blocker(&machine(19041, false, true)), None);
    }

    #[test]
    fn outcomes() {
        assert_eq!(outcome(Some(0), "", ""), Outcome::Done);
        assert_eq!(outcome(Some(10), "", ""), Outcome::SignOut);
        assert_eq!(outcome(Some(11), "", ""), Outcome::Restart);
        assert_eq!(outcome(Some(1223), "", ""), Outcome::Declined);
        let log = "\u{feff}STEP: Turning on WSL\nERROR: first\nSTEP: Installing\nERROR: the installer stopped\n";
        assert_eq!(
            outcome(Some(1), log, "ignored"),
            Outcome::Failed("the installer stopped".into())
        );
        assert_eq!(
            outcome(Some(1), "", " no access \n"),
            Outcome::Failed("no access".into())
        );
        assert!(matches!(outcome(None, "", ""), Outcome::Failed(m) if m.contains("none")));
    }

    #[test]
    fn quoting() {
        assert_eq!(ps_quote(r"C:\Users\o'brien\x"), r"'C:\Users\o''brien\x'");
        assert_eq!(ps_quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
        assert_eq!(ps_quote(""), "''");
    }

    #[test]
    fn encoding_is_utf16le_base64() {
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('exit 0'))
        assert_eq!(encode("exit 0"), "ZQB4AGkAdAAgADAA");
    }

    #[test]
    fn script_is_filled_in() {
        let s = elevated_script(
            Path::new(r"C:\Users\o'brien\logs\docker-setup.log"),
            None,
            r"PC\o'brien",
        );
        assert!(s.contains(r"$log = 'C:\Users\o''brien\logs\docker-setup.log'"));
        assert!(s.contains("$installer = ''"));
        assert!(s.contains(r"$user = 'PC\o''brien'"));
        assert!(!s.contains("@@"));
        // The exit codes the script uses are the ones read back.
        assert!(s.contains(&format!("exit {EXIT_RESTART}")));
        assert!(s.contains(&format!("exit {EXIT_SIGN_OUT}")));
    }

    #[test]
    fn stub_checks_the_hash() {
        // [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes('exit 0'))).Replace('-', '')
        let hash = sha256_hex("exit 0");
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, hash.to_uppercase());
        let s = stub(Path::new(r"C:\Users\o'brien\docker-setup.ps1"), &hash);
        assert!(s.contains(r"ReadAllText('C:\Users\o''brien\docker-setup.ps1')"));
        assert!(s.contains(&format!("if ($h -ne '{hash}') {{ exit {EXIT_CHANGED} }}")));
        // Short enough for any elevated command line, encoded and all.
        assert!(encode(&s).len() < 1500, "{}", encode(&s).len());
        assert!(
            matches!(outcome(Some(EXIT_CHANGED), "", ""), Outcome::Failed(m) if m.contains("changed"))
        );
    }

    #[test]
    fn launcher_has_no_double_quotes() {
        let l = launcher(
            Path::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            "QUJD",
        );
        assert!(!l.contains('"'));
        assert!(l.contains("-EncodedCommand QUJD'"));
        assert!(l.contains("exit 1223"));
    }

    /// PowerShell's own parser accepts every script Cedar runs, and the PC check reads back.
    #[cfg(windows)]
    #[test]
    fn powershell_takes_the_scripts() {
        let ps = windows::powershell();
        let scripts = [
            elevated_script(
                Path::new(r"C:\Users\o'brien\logs\docker-setup.log"),
                Some(Path::new(r"C:\Temp\Docker Desktop Installer.exe")),
                r"PC\o'brien",
            ),
            MACHINE_SCRIPT.to_string(),
            stub(
                Path::new(r"C:\Temp\docker-setup.ps1"),
                &sha256_hex("exit 0"),
            ),
            launcher(&ps, &encode("exit 0")),
        ];
        for script in scripts {
            let check = format!(
                "$errors = $null; \
                 [void][System.Management.Automation.Language.Parser]::ParseInput({}, [ref]$null, [ref]$errors); \
                 if ($errors) {{ $errors | ForEach-Object {{ [Console]::Error.WriteLine($_.ToString()) }}; exit 1 }}",
                ps_quote(&script)
            );
            let out = std::process::Command::new(&ps)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-EncodedCommand",
                    &encode(&check),
                ])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n---\n{script}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        // The stub runs the script it hashed, and its `exit` ends PowerShell with that code.
        let path = std::env::temp_dir().join(format!("cedar-stub-test-{}.ps1", std::process::id()));
        let script = "function Done($code) { exit $code }\nDone 7\n";
        std::fs::write(&path, script).unwrap();
        let run_stub = || {
            std::process::Command::new(&ps)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-EncodedCommand",
                ])
                .arg(encode(&stub(&path, &sha256_hex(script))))
                .status()
                .unwrap()
                .code()
        };
        assert_eq!(run_stub(), Some(7));
        std::fs::write(&path, "exit 0\n").unwrap();
        assert_eq!(run_stub(), Some(EXIT_CHANGED));
        std::fs::remove_file(&path).unwrap();

        let out = std::process::Command::new(&ps)
            .args(["-NoProfile", "-NonInteractive", "-Command", MACHINE_SCRIPT])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let m: Machine = serde_json::from_slice(&out.stdout).unwrap();
        assert!(m.build >= 10240, "{m:?}");
    }
}
