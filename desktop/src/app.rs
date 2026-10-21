//! Shared state and the launch sequence: find Docker, pull the images, start the servers,
//! share the chat on the tailnet if asked, register the model, show the chat. Everything the setup page shows comes from
//! [`AppState::status`].

use crate::config::{Config, VERSION};
use crate::docker::{self, DockerCli};
use crate::models;
use crate::paths::Paths;
use crate::supervisor::{ServiceState, Supervisor};
use crate::tailscale;
use crate::window;
use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use url::Url;

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum DockerStatus {
    Checking,
    Missing,
    NotRunning(String),
    Ok(String),
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ImagesStatus {
    Waiting,
    Pulling(String),
    Ready,
    Failed(String),
}

/// Whether cedar's registry holds the model the user chose.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ModelStatus {
    /// Nothing chosen yet.
    Unset,
    Registering,
    Registered(String),
    Failed(String),
}

/// Whether the chat is served on the tailnet for the iOS app (see tailscale.rs).
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ShareStatus {
    Off,
    Starting,
    /// The chat's HTTPS address on the tailnet.
    On(String),
    Failed(String),
}

pub struct AppState {
    pub paths: Paths,
    config: Mutex<Config>,
    docker: Mutex<DockerStatus>,
    docker_cli: Mutex<Option<DockerCli>>,
    images: Mutex<ImagesStatus>,
    model: Mutex<ModelStatus>,
    share: Mutex<ShareStatus>,
    /// The tailnet port `tailscale serve` was set up on by this app, to turn off again.
    served_port: Mutex<Option<u16>>,
    /// The `CHAT_PUBLIC_URL` the running chat was started with.
    started_public_url: Mutex<Option<String>>,
    pub supervisor: Supervisor,
    /// Only one launch sequence at a time.
    bootstrap: tokio::sync::Mutex<()>,
    /// The setup page's own URL (`tauri://localhost` on a Mac, `http://tauri.localhost` on
    /// Windows), to come back to from the chat.
    setup_url: Mutex<Option<Url>>,
}

/// What the setup page renders.
#[derive(Serialize, Clone, Debug)]
pub struct Status {
    pub version: String,
    /// `macos` or `windows` (`std::env::consts::OS`), for the page's wording.
    pub platform: &'static str,
    pub docker: DockerStatus,
    pub images: ImagesStatus,
    pub services: ServiceState,
    pub model: ModelStatus,
    pub share: ShareStatus,
    /// Sharing is turned on (it is served once the servers run and Tailscale is ready).
    pub tailscale_share: bool,
    pub configured: bool,
    pub llm_base_url: String,
    pub llm_model: String,
    pub has_api_key: bool,
    pub browser: bool,
    pub chat_url: String,
    pub access_token: String,
    pub logs_dir: String,
    pub config_path: String,
    pub sandbox_image: String,
}

impl AppState {
    pub fn new(paths: Paths, config: Config, exe_dir: PathBuf) -> Self {
        Self {
            supervisor: Supervisor::new(exe_dir, paths.clone()),
            paths,
            config: Mutex::new(config),
            docker: Mutex::new(DockerStatus::Checking),
            docker_cli: Mutex::new(None),
            images: Mutex::new(ImagesStatus::Waiting),
            model: Mutex::new(ModelStatus::Unset),
            share: Mutex::new(ShareStatus::Off),
            served_port: Mutex::new(None),
            started_public_url: Mutex::new(None),
            bootstrap: tokio::sync::Mutex::new(()),
            setup_url: Mutex::new(None),
        }
    }

    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    /// Changes the config and writes it out.
    pub fn update_config(&self, change: impl FnOnce(&mut Config)) -> Result<Config> {
        let mut cfg = self.config.lock().unwrap();
        change(&mut cfg);
        cfg.save(&self.paths.config())?;
        Ok(cfg.clone())
    }

    pub fn set_setup_url(&self, url: Url) {
        *self.setup_url.lock().unwrap() = Some(url);
    }

    pub fn setup_url(&self) -> Option<Url> {
        self.setup_url.lock().unwrap().clone()
    }

    fn set_docker(&self, s: DockerStatus) {
        *self.docker.lock().unwrap() = s;
    }

    fn set_images(&self, s: ImagesStatus) {
        *self.images.lock().unwrap() = s;
    }

