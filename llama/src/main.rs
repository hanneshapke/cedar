//! `cedar-llama`: sets llama.cpp up as Cedar's model server on a Linux machine with a GPU (the
//! desktop app does the same from its setup page on macOS and Windows).
//!
//!   cedar-llama detect     what GPU this machine has, and what would be installed
//!   cedar-llama install    download the llama.cpp build and the model
//!   cedar-llama serve      run llama-server with the installed model

use cedar_llama::catalog::{self, Model};
use cedar_llama::install::{self, Event};
use cedar_llama::release::{Build, Manifest, Os};
use cedar_llama::{probe, server};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tokio_util::sync::CancellationToken;

const USAGE: &str = "\
Sets up llama.cpp as Cedar's model server.

  cedar-llama detect
  cedar-llama install [--dir DIR] [--build BUILD] [--model MODEL]
  cedar-llama serve   [--dir DIR] [--model MODEL] [--port PORT]

  --dir DIR      where things are kept (default: $XDG_DATA_HOME/cedar or ~/.local/share/cedar)
  --build BUILD  the llama.cpp build, instead of the one chosen for the GPU
  --model MODEL  the model, instead of the one chosen for the GPU's memory
  --port PORT    the port llama-server listens on, on this computer only (default: 18081)

`cedar-llama detect` lists the builds and models. An API key for llama-server, if you want one,
goes into the LLAMA_API_KEY environment variable of `serve`.";

const DEFAULT_PORT: u16 = 18081;

struct Options {
    dir: PathBuf,
    build: Option<Build>,
    model: Option<&'static Model>,
    port: u16,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        dir: default_dir()?,
        build: None,
        model: None,
        port: DEFAULT_PORT,
    };
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--dir" => options.dir = PathBuf::from(value),
            "--build" => {
                let known = || Build::ALL.map(Build::id).join(", ");
                options.build = Some(
                    Build::from_id(value)
                        .ok_or_else(|| format!("no build {value}; there are: {}", known()))?,
                );
            }
            "--model" => {
                let known = || {
                    catalog::MODELS
                        .iter()
                        .map(|m| m.id)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                options.model = Some(
                    catalog::by_id(value)
                        .ok_or_else(|| format!("no model {value}; there are: {}", known()))?,
                );
            }
            "--port" => {
                options.port = value
                    .parse()
                    .map_err(|_| format!("{value} is not a port"))?
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    Ok(options)
}

fn default_dir() -> Result<PathBuf, String> {
    let set = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(data) = set("XDG_DATA_HOME") {
        return Ok(data.join("cedar"));
    }
    let home = set("HOME")
        .or_else(|| set("USERPROFILE"))
        .ok_or("no home folder; pass --dir")?;
    Ok(home.join(".local").join("share").join("cedar"))
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("detect") if args.len() == 1 => detect().await,
        Some("install") => match parse(&args[1..]) {
            Ok(options) => install(options).await,
            Err(e) => Err(e),
        },
        Some("serve") => match parse(&args[1..]) {
            Ok(options) => serve(options).await,
            Err(e) => Err(e),
        },
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn machine() -> Result<probe::Machine, String> {
    probe::machine()
        .await
        .ok_or_else(|| "llama.cpp is not installed on this kind of system".to_string())
}

fn gib(mb: u64) -> String {
    format!("{:.1} GB", mb as f64 / 1024.0)
}

async fn detect() -> Result<(), String> {
    let machine = machine().await?;
    if machine.gpus.is_empty() {
        println!("GPU:    none found (NVIDIA cards need nvidia-smi from the driver; AMD cards the amdgpu driver)");
    }
    for gpu in &machine.gpus {
        let shared = if gpu.unified {
            " shared with the system"
        } else {
            ""
        };
        println!("GPU:    {}, {}{shared}", gpu.name, gib(gpu.memory_mb));
    }
    match cedar_llama::plan(machine.os, machine.arch, &machine.gpus) {
        Some(plan) => {
            println!(
                "Build:  {} (llama.cpp {})",
                plan.build.id(),
                Manifest::pinned().tag
            );
            println!(
                "Model:  {} ({}, a download of {:.1} GB)",
                plan.model.id,
                plan.model.label,
                plan.model.size() as f64 / 1e9
            );
            println!("\nRun `cedar-llama install` to download both.");
        }
        None => {
            println!("\nNo GPU here has the memory for a model Cedar can use (12 GB at least).");
            println!("To install one anyway, name a build and a model:");
            println!("  builds: {}", Build::ALL.map(Build::id).join(", "));
            for model in &catalog::MODELS {
                println!(
                    "  model:  {} needs {} of GPU memory",
                    model.id,
                    gib(model.min_budget_mb)
                );
            }
        }
    }
    Ok(())
}

fn megabytes(got: u64, total: u64) -> String {
    format!("{} of {} MB", got / 1_000_000, total / 1_000_000)
}

/// One line that is rewritten as the download goes on.
fn show(what: &str) -> impl Fn(Event) + '_ {
    move |event| match event {
        Event::Downloading { got, total } => {
            eprint!("\rDownloading {what}: {}   ", megabytes(got, total))
        }
        Event::Checking => eprint!("\rChecking {what}                              "),
        Event::Unpacking => eprint!("\rUnpacking {what}                             "),
    }
}

async fn install(options: Options) -> Result<(), String> {
    let machine = machine().await?;
    let plan = cedar_llama::plan(machine.os, machine.arch, &machine.gpus);
    let build = options.build.or(plan.as_ref().map(|p| p.build));
    let model = options.model.or(plan.as_ref().map(|p| p.model));
    let (Some(build), Some(model)) = (build, model) else {
        return Err("No GPU here has the memory for a model Cedar works well with. \
            Run `cedar-llama detect` to see what there is, and pass --build and --model to install anyway."
            .into());
    };
    if build == Build::LinuxVulkan && machine.os == Os::Linux && !has_vulkan() {
        eprintln!("Note: the Vulkan loader (libvulkan.so.1) was not found. Install it and your GPU's Vulkan");
        eprintln!("driver, or llama-server will run on the CPU: `sudo apt install libvulkan1 mesa-vulkan-drivers`.");
    }

    // Ctrl-C stops the download where it is; the next `install` continues it.
    let cancel = CancellationToken::new();
    let on_ctrl_c = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_ctrl_c.cancel();
        }
    });

    let manifest = Manifest::pinned();
    let label = format!("llama.cpp {} ({})", manifest.tag, build.id());
    let exe = install::install_build(
        &manifest,
        build,
        &options.dir.join("llama"),
        &cancel,
        show(&label),
    )
    .await;
    eprintln!();
    let exe = exe.map_err(|e| e.to_string())?;
    let file = install::install_model(
        model,
        &options.dir.join("models"),
        &cancel,
        show(model.label),
    )
    .await;
    eprintln!();
    let file = file.map_err(|e| e.to_string())?;

    println!(
        "Installed {} and {}.",
        exe.display(),
        file.weights.display()
    );
    println!("\nStart the model server (it answers on this computer only):");
    println!("  cedar-llama serve");
    println!("\nThen register it with cedar, with one of its admin keys:");
    println!(
        "  curl -X PUT -H \"Authorization: Bearer $KEY\" -H 'content-type: application/json' \\"
    );
    println!("    -d '{}' \\", registration(model, options.port));
    println!("    localhost:8080/v1/admin/models/{}", model.id);
    Ok(())
}

