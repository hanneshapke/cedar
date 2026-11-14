//! Shared state and the launch sequence: find Docker, pull the images, start the servers,
//! share the chat on the tailnet if asked, register the model, show the chat. With a chat server on
//! another computer set (see remote.rs), only: check that it answers, show it. Everything the setup
//! page shows comes from [`AppState::status`].

use crate::config::{Config, VERSION};
use crate::docker::{self, DockerCli};
use crate::llamaserver::LlamaServer;
use crate::llamasetup;
use crate::pull::PullProgress;
use crate::models;
use crate::paths::Paths;
use crate::supervisor::{ServiceState, Supervisor};
use crate::tailscale;
use crate::update::UpdateStatus;
use crate::window;
use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum DockerStatus {
    Checking,
    Missing,
    /// Docker Desktop was started because its daemon didn't answer; waiting for it.
    Starting,
    NotRunning(String),
    Ok(String),
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ImagesStatus {
    Waiting,
    /// The image and docker's latest line about it, and how many of its layers are in.
    Pulling {
        text: String,
        #[serde(flatten)]
        progress: PullProgress,
    },
    Ready,
    Failed(String),
}

/// Setting Docker up from the setup page (Windows and macOS, see winsetup.rs and macsetup.rs),
/// installing Tailscale (tssetup.rs), or installing llama.cpp and a model (llamasetup.rs).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum SetupStatus {
    Idle,
    /// The step it is on.
    Running(String),
    /// Only the Windows setup (winsetup.rs) ends in these two; elsewhere they are only read.
    #[cfg_attr(not(windows), allow(dead_code))]
    NeedsSignOut,
    #[cfg_attr(not(windows), allow(dead_code))]
    NeedsRestart,
    /// Virtualization is off in the firmware, which only the user can turn on.
    NeedsVirtualization,
    /// Installed; what is left is the user's (logging in to Tailscale).
    Installed,
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

/// The chat server on another computer, when one is set (see remote.rs).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum RemoteStatus {
    /// None set: the servers run on this computer.
    Off,
    Checking,
    Connected,
    Failed(String),
}

/// Whether the chat is served on the tailnet for the phone apps (see tailscale.rs).
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ShareStatus {
    Off,
    Starting,
    /// The chat's HTTPS address on the tailnet.
    On(String),
    Failed(String),
}

/// llama.cpp as the model server, set up and run by the app (llamasetup.rs, llamaserver.rs).
#[derive(Serialize, Clone, Debug)]
pub struct LlamaStatus {
    /// What the page offers to install: this computer's GPU can run one of Cedar's models.
    pub offer: Option<llamasetup::Offer>,
    pub setup: SetupStatus,
    /// How far the download is, from 0 to 1, while there is one.
    pub progress: Option<f64>,
    /// The model it runs, once installed; `None` while the model server is the user's own.
    pub model: Option<String>,
    pub server: ServiceState,
}

pub struct AppState {
    pub paths: Paths,
    config: Mutex<Config>,
    docker: Mutex<DockerStatus>,
    docker_cli: Mutex<Option<DockerCli>>,
    docker_setup: Mutex<SetupStatus>,
    tailscale_setup: Mutex<SetupStatus>,
    images: Mutex<ImagesStatus>,
    model: Mutex<ModelStatus>,
    share: Mutex<ShareStatus>,
    remote: Mutex<RemoteStatus>,
    /// The tailnet port `tailscale serve` was set up on by this app, to turn off again.
    served_port: Mutex<Option<u16>>,
    /// The `CHAT_PUBLIC_URL` the running chat was started with.
    started_public_url: Mutex<Option<String>>,
    /// Whether a newer release is out, and how far installing it got (see update.rs).
    update: Mutex<UpdateStatus>,
    pub supervisor: Supervisor,
    /// What suits this computer's GPU, once it was looked at (see `llamasetup::detect`).
    llama_plan: Mutex<Option<cedar_llama::Plan>>,
    llama_setup: Mutex<(SetupStatus, Option<f64>)>,
    /// Stops the download that is under way.
    llama_cancel: Mutex<CancellationToken>,
    pub llama: LlamaServer,
    /// The entry to make cedar's default at the next registration (see [`Self::choose`]).
    chosen: Mutex<Chosen>,
    /// Only one launch sequence at a time.
    bootstrap: tokio::sync::Mutex<()>,
}

