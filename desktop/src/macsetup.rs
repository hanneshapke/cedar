//! Setting Docker up on a Mac from one button on the setup page, for people who have never
//! installed it: download Docker Desktop for the Mac's chip, install it into Applications with the
//! privileged parts done up front (so its first start asks for nothing), and start it. A Mac that
//! can't run virtual machines is checked for first and explained.
//!
//! The part that needs administrator rights is one shell script, run by `osascript`'s `do shell
//! script … with administrator privileges`, which asks for an administrator's password once. The
//! script goes in as an argument, never as a file, so nothing can change it between writing and
//! running. It copies the disk image into a folder only root can write to, checks that the app on
//! it is signed by Docker, and runs Docker's own `install --accept-license --user=<you>`. It
//! reports its progress in `docker-setup.log` in the logs folder.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::installer::{last_tagged, ERROR};
use std::path::Path;

const DMG_ARM64: &str = "https://desktop.docker.com/mac/main/arm64/Docker.dmg";
const DMG_X64: &str = "https://desktop.docker.com/mac/main/amd64/Docker.dmg";

/// Docker Inc's Apple Developer team. Its apps are signed with a Developer ID certificate that
/// carries it.
const DOCKER_TEAM: &str = "9BNSXJN65R";

/// What the Mac has, from `sysctl`.
#[derive(Debug, PartialEq)]
struct Machine {
    /// Apple Silicon, even when Cedar itself runs under Rosetta.
    arm64: bool,
    /// The Hypervisor framework works here, which Docker Desktop's VM needs.
    hypervisor: bool,
}

/// A `sysctl -n` flag: `1` is on; anything else (`0`, or nothing for a name this Mac doesn't
/// have, like `hw.optional.arm64` on an Intel Mac) is off.
fn sysctl_flag(out: &str) -> bool {
    out.trim() == "1"
}

/// Why this Mac can't run Docker Desktop, if it can't.
fn blocker(m: &Machine) -> Option<crate::app::SetupStatus> {
    (!m.hypervisor).then(|| {
        crate::app::SetupStatus::Failed(
            "This Mac can't run virtual machines (macOS reports no Hypervisor support), and \
             Docker runs its containers in one. That happens on Macs from before 2010 and inside \
             a virtual machine. Run Docker on another computer, or use a Mac that supports it."
                .into(),
        )
    })
}

/// How the elevated script ended.
#[derive(Debug, PartialEq)]
enum Outcome {
    Done,
    Declined,
    Failed(String),
}

/// `osascript` exits with 1 on any error and says which on stderr: `(-128)` is the password
/// prompt being cancelled.
pub fn declined(stderr: &str) -> bool {
    stderr.contains("(-128)") || stderr.contains("User canceled")
}

fn outcome(code: Option<i32>, log: &str, stderr: &str) -> Outcome {
    if code == Some(0) {
        return Outcome::Done;
    }
    if declined(stderr) {
        return Outcome::Declined;
    }
    Outcome::Failed(
        last_tagged(log, ERROR)
            .or_else(|| Some(stderr.trim().to_string()).filter(|s| !s.is_empty()))
            .unwrap_or_else(|| {
                format!(
                    "setting up Docker stopped (exit code {}); docker-setup.log in the logs \
                     folder says how far it got",
                    code.map_or("none".to_string(), |c| c.to_string())
                )
            }),
    )
}

/// A shell word: single quotes expand nothing, and a quote ends them, is escaped, and opens them
/// again.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The part that needs administrator rights, for `/bin/sh`. `@@LOG@@`, `@@DMG@@`, `@@USER@@` and
/// `@@TEAM@@` are filled in by [`elevated_script`].
const ELEVATED_SCRIPT: &str = r#"
log=@@LOG@@
dmg=@@DMG@@
user=@@USER@@
note() { printf '%s\n' "$1" >> "$log"; }
fail() { note "ERROR: $1"; exit 1; }
# A folder only root can write to, so nothing else can swap the disk image after it is checked.
dir=$(/usr/bin/mktemp -d /private/var/tmp/cedar-docker.XXXXXX) || fail 'could not make a folder for the Docker Desktop disk image'
mnt="$dir/mnt"
cleanup() {
  /usr/bin/hdiutil detach -quiet -force "$mnt" >/dev/null 2>&1
  /bin/rm -rf "$dir"
}
trap cleanup EXIT

