//! cedar: a backend service for agent runs.
//!
//! Run:  CEDAR_API_KEYS=devkey cargo run   (needs an OpenAI-compatible LLM server, e.g. vLLM)
//! See README.md for the HTTP API and configuration.

use anyhow::{Context, Result};
use cedar::api::{self, AppState};
use cedar::config::Config;
use cedar::llm::ChatCompletionsClient;
use cedar::sandbox::DockerSandboxFactory;
use cedar::store::Store;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .init();

    let cfg = Config::from_env()?;
    let store = Store::connect(&cfg.database_url).await?;
    let llm = Arc::new(ChatCompletionsClient::new(
        cfg.llm_base_url.clone(),
        cfg.llm_api_key.clone(),
    )?);
    let sandboxes = Arc::new(DockerSandboxFactory::new(cfg.sandbox.clone()));
    let bind_addr = cfg.bind_addr;
    let workspace_ttl = cfg.workspace_ttl;

    tracing::info!(web_search = cfg.brave_api_key.is_some(), "tools configured");

    let state = AppState::new(cfg, store, llm, sandboxes)?;
    state.manager.recover().await?;
    state.manager.start_janitor(workspace_ttl);

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!(%bind_addr, "listening");

    let manager = state.manager.clone();
    let on_signal = manager.clone();
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("shutting down; cancelling active runs");
            // Cancel runs first: open event streams only close once their run reaches a terminal event.
            on_signal.begin_shutdown();
        })
        .await?;

    manager.drain(SHUTDOWN_GRACE).await;
    tracing::info!("stopped");
    Ok(())
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
    }
}
