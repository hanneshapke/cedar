//! Setting llama.cpp up as the model server from the setup page: on a computer whose GPU can run
//! one of Cedar's models, the page offers to download llama.cpp's `llama-server` and that model,
//! and the app then runs it (llamaserver.rs) and registers it with cedar like any other server.
//!
//! What suits the GPU, the files and their hashes come from cedar-llama. Nothing here needs
//! administrator rights: the build goes into the app's data folder, the model beside it.

use crate::app::{self, AppState, SetupStatus};
use crate::config::Llama;
use crate::i18n::{t, tf};
use crate::installer;
use crate::llamaserver::Spec;
use crate::proc;
use cedar_llama::catalog::{self, Model};
use cedar_llama::install::{self, Event};
use cedar_llama::release::{Build, Manifest};
use cedar_llama::{probe, Plan};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

/// What the page offers to install, in its words.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Offer {
    /// The GPU's name and its memory in MiB (all of the computer's on Apple Silicon).
    pub gpu: String,
    pub memory_mb: u64,
    pub unified: bool,
    pub model: String,
    /// The model's page, which has its license.
    pub model_page: String,
    pub license: String,
    /// llama.cpp and the model together, in bytes.
    pub download_bytes: u64,
}

pub fn offer(plan: &Plan) -> Offer {
    let build: u64 = Manifest::pinned()
        .assets(plan.build)
        .map(|assets| assets.iter().map(|a| a.size).sum())
        .unwrap_or(0);
    Offer {
        gpu: plan.gpu.name.clone(),
        memory_mb: plan.gpu.memory_mb,
        unified: plan.gpu.unified,
        model: plan.model.label.to_string(),
        model_page: plan.model.page(),
        license: plan.model.license.to_string(),
        download_bytes: build + plan.model.size,
    }
}

/// Looks at this computer's GPU once, when the app starts; the page offers the setup if a model
/// fits it.
pub async fn detect(app: AppHandle) {
    let plan = match probe::machine().await {
        Some(machine) => cedar_llama::plan(machine.os, machine.arch, &machine.gpus),
        None => None,
    };
    app.state::<AppState>().set_llama_plan(plan);
    crate::window::push_llama(&app);
}

/// Why a setup ended without an install.
#[derive(Debug, PartialEq)]
enum Stop {
    Cancelled,
    Failed(String),
}

/// An install error in the user's language. `what` is what was being downloaded.
fn say(e: install::Error, what: &str) -> Stop {
    let gb = |bytes: u64| bytes.div_ceil(1_000_000_000).to_string();
    Stop::Failed(match e {
        install::Error::Cancelled => return Stop::Cancelled,
        install::Error::Download(error) => tf("llama.download_failed", &[("what", what), ("error", &error)]),
        install::Error::Stalled => tf("setup.download_stalled", &[("what", what)]),
        install::Error::Cut => tf("setup.download_cut", &[("what", what)]),
        install::Error::Changed(_) => tf("llama.changed", &[("what", what)]),
        install::Error::NoSpace { need, free } => tf("llama.no_space", &[("need", &gb(need)), ("free", &gb(free))]),
        e @ (install::Error::Unpack(_) | install::Error::Io(_)) => e.to_string(),
    })
}

/// Shows an install's progress on the page: the step, and how far a download is.
fn show<'a>(state: &'a AppState, what: &'a str) -> impl Fn(Event) + 'a {
    move |event| match event {
        Event::Downloading { got, total } => state.set_llama_setup(
            SetupStatus::Running(installer::progress(what, got, Some(total))),
            Some(got as f64 / total.max(1) as f64),
        ),
        Event::Checking => state.set_llama_setup(SetupStatus::Running(tf("llama.checking", &[("what", what)])), None),
        Event::Unpacking => state.set_llama_setup(SetupStatus::Running(tf("llama.unpacking", &[("what", what)])), None),
    }
}