/// The body of cedar's `PUT /v1/admin/models/<id>` for a model served here.
fn registration(model: &Model, port: u16) -> String {
    let mut body = serde_json::json!({
        "base_url": server::base_url(port),
        "model": model.id,
        "label": model.label,
        "max_tokens": model.max_tokens,
        // Every model here comes with its vision projector, so cedar may send it images.
        "multimodal": true,
    });
    if !model.reasoning_levels.is_empty() {
        body["reasoning_levels"] = model.reasoning_levels.into();
    }
    body.to_string()
}

fn has_vulkan() -> bool {
    [
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib64",
        "/usr/lib",
        "/lib/x86_64-linux-gnu",
        "/lib64",
    ]
    .iter()
    .any(|dir| Path::new(dir).join("libvulkan.so.1").exists())
}

async fn serve(options: Options) -> Result<(), String> {
    let manifest = Manifest::pinned();
    let root = options.dir.join("llama");
    let builds = options
        .build
        .map(|b| vec![b])
        .unwrap_or_else(|| Build::ALL.to_vec());
    let exe = builds
        .into_iter()
        .find_map(|b| install::installed_server(&root, &manifest.tag, b))
        .ok_or("llama.cpp is not installed here; run `cedar-llama install` first")?;
    let models = options.dir.join("models");
    // The largest installed one, unless one was named.
    let (model, file) = match options.model {
        Some(model) => install::installed_model(&models, model).map(|file| (model, file)),
        None => catalog::MODELS
            .iter()
            .rev()
            .find_map(|m| install::installed_model(&models, m).map(|file| (m, file))),
    }
    .ok_or("that model is not downloaded; run `cedar-llama install` first")?;

    let args = server::args(model, &file, options.port);
    eprintln!("Serving {} at {}", model.id, server::base_url(options.port));
    let mut command = std::process::Command::new(&exe);
    command.args(args);
    run(command).map_err(|e| format!("running {}: {e}", exe.display()))
}

/// Becomes the server, so that a service manager's signals reach it directly.
#[cfg(unix)]
fn run(mut command: std::process::Command) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    Err(command.exec())
}

#[cfg(not(unix))]
fn run(mut command: std::process::Command) -> std::io::Result<()> {
    let status = command.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options() {
        let o = parse(&args(&[
            "--dir",
            "/data",
            "--build",
            "ubuntu-vulkan-x64",
            "--model",
            "gemma-4-e4b",
            "--port",
            "9000",
        ]))
        .unwrap();
        assert_eq!(
            (o.dir, o.build, o.port),
            (PathBuf::from("/data"), Some(Build::LinuxVulkan), 9000)
        );
        assert_eq!(o.model.unwrap().id, "gemma-4-e4b");
        assert!(parse(&args(&["--model", "gpt-5"]))
            .err()
            .unwrap()
            .contains("gemma-4-e4b"));
        assert!(parse(&args(&["--build", "x"]))
            .err()
            .unwrap()
            .contains("macos-arm64"));
        assert!(parse(&args(&["--port"])).is_err());
        assert!(parse(&args(&["--port", "http"])).is_err());
        assert!(parse(&args(&["--frobnicate", "1"])).is_err());
    }

    #[test]
    fn what_cedar_is_told() {
        let model = catalog::by_id("gemma-4-e4b").unwrap();
        let body: serde_json::Value = serde_json::from_str(&registration(model, 18081)).unwrap();
        assert_eq!(body["base_url"], "http://127.0.0.1:18081/v1");
        assert_eq!(body["model"], "gemma-4-e4b");
        assert_eq!(body["label"], "Gemma 4 E4B");
        assert_eq!(body["max_tokens"], 8192);
        assert_eq!(body["multimodal"], true);
        // A model without reasoning levels leaves the field out, which keeps cedar's own.
        assert!(body.get("reasoning_levels").is_none());
    }
}