    fn set_model(&self, s: ModelStatus) {
        *self.model.lock().unwrap() = s;
    }

    fn set_share(&self, s: ShareStatus) {
        *self.share.lock().unwrap() = s;
    }

    /// The chat's tailnet address while it is served there.
    pub fn share_url(&self) -> Option<String> {
        match &*self.share.lock().unwrap() {
            ShareStatus::On(url) => Some(url.clone()),
            _ => None,
        }
    }

    /// The address to start the chat with when sharing is on, or `None` (with the reason on the
    /// page) when it is off or Tailscale isn't ready. Sharing that was turned off is undone here.
    async fn prepare_share(&self, cfg: &Config) -> Option<String> {
        if !cfg.tailscale_share {
            self.stop_share().await;
            return None;
        }
        self.set_share(ShareStatus::Starting);
        let found = match tailscale::find() {
            Some(cli) => cli.url(cfg.tailscale_https_port).await,
            None => Err(anyhow::anyhow!(tailscale::MISSING)),
        };
        match found {
            Ok(url) => Some(url),
            Err(e) => {
                self.set_share(ShareStatus::Failed(format!("{e:#}")));
                None
            }
        }
    }

    /// `tailscale serve` for the running chat.
    async fn serve_share(&self, cfg: &Config, url: String) {
        let Some(cli) = tailscale::find() else {
            self.set_share(ShareStatus::Failed(tailscale::MISSING.into()));
            return;
        };
        match cli.serve(cfg.tailscale_https_port, cfg.chat_port).await {
            Ok(()) => {
                *self.served_port.lock().unwrap() = Some(cfg.tailscale_https_port);
                self.set_share(ShareStatus::On(url));
            }
            Err(e) => self.set_share(ShareStatus::Failed(format!("{e:#}"))),
        }
    }

    /// Turns off what [`Self::serve_share`] set up, if anything. Also run when the app quits.
    pub async fn stop_share(&self) {
        let port = self.served_port.lock().unwrap().take();
        if let (Some(port), Some(cli)) = (port, tailscale::find()) {
            if let Err(e) = cli.unserve(port).await {
                eprintln!("turning off tailscale serve: {e:#}");
            }
        }
        self.set_share(ShareStatus::Off);
    }

    pub fn status(&self) -> Status {
        let cfg = self.config();
        Status {
            version: VERSION.to_string(),
            platform: std::env::consts::OS,
            docker: self.docker.lock().unwrap().clone(),
            images: self.images.lock().unwrap().clone(),
            services: self.supervisor.state(),
            model: self.model.lock().unwrap().clone(),
            share: self.share.lock().unwrap().clone(),
            tailscale_share: cfg.tailscale_share,
            configured: cfg.configured(),
            llm_base_url: cfg.llm_base_url.clone(),
            llm_model: cfg.llm_model.clone(),
            has_api_key: !cfg.llm_api_key.trim().is_empty(),
            browser: cfg.browser,
            chat_url: cfg.chat_url(),
            access_token: cfg.secrets.chat_access_token.clone(),
            logs_dir: self.paths.logs.to_string_lossy().into_owned(),
            config_path: self.paths.config().to_string_lossy().into_owned(),
            sandbox_image: cfg.sandbox_image(),
        }
    }

    /// Puts the chosen model server into cedar's registry. cedar must be running.
    async fn register_model(&self) -> bool {
        let cfg = self.config();
        if !cfg.configured() {
            self.set_model(ModelStatus::Unset);
            return false;
        }
        self.set_model(ModelStatus::Registering);
        let result = models::register(
            &cfg.cedar_url(),
            &cfg.secrets.cedar_api_key,
            &cfg.llm_base_url,
            &cfg.llm_api_key,
            &cfg.llm_model,
        )
        .await;
        match result {
            Ok(()) => {
                self.set_model(ModelStatus::Registered(format!(
                    "{} at {}",
                    cfg.llm_model.trim(),
                    cfg.llm_base_url.trim()
                )));
                true
            }
            Err(e) => {
                self.set_model(ModelStatus::Failed(format!("{e:#}")));
                false
            }
        }
    }
}

