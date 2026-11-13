//! Running llama.cpp's `llama-server` as Cedar's model server, shared by the desktop app and the
//! Linux command: what GPU a machine has ([`gpu`]), which pinned llama.cpp build suits it
//! ([`release`]), which model fits its memory ([`catalog`]), and how the server is started
//! ([`server`]). Those four only read and decide; [`install`] downloads and unpacks what they chose.

pub mod catalog;
pub mod gpu;
pub mod install;
pub mod probe;
pub mod release;
pub mod server;

use serde::Serialize;

/// What Cedar would install on a machine: the GPU it chose, the llama.cpp build for it, and the
/// model that fits.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub gpu: gpu::Gpu,
    pub build: release::Build,
    pub model: &'static catalog::Model,
}

/// The plan for a machine, or nothing when no GPU on it can run a model from the catalog (the
/// caller then offers only the model servers the user sets up themselves). With several GPUs the
/// one with the most memory is used; they are not added up.
pub fn plan(os: release::Os, arch: release::Arch, gpus: &[gpu::Gpu]) -> Option<Plan> {
    let gpu = gpus.iter().max_by_key(|g| g.budget_mb())?;
    let build = release::choose_build(os, arch, gpu)?;
    let model = catalog::choose_model(gpu.budget_mb())?;
    Some(Plan {
        gpu: gpu.clone(),
        build,
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpu::{Gpu, Vendor};
    use release::{Arch, Build, Os};

    #[test]
    fn the_largest_gpu_decides() {
        let small = Gpu::dedicated(Vendor::Intel, "Intel(R) UHD Graphics 770", 1024);
        let big = Gpu {
            driver: Some((560, 94)),
            ..Gpu::dedicated(Vendor::Nvidia, "NVIDIA GeForce RTX 4090", 24564)
        };
        let plan = plan(Os::Windows, Arch::X64, &[small.clone(), big.clone()]).unwrap();
        assert_eq!(plan.gpu, big);
        assert_eq!(plan.build, Build::WinCuda);
        assert_eq!(plan.model.id, "qwen3.6-27b");
        // The integrated one alone is not enough for any model.
        assert_eq!(super::plan(Os::Windows, Arch::X64, &[small]), None);
        assert_eq!(super::plan(Os::Windows, Arch::X64, &[]), None);
    }

    #[test]
    fn macs_by_memory() {
        let mac = |gb: u64| plan(Os::Macos, Arch::Arm64, &[Gpu::apple(gb << 30)]);
        assert_eq!(mac(16), None);
        assert_eq!(mac(32).unwrap().model.id, "gpt-oss-20b");
        assert_eq!(mac(48).unwrap().model.id, "qwen3.6-35b-a3b");
        assert_eq!(mac(128).unwrap().model.id, "gpt-oss-120b");
        assert_eq!(mac(32).unwrap().build, Build::MacosMetal);
        // An Intel Mac has no build with Metal.
        assert_eq!(plan(Os::Macos, Arch::X64, &[Gpu::apple(64 << 30)]), None);
    }
}
