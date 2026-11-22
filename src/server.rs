//! Running cedar's HTTP server, with room for a downstream build to extend it.
//!
//! `src/main.rs` calls [`serve`] with no [`Extensions`]. A separate crate that depends on cedar
//! (an enterprise edition, an in-house fork) can ship its own binary that calls it with some:
//!
//! ```no_run
//! use axum::routing::get;
//! use cedar::server::{serve, Extensions};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let cfg = cedar::config::Config::from_env()?;
//! let ext = Extensions::default()
//!     .v1_routes(axum::Router::new().route("/audit", get(|| async { "[]" })))
//!     .on_start(|state| async move {
//!         // e.g. state.store.run_extension_migrations(sqlx::migrate!("./migrations")).await
//!         let _ = state;
//!         Ok(())
//!     });
//! serve(cfg, ext).await
//! # }
//! ```

use crate::api::{self, AppState};
use crate::config::Config;
use crate::llm::LlmRouter;
use crate::sandbox::Sandboxes;
use crate::store::Store;
use anyhow::{Context, Result};
use axum::Router;
use futures_util::future::BoxFuture;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Runs once at startup with the wired-up state; an error stops the server from starting.
pub type StartHook = Box<dyn FnOnce(AppState) -> BoxFuture<'static, Result<()>> + Send>;
/// Wraps the finished router, e.g. in a `tower` middleware.
pub type RouterLayer = Box<dyn FnOnce(Router) -> Router + Send>;

/// What a downstream build adds to cedar's server. The default adds nothing.
#[derive(Default)]
pub struct Extensions {
    /// Nested under `/v1`, behind the same authentication as cedar's own routes.
    pub v1_routes: Router<AppState>,
    /// Mounted at the top level without authentication.
    pub public_routes: Router<AppState>,
    /// Run in order after the model registry is loaded, before interrupted runs are recovered
    /// and before the server listens. Apply migrations here
    /// ([`Store::run_extension_migrations`]) and spawn background tasks (they must not block).
    pub on_start: Vec<StartHook>,
    /// Applied in order to the finished router, so the last one is outermost.
    pub layers: Vec<RouterLayer>,
}

impl Extensions {
    /// Add routes under `/v1` (authenticated). May be called more than once.
    pub fn v1_routes(mut self, routes: Router<AppState>) -> Self {
        self.v1_routes = self.v1_routes.merge(routes);
        self
    }

    /// Add top-level routes (not authenticated). May be called more than once.
    pub fn public_routes(mut self, routes: Router<AppState>) -> Self {
        self.public_routes = self.public_routes.merge(routes);
        self
    }

    /// Add a startup hook (see [`Self::on_start`](Extensions#structfield.on_start)).
    pub fn on_start<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce(AppState) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.on_start
            .push(Box::new(move |state| Box::pin(hook(state))));
        self
    }

    /// Wrap the finished router, e.g. `|r| r.layer(my_middleware)`.
    pub fn layer(mut self, layer: impl FnOnce(Router) -> Router + Send + 'static) -> Self {
        self.layers.push(Box::new(layer));
        self
    }
}