/// The launch sequence. Safe to run again at any time (after a setting changed, say): it stops
/// at the first thing that isn't ready and the page says what that is.
pub async fn bootstrap(app: AppHandle) {
    let state = app.state::<AppState>();
    let _one_at_a_time = state.bootstrap.lock().await;

    // 1. The docker CLI and its daemon.
    state.set_docker(DockerStatus::Checking);
    let Some(cli) = docker::find() else {
        state.set_docker(DockerStatus::Missing);
        return;
    };
    match cli.server_version().await {
        Ok(version) => state.set_docker(DockerStatus::Ok(format!(
            "Docker {version} via {}",
            cli.bin.display()
        ))),
        Err(e) => {
            state.set_docker(DockerStatus::NotRunning(format!("{e:#}")));
            return;
        }
    }
    *state.docker_cli.lock().unwrap() = Some(cli.clone());

    // 2. The images the agent's tools run in. The tag carries the app version, so an upgrade
    //    pulls the matching images once.
    let cfg = state.config();
    let mut images = vec![cfg.sandbox_image()];
    if cfg.browser {
        images.push(cfg.browser_image());
    }
    for image in images {
        if cli.image_present(&image).await {
            continue;
        }
        state.set_images(ImagesStatus::Pulling(format!("{image}: starting")));
        let pulled = cli
            .pull(&image, |line| {
                state.set_images(ImagesStatus::Pulling(format!("{image}: {line}")))
            })
            .await;
        if let Err(e) = pulled {
            state.set_images(ImagesStatus::Failed(format!("{e:#}")));
            return;
        }
    }
    state.set_images(ImagesStatus::Ready);

    // 3. The servers, once the model server is known. When the chat is shared on the tailnet,
    //    it is told that address first, then served there once it is up.
    if !cfg.configured() {
        return;
    }
    let public_url = state.prepare_share(&cfg).await;
    *state.started_public_url.lock().unwrap() = public_url.clone();
    if let Err(e) = state.supervisor.start(cfg.clone(), cli, public_url.clone()).await {
        eprintln!("starting the services: {e:#}");
        return;
    }
    if let Some(url) = public_url {
        state.serve_share(&cfg, url).await;
    }

    // 4. The model, into cedar's registry. A failure here (the model server is down, say) leaves
    //    the servers up: cedar keeps the last registered model, and the page says what's wrong.
    if state.register_model().await {
        if let Err(e) = window::show_chat(&app) {
            eprintln!("showing the chat: {e:#}");
        }
    }
}

/// After the model server was changed on the setup page: registers it with the running cedar,
/// or runs the launch sequence if the servers aren't up yet. No restart, so a run in progress
/// goes on.
pub async fn apply_model_server(app: AppHandle) {
    let state = app.state::<AppState>();
    if state.supervisor.state() != ServiceState::Running {
        bootstrap(app.clone()).await;
        return;
    }
    if state.register_model().await {
        if let Err(e) = window::show_chat(&app) {
            eprintln!("showing the chat: {e:#}");
        }
    }
}

/// Turns sharing on the tailnet on or off. The chat learns its address only when it starts, so
/// this restarts the servers, unless they already run with the address sharing gives them.
/// Turning it on with Tailscale not ready fails here, with nothing changed.
pub async fn set_share(app: AppHandle, enabled: bool) -> Result<()> {
    let state = app.state::<AppState>();
    let cfg = state.config();
    let url = if enabled {
        let cli = tailscale::find().ok_or_else(|| anyhow::anyhow!(tailscale::MISSING))?;
        Some(cli.url(cfg.tailscale_https_port).await?)
    } else {
        None
    };
    let cfg = state.update_config(|c| c.tailscale_share = enabled)?;
    let running = state.supervisor.state() == ServiceState::Running;
    let same_address = *state.started_public_url.lock().unwrap() == url;
    if running && same_address {
        match url {
            Some(url) => state.serve_share(&cfg, url).await,
            None => state.stop_share().await,
        }
    } else {
        if !enabled {
            state.stop_share().await;
        }
        tauri::async_runtime::spawn(restart(app.clone()));
    }
    Ok(())
}

/// Stops the servers and runs the launch sequence again.
pub async fn restart(app: AppHandle) {
    let state = app.state::<AppState>();
    state.supervisor.stop().await;
    if let Err(e) = window::show_setup(&app) {
        eprintln!("showing the setup page: {e:#}");
    }
    bootstrap(app.clone()).await;
}
