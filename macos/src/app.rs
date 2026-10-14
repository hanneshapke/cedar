//! Shared state and the launch sequence: find Docker, pull the images, start the servers,
//! register the model, show the chat. Everything the setup page shows comes from
//! [`AppState::status`].

use crate::config::{Config, VERSION};
use crate::docker::{self, DockerCli};
use crate::models;
use crate::paths::Paths;
use crate::supervisor::{ServiceState, Supervisor};
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

pub struct AppState {
    pub paths: Paths,
    config: Mutex<Config>,
    docker: Mutex<DockerStatus>,
    docker_cli: Mutex<Option<DockerCli>>,
    images: Mutex<ImagesStatus>,
    model: Mutex<ModelStatus>,
    pub supervisor: Supervisor,
    /// Only one launch sequence at a time.
    bootstrap: tokio::sync::Mutex<()>,
    /// The setup page's own URL (a `tauri://` address), to come back to from the chat.
    setup_url: Mutex<Option<Url>>,
}

/// What the setup page renders.
#[derive(Serialize, Clone, Debug)]
pub struct Status {
    pub version: String,
    pub docker: DockerStatus,
    pub images: ImagesStatus,
    pub services: ServiceState,
    pub model: ModelStatus,
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

    pub fn status(&self) -> Status {
        let cfg = self.config();
        Status {
            version: VERSION.to_string(),
            docker: self.docker.lock().unwrap().clone(),
            images: self.images.lock().unwrap().clone(),
            services: self.supervisor.state(),
            model: self.model.lock().unwrap().clone(),
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

    // 3. The servers, once the model server is known.
    if !cfg.configured() {
        return;
    }
    if let Err(e) = state.supervisor.start(cfg, cli).await {
        eprintln!("starting the services: {e:#}");
        return;
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

/// Stops the servers and runs the launch sequence again.
pub async fn restart(app: AppHandle) {
    let state = app.state::<AppState>();
    state.supervisor.stop().await;
    if let Err(e) = window::show_setup(&app) {
        eprintln!("showing the setup page: {e:#}");
    }
    bootstrap(app.clone()).await;
}
