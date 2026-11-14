//! The llama.cpp release Cedar installs and the build of it that suits a machine. The release is
//! pinned: `assets.txt` names its tag and the size and SHA-256 of every file Cedar may download,
//! so what gets installed changes only with Cedar itself (see `scripts/update-llama-manifest.sh`).

use crate::gpu::{Gpu, Vendor};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Macos,
    Windows,
    Linux,
}

impl Os {
    /// The system this program was built for, if it is one llama.cpp is installed on.
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "macos" => Some(Self::Macos),
            "windows" => Some(Self::Windows),
            "linux" => Some(Self::Linux),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

impl Arch {
    /// From a Rust target architecture (`std::env::consts::ARCH`). This is the program's, which
    /// under emulation is not the machine's: the desktop app asks the system instead.
    pub fn from_target(arch: &str) -> Option<Self> {
        match arch {
            "x86_64" => Some(Self::X64),
            "aarch64" => Some(Self::Arm64),
            _ => None,
        }
    }
}

/// A build of llama.cpp that upstream publishes and Cedar installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Build {
    /// Apple Silicon, Metal.
    MacosMetal,
    /// Windows x64, CUDA 12.4, with NVIDIA's runtime libraries from a second archive.
    WinCuda,
    /// Windows x64, Vulkan: AMD and Intel cards, and NVIDIA ones whose driver is too old for CUDA.
    WinVulkan,
    /// Linux x86_64, CUDA 12.8, with NVIDIA's runtime libraries from a second archive.
    LinuxCuda,
    /// Linux x86_64, Vulkan.
    LinuxVulkan,
}

/// The oldest NVIDIA drivers that run the CUDA builds, as (major, minor): the ones NVIDIA shipped
/// CUDA 12.4 with on Windows and CUDA 12.8 with on Linux. An older driver gets the Vulkan build,
/// which is slower but works.
const WIN_CUDA_DRIVER: (u32, u32) = (551, 61);
const LINUX_CUDA_DRIVER: (u32, u32) = (570, 26);

impl Build {
    pub const ALL: [Build; 5] = [
        Self::MacosMetal,
        Self::WinCuda,
        Self::WinVulkan,
        Self::LinuxCuda,
        Self::LinuxVulkan,
    ];

    /// The name the build is saved under in a config file, and the middle of its file name.
    pub fn id(self) -> &'static str {
        match self {
            Self::MacosMetal => "macos-arm64",
            Self::WinCuda => "win-cuda-12.4-x64",
            Self::WinVulkan => "win-vulkan-x64",
            Self::LinuxCuda => "ubuntu-cuda-12.8-x64",
            Self::LinuxVulkan => "ubuntu-vulkan-x64",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.id() == id)
    }

    fn windows(self) -> bool {
        matches!(self, Self::WinCuda | Self::WinVulkan)
    }

    /// The files of the build in release `tag`, the one with `llama-server` first. All of them
    /// are unpacked into one folder. Windows builds are zip archives, the others gzipped tar
    /// archives whose files sit in a top folder.
    pub fn assets(self, tag: &str) -> Vec<String> {
        let ext = if self.windows() { "zip" } else { "tar.gz" };
        let mut names = vec![format!("llama-{tag}-bin-{}.{ext}", self.id())];
        match self {
            // Upstream leaves the tag out of this one name.
            Self::WinCuda => names.push(format!("cudart-llama-bin-{}.zip", self.id())),
            Self::LinuxCuda => names.push(format!("cudart-llama-{tag}-bin-{}.tar.gz", self.id())),
            _ => {}
        }
        names
    }

    /// The server's file name inside the unpacked build.
    pub fn server_exe(self) -> &'static str {
        if self.windows() {
            "llama-server.exe"
        } else {
            "llama-server"
        }
    }
}