/// Downloads what is not there yet of a build and a model, and says how to run them. With both
/// in place nothing is downloaded and the page shows nothing.
async fn prepare(state: &AppState, build: Build, model: &'static Model, cancel: &CancellationToken) -> Result<Spec, Stop> {
    const LLAMA: &str = "llama.cpp";
    let manifest = Manifest::pinned();
    let exe = install::install_build(&manifest, build, &state.paths.llama(), cancel, show(state, LLAMA))
        .await
        .map_err(|e| say(e, LLAMA))?;
    let file = install::install_model(model, &state.paths.models(), cancel, show(state, model.label))
        .await
        .map_err(|e| say(e, model.label))?;
    let cfg = state.config();
    Ok(Spec { exe, model, file, port: cfg.llama_port, api_key: cfg.secrets.llama_api_key })
}

/// The devices `llama-server --list-devices` prints under "Available devices:": the GPUs it can
/// use, each on a line of its own, or `(none)`.
fn devices(output: &str) -> Vec<String> {
    output
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("Available devices"))
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "(none)")
        .map(str::to_string)
        .collect()
}

async fn list_devices(exe: &Path) -> Vec<String> {
    let mut command = proc::command(exe);
    command
        .arg("--list-devices")
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(60), command.output()).await {
        Ok(Ok(out)) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            devices(&text)
        }
        _ => Vec::new(),
    }
}

/// The setup page's "Set up for me": installs what [`detect`] chose, checks that llama.cpp sees
/// the GPU, adds it to cedar's models, and starts it. The page follows along in `llama.setup`.
pub async fn install(app: AppHandle) {
    let state = app.state::<AppState>();
    let cancel = state.new_llama_cancel();
    let outcome = run(&state, &cancel).await;
    match outcome {
        Ok(()) => {
            state.set_llama_setup(SetupStatus::Idle, None);
            // The one just set up is the one new chats use.
            state.choose(app::Chosen::Llama);
            app::apply_model_server(app.clone()).await;
        }
        Err(Stop::Cancelled) => state.set_llama_setup(SetupStatus::Idle, None),
        Err(Stop::Failed(why)) => state.set_llama_setup(SetupStatus::Failed(why), None),
    }
}

async fn run(state: &AppState, cancel: &CancellationToken) -> Result<(), Stop> {
    let failed = |e: anyhow::Error| Stop::Failed(format!("{e:#}"));
    app::refuse_when_remote(&state.config()).map_err(failed)?;
    let plan = state.llama_plan().ok_or_else(|| Stop::Failed(t("llama.no_plan")))?;
    let spec = prepare(state, plan.build, plan.model, cancel).await?;

    // A driver that is missing or too old leaves llama.cpp with the processor alone, where a
    // model of this size answers too slowly for cedar; better to say so than to set that up.
    state.set_llama_setup(SetupStatus::Running(t("llama.testing")), None);
    if list_devices(&spec.exe).await.is_empty() {
        return Err(Stop::Failed(t("llama.no_gpu_seen")));
    }
    state
        .update_config(|c| {
            c.llama = Some(Llama { build: plan.build.id().to_string(), model: plan.model.id.to_string() });
        })
        .map_err(failed)?;
    Ok(())
}

/// What to run when llama.cpp is the model server, or `None` when the user has their own. After
/// an update of the app that pins a newer llama.cpp, this downloads that build first (the model
/// stays), with the progress on the page.
pub async fn ensure(state: &AppState) -> Result<Option<Spec>, String> {
    let Some(llama) = state.config().llama else {
        return Ok(None);
    };
    let (Some(build), Some(model)) = (Build::from_id(&llama.build), catalog::by_id(&llama.model)) else {
        return Err(tf("llama.unknown", &[("build", &llama.build), ("model", &llama.model)]));
    };
    if !state.begin_llama_setup() {
        return Err(t("cmd.llama_busy"));
    }
    let cancel = state.new_llama_cancel();
    let prepared = prepare(state, build, model, &cancel).await;
    let (status, result) = match prepared {
        Ok(spec) => (SetupStatus::Idle, Ok(Some(spec))),
        Err(Stop::Cancelled) => (SetupStatus::Idle, Err(t("llama.cancelled"))),
        Err(Stop::Failed(why)) => (SetupStatus::Failed(why.clone()), Err(why)),
    };
    state.set_llama_setup(status, None);
    result
}