/// What the setup page renders.
#[derive(Serialize, Clone, Debug)]
pub struct Status {
    pub version: String,
    /// `macos` or `windows` (`std::env::consts::OS`), for the page's wording.
    pub platform: &'static str,
    pub docker: DockerStatus,
    pub docker_setup: SetupStatus,
    /// The page offers to set Docker up, and to install Tailscale (on Windows and macOS).
    pub docker_setup_available: bool,
    pub tailscale_setup: SetupStatus,
    /// The tailscale CLI was found.
    pub tailscale_installed: bool,
    pub images: ImagesStatus,
    pub services: ServiceState,
    pub model: ModelStatus,
    pub share: ShareStatus,
    /// Sharing is turned on (it is served once the servers run and Tailscale is ready).
    pub tailscale_share: bool,
    pub configured: bool,
    /// The setup page asks whether Cedar runs here or uses another computer's (once; see
    /// `Config::asks_choice`). With `configured` (an earlier setup) the servers run meanwhile.
    pub needs_choice: bool,
    /// The chat server on another computer this app uses instead of its own, or empty.
    pub remote_server: String,
    pub remote: RemoteStatus,
    pub llm_base_url: String,
    pub llm_model: String,
    pub has_api_key: bool,
    /// How the model server was named on the page (`own` or `hosted`), or empty while none is.
    /// llama.cpp run by the app is beside it, in `llama`.
    pub model_source: &'static str,
    /// The hosted providers and the presets of servers people run themselves, for the page's forms.
    pub providers: &'static [models::Provider],
    pub presets: &'static [models::Preset],
    pub browser: bool,
    pub chat_url: String,
    pub access_token: String,
    pub logs_dir: String,
    pub config_path: String,
    pub sandbox_image: String,
    pub update: UpdateStatus,
    pub llama: LlamaStatus,
    pub check_updates: bool,
    /// The language picked on the page; empty until one is (the page then asks first).
    pub locale: String,
}