/// Open the database, wire up the services, apply `ext`, and serve the API on `cfg.bind_addr`
/// until SIGINT/SIGTERM (or stdin closing, with `CEDAR_EXIT_ON_STDIN_EOF=1`). Active runs are
/// cancelled and given [`SHUTDOWN_GRACE`] to finish. Logging is the caller's to set up.
pub async fn serve(cfg: Config, ext: Extensions) -> Result<()> {
    let store = Store::connect(&cfg.database_url).await?;
    // One client per registered model; Integrations fills it from the registry and keeps it
    // current as admins change it.
    let llm = Arc::new(LlmRouter::new());
    let sandboxes = Arc::new(Sandboxes::new(cfg.sandbox.clone(), store.clone()));
    let bind_addr = cfg.bind_addr;
    let workspace_ttl = cfg.workspace_ttl;

    tracing::info!(
        web_search = cfg.brave_api_key.is_some(),
        web_fetch = cfg.web_fetch,
        browser = cfg.browser.enabled,
        computer = cfg.computer.enabled,
        weather = cfg.weather,
        google_api = ?cfg.google_apis.iter().map(|s| s.prefix.as_str()).collect::<Vec<_>>(),
        mcp = ?cfg.mcp_servers.iter().map(|s| s.prefix.as_str()).collect::<Vec<_>>(),
        "tools configured"
    );
    if cfg.browser.enabled {
        tracing::warn!(
            network = %cfg.browser.network,
            "BROWSER is on: only the URLs the model navigates to are checked, so pages the browser \
             reaches by itself (links, redirects, scripts) can reach whatever its Docker network can"
        );
    }
    if cfg.computer.enabled && cfg.computer.native() && !cfg!(feature = "native-computer") {
        // Not a reason to refuse to start: an app that bundles such a cedar would not come up.
        tracing::warn!(
            "COMPUTER_RUNTIME=native, but this cedar was built without the driver for its own \
             computer (build it with `--features native-computer`): the computer tool works a \
             desktop in a container instead"
        );
    } else if cfg.computer.enabled && cfg.computer.native() {
        tracing::warn!(
            "COMPUTER is on with COMPUTER_RUNTIME=native: in attended runs the model can see this \
             computer's screen and use its mouse and keyboard, once the user allows it"
        );
    } else if cfg.computer.enabled && cfg.computer.online() {
        tracing::warn!(
            network = %cfg.computer.network,
            "COMPUTER is on with network access: the model works a full desktop with a web browser, \
             which can reach whatever its Docker network can, and sees the conversation's workspace"
        );
    }
    if cfg.web_fetch_allow_private {
        tracing::warn!(
            "WEB_FETCH_ALLOW_PRIVATE is on: fetch_url and download_file can reach this host and its internal network"
        );
    }
    if cfg.brave_api_key.is_none() && cfg.secret_key.is_none() {
        tracing::warn!(
            "BRAVE_API_KEY is not set: web_search is disabled and the model cannot search the web"
        );
    }

    let Extensions {
        v1_routes,
        public_routes,
        on_start,
        layers,
    } = ext;

    let state = AppState::new(cfg, store, llm.clone(), sandboxes)?;
    state.integrations.attach_router(llm);
    // Loads the model registry and adds the integrations saved through the admin API (keys) to
    // what the environment configures.
    state.integrations.reload().await?;
    // Before runs are recovered or a request is served, so an extension's tables and tasks are
    // ready for both.
    for hook in on_start {
        hook(state.clone()).await?;
    }
    match state.integrations.models().len() {
        0 => tracing::warn!(
            "no model is registered: runs are refused until an admin adds one (PUT /v1/admin/models/{{id}}, or Settings in the chat UI)"
        ),
        n => tracing::info!(models = n, "model registry loaded"),
    }
    state.manager.recover().await?;
    state.manager.start_janitor(workspace_ttl);
    // Delivers the webhooks of runs that end (and any left undelivered by a previous process).
    let webhooks = state.webhooks.start();

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!(%bind_addr, "listening");

    let manager = state.manager.clone();
    let on_signal = manager.clone();
    let state_for_shutdown = state.webhooks.clone();
    let mut app = api::router_with(state, v1_routes, public_routes);
    for layer in layers {
        app = layer(app);
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("shutting down; cancelling active runs");
            // Cancel runs first: open event streams only close once their run reaches a terminal event.
            on_signal.begin_shutdown();
        })
        .await?;

    manager.drain(SHUTDOWN_GRACE).await;
    // The runs just cancelled have webhooks to send; give them a moment.
    state_webhooks_stop(&state_for_shutdown, webhooks).await;
    tracing::info!("stopped");
    Ok(())
}

/// Let the worker finish the deliveries that shutdown's cancellations queued, briefly.
async fn state_webhooks_stop(
    webhooks: &Arc<crate::webhooks::Webhooks>,
    worker: tokio::task::JoinHandle<()>,
) {
    if let Err(e) = tokio::time::timeout(Duration::from_secs(2), webhooks.deliver_due()).await {
        tracing::warn!(error = %e, "webhooks were still being delivered at shutdown");
    }
    webhooks.stop();
    let _ = worker.await;
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
        _ = stdin_closed() => {
            tracing::info!("stdin closed; the launcher is gone");
        }
    }
}

/// Resolves when stdin reaches end-of-file, if `CEDAR_EXIT_ON_STDIN_EOF=1`. A launcher that holds
/// the other end of a pipe (the macOS app) thereby stops cedar when it quits or crashes, so no
/// server is left behind on the port. Otherwise never resolves.
async fn stdin_closed() {
    if !exit_on_stdin_eof() {
        return std::future::pending().await;
    }
    // A plain thread, not `tokio::io::stdin`: tokio reads stdin on its blocking pool, and dropping
    // the runtime waits for that read, which only ends when the launcher closes the pipe. The
    // launcher keeps it open until the process exits, so every SIGTERM ended in its SIGKILL.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 256];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = tx.send(());
    });
    let _ = rx.await;
}

fn exit_on_stdin_eof() -> bool {
    matches!(
        std::env::var("CEDAR_EXIT_ON_STDIN_EOF")
            .as_deref()
            .map(str::trim),
        Ok("1") | Ok("on") | Ok("true") | Ok("yes")
    )
}