note 'STEP: Checking the Docker Desktop disk image'
/bin/cp "$dmg" "$dir/Docker.dmg" || fail "could not copy $dmg"
/bin/mkdir "$mnt" || fail "could not make $mnt"
/usr/bin/hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$dir/Docker.dmg" </dev/null >/dev/null 2>&1 || {
  /bin/rm -f "$dmg"
  fail 'the downloaded Docker Desktop disk image could not be opened. It was deleted; click Try again to download it again.'
}
app="$mnt/Docker.app"
if ! /usr/bin/codesign --verify --deep --strict -R='anchor apple generic and certificate leaf[subject.OU] = "@@TEAM@@"' "$app" >/dev/null 2>&1; then
  /bin/rm -f "$dmg"
  fail 'the downloaded Docker Desktop is not signed by Docker. It was deleted; click Try again to download it again.'
fi

note 'STEP: Installing Docker Desktop into Applications. This takes a minute or two.'
"$app/Contents/MacOS/install" --accept-license --user="$user" </dev/null >>"$log" 2>&1 || fail "the Docker Desktop installer stopped with code $?. Its output is in docker-setup.log in the logs folder."
exit 0
"#;

fn elevated_script(log: &Path, dmg: &Path, user: &str) -> String {
    ELEVATED_SCRIPT
        .replace("@@LOG@@", &sh_quote(&log.display().to_string()))
        .replace("@@DMG@@", &sh_quote(&dmg.display().to_string()))
        .replace("@@USER@@", &sh_quote(user))
        .replace("@@TEAM@@", DOCKER_TEAM)
}

/// What `osascript` runs: the script and the prompt come in as arguments, so neither needs
/// AppleScript quoting.
const APPLESCRIPT: [&str; 3] = [
    "on run argv",
    "do shell script (item 1 of argv) with prompt (item 2 of argv) with administrator privileges",
    "end run",
];

const PROMPT: &str = "Cedar wants to install Docker Desktop.";

/// What the page says while macOS waits for the password.
pub const WAITING_FOR_PASSWORD: &str =
    "Waiting for permission: macOS asks for an administrator's name and password so Cedar can \
     install it.";

