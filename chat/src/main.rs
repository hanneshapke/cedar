//! cedar-chat: a web chat for a cedar server.
//!
//! Run:  CEDAR_API_KEY=devkey cargo run -p cedar-chat   then open http://127.0.0.1:3000
//! See chat/README.md for configuration.

use anyhow::{Context, Result};
use cedar_chat::api::{self, AppState};
use cedar_chat::config::Config;
use cedar_chat::schedule;
use cedar_chat::store::Store;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")))
        .init();

    let cfg = Config::from_env()?;
    let bind_addr = cfg.bind_addr;
    if cfg.access_token.is_none() && !bind_addr.ip().is_loopback() {
        tracing::warn!(%bind_addr, "CHAT_ACCESS_TOKEN is not set: anyone who can reach this address can use the agent");
    }
    let store = Store::connect(&cfg.database_url).await?;
    tracing::info!(cedar_url = %cfg.cedar_url, "using cedar server");
    let state = AppState::new(cfg, store)?;

    // Scheduled agents run from this process: due schedules are started in the background.
    let stop = CancellationToken::new();
    let scheduler = schedule::start(state.clone(), stop.clone());

    let listener = tokio::net::TcpListener::bind(bind_addr).await.with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!("open http://{bind_addr}");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    // Let a tick that is starting runs finish recording them.
    stop.cancel();
    if tokio::time::timeout(Duration::from_secs(10), scheduler).await.is_err() {
        tracing::warn!("the scheduler was still starting runs at shutdown");
    }
    Ok(())
}