/// The build for a GPU, or nothing when upstream has none that uses it (an Intel Mac, Windows on
/// Arm) or Cedar does not install there yet (Linux on Arm).
pub fn choose_build(os: Os, arch: Arch, gpu: &Gpu) -> Option<Build> {
    let cuda = |least: (u32, u32)| gpu.driver.is_some_and(|d| d >= least);
    match (os, arch, gpu.vendor) {
        (Os::Macos, Arch::Arm64, Vendor::Apple) => Some(Build::MacosMetal),
        (Os::Windows, Arch::X64, Vendor::Nvidia) if cuda(WIN_CUDA_DRIVER) => Some(Build::WinCuda),
        (Os::Windows, Arch::X64, Vendor::Nvidia | Vendor::Amd | Vendor::Intel) => {
            Some(Build::WinVulkan)
        }
        (Os::Linux, Arch::X64, Vendor::Nvidia) if cuda(LINUX_CUDA_DRIVER) => Some(Build::LinuxCuda),
        (Os::Linux, Arch::X64, Vendor::Nvidia | Vendor::Amd | Vendor::Intel) => {
            Some(Build::LinuxVulkan)
        }
        _ => None,
    }
}

/// A file to download, with what it must be once it has arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hexadecimal.
    pub sha256: String,
}

/// The pinned release: its tag and the files of every build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub tag: String,
    files: Vec<(String, u64, String)>,
}

impl Manifest {
    /// The release this version of Cedar installs.
    pub fn pinned() -> Self {
        Self::parse(include_str!("../assets.txt")).expect("llama/assets.txt is checked by a test")
    }

