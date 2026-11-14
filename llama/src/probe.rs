//! Asking the machine what it is: runs the commands whose output [`crate::gpu`] reads.

use crate::gpu::{self, Gpu};
use crate::release::{Arch, Os};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What a machine is, as far as choosing a build and a model goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub os: Os,
    pub arch: Arch,
    pub gpus: Vec<Gpu>,
}

/// What a command prints, or nothing if it is missing, fails or takes more than 20 seconds.
async fn output(program: &Path, args: &[&str]) -> String {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    // No console window flashing up behind a desktop app.
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    match tokio::time::timeout(Duration::from_secs(20), command.output()).await {
        Ok(Ok(out)) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => String::new(),
    }
}

fn system32(program: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join(program)
}

/// This machine, or nothing on a system llama.cpp is not installed on.
pub async fn machine() -> Option<Machine> {
    let os = Os::current()?;
    let mut arch = Arch::from_target(std::env::consts::ARCH)?;
    let gpus = match os {
        Os::Macos => {
            let sysctl = Path::new("/usr/sbin/sysctl");
            // An Apple Silicon Mac says so even to a program that runs under Rosetta.
            if output(sysctl, &["-n", "hw.optional.arm64"]).await.trim() == "1" {
                arch = Arch::Arm64;
            }
            let memory = output(sysctl, &["-n", "hw.memsize"])
                .await
                .trim()
                .parse::<u64>()
                .ok();
            match (arch, memory) {
                (Arch::Arm64, Some(bytes)) => vec![Gpu::apple(bytes)],
                _ => Vec::new(),
            }
        }
        Os::Windows => {
            // The driver puts nvidia-smi into System32.
            let nvidia = gpu::parse_nvidia_smi(
                &output(&system32("nvidia-smi.exe"), gpu::NVIDIA_SMI_ARGS).await,
            );
            let powershell = system32("WindowsPowerShell\\v1.0\\powershell.exe");
            let args = [
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                gpu::WINDOWS_ADAPTERS_SCRIPT,
            ];
            gpu::merge(
                nvidia,
                gpu::parse_windows_adapters(&output(&powershell, &args).await),
            )
        }
        Os::Linux => {
            let nvidia =
                gpu::parse_nvidia_smi(&output(Path::new("nvidia-smi"), gpu::NVIDIA_SMI_ARGS).await);
            gpu::merge(nvidia, linux_sysfs(Path::new("/sys/class/drm")))
        }
    };
    Some(Machine { os, arch, gpus })
}

/// The cards under `/sys/class/drm` whose driver says how much memory they have (`card0`, not
/// its outputs such as `card0-DP-1`).
fn linux_sysfs(drm: &Path) -> Vec<Gpu> {
    let Ok(entries) = std::fs::read_dir(drm) else {
        return Vec::new();
    };
    let mut cards: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|e| e.path().join("device"))
        .collect();
    cards.sort();
    let read = |path: PathBuf| std::fs::read_to_string(path).unwrap_or_default();
    cards
        .into_iter()
        .filter_map(|device| {
            gpu::from_sysfs(
                &read(device.join("vendor")),
                &read(device.join("mem_info_vram_total")),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::Vendor;

    #[test]
    fn sysfs_cards() {
        let drm = std::env::temp_dir().join(format!("cedar-llama-test-drm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&drm);
        let card = |name: &str, vendor: &str, vram: Option<&str>| {
            let device = drm.join(name).join("device");
            std::fs::create_dir_all(&device).unwrap();
            std::fs::write(device.join("vendor"), vendor).unwrap();
            if let Some(vram) = vram {
                std::fs::write(device.join("mem_info_vram_total"), vram).unwrap();
            }
        };
        card("card1", "0x1002\n", Some("17163091968\n"));
        card("card0", "0x8086\n", None); // i915 has no such file
        card("card1-DP-1", "0x1002\n", Some("17163091968\n"));
        let gpus = linux_sysfs(&drm);
        assert_eq!(gpus.len(), 1);
        assert_eq!((gpus[0].vendor, gpus[0].memory_mb), (Vendor::Amd, 16368));
        assert!(linux_sysfs(&drm.join("missing")).is_empty());
    }

    #[tokio::test]
    async fn a_missing_command_prints_nothing() {
        assert_eq!(output(Path::new("cedar-no-such-command"), &["x"]).await, "");
        #[cfg(unix)]
        {
            assert_eq!(
                output(Path::new("/bin/sh"), &["-c", "echo hi"]).await,
                "hi\n"
            );
            assert_eq!(
                output(Path::new("/bin/sh"), &["-c", "echo hi; exit 3"]).await,
                ""
            );
        }
    }

    #[tokio::test]
    async fn this_machine_is_read() {
        // Whatever it has, the probes must not fail on it.
        let machine = machine().await;
        assert_eq!(
            machine.is_some(),
            Os::current().is_some() && Arch::from_target(std::env::consts::ARCH).is_some()
        );
    }
}
