//! The GPUs of a machine, read from what the system's own tools print. Each platform has a
//! command to run (the caller runs it) and a function here that reads its output.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    Apple,
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl Vendor {
    /// From a PCI vendor id in hexadecimal, with or without `0x` (`10de`, `0x1002`).
    pub fn from_pci(id: &str) -> Self {
        let id = id.trim().trim_start_matches("0x").trim_start_matches("0X");
        match id.to_ascii_lowercase().as_str() {
            "10de" => Self::Nvidia,
            "1002" | "1022" => Self::Amd,
            "8086" => Self::Intel,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gpu {
    pub vendor: Vendor,
    pub name: String,
    /// Its memory in MiB: the card's own, or all of the machine's when `unified`.
    pub memory_mb: u64,
    /// The GPU shares the machine's memory (Apple Silicon), so only part of it is the model's.
    pub unified: bool,
    /// NVIDIA's driver version as (major, minor), which decides whether the CUDA build can run.
    pub driver: Option<(u32, u32)>,
}

impl Gpu {
    pub fn dedicated(vendor: Vendor, name: &str, memory_mb: u64) -> Self {
        Self {
            vendor,
            name: name.trim().to_string(),
            memory_mb,
            unified: false,
            driver: None,
        }
    }

    /// An Apple Silicon Mac, from `sysctl -n hw.memsize` (bytes).
    pub fn apple(memsize_bytes: u64) -> Self {
        Self {
            vendor: Vendor::Apple,
            name: "Apple Silicon".to_string(),
            memory_mb: memsize_bytes >> 20,
            unified: true,
            driver: None,
        }
    }

    /// The memory a model and its context may take, in MiB. macOS lets the GPU have roughly two
    /// thirds to three quarters of unified memory, and the rest of the system needs its share.
    pub fn budget_mb(&self) -> u64 {
        if self.unified {
            self.memory_mb * 2 / 3
        } else {
            self.memory_mb
        }
    }
}

/// The NVIDIA cards of a machine. Windows and Linux; the command prints nothing useful (or is
/// missing) without NVIDIA's driver.
pub const NVIDIA_SMI_ARGS: &[&str] = &[
    "--query-gpu=name,memory.total,driver_version",
    "--format=csv,noheader,nounits",
];

/// Reads `nvidia-smi`'s answer to [`NVIDIA_SMI_ARGS`]: a line per card such as
/// `NVIDIA GeForce RTX 4090, 24564, 560.94`. Lines that are not that (an error message) are skipped.
pub fn parse_nvidia_smi(out: &str) -> Vec<Gpu> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.rsplitn(3, ',').map(str::trim);
            let driver = parts.next()?;
            let memory_mb = parts.next()?.parse().ok()?;
            let name = parts.next()?;
            Some(Gpu {
                driver: parse_driver(driver),
                ..Gpu::dedicated(Vendor::Nvidia, name, memory_mb)
            })
        })
        .collect()
}

/// `560.94` and `570.124.06` as (560, 94) and (570, 124).
fn parse_driver(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// PowerShell that lists a Windows PC's display adapters as JSON. The memory comes from the
/// registry value the driver writes, since `Win32_VideoController.AdapterRAM` is 32 bits wide and
/// stops at 4 GB.
pub const WINDOWS_ADAPTERS_SCRIPT: &str = "$ErrorActionPreference = 'SilentlyContinue'; \
    $class = 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\0*'; \
    ConvertTo-Json -Compress @(Get-ItemProperty $class | Where-Object { $_.DriverDesc } | ForEach-Object { \
    [pscustomobject]@{ name = [string]$_.DriverDesc; id = [string]$_.MatchingDeviceId; \
    memory = [uint64]$_.'HardwareInformation.qwMemorySize' } })";

#[derive(Deserialize)]
struct WindowsAdapter {
    name: String,
    /// The hardware id, such as `pci\ven_10de&dev_2684`.
    #[serde(default)]
    id: String,
    /// Bytes.
    #[serde(default)]
    memory: u64,
}

/// Reads the answer of [`WINDOWS_ADAPTERS_SCRIPT`]. PowerShell 5 prints a lone adapter as an
/// object rather than a list of one, so both are taken. Adapters without a PCI vendor (remote
/// desktop and other virtual displays) are left out.
pub fn parse_windows_adapters(json: &str) -> Vec<Gpu> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Answer {
        Many(Vec<WindowsAdapter>),
        One(WindowsAdapter),
    }
    let adapters = match serde_json::from_str(json.trim().trim_start_matches('\u{feff}')) {
        Ok(Answer::Many(list)) => list,
        Ok(Answer::One(one)) => vec![one],
        Err(_) => Vec::new(),
    };
    adapters
        .into_iter()
        .filter_map(|a| {
            let id = a.id.to_ascii_lowercase();
            let vendor = Vendor::from_pci(id.split("ven_").nth(1)?.get(..4)?);
            Some(Gpu::dedicated(vendor, &a.name, a.memory >> 20))
        })
        .collect()
}