impl AppState {
    pub fn new(paths: Paths, config: Config, exe_dir: PathBuf) -> Self {
        Self {
            supervisor: Supervisor::new(exe_dir, paths.clone()),
            llama_plan: Mutex::new(None),
            llama_setup: Mutex::new((SetupStatus::Idle, None)),
            llama_cancel: Mutex::new(CancellationToken::new()),
            llama: LlamaServer::new(paths.clone()),
            chosen: Mutex::new(Chosen::Neither),
            paths,
            config: Mutex::new(config),
            docker: Mutex::new(DockerStatus::Checking),
            docker_cli: Mutex::new(None),
            docker_setup: Mutex::new(SetupStatus::Idle),
            tailscale_setup: Mutex::new(SetupStatus::Idle),
            images: Mutex::new(ImagesStatus::Waiting),
            model: Mutex::new(ModelStatus::Unset),
            share: Mutex::new(ShareStatus::Off),
            remote: Mutex::new(RemoteStatus::Off),
            served_port: Mutex::new(None),
            started_public_url: Mutex::new(None),
            update: Mutex::new(UpdateStatus::default()),
            bootstrap: tokio::sync::Mutex::new(()),
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

    fn set_docker(&self, s: DockerStatus) {
        *self.docker.lock().unwrap() = s;
    }

    pub fn set_docker_setup(&self, s: SetupStatus) {
        *self.docker_setup.lock().unwrap() = s;
    }

    /// Marks the Docker setup as started, unless it already runs.
    pub fn begin_docker_setup(&self) -> bool {
        begin(&self.docker_setup)
    }

    pub fn docker_setup(&self) -> SetupStatus {
        self.docker_setup.lock().unwrap().clone()
    }

    pub fn set_tailscale_setup(&self, s: SetupStatus) {
        *self.tailscale_setup.lock().unwrap() = s;
    }

    /// Marks installing Tailscale as started, unless it already runs.
    pub fn begin_tailscale_setup(&self) -> bool {
        begin(&self.tailscale_setup)
    }

    pub fn set_llama_plan(&self, plan: Option<cedar_llama::Plan>) {
        *self.llama_plan.lock().unwrap() = plan;
    }

    pub fn llama_plan(&self) -> Option<cedar_llama::Plan> {
        self.llama_plan.lock().unwrap().clone()
    }

    /// The step the llama.cpp setup is on and, during a download, how far it is.
    pub fn set_llama_setup(&self, s: SetupStatus, progress: Option<f64>) {
        *self.llama_setup.lock().unwrap() = (s, progress);
    }

    /// Marks the llama.cpp setup as started, unless it already runs.
    pub fn begin_llama_setup(&self) -> bool {
        let mut setup = self.llama_setup.lock().unwrap();
        if matches!(setup.0, SetupStatus::Running(_)) {
            return false;
        }
        *setup = (SetupStatus::Running(crate::i18n::t("setup.starting")), None);
        true
    }

    /// A fresh token for a download that starts now; [`Self::cancel_llama`] stops that download.
    pub fn new_llama_cancel(&self) -> CancellationToken {
        let token = CancellationToken::new();
        *self.llama_cancel.lock().unwrap() = token.clone();
        token
    }

    /// Stops the llama.cpp download; what arrived stays for the next try.
    pub fn cancel_llama(&self) {
        self.llama_cancel.lock().unwrap().cancel();
    }

    /// What the chat's Settings needs to know to offer llama.cpp when a model is added: `remote`
    /// (the chat is another computer's), `installed` (the app runs one already, and one is all
    /// the GPU's memory holds), `available` (this GPU can run one), or `no_gpu`.
    pub fn llama_for_chat(&self) -> serde_json::Value {
        let cfg = self.config();
        let label = |id: &str| cedar_llama::catalog::by_id(id).map_or_else(|| id.to_string(), |m| m.label.to_string());
        let (state, model) = match (&cfg.llama, self.llama_plan()) {
            _ if cfg.remote_server().is_some() => ("remote", String::new()),
            (Some(llama), _) => ("installed", label(&llama.model)),
            (None, Some(plan)) => ("available", plan.model.label.to_string()),
            (None, None) => ("no_gpu", String::new()),
        };
        serde_json::json!({ "state": state, "model": model })
    }

    fn llama_status(&self, cfg: &Config) -> LlamaStatus {
        let (setup, progress) = self.llama_setup.lock().unwrap().clone();
        let installed = cfg.llama.as_ref().map(|l| {
            let known = cedar_llama::catalog::by_id(&l.model);
            known.map_or_else(|| l.model.clone(), |m| m.label.to_string())
        });
        LlamaStatus {
            offer: self.llama_plan().as_ref().map(llamasetup::offer),
            setup,
            progress,
            model: installed,
            server: self.llama.state(),
        }
    }

    pub fn update(&self) -> UpdateStatus {
        self.update.lock().unwrap().clone()
    }

    /// Changes the update status and returns it as it is now.
    pub fn update_status(&self, change: impl FnOnce(&mut UpdateStatus)) -> UpdateStatus {
        let mut s = self.update.lock().unwrap();
        change(&mut s);
        s.clone()
    }

    fn set_images(&self, s: ImagesStatus) {
        *self.images.lock().unwrap() = s;
    }

    fn set_model(&self, s: ModelStatus) {
        *self.model.lock().unwrap() = s;
    }

    fn set_remote(&self, s: RemoteStatus) {
        *self.remote.lock().unwrap() = s;
    }

    /// Whether the window can show the chat: this app's servers run, or the remote chat answered.
    pub fn chat_ready(&self) -> bool {
        if self.config().remote_server().is_some() {
            *self.remote.lock().unwrap() == RemoteStatus::Connected
        } else {
            self.supervisor.state() == ServiceState::Running
        }
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
            docker_setup: self.docker_setup(),
            docker_setup_available: cfg!(any(windows, target_os = "macos")),
            tailscale_setup: self.tailscale_setup.lock().unwrap().clone(),
            tailscale_installed: tailscale::find().is_some(),
            images: self.images.lock().unwrap().clone(),
            services: self.supervisor.state(),
            model: self.model.lock().unwrap().clone(),
            share: self.share.lock().unwrap().clone(),
            tailscale_share: cfg.tailscale_share,
            configured: cfg.configured(),
            needs_choice: cfg.asks_choice(),
            remote_server: cfg.remote_server().map(|r| r.server.clone()).unwrap_or_default(),
            remote: self.remote.lock().unwrap().clone(),
            llm_base_url: cfg.llm_base_url.clone(),
            llm_model: cfg.llm_model.clone(),
            has_api_key: !cfg.llm_api_key.trim().is_empty(),
            model_source: if cfg.has_server() { models::source(&cfg.model_source, &cfg.llm_base_url) } else { "" },
            providers: &models::model_servers().providers,
            presets: &models::model_servers().presets,
            browser: cfg.browser,
            chat_url: cfg.chat_url(),
            access_token: cfg.chat_token().to_string(),
            logs_dir: self.paths.logs.to_string_lossy().into_owned(),
            config_path: self.paths.config().to_string_lossy().into_owned(),
            sandbox_image: cfg.sandbox_image(),
            update: self.update(),
            llama: self.llama_status(&cfg),
            check_updates: cfg.check_updates,
            locale: cfg.locale.clone(),
        }
    }

    /// Records which of the app's two entries the user just chose; the next registration makes it
    /// the model new chats use.
    pub fn choose(&self, chosen: Chosen) {
        *self.chosen.lock().unwrap() = chosen;
    }

    /// Puts what the app manages into cedar's registry: the model it runs with llama.cpp (entry
    /// `llama`) and the model server named on the setup page (entry `default`), whichever there
    /// are. cedar must be running, and llama-server too if there is one. Neither is made the
    /// default unless the user just chose it ([`Self::choose`]), so a default picked in the chat's
    /// Settings stays.
    async fn register_model(&self) -> bool {
        let cfg = self.config();
        let (cedar, key) = (cfg.cedar_url(), cfg.secrets.cedar_api_key.clone());
        let llama_url = cfg.llama_url();
        // An entry for llama.cpp that the app no longer runs (removed while cedar was down).
        if cfg.llama.is_none() {
            let _ = models::unregister(&cedar, &key, models::LLAMA_ID).await;
        }
        if !cfg.configured() {
            self.set_model(ModelStatus::Unset);
            return false;
        }
        self.set_model(ModelStatus::Registering);
        let chosen = *self.chosen.lock().unwrap();
        let mut names = Vec::new();
        let mut result = Ok(());
        let installed = cfg.llama.as_ref().and_then(|l| cedar_llama::catalog::by_id(&l.model));
        if let Some(model) = installed {
            let entry = models::Entry {
                id: models::LLAMA_ID,
                base_url: &llama_url,
                api_key: &cfg.secrets.llama_api_key,
                model: model.id,
                extras: Some(models::Extras {
                    label: model.label,
                    max_tokens: model.max_tokens,
                    reasoning_levels: model.reasoning_levels,
                }),
                make_default: chosen == Chosen::Llama,
            };
            result = models::register(&cedar, &key, entry).await;
            names.push(model.label.to_string());
            // What an early build registered llama.cpp as: the `default` entry, at this address.
            if result.is_ok() && !cfg.has_server() {
                let stale = models::registered_address(&cedar, &key, models::REGISTRY_ID).await;
                if stale.as_deref() == Some(llama_url.as_str()) {
                    let _ = models::unregister(&cedar, &key, models::REGISTRY_ID).await;
                }
            }
        }
        if result.is_ok() && cfg.has_server() {
            let entry = models::Entry {
                id: models::REGISTRY_ID,
                base_url: &cfg.llm_base_url,
                api_key: &cfg.llm_api_key,
                model: &cfg.llm_model,
                extras: None,
                make_default: chosen == Chosen::Server,
            };
            result = models::register(&cedar, &key, entry).await;
            names.push(format!("{} at {}", cfg.llm_model.trim(), cfg.llm_base_url.trim()));
        }
        match result {
            Ok(()) => {
                self.choose(Chosen::Neither);
                self.set_model(ModelStatus::Registered(names.join(", ")));
                true
            }
            Err(e) => {
                self.set_model(ModelStatus::Failed(format!("{e:#}")));
                false
            }
        }
    }
}

/// Which of the app's two registry entries the user chose last on the setup page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Chosen {
    Neither,
    /// The model server they named (`default`).
    Server,
    /// The model the app runs with llama.cpp (`llama`).
    Llama,
}

