//! The HTTP API.

use crate::agent::AgentEnv;
use crate::auth::{self, AuthKeys, Owner};
use crate::config::Config;
use crate::error::AppError;
use crate::events::EventHub;
use crate::llm::LlmClient;
use crate::manager::RunManager;
use crate::models::{CreateRunRequest, NewRun, Run, RunStatus, StoredMessage};
use crate::sandbox::SandboxFactory;
use crate::sse;
use crate::store::{InsertRefused, Store};
use crate::tools::ToolSet;
use anyhow::Result;
use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware;
use axum::response::sse::Sse;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_PROMPT_BYTES: usize = 100_000;
const MAX_SYSTEM_BYTES: usize = 20_000;
const MAX_MODEL_LEN: usize = 100;
const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 100;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub store: Store,
    pub hub: Arc<EventHub>,
    pub manager: RunManager,
    pub sandboxes: Arc<dyn SandboxFactory>,
    pub auth: AuthKeys,
}

impl AppState {
    /// Wire the services together. The LLM client and sandbox factory are injected so tests can
    /// substitute fakes. Fails only if a tool can't be set up (e.g. the HTTP client for web search).
    pub fn new(
        cfg: Config,
        store: Store,
        llm: Arc<dyn LlmClient>,
        sandboxes: Arc<dyn SandboxFactory>,
    ) -> Result<Self> {
        let hub = Arc::new(EventHub::new());
        let env = AgentEnv {
            llm,
            tools: Arc::new(ToolSet::from_config(&cfg)?),
            store: store.clone(),
            max_tokens: cfg.max_tokens,
            tool_timeout: cfg.tool_timeout,
            tool_output_max_bytes: cfg.tool_output_max_bytes,
        };
        let manager = RunManager::new(
            store.clone(),
            hub.clone(),
            env,
            sandboxes.clone(),
            cfg.max_concurrent_runs,
            cfg.run_timeout,
        );
        let auth = AuthKeys::new(&cfg.api_keys);
        Ok(Self {
            cfg: Arc::new(cfg),
            store,
            hub,
            manager,
            sandboxes,
            auth,
        })
    }
}

pub fn router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/runs", post(create_run).get(list_runs))
        .route("/runs/{id}", get(get_run).delete(delete_run))
        .route("/runs/{id}/messages", get(get_messages))
        .route("/runs/{id}/events", get(stream_events))
        .route("/runs/{id}/cancel", post(cancel_run))
        .route_layer(middleware::from_fn_with_state(
            state.auth.clone(),
            auth::require_auth,
        ));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .nest("/v1", v1)
        .fallback(|| async { AppError::NotFound("no such route") })
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// `Json` / `Query` that report malformed input in our own error format.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
struct ApiJson<T>(T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(AppError))]
struct ApiQuery<T>(T);

fn parse_run_id(raw: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw).map_err(|_| AppError::NotFound("run not found"))
}

/// Fetch a run the caller owns. Other people's runs look exactly like missing ones.
async fn owned_run(state: &AppState, owner: &Owner, raw_id: &str) -> Result<Run, AppError> {
    let id = parse_run_id(raw_id)?;
    match state.store.get_run(id).await? {
        Some(run) if run.owner == owner.0 => Ok(run),
        _ => Err(AppError::NotFound("run not found")),
    }
}

// -- runs -----------------------------------------------------------------------------------

async fn create_run(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    ApiJson(req): ApiJson<CreateRunRequest>,
) -> Result<(StatusCode, Json<Run>), AppError> {
    let cfg = &state.cfg;

    if req.prompt.trim().is_empty() {
        return Err(AppError::BadRequest("prompt must not be empty".into()));
    }
    if req.prompt.len() > MAX_PROMPT_BYTES {
        return Err(AppError::BadRequest(format!(
            "prompt is longer than {MAX_PROMPT_BYTES} bytes"
        )));
    }

    // A continuation inherits its parent's model and system prompt unless the request overrides them.
    let parent = match req.parent_id {
        None => None,
        Some(id) => match state.store.get_run(id).await? {
            Some(run) if run.owner == owner.0 => Some(run),
            _ => return Err(AppError::NotFound("parent run not found")),
        },
    };
    let requested_model = req
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    let model = match requested_model.or(parent.as_ref().map(|p| p.model.as_str())) {
        None => cfg.default_model.clone(),
        Some(m) if m.len() > MAX_MODEL_LEN => {
            return Err(AppError::BadRequest("model name is too long".into()))
        }
        Some(m)
            if m == cfg.default_model
                || cfg.allowed_models.is_empty()
                || cfg.allowed_models.iter().any(|a| a == m) =>
        {
            m.to_string()
        }
        Some(m) => return Err(AppError::BadRequest(format!("model {m:?} is not allowed"))),
    };

    let system = match req.system.filter(|s| !s.trim().is_empty()) {
        None => parent
            .as_ref()
            .map_or_else(|| cfg.default_system_prompt.clone(), |p| p.system.clone()),
        Some(s) if s.len() > MAX_SYSTEM_BYTES => {
            return Err(AppError::BadRequest(format!(
                "system is longer than {MAX_SYSTEM_BYTES} bytes"
            )))
        }
        Some(s) => s,
    };

    let max_turns = match req.max_turns {
        None => cfg.max_turns_cap,
        Some(0) => return Err(AppError::BadRequest("max_turns must be at least 1".into())),
        Some(n) => n.min(cfg.max_turns_cap),
    };

    let new = NewRun {
        owner: owner.0,
        prompt: req.prompt,
        system,
        model,
        max_turns,
        parent_id: req.parent_id,
    };
    let run = state.store.insert_run(&new, cfg.max_active_runs_per_key).await?.map_err(|refused| match refused {
        InsertRefused::TooManyActive => AppError::TooManyRequests(format!(
            "at most {} runs may be queued or running per API key",
            cfg.max_active_runs_per_key
        )),
        InsertRefused::ParentNotFound => AppError::NotFound("parent run not found"),
        InsertRefused::ParentActive => AppError::Conflict("the parent run has not finished yet".into()),
        InsertRefused::ParentContinued(child) => AppError::Conflict(format!(
            "the parent run was already continued by run {child}; continue from the latest run instead"
        )),
    })?;

    state.manager.spawn(run.clone());
    Ok((StatusCode::ACCEPTED, Json(run)))
}