/// A Linux GPU from its `/sys/class/drm/card*/device` folder: the `vendor` file and, where the
/// driver has one (amdgpu), `mem_info_vram_total` in bytes. Nothing when the memory is unknown.
pub fn from_sysfs(vendor: &str, vram_total: &str) -> Option<Gpu> {
    let vendor = Vendor::from_pci(vendor);
    let bytes: u64 = vram_total.trim().parse().ok()?;
    let name = match vendor {
        Vendor::Amd => "AMD Radeon",
        Vendor::Intel => "Intel Arc",
        Vendor::Nvidia => "NVIDIA",
        _ => "GPU",
    };
    Some(Gpu::dedicated(vendor, name, bytes >> 20))
}

/// What `nvidia-smi` and the adapter list both report, once: `nvidia-smi` knows the driver
/// version, so its entries replace the NVIDIA ones of the other list.
pub fn merge(nvidia: Vec<Gpu>, others: Vec<Gpu>) -> Vec<Gpu> {
    let has_nvidia = !nvidia.is_empty();
    let mut all = nvidia;
    all.extend(
        others
            .into_iter()
            .filter(|g| !(has_nvidia && g.vendor == Vendor::Nvidia)),
    );
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_smi_lines() {
        let out = "NVIDIA GeForce RTX 4090, 24564, 560.94\r\nNVIDIA RTX A6000, 49140, 570.124.06\n";
        let gpus = parse_nvidia_smi(out);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(
            (gpus[0].memory_mb, gpus[0].driver),
            (24564, Some((560, 94)))
        );
        assert_eq!(
            (gpus[1].memory_mb, gpus[1].driver),
            (49140, Some((570, 124)))
        );
        assert!(gpus
            .iter()
            .all(|g| g.vendor == Vendor::Nvidia && !g.unified));
        // What it prints when the driver is not loaded, and nothing at all.
        let failed =
            "NVIDIA-SMI has failed because it couldn't communicate with the NVIDIA driver.";
        assert!(parse_nvidia_smi(failed).is_empty());
        assert!(parse_nvidia_smi("").is_empty());
    }

    #[test]
    fn windows_adapters() {
        let json = r#"[{"name":"AMD Radeon RX 7900 XTX","id":"PCI\\VEN_1002&DEV_744C","memory":25753026560},
            {"name":"Intel(R) UHD Graphics 770","id":"pci\\ven_8086&dev_a780","memory":1073741824},
            {"name":"Microsoft Remote Display Adapter","id":"rdpidd_indirectdisplay","memory":0}]"#;
        let gpus = parse_windows_adapters(json);
        assert_eq!(gpus.len(), 2);
        assert_eq!((gpus[0].vendor, gpus[0].memory_mb), (Vendor::Amd, 24560));
        assert_eq!((gpus[1].vendor, gpus[1].memory_mb), (Vendor::Intel, 1024));
        // A lone adapter comes as an object, here with a byte order mark in front.
        let one = "\u{feff}{\"name\":\"Intel(R) Arc(TM) A770 Graphics\",\"id\":\"PCI\\\\VEN_8086&DEV_56A0\",\"memory\":17179869184}";
        assert_eq!(
            parse_windows_adapters(one),
            vec![Gpu::dedicated(
                Vendor::Intel,
                "Intel(R) Arc(TM) A770 Graphics",
                16384
            )]
        );
        assert!(parse_windows_adapters("").is_empty());
        assert!(parse_windows_adapters("not json").is_empty());
    }

    #[test]
    fn linux_sysfs() {
        let gpu = from_sysfs("0x1002\n", "25753026560\n").unwrap();
        assert_eq!((gpu.vendor, gpu.memory_mb), (Vendor::Amd, 24560));
        assert_eq!(from_sysfs("0x8086", ""), None);
    }

    #[test]
    fn unified_memory_is_shared() {
        let mac = Gpu::apple(64 << 30);
        assert_eq!((mac.memory_mb, mac.budget_mb()), (65536, 43690));
        assert_eq!(Gpu::dedicated(Vendor::Amd, "x", 16368).budget_mb(), 16368);
    }

    #[test]
    fn nvidia_smi_wins_over_the_adapter_list() {
        let smi = parse_nvidia_smi("NVIDIA GeForce RTX 4080, 16376, 551.61");
        let listed = vec![
            Gpu::dedicated(Vendor::Nvidia, "NVIDIA GeForce RTX 4080", 16376),
            Gpu::dedicated(Vendor::Intel, "Intel(R) UHD Graphics", 512),
        ];
        let all = merge(smi, listed.clone());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].driver, Some((551, 61)));
        // Without nvidia-smi the adapter list's entry stays.
        assert_eq!(merge(Vec::new(), listed).len(), 2);
    }
}