/// Marks a setup as started, unless it already runs.
fn begin(setup: &Mutex<SetupStatus>) -> bool {
    let mut setup = setup.lock().unwrap();
    if matches!(*setup, SetupStatus::Running(_)) {
        return false;
    }
    *setup = SetupStatus::Running(crate::i18n::t("setup.starting"));
    true
}

/// The launch sequence. Safe to run again at any time (after a setting changed, say): it stops
/// at the first thing that isn't ready and the page says what that is.
pub async fn bootstrap(app: AppHandle) {
    let state = app.state::<AppState>();
    let _one_at_a_time = state.bootstrap.lock().await;

    // 0. A chat server on another computer: nothing to start here.
    if let Some(remote) = state.config().remote_server().cloned() {
        state.set_remote(RemoteStatus::Checking);
        match crate::remote::check(&remote).await {
            Ok(()) => {
                state.set_remote(RemoteStatus::Connected);
                if let Err(e) = window::show_chat(&app) {
                    eprintln!("showing the chat: {e:#}");
                }
            }
            Err(e) => state.set_remote(RemoteStatus::Failed(format!("{e:#}"))),
        }
        return;
    }
    // Not before the user said Cedar runs here: Docker isn't looked for (or started), and no
    // image is downloaded, for someone about to connect to another computer instead. An earlier
    // setup starts as before while the question is open (see step 4).
    if state.config().holds_start() {
        return;
    }

    // 1. The docker CLI and its daemon.
    state.set_docker(DockerStatus::Checking);
    let Some(cli) = docker::find() else {
        state.set_docker(DockerStatus::Missing);
        return;
    };
    let mut answer = cli.server_version().await;
    if answer.is_err() {
        answer = start_docker_desktop(&state, &cli).await.unwrap_or(answer);
    }
    match answer {
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
        state.set_images(ImagesStatus::Pulling {
            text: format!("{image}: {}", crate::i18n::t("images.starting")),
            progress: PullProgress::default(),
        });
        let pulled = cli
            .pull(&image, |line, progress| {
                state.set_images(ImagesStatus::Pulling {
                    text: format!("{image}: {line}"),
                    progress,
                })
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

    // 4. llama.cpp, when the app runs it as the model server: the model is loaded before cedar
    //    is told about it, since cedar asks a model server for its models when it registers one.
    if !start_llama(&state).await {
        return;
    }

    // 5. The models, into cedar's registry. A failure here (the model server is down, say) leaves
    //    the servers up: cedar keeps the last registered model, and the page says what's wrong.
    //    While the setup page asks how Cedar is used, the window stays on the question.
    if state.register_model().await && !state.config().asks_choice() {
        if let Err(e) = window::show_chat(&app) {
            eprintln!("showing the chat: {e:#}");
        }
    }
}

/// Starts the app's llama-server when the app runs a model with llama.cpp (downloading a build
/// first if an update of the app pinned a newer one), and stops it when it does not. `false` with
/// the reason on the page when the model could not be loaded.
async fn start_llama(state: &AppState) -> bool {
    let started = match llamasetup::ensure(state).await {
        Ok(None) => {
            state.llama.stop().await;
            return true;
        }
        Ok(Some(spec)) => state.llama.start(spec).await.map_err(|e| format!("{e:#}")),
        Err(why) => Err(why),
    };
    match started {
        Ok(()) => true,
        Err(why) => {
            state.set_model(ModelStatus::Failed(why));
            false
        }
    }
}

/// The question's "this computer": recorded, then the chat once the servers run and the model is
/// registered. An earlier setup's servers are already up (or the launch sequence is still on its
/// way and shows the chat at its end), so they are not started again, which would restart them.
pub fn choose_local(app: AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    state.update_config(|c| c.run_locally = true)?;
    let launching = state.bootstrap.try_lock().is_err();
    let registered = matches!(*state.model.lock().unwrap(), ModelStatus::Registered(_));
    if launching {
        return Ok(());
    }
    if state.supervisor.state() == ServiceState::Running {
        if registered {
            window::show_chat(&app)?;
        }
        return Ok(());
    }
    tauri::async_runtime::spawn(bootstrap(app.clone()));
    Ok(())
}

/// Starts Docker Desktop (on a Mac, whichever app the CLI comes from) when its daemon doesn't
/// answer, and waits for it, or `None` if there is nothing to start. Not while the setup is
/// installing it or waiting for the restart it needs.
async fn start_docker_desktop(state: &AppState, cli: &DockerCli) -> Option<Result<String>> {
    let desktop = docker::provider_app(cli)?;
    if !matches!(
        state.docker_setup(),
        SetupStatus::Idle | SetupStatus::Failed(_)
    ) {
        return None;
    }
    if let Err(e) = docker::launch(&desktop) {
        eprintln!("starting {}: {e:#}", desktop.display());
        return None;
    }
    state.set_docker(DockerStatus::Starting);
    // Its first start after installing sets up its WSL distribution or VM, which takes a while.
    Some(cli.wait_until_answering(Duration::from_secs(240)).await)
}

/// After a model was chosen or removed on the setup page: registers what there is with the
/// running cedar, or runs the launch sequence if the servers aren't up yet. No restart, so a run
/// in progress goes on.
pub async fn apply_model_server(app: AppHandle) {
    let state = app.state::<AppState>();
    if state.supervisor.state() != ServiceState::Running {
        bootstrap(app.clone()).await;
        return;
    }
    if !start_llama(&state).await {
        return;
    }
    let registered = state.register_model().await;
    window::push_llama(&app);
    if registered {
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
    refuse_when_remote(&cfg)?;
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

/// The Phone page's "Turn on sharing" (chat/static/pairing.js, through the window): the same as
/// ticking the switch on the setup page. The chat can't show the outcome, since turning sharing
/// on restarts it, so when it can't be turned on the setup page says why instead.
pub async fn share_from_chat(app: AppHandle) {
    if let Err(e) = set_share(app.clone(), true).await {
        let state = app.state::<AppState>();
        state.set_share(ShareStatus::Failed(format!("{e:#}")));
        if let Err(e) = window::show_setup(&app) {
            eprintln!("showing the setup page: {e:#}");
        }
    }
}

/// The settings of this computer's own servers (model server, browser tool, sharing) don't apply
/// while the app uses a chat server on another computer; that server's own Settings has them.
pub fn refuse_when_remote(cfg: &Config) -> Result<()> {
    match cfg.remote_server() {
        Some(r) => Err(anyhow::anyhow!(crate::i18n::tf(
            "remote.in_use",
            &[("server", &r.server)]
        ))),
        None => Ok(()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Llama, Remote};
    use cedar_llama::gpu::Gpu;
    use cedar_llama::release::{Arch, Os};

    fn state(cfg: Config) -> AppState {
        let dir = PathBuf::from("/tmp/cedar-app-test");
        let paths = Paths { data: dir.clone(), logs: dir.clone(), tools: dir.clone() };
        AppState::new(paths, cfg, dir)
    }

    /// What the chat's Settings is told about llama.cpp, for its "Add a model".
    #[test]
    fn what_settings_learns_about_llama() {
        let plan = cedar_llama::plan(Os::Macos, Arch::Arm64, &[Gpu::apple(32 << 30)]);
        let say = |s: &AppState| s.llama_for_chat();

        let none = state(Config::default());
        assert_eq!(say(&none), serde_json::json!({ "state": "no_gpu", "model": "" }));
        none.set_llama_plan(plan.clone());
        assert_eq!(say(&none), serde_json::json!({ "state": "available", "model": "gpt-oss 20B" }));

        // One is installed: that is the one, whatever else the GPU could run.
        let llama = Some(Llama { build: "macos-arm64".into(), model: "qwen3.6-35b-a3b".into() });
        let installed = state(Config { llama: llama.clone(), ..Config::default() });
        installed.set_llama_plan(plan.clone());
        assert_eq!(say(&installed), serde_json::json!({ "state": "installed", "model": "Qwen3.6 35B-A3B" }));

        // The chat is another computer's: nothing runs here.
        let remote = Some(Remote { server: "https://spark.t.ts.net".into(), token: "t".into() });
        let away = state(Config { llama, remote, ..Config::default() });
        away.set_llama_plan(plan);
        assert_eq!(say(&away), serde_json::json!({ "state": "remote", "model": "" }));
    }

    /// The page's view of the two things the app manages: they are independent.
    #[test]
    fn a_model_server_beside_llama() {
        let cfg = Config {
            llama: Some(Llama { build: "macos-arm64".into(), model: "gpt-oss-20b".into() }),
            llm_base_url: "https://api.groq.com/openai/v1".into(),
            llm_model: "openai/gpt-oss-120b".into(),
            ..Config::default()
        };
        let both = state(cfg.clone()).status();
        assert!(both.configured);
        assert_eq!(both.model_source, "hosted");
        assert_eq!(both.llama.model.as_deref(), Some("gpt-oss 20B"));

        let only_llama = state(Config { llm_base_url: String::new(), llm_model: String::new(), ..cfg }).status();
        assert!(only_llama.configured);
        assert_eq!(only_llama.model_source, "");
        assert_eq!(only_llama.llama.model.as_deref(), Some("gpt-oss 20B"));
        assert_eq!(only_llama.presets.len(), 5);
    }

    /// A stand-in for cedar's admin API that records each request as "METHOD path body" and
    /// lists `default` at `default_at`.
    async fn cedar(default_at: &'static str) -> (u16, std::sync::Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                let (head, body) = loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request).into_owned();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
                    let length = head
                        .lines()
                        .find_map(|l| l.to_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    if body.len() >= length || n == 0 {
                        break (head.to_string(), body.to_string());
                    }
                };
                let line = head.lines().next().unwrap_or_default().trim_end_matches(" HTTP/1.1").to_string();
                log.lock().unwrap().push(format!("{line} {body}").trim().to_string());
                let answer = format!(r#"{{"models":[{{"id":"default","base_url":"{default_at}"}}]}}"#);
                let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}", answer.len());
                let _ = socket.write_all(reply.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        (port, seen)
    }

    fn sent(seen: &std::sync::Arc<Mutex<Vec<String>>>) -> Vec<String> {
        std::mem::take(&mut *seen.lock().unwrap())
    }

    /// What goes into cedar's registry for a model server beside a model the app runs: two
    /// entries, and only the one just chosen is made the default, once.
    #[tokio::test]
    async fn two_entries_and_the_chosen_one_is_the_default() {
        let (port, seen) = cedar("https://api.groq.com/openai/v1").await;
        let app = state(Config {
            cedar_port: port,
            llama: Some(Llama { build: "macos-arm64".into(), model: "gpt-oss-20b".into() }),
            llm_base_url: "https://api.groq.com/openai/v1".into(),
            llm_model: "openai/gpt-oss-120b".into(),
            llm_api_key: "gsk".into(),
            ..Config::default()
        });
        app.update_config(|c| c.secrets.llama_api_key = "lk".into()).ok();

        // A launch: both, neither made the default.
        assert!(app.register_model().await);
        let requests = sent(&seen);
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(requests[0].starts_with("PUT /v1/admin/models/llama "), "{}", requests[0]);
        let llama: serde_json::Value = serde_json::from_str(requests[0].split_once(' ').unwrap().1.split_once(' ').unwrap().1).unwrap();
        assert_eq!(llama["base_url"], "http://127.0.0.1:18081/v1");
        assert_eq!((&llama["model"], &llama["label"], &llama["api_key"]), (&"gpt-oss-20b".into(), &"gpt-oss 20B".into(), &"lk".into()));
        assert_eq!(llama["max_tokens"], 8192);
        assert!(llama.get("is_default").is_none());
        assert!(requests[1].starts_with("PUT /v1/admin/models/default "), "{}", requests[1]);
        assert!(requests[1].contains(r#""base_url":"https://api.groq.com/openai/v1""#) && !requests[1].contains("is_default"));
        assert!(matches!(&*app.model.lock().unwrap(), ModelStatus::Registered(d) if d == "gpt-oss 20B, openai/gpt-oss-120b at https://api.groq.com/openai/v1"));

        // llama.cpp was just set up: it is made the default, the model server is not.
        app.choose(Chosen::Llama);
        assert!(app.register_model().await);
        let requests = sent(&seen);
        assert!(requests[0].contains(r#""is_default":true"#) && !requests[1].contains("is_default"), "{requests:?}");
        // Only that once.
        assert!(app.register_model().await);
        assert!(sent(&seen).iter().all(|r| !r.contains("is_default")));
        // A model server was just saved: the other way round.
        app.choose(Chosen::Server);
        assert!(app.register_model().await);
        let requests = sent(&seen);
        assert!(!requests[0].contains("is_default") && requests[1].contains(r#""is_default":true"#), "{requests:?}");
    }

    /// llama.cpp removed: its entry leaves the registry and the model server stays. With nothing
    /// left there is nothing to register.
    #[tokio::test]
    async fn a_removed_llama_leaves_the_registry() {
        let (port, seen) = cedar("https://api.groq.com/openai/v1").await;
        let app = state(Config {
            cedar_port: port,
            llm_base_url: "https://api.groq.com/openai/v1".into(),
            llm_model: "m".into(),
            ..Config::default()
        });
        assert!(app.register_model().await);
        let requests = sent(&seen);
        assert_eq!(requests[0], "DELETE /v1/admin/models/llama");
        assert!(requests[1].starts_with("PUT /v1/admin/models/default "));
        assert_eq!(requests.len(), 2);

        let (port, seen) = cedar("https://api.groq.com/openai/v1").await;
        let empty = state(Config { cedar_port: port, ..Config::default() });
        assert!(!empty.register_model().await);
        assert_eq!(sent(&seen), ["DELETE /v1/admin/models/llama"]);
        assert!(matches!(&*empty.model.lock().unwrap(), ModelStatus::Unset));
    }

    /// What an early build registered: llama.cpp as the `default` entry. With no model server of
    /// the user's, that entry is taken out; one that points elsewhere is not touched.
    #[tokio::test]
    async fn the_doubled_default_entry_is_taken_out() {
        let llama = Some(Llama { build: "macos-arm64".into(), model: "gpt-oss-20b".into() });
        let (port, seen) = cedar("http://127.0.0.1:18081/v1").await;
        let app = state(Config { cedar_port: port, llama: llama.clone(), ..Config::default() });
        assert!(app.register_model().await);
        let requests = sent(&seen);
        assert!(requests[0].starts_with("PUT /v1/admin/models/llama "));
        assert_eq!(&requests[1..], ["GET /v1/admin/models", "DELETE /v1/admin/models/default"]);

        let (port, seen) = cedar("http://10.0.0.5:8000/v1").await;
        let app = state(Config { cedar_port: port, llama, ..Config::default() });
        assert!(app.register_model().await);
        assert_eq!(sent(&seen).len(), 2, "the PUT and the GET, no DELETE");
    }
}