    /// Reads `assets.txt`: `#` comments, a `tag <name>` line, then `<file> <bytes> <sha256>` lines.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut tag = None;
        let mut files = Vec::new();
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts[..] {
                ["tag", name] => tag = Some(name.to_string()),
                [name, size, sha256] => {
                    let size: u64 = size
                        .parse()
                        .map_err(|_| format!("{name}: {size} is not a size"))?;
                    let hex = sha256.len() == 64
                        && sha256
                            .bytes()
                            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
                    if size == 0 || !hex {
                        return Err(format!("{name}: an empty file or not a SHA-256"));
                    }
                    files.push((name.to_string(), size, sha256.to_string()));
                }
                _ => return Err(format!("cannot read the line {line:?}")),
            }
        }
        Ok(Self {
            tag: tag.ok_or("no tag line")?,
            files,
        })
    }

    /// The files of a build, or nothing if the manifest lacks one of them.
    pub fn assets(&self, build: Build) -> Option<Vec<Asset>> {
        build
            .assets(&self.tag)
            .into_iter()
            .map(|name| {
                let (_, size, sha256) = self.files.iter().find(|(n, ..)| *n == name)?;
                Some(Asset {
                    url: format!(
                        "https://github.com/ggml-org/llama.cpp/releases/download/{}/{name}",
                        self.tag
                    ),
                    name,
                    size: *size,
                    sha256: sha256.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvidia(driver: Option<(u32, u32)>) -> Gpu {
        Gpu {
            driver,
            ..Gpu::dedicated(Vendor::Nvidia, "NVIDIA GeForce RTX 4090", 24564)
        }
    }

    #[test]
    fn builds_by_machine() {
        let amd = Gpu::dedicated(Vendor::Amd, "AMD Radeon RX 7900 XTX", 24560);
        let arc = Gpu::dedicated(Vendor::Intel, "Intel(R) Arc(TM) A770 Graphics", 16384);
        let mac = Gpu::apple(64 << 30);
        let cases = [
            (Os::Macos, Arch::Arm64, &mac, Some(Build::MacosMetal)),
            (Os::Macos, Arch::X64, &mac, None),
            (
                Os::Windows,
                Arch::X64,
                &nvidia(Some((551, 61))),
                Some(Build::WinCuda),
            ),
            (
                Os::Windows,
                Arch::X64,
                &nvidia(Some((551, 23))),
                Some(Build::WinVulkan),
            ),
            (
                Os::Windows,
                Arch::X64,
                &nvidia(Some((472, 12))),
                Some(Build::WinVulkan),
            ),
            (
                Os::Windows,
                Arch::X64,
                &nvidia(None),
                Some(Build::WinVulkan),
            ),
            (Os::Windows, Arch::X64, &amd, Some(Build::WinVulkan)),
            (Os::Windows, Arch::X64, &arc, Some(Build::WinVulkan)),
            (Os::Windows, Arch::Arm64, &nvidia(Some((580, 0))), None),
            (
                Os::Linux,
                Arch::X64,
                &nvidia(Some((570, 124))),
                Some(Build::LinuxCuda),
            ),
            (
                Os::Linux,
                Arch::X64,
                &nvidia(Some((550, 54))),
                Some(Build::LinuxVulkan),
            ),
            (Os::Linux, Arch::X64, &amd, Some(Build::LinuxVulkan)),
            (Os::Linux, Arch::Arm64, &nvidia(Some((580, 0))), None),
            (
                Os::Linux,
                Arch::X64,
                &Gpu::dedicated(Vendor::Other, "llvmpipe", 65536),
                None,
            ),
        ];
        for (os, arch, gpu, want) in cases {
            assert_eq!(choose_build(os, arch, gpu), want, "{os:?} {arch:?} {gpu:?}");
        }
    }

    #[test]
    fn file_names_are_upstreams() {
        assert_eq!(
            Build::MacosMetal.assets("b1"),
            ["llama-b1-bin-macos-arm64.tar.gz"]
        );
        assert_eq!(
            Build::WinCuda.assets("b1"),
            [
                "llama-b1-bin-win-cuda-12.4-x64.zip",
                "cudart-llama-bin-win-cuda-12.4-x64.zip"
            ]
        );
        assert_eq!(
            Build::LinuxCuda.assets("b1"),
            [
                "llama-b1-bin-ubuntu-cuda-12.8-x64.tar.gz",
                "cudart-llama-b1-bin-ubuntu-cuda-12.8-x64.tar.gz"
            ]
        );
        for build in Build::ALL {
            assert_eq!(Build::from_id(build.id()), Some(build));
        }
        assert_eq!(Build::from_id("win-cpu-x64"), None);
    }

    #[test]
    fn the_pinned_release_has_every_build() {
        let manifest = Manifest::pinned();
        assert!(manifest.tag.starts_with('b') && manifest.tag[1..].parse::<u32>().is_ok());
        let mut used = 0;
        for build in Build::ALL {
            let assets = manifest
                .assets(build)
                .unwrap_or_else(|| panic!("{build:?} lacks a file"));
            for asset in &assets {
                assert!(asset
                    .url
                    .starts_with("https://github.com/ggml-org/llama.cpp/releases/download/"));
                assert!(asset.url.ends_with(&asset.name));
            }
            used += assets.len();
        }
        // Nothing in the file is left over from a build that is gone.
        assert_eq!(used, manifest.files.len());
    }

    #[test]
    fn a_broken_manifest_is_refused() {
        let hash = "a".repeat(64);
        assert!(Manifest::parse(&format!("tag b1\nfile.zip 12 {hash}\n")).is_ok());
        assert!(
            Manifest::parse(&format!("file.zip 12 {hash}\n")).is_err(),
            "no tag"
        );
        assert!(
            Manifest::parse(&format!("tag b1\nfile.zip 0 {hash}\n")).is_err(),
            "empty file"
        );
        assert!(
            Manifest::parse("tag b1\nfile.zip 12 abc\n").is_err(),
            "short hash"
        );
        assert!(
            Manifest::parse(&format!("tag b1\nfile.zip 12 {}\n", "A".repeat(64))).is_err(),
            "uppercase"
        );
        assert!(Manifest::parse("tag b1\njunk\n").is_err());
        // A build whose file is missing has no assets.
        let manifest = Manifest::parse(&format!(
            "tag b1\nllama-b1-bin-win-cuda-12.4-x64.zip 12 {hash}\n"
        ))
        .unwrap();
        assert_eq!(manifest.assets(Build::WinCuda), None);
    }
}
