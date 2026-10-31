//! cedar-contrib: run with CONTRIB_HMAC_KEY and CONTRIB_BUCKET set (see contrib/README.md).

use anyhow::{Context, Result};
use cedar_contrib::config::Config;
use cedar_contrib::gcs::Gcs;
use cedar_contrib::store::{GcsStore, MemoryStore, Store};
use cedar_contrib::{router, AppState};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let cfg = Config::from_env()?;
    let store: Arc<dyn Store> = match &cfg.bucket {
        Some(bucket) => {
            tracing::info!(%bucket, "quarantine bucket");
            Arc::new(GcsStore::new(Gcs::new(bucket, cfg.access_token.clone())?))
        }
        None => {
            tracing::warn!("CONTRIB_BUCKET is not set: keeping everything in memory");
            Arc::new(MemoryStore::default())
        }
    };
    let bind_addr = cfg.bind_addr;
    let state = AppState::new(cfg, store);
    let listener = tokio::net::TcpListener::bind(bind_addr).await.with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!("listening on {bind_addr}");
    axum::serve(listener, router(state).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
