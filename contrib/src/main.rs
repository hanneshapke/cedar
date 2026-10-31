//! cedar-contrib: the ingest (`cedar-contrib`, the default), the screener (`screen`), the
//! release job (`release`), the ablation evaluation (`ablation`) and the statistics (`stats`).
//! See contrib/README.md.

use anyhow::{bail, Context, Result};
use cedar_contrib::config::Config;
use cedar_contrib::gcs::Gcs;
use cedar_contrib::judge::{HttpJudge, Judge};
use cedar_contrib::store::{GcsStore, MemoryStore, Store};
use cedar_contrib::{ablation, release, router, screen, stats, AppState};
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
    let judge: Option<Box<dyn Judge>> = match &cfg.judge_url {
        Some(url) => Some(Box::new(HttpJudge::new(url, &cfg.judge_model, cfg.judge_api_key.clone())?)),
        None => None,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("screen") => {
            let rescreen = args.iter().any(|a| a == "--rescreen");
            let force = args.iter().any(|a| a == "--force");
            let batch = arg_value(&args, "--batch").map(|v| v.parse()).transpose()?.unwrap_or(cfg.screen_batch);
            if judge.is_none() {
                tracing::warn!("no judge (CONTRIB_JUDGE_URL): deterministic checks only; injected results are held for review");
            }
            let n = screen::run(store.as_ref(), judge.as_deref(), batch, rescreen, force).await?;
            println!("screened {n}");
        }
        Some("ablation") => {
            let mut release_opts = release::Options { aging_days: cfg.aging_days, dry_run: true, ..Default::default() };
            if let Some(v) = arg_value(&args, "--version") {
                release_opts.version = v.to_string();
            }
            let opts = ablation::AblationOptions {
                release: release_opts,
                out_dir: arg_value(&args, "--out").map(Into::into).unwrap_or_else(|| std::env::temp_dir().join("cedar-contrib-ablation")),
                hook: arg_value(&args, "--hook").map(String::from).or_else(|| cfg.ablation_hook.clone()),
                delta: arg_value(&args, "--delta").map(|v| v.parse()).transpose()?.unwrap_or(0.02),
                apply: args.iter().any(|a| a == "--apply"),
            };
            let report = ablation::run(store.as_ref(), judge.as_deref(), &opts).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Some("stats") => {
            let days = arg_value(&args, "--days").map(|v| v.parse()).transpose()?.unwrap_or(30);
            let save = args.iter().any(|a| a == "--save");
            println!("{}", serde_json::to_string_pretty(&stats::run(store.as_ref(), days, save).await?)?);
        }
        Some("release") => {
            let mut opts = release::Options { aging_days: cfg.aging_days, release_key: cfg.release_key.clone(), ..Default::default() };
            if let Some(v) = arg_value(&args, "--version") {
                opts.version = v.to_string();
            }
            if let Some(d) = arg_value(&args, "--out") {
                opts.out_dir = Some(d.into());
            }
            opts.dry_run = args.iter().any(|a| a == "--dry-run");
            if !opts.version.starts_with('v') || opts.version.contains('/') {
                bail!("--version must look like v2026.11");
            }
            let summary = release::run(store.as_ref(), judge.as_deref(), &opts).await?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        Some(other) if other.starts_with('-') || other == "serve" => serve(cfg, store).await?,
        Some(other) => bail!("unknown command {other:?}; use: serve (default), screen [--batch N] [--rescreen] [--force], release [--version vYYYY.MM] [--out DIR] [--dry-run], ablation [--version V] [--out DIR] [--hook CMD] [--delta D] [--apply], stats [--days N] [--save]"),
        None => serve(cfg, store).await?,
    }
    Ok(())
}

fn arg_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

async fn serve(cfg: Config, store: Arc<dyn Store>) -> Result<()> {
    let bind_addr = cfg.bind_addr;
    if cfg.review_token.is_some() {
        tracing::info!("review page at /review");
    }
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
