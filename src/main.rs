//! cedar: a backend service for agent runs.
//!
//! Run:  CEDAR_API_KEYS=devkey cargo run   (needs an OpenAI-compatible LLM server, e.g. vLLM)
//! See README.md for the HTTP API and configuration.

use anyhow::{Context, Result};
use cedar::api::{self, AppState};
use cedar::config::Config;
use cedar::llm::LlmRouter;
use cedar::sandbox::OpenShellSandboxFactory;
use cedar::store::Store;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<()> {
    // For scripts/cedar-update.sh, which compares the installed version with the release's.
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("cedar {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .init();

    let cfg = Config::from_env()?;
    let store = Store::connect(&cfg.database_url).await?;
    // One client per registered model; Integrations fills it from the registry and keeps it
    // current as admins change it.
    let llm = Arc::new(LlmRouter::new());
    let sandboxes = Arc::new(OpenShellSandboxFactory::new(cfg.sandbox.clone()));
    let bind_addr = cfg.bind_addr;
    let workspace_ttl = cfg.workspace_ttl;

    tracing::info!(
        web_search = cfg.brave_api_key.is_some(),
        web_fetch = cfg.web_fetch,
        browser = cfg.browser.enabled,
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

    let state = AppState::new(cfg, store, llm.clone(), sandboxes)?;
    state.integrations.attach_router(llm);
    // Loads the model registry and adds the integrations saved through the admin API (keys) to
    // what the environment configures.
    state.integrations.reload().await?;
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
    axum::serve(listener, api::router(state))
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
    webhooks: &std::sync::Arc<cedar::webhooks::Webhooks>,
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
        std::env::var("CEDAR_EXIT_ON_STDIN_EOF").as_deref().map(str::trim),
        Ok("1") | Ok("on") | Ok("true") | Ok("yes")
    )
}
