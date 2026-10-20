//! cedar-chat: a web chat for a cedar server.
//!
//! Run:  CEDAR_API_KEY=devkey cargo run -p cedar-chat   then open http://127.0.0.1:3000
//! See chat/README.md for configuration.

use anyhow::{Context, Result};
use cedar_chat::api::{self, AppState};
use cedar_chat::config::Config;
use cedar_chat::{notify, schedule};
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
    // Finished runs are learned from cedar's event stream, so a response that ends while no
    // tab is open is still marked unread and announced.
    let notifier = notify::start(state.clone(), stop.clone());

    let listener = tokio::net::TcpListener::bind(bind_addr).await.with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!("open http://{bind_addr}");
    let streams = state.shutdown.clone();
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("shutting down");
            // End the event streams first: the shutdown waits for every connection to close, and
            // theirs never would.
            streams.cancel();
        })
        .await?;

    // Let a tick that is starting runs finish recording them.
    stop.cancel();
    if tokio::time::timeout(Duration::from_secs(10), scheduler).await.is_err() {
        tracing::warn!("the scheduler was still starting runs at shutdown");
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), notifier).await;
    Ok(())
}

/// Ctrl-C, SIGTERM (what systemd and the macOS app send), or, with `CEDAR_EXIT_ON_STDIN_EOF=1`,
/// stdin reaching end-of-file because the launcher holding the pipe is gone.
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

async fn stdin_closed() {
    let on = matches!(
        std::env::var("CEDAR_EXIT_ON_STDIN_EOF").as_deref().map(str::trim),
        Ok("1") | Ok("on") | Ok("true") | Ok("yes")
    );
    if !on {
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