#[derive(Deserialize)]
struct ListParams {
    status: Option<String>,
    limit: Option<u32>,
    before: Option<String>,
}

#[derive(Serialize)]
struct RunList {
    runs: Vec<Run>,
    /// Pass as `before` to fetch the next (older) page; absent on the last page.
    next_before: Option<Uuid>,
}

async fn list_runs(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    ApiQuery(params): ApiQuery<ListParams>,
) -> Result<Json<RunList>, AppError> {
    let status = params
        .status
        .as_deref()
        .map(|s| {
            RunStatus::parse(s).ok_or_else(|| AppError::BadRequest(format!("unknown status {s:?}")))
        })
        .transpose()?;
    let before = params
        .before
        .as_deref()
        .map(|b| {
            Uuid::parse_str(b).map_err(|_| AppError::BadRequest("before must be a run id".into()))
        })
        .transpose()?;
    let limit = params.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);

    let runs = state
        .store
        .list_runs(&owner.0, status, before, limit)
        .await?;
    let next_before = (runs.len() as u32 == limit)
        .then(|| runs.last().map(|r| r.id))
        .flatten();
    Ok(Json(RunList { runs, next_before }))
}

async fn get_run(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    Path(id): Path<String>,
) -> Result<Json<Run>, AppError> {
    Ok(Json(owned_run(&state, &owner, &id).await?))
}

async fn get_messages(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let run = owned_run(&state, &owner, &id).await?;
    let messages: Vec<StoredMessage> = state.store.list_messages(run.id).await?;
    Ok(Json(json!({ "messages": messages })))
}

async fn cancel_run(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Run>), AppError> {
    let run = owned_run(&state, &owner, &id).await?;
    if run.status.is_terminal() {
        return Err(AppError::Conflict(format!(
            "run already {}",
            run.status.as_str()
        )));
    }
    if !state.manager.cancel(run.id) {
        return Err(AppError::Conflict("run is not active".into()));
    }
    // Cancellation is asynchronous; follow the event stream or poll the run for the final state.
    Ok((StatusCode::ACCEPTED, Json(run)))
}

async fn delete_run(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let run = owned_run(&state, &owner, &id).await?;
    if !state.store.delete_finished_run(run.id).await? {
        return Err(AppError::Conflict(
            "run is still active; cancel it first".into(),
        ));
    }
    // The workspace goes with the last run that uses it.
    if state
        .store
        .release_workspace_if_unused(run.workspace_id)
        .await?
    {
        state.manager.remove_workspace(run.workspace_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct EventsParams {
    after: Option<i64>,
}

async fn stream_events(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    Path(id): Path<String>,
    ApiQuery(params): ApiQuery<EventsParams>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>>, AppError> {
    let run = owned_run(&state, &owner, &id).await?;
    // EventSource reconnects send the last seen id in this header.
    let after = params
        .after
        .or_else(|| {
            headers
                .get("last-event-id")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0)
        .max(0);
    Ok(sse::event_stream(
        state.store.clone(),
        state.hub.clone(),
        run.id,
        after,
    ))
}

// -- health ---------------------------------------------------------------------------------

async fn healthz() -> &'static str {
    "ok"
}

/// Ready when the database answers and the sandbox runtime is usable. Details go to the log, not
/// to this unauthenticated endpoint.
async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    let database = state.store.ping().await;
    let sandbox = state.sandboxes.health().await;
    for (name, result) in [
        ("database", database.as_ref().err()),
        ("sandbox", sandbox.as_ref().err()),
    ] {
        if let Some(e) = result {
            tracing::warn!(check = name, error = %format!("{e:#}"), "readiness check failed");
        }
    }
    let status = |ok: bool| if ok { "ok" } else { "failed" };
    let ready = database.is_ok() && sandbox.is_ok();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": if ready { "ok" } else { "unavailable" },
            "checks": { "database": status(database.is_ok()), "sandbox": status(sandbox.is_ok()) },
        })),
    )
}