/// The page's "Remove": stops the server, deletes llama.cpp and the model, and takes its entry
/// out of cedar's registry. A model server named beside it is left as it is, and so are model
/// files the app did not pick.
pub async fn remove(app: AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let Some(llama) = state.config().llama else {
        return Ok(());
    };
    state.llama.stop().await;
    state.update_config(|c| c.llama = None)?;
    let _ = tokio::fs::remove_dir_all(state.paths.llama()).await;
    if let Some(model) = catalog::by_id(&llama.model) {
        let _ = tokio::fs::remove_file(install::model_path(&state.paths.models(), model)).await;
    }
    // Registering again takes its entry out of cedar's registry; a model server beside it stays.
    app::apply_model_server(app.clone()).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cedar_llama::gpu::Gpu;
    use cedar_llama::release::{Arch, Os};

    #[test]
    fn devices_llama_server_lists() {
        let none = "ggml_vulkan: no devices found\nAvailable devices:\n  (none)\n";
        assert!(devices(none).is_empty());
        let one = "Available devices:\n  Vulkan0: AMD Radeon RX 7900 XTX (24560 MiB, 23912 MiB free)\n";
        assert_eq!(devices(one), ["Vulkan0: AMD Radeon RX 7900 XTX (24560 MiB, 23912 MiB free)"]);
        let two = "load_backend: loaded CUDA backend\nAvailable devices:\n  CUDA0: NVIDIA GeForce RTX 4090 (24564 MiB, 23000 MiB free)\n  CUDA1: NVIDIA GeForce RTX 3090 (24576 MiB, 24000 MiB free)\n\n";
        assert_eq!(devices(two).len(), 2);
        // No such section at all (it did not start): nothing found.
        assert!(devices("error while loading shared libraries: libvulkan.so.1").is_empty());
        assert!(devices("").is_empty());
    }

    #[test]
    fn the_offer_names_the_download() {
        let plan = cedar_llama::plan(Os::Macos, Arch::Arm64, &[Gpu::apple(32 << 30)]).unwrap();
        let offer = offer(&plan);
        assert_eq!((offer.gpu.as_str(), offer.memory_mb, offer.unified), ("Apple Silicon", 32768, true));
        assert_eq!(offer.model, "gpt-oss 20B");
        assert_eq!(offer.model_page, "https://huggingface.co/ggml-org/gpt-oss-20b-GGUF");
        // The model, and the few megabytes of llama.cpp for a Mac on top.
        assert!(offer.download_bytes > plan.model.size && offer.download_bytes < plan.model.size + 100_000_000);
        let json = serde_json::to_value(&offer).unwrap();
        assert_eq!(json["license"], "Apache-2.0");
    }

    #[test]
    fn install_errors_in_words() {
        assert_eq!(say(install::Error::Cancelled, "llama.cpp"), Stop::Cancelled);
        let Stop::Failed(text) = say(install::Error::NoSpace { need: 20_600_000_000, free: 4_100_000_000 }, "x") else {
            panic!("not a failure");
        };
        assert!(text.contains("21") && text.contains('5'), "{text}");
        let Stop::Failed(text) = say(install::Error::Download("dns error".into()), "gpt-oss 20B") else {
            panic!("not a failure");
        };
        assert!(text.contains("gpt-oss 20B") && text.contains("dns error"), "{text}");
        for e in [install::Error::Stalled, install::Error::Cut, install::Error::Changed("f".into())] {
            assert!(matches!(say(e, "llama.cpp"), Stop::Failed(text) if text.contains("llama.cpp")));
        }
    }
}