#[cfg(target_os = "macos")]
pub use mac::{install, run_as_admin};

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::app::{AppState, SetupStatus};
    use crate::{installer, proc};
    use anyhow::{bail, Context, Result};
    use std::process::Stdio;
    use std::time::Duration;
    use tauri::{AppHandle, Manager};

    /// Runs the whole setup; the page follows along in [`AppState::docker_setup`]. Once it is
    /// done, the launch sequence runs again and starts Docker Desktop.
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
        state.set_docker_setup(SetupStatus::Running("Checking this Mac".into()));
        let machine = Machine {
            arm64: sysctl_flag(&sysctl("hw.optional.arm64").await),
            hypervisor: sysctl_flag(&sysctl("kern.hv_support").await),
        };
        if let Some(blocked) = blocker(&machine) {
            return Ok(blocked);
        }
        let user = current_user().await?;
        let url = if machine.arm64 { DMG_ARM64 } else { DMG_X64 };
        let dmg = installer::download("Docker Desktop", url, "Docker.dmg", |p| {
            state.set_docker_setup(SetupStatus::Running(p))
        })
        .await?;

        state.set_docker_setup(SetupStatus::Running(WAITING_FOR_PASSWORD.into()));
        let log = state.paths.logs.join("docker-setup.log");
        let script = elevated_script(&log, &dmg, &user);
        let (code, stderr, log_text) =
            run_as_admin(&state.paths.logs, "docker-setup", &script, PROMPT, |step| {
                state.set_docker_setup(SetupStatus::Running(step))
            })
            .await?;

        let result = outcome(code, &log_text, &stderr);
        // Kept after a failure, so trying again doesn't download it again.
        if result == Outcome::Done {
            if let Some(dir) = dmg.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        Ok(match result {
            Outcome::Done => SetupStatus::Idle,
            Outcome::Declined => SetupStatus::Failed(
                "macOS did not get an administrator's password, so nothing was installed. Click \
                 Try again and enter your password; if you aren't an administrator of this Mac, \
                 one can enter theirs."
                    .into(),
            ),
            Outcome::Failed(why) => SetupStatus::Failed(why),
        })
    }

    /// Runs `script` with `/bin/sh` as root, after macOS asks for an administrator's password
    /// once with `prompt`. The script writes its `STEP:` and `ERROR:` lines to `<name>.log` in
    /// `logs` (emptied first), and each step goes to `on_step` while it runs. `osascript`'s exit
    /// code, its stderr, and the log.
    pub async fn run_as_admin(
        logs: &Path,
        name: &str,
        script: &str,
        prompt: &str,
        on_step: impl Fn(String),
    ) -> Result<(Option<i32>, String, String)> {
        let log = logs.join(format!("{name}.log"));
        std::fs::write(&log, "").with_context(|| format!("creating {}", log.display()))?;
        let child = proc::command("/usr/bin/osascript")
            .args(APPLESCRIPT.iter().flat_map(|line| ["-e", *line]))
            .arg(script)
            .arg(prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("running osascript")?;
        let (code, stderr) = installer::follow(child, &log, on_step).await?;
        let log_text = std::fs::read_to_string(&log).unwrap_or_default();
        Ok((code, stderr, log_text))
    }

    /// `sysctl -n name`, or nothing if it has no such value.
    async fn sysctl(name: &str) -> String {
        let out = tokio::time::timeout(
            Duration::from_secs(20),
            proc::command("/usr/sbin/sysctl")
                .args(["-n", name])
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await;
        match out {
            Ok(Ok(out)) if out.status.success() => String::from_utf8_lossy(&out.stdout).into(),
            _ => String::new(),
        }
    }

    /// The signed-in user's short name, whom Docker Desktop is set up for: the elevated script
    /// runs as root.
    async fn current_user() -> Result<String> {
        if let Ok(user) = std::env::var("USER") {
            if !user.is_empty() && user != "root" {
                return Ok(user);
            }
        }
        let out = proc::command("/usr/bin/id")
            .arg("-un")
            .stdin(Stdio::null())
            .output()
            .await
            .context("running id")?;
        let user = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || user.is_empty() {
            bail!("could not tell which user is signed in");
        }
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SetupStatus;

    #[test]
    fn sysctl_flags() {
        assert!(sysctl_flag("1\n"));
        assert!(!sysctl_flag("0\n"));
        assert!(!sysctl_flag(""));
    }

    #[test]
    fn blockers() {
        assert!(matches!(
            blocker(&Machine {
                arm64: false,
                hypervisor: false
            }),
            Some(SetupStatus::Failed(_))
        ));
        assert_eq!(
            blocker(&Machine {
                arm64: true,
                hypervisor: true
            }),
            None
        );
    }

    #[test]
    fn outcomes() {
        assert_eq!(outcome(Some(0), "", ""), Outcome::Done);
        assert_eq!(
            outcome(Some(1), "", "0:150: execution error: User canceled. (-128)\n"),
            Outcome::Declined
        );
        let log = "STEP: Checking\nERROR: the downloaded Docker Desktop is not signed by Docker.\n";
        assert_eq!(
            outcome(Some(1), log, "0:150: execution error: (1)"),
            Outcome::Failed("the downloaded Docker Desktop is not signed by Docker.".into())
        );
        assert_eq!(
            outcome(Some(1), "", " no access \n"),
            Outcome::Failed("no access".into())
        );
        assert!(matches!(outcome(None, "", ""), Outcome::Failed(m) if m.contains("none")));
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("/Users/o'brien/x"), r"'/Users/o'\''brien/x'");
        assert_eq!(sh_quote("a b$c"), "'a b$c'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn script_is_filled_in() {
        let s = elevated_script(
            Path::new("/Users/o'brien/Library/Logs/cedar/docker-setup.log"),
            Path::new("/var/folders/x/T/cedar-docker-setup/Docker.dmg"),
            "o'brien",
        );
        assert!(s.contains(r"log='/Users/o'\''brien/Library/Logs/cedar/docker-setup.log'"));
        assert!(s.contains("dmg='/var/folders/x/T/cedar-docker-setup/Docker.dmg'"));
        assert!(s.contains(r"user='o'\''brien'"));
        assert!(s.contains(&format!("certificate leaf[subject.OU] = \"{DOCKER_TEAM}\"")));
        assert!(!s.contains("@@"));
    }

    /// `/bin/sh` accepts the script, and run with stand-ins for the tools it calls (no root, no
    /// Docker), it writes its steps and stops at the first failure with an `ERROR:` line.
    #[cfg(unix)]
    #[test]
    fn sh_takes_the_script() {
        let dir = std::env::temp_dir().join(format!("cedar-macsetup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("it's.log");
        let script = elevated_script(&log, &dir.join("missing.dmg"), "someone");
        let check = std::process::Command::new("/bin/sh")
            .args(["-n", "-c", &script])
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "{}",
            String::from_utf8_lossy(&check.stderr)
        );

        // The disk image isn't there, so it stops at the copy (or, on Linux, at mktemp's folder).
        let run = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(1));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(last_tagged(&text, ERROR).is_some(), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
