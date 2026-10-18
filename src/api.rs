//! The HTTP API.

use crate::agent::AgentEnv;
use crate::auth::{self, Admin, AuthKeys, Owner};
use crate::browser::{BrowserFactory, DockerBrowserFactory};
use crate::compact;
use crate::config::Config;
use crate::error::AppError;
use crate::events::EventHub;
use crate::integrations::{Integrations, ModelSpec, ServerModelsQuery};
use crate::llm::LlmRouter;
use crate::manager::RunManager;
use crate::models::{
    Attachment, CreateRunRequest, ModelEntry, NewRun, Run, RunStatus, StoredMessage,
    ALLOWED_ATTACHMENT_TYPES,
};
use crate::sandbox::{check_workspace_path, SandboxFactory, WorkspaceListing, WorkspaceMissing};
use crate::sse;
use crate::store::{InsertRefused, Store};
use crate::webhooks::{self, Webhooks};
use anyhow::Result;
use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::sse::Sse;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::Engine;
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
pub(crate) const MAX_MODEL_LEN: usize = 100;
/// Attachments (images and PDFs sent with a prompt): how many, how big each and all together
/// (decoded), and the body limit that leaves room for their base64 on `POST /v1/runs` only.
pub const MAX_ATTACHMENTS: usize = 10;
pub const MAX_ATTACHMENT_BYTES: usize = 20 << 20;
pub const MAX_ATTACHMENTS_TOTAL_BYTES: usize = 32 << 20;
const MAX_RUN_BODY_BYTES: usize = 48 << 20;
const MAX_ATTACHMENT_NAME_CHARS: usize = 200;
const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 100;
/// Largest workspace file that can be downloaded. It is read into memory before being sent.
pub const MAX_DOWNLOAD_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub store: Store,
    pub hub: Arc<EventHub>,
    pub manager: RunManager,
    pub sandboxes: Arc<dyn SandboxFactory>,
    /// `None` when the browser is turned off.
    pub browsers: Option<Arc<dyn BrowserFactory>>,
    pub auth: AuthKeys,
    /// The registered models' clients, for model calls outside a run (compaction).
    pub llm: Arc<LlmRouter>,
    /// The current tools, and the credentials admins can set up for them.
    pub integrations: Arc<Integrations>,
    /// Delivers runs' `callback_url` notifications; `main` starts its loop.
    pub webhooks: Arc<Webhooks>,
}

impl AppState {
    /// Wire the services together. The LLM client and sandbox factory are injected so tests can
    /// substitute fakes; browsers run in Docker when the browser is on (see [`Self::with_browsers`]).
    /// Fails only if a tool can't be set up (e.g. the HTTP client for web search).
    pub fn new(
        cfg: Config,
        store: Store,
        llm: Arc<LlmRouter>,
        sandboxes: Arc<dyn SandboxFactory>,
    ) -> Result<Self> {
        let browsers = Arc::new(DockerBrowserFactory::new(cfg.browser.clone()));
        Self::with_browsers(cfg, store, llm, sandboxes, browsers)
    }

    /// Like [`Self::new`], with the browser factory injected too. It is only used if the
    /// configuration turns the browser on.
    pub fn with_browsers(
        cfg: Config,
        store: Store,
        llm: Arc<LlmRouter>,
        sandboxes: Arc<dyn SandboxFactory>,
        browsers: Arc<dyn BrowserFactory>,
    ) -> Result<Self> {
        let browsers = cfg.browser.enabled.then_some(browsers);
        let hub = Arc::new(EventHub::new());
        let cfg = Arc::new(cfg);
        let integrations = Arc::new(Integrations::new(cfg.clone(), store.clone())?);
        let env = AgentEnv {
            llm: llm.clone(),
            sandbox_network: cfg.sandbox.network.clone(),
            sandbox_info: json!({
                "network": cfg.sandbox.network,
                "memory": cfg.sandbox.memory,
                // A number when it is one ("1", "0.5"), as Docker takes it.
                "cpus": cfg.sandbox.cpus.parse::<f64>().map_or_else(|_| json!(cfg.sandbox.cpus), |n| json!(n)),
            }),
            approvals: cfg.tool_approvals.clone(),
            approval_timeout: cfg.approval_timeout,
            waits: Arc::default(),
            tools: integrations.tools(),
            store: store.clone(),
            tool_timeout: cfg.tool_timeout,
            tool_output_max_bytes: cfg.tool_output_max_bytes,
            integrations: integrations.clone(),
            pdf_preview_pages: cfg.pdf_preview_pages,
        };
        let manager = RunManager::new(
            store.clone(),
            hub.clone(),
            env,
            sandboxes.clone(),
            browsers.clone(),
            cfg.max_concurrent_runs,
            cfg.run_timeout,
        );
        let auth = AuthKeys::with_admins(&cfg.api_keys, &cfg.admin_keys);
        let webhooks = Arc::new(Webhooks::new(
            store.clone(),
            hub.clone(),
            cfg.webhook_allow_private,
            cfg.webhook_retry,
        )?);
        Ok(Self {
            cfg,
            store,
            hub,
            manager,
            sandboxes,
            browsers,
            auth,
            llm,
            integrations,
            webhooks,
        })
    }
}

pub fn router(state: AppState) -> Router {
    let v1 = Router::new()
        // Only this route takes attachments, so only it gets the room for them.
        .route(
            "/runs",
            post(create_run)
                .get(list_runs)
                .layer(DefaultBodyLimit::max(MAX_RUN_BODY_BYTES)),
        )
        .route("/runs/{id}", get(get_run).delete(delete_run))
        .route("/runs/{id}/messages", get(get_messages))
        .route("/runs/{id}/events", get(stream_events))
        .route("/runs/{id}/cancel", post(cancel_run))
        .route("/runs/{id}/plan", post(answer_plan))
        .route("/runs/{id}/approvals/{approval_id}", post(decide_approval))
        .route("/runs/{id}/files", get(list_files))
        .route("/runs/{id}/files/{*path}", get(download_file))
        .route("/events", get(stream_notifications))
        .route("/compact", post(compact_conversation))
        .route("/tools", get(list_tools))
        .route("/models", get(list_models))
        .route("/defaults", get(defaults))
        .route("/admin/integrations", get(get_integrations))
        .route("/admin/models", get(get_models))
        .route("/admin/server-models", post(list_server_models))
        .route(
            "/admin/models/{id}",
            axum::routing::put(put_model).delete(delete_model),
        )
        .route(
            "/admin/integrations/brave",
            axum::routing::put(put_brave_key).delete(delete_brave_key),
        )
        .route(
            "/admin/integrations/google",
            axum::routing::delete(delete_google),
        )
        .route(
            "/admin/integrations/google/authorize",
            post(authorize_google),
        )
        .route("/admin/integrations/google/callback", post(google_callback))
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

/// The model a request gets and the reasoning level it sends.
struct ResolvedModel {
    id: String,
    name: String,
    reasoning: Option<String>,
    /// Whether the entry takes images (and is shown attached PDFs' first pages).
    multimodal: bool,
    /// The most tokens one reply of the entry may take.
    max_tokens: u32,
}

/// The model a request gets: the registered entry it named (by id, or by model name when only
/// one entry has it), else the parent's, else the default entry. The reasoning level is the one
/// asked for, else the parent's when it used the same entry, else the entry's default; it must be
/// one the entry offers.
fn resolve_model(
    models: &[ModelEntry],
    requested: Option<&str>,
    requested_reasoning: Option<&str>,
    parent: Option<&Run>,
) -> Result<ResolvedModel, AppError> {
    if models.is_empty() {
        return Err(AppError::Conflict(
            "no model is registered; an admin adds one under Settings or with PUT /v1/admin/models/{id}"
                .into(),
        ));
    }
    let requested = requested.map(str::trim).filter(|m| !m.is_empty());
    let entry = match requested {
        Some(m) if m.len() > MAX_MODEL_LEN => {
            return Err(AppError::BadRequest("model name is too long".into()))
        }
        Some(m) => {
            let by_id = models.iter().find(|e| e.id == m);
            let by_name = || {
                let mut named = models.iter().filter(|e| e.model == m);
                match (named.next(), named.next()) {
                    (Some(one), None) => Some(one),
                    _ => None,
                }
            };
            by_id
                .or_else(by_name)
                .ok_or_else(|| AppError::BadRequest(format!("model {m:?} is not registered")))?
        }
        None => match parent {
            Some(p) => models.iter().find(|e| e.id == p.model_id).ok_or_else(|| {
                AppError::BadRequest(format!(
                    "the parent run's model {:?} is no longer registered; name another",
                    p.model_id
                ))
            })?,
            None => models
                .iter()
                .find(|e| e.is_default)
                .or_else(|| models.first())
                .expect("checked non-empty"),
        },
    };
    let inherited = parent
        .filter(|p| p.model_id == entry.id)
        .and_then(|p| p.reasoning.as_deref());
    let reasoning = match requested_reasoning.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => Some(r),
        None => inherited.or(entry.default_reasoning.as_deref()),
    };
    if let Some(r) = reasoning {
        if !entry.reasoning_levels.iter().any(|l| l == r) {
            return Err(AppError::BadRequest(if entry.reasoning_levels.is_empty() {
                format!("model {:?} offers no reasoning levels", entry.id)
            } else {
                format!(
                    "reasoning {r:?} is not offered by model {:?} (one of {})",
                    entry.id,
                    entry.reasoning_levels.join(", ")
                )
            }));
        }
    }
    Ok(ResolvedModel {
        id: entry.id.clone(),
        name: entry.model.clone(),
        reasoning: reasoning.map(String::from),
        multimodal: entry.multimodal,
        max_tokens: entry.max_tokens,
    })
}

/// Check the files sent with a prompt: count, name, media type, that `data` is real base64, and
/// the decoded sizes. The base64 is kept as sent; decoding here only validates it.
fn validate_attachments(attachments: &[Attachment]) -> Result<(), AppError> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(AppError::BadRequest(format!(
            "at most {MAX_ATTACHMENTS} attachments per run"
        )));
    }
    let mut total = 0usize;
    for (i, a) in attachments.iter().enumerate() {
        let which = |what: &str| {
            AppError::BadRequest(format!("attachment {} ({:?}): {what}", i + 1, a.name))
        };
        let name = a.name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest(format!(
                "attachment {}: name must not be empty",
                i + 1
            )));
        }
        if name.chars().count() > MAX_ATTACHMENT_NAME_CHARS || name.chars().any(char::is_control) {
            return Err(which("name is malformed"));
        }
        if !ALLOWED_ATTACHMENT_TYPES.contains(&a.media_type.as_str()) {
            return Err(which(&format!(
                "media type {:?} is not supported (use one of {})",
                a.media_type,
                ALLOWED_ATTACHMENT_TYPES.join(", ")
            )));
        }
        if a.data.is_empty() {
            return Err(which("data is empty"));
        }
        let too_big = which(&format!("larger than {} MiB", MAX_ATTACHMENT_BYTES >> 20));
        if base64::decoded_len_estimate(a.data.len()) > MAX_ATTACHMENT_BYTES {
            return Err(too_big);
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&a.data)
            .map_err(|_| which("data is not valid base64"))?;
        if decoded.len() > MAX_ATTACHMENT_BYTES {
            return Err(too_big);
        }
        total += decoded.len();
        if total > MAX_ATTACHMENTS_TOTAL_BYTES {
            return Err(AppError::BadRequest(format!(
                "attachments add up to more than {} MiB",
                MAX_ATTACHMENTS_TOTAL_BYTES >> 20
            )));
        }
    }
    Ok(())
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
    let attachments = req.attachments.unwrap_or_default();
    validate_attachments(&attachments)?;

    // A continuation inherits its parent's model and system prompt unless the request overrides them.
    let parent = match req.parent_id {
        None => None,
        Some(id) => match state.store.get_run(id).await? {
            Some(run) if run.owner == owner.0 => Some(run),
            _ => return Err(AppError::NotFound("parent run not found")),
        },
    };
    let model = resolve_model(
        &state.integrations.models(),
        req.model.as_deref(),
        req.reasoning.as_deref(),
        parent.as_ref(),
    )?;
    // PDFs are fine for any model: they are saved in the workspace for the agent to read.
    if attachments.iter().any(Attachment::is_image) && !model.multimodal {
        return Err(AppError::BadRequest(format!(
            "model {:?} does not accept images (an admin registers it as accepting them under \
             Settings, or with \"multimodal\": true on PUT /v1/admin/models/{{id}}); PDFs can be \
             attached for any model",
            model.id
        )));
    }

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

    let tools = match req.tools {
        None => parent.as_ref().and_then(|p| p.tools.clone()),
        Some(mut tools) => {
            let current = state.integrations.tools().load_full();
            current
                .validate_selection(&tools)
                .map_err(AppError::BadRequest)?;
            tools.sort();
            tools.dedup();
            Some(tools)
        }
    };

    let max_turns = match req.max_turns {
        None => cfg.max_turns_cap,
        Some(0) => return Err(AppError::BadRequest("max_turns must be at least 1".into())),
        Some(n) => n.min(cfg.max_turns_cap),
    };

    // A continuation without a callback of its own keeps its parent's (the store copies it).
    let callback_url = req
        .callback_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty());
    let callback_secret = match req.callback_secret.filter(|s| !s.is_empty()) {
        None => None,
        Some(_) if callback_url.is_none() => {
            return Err(AppError::BadRequest(
                "callback_secret needs a callback_url to sign deliveries to".into(),
            ))
        }
        Some(s) if s.len() > webhooks::MAX_SECRET_LEN => {
            return Err(AppError::BadRequest(format!(
                "callback_secret is longer than {} bytes",
                webhooks::MAX_SECRET_LEN
            )))
        }
        Some(s) => Some(s),
    };
    let callback_url = match callback_url {
        None => None,
        Some(raw) => Some(
            webhooks::check_callback_url(raw, cfg.webhook_allow_private)
                .await
                .map_err(AppError::BadRequest)?,
        ),
    };

    let new = NewRun {
        owner: owner.0,
        prompt: req.prompt,
        system,
        model_id: model.id,
        model: model.name,
        reasoning: model.reasoning,
        max_turns,
        parent_id: req.parent_id,
        tools,
        attachments,
        callback_url,
        callback_secret,
        plan: req.plan.unwrap_or(cfg.plan),
        interactive: req.interactive.unwrap_or(false),
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

// -- tools and integrations -----------------------------------------------------------------

/// The tool groups a run can switch on or off, with what this server offers in each.
async fn list_tools(State(state): State<AppState>) -> Json<serde_json::Value> {
    let tools = state.integrations.tools().load_full();
    Json(tools.catalog(&state.integrations.unavailable_reasons()))
}

/// The registered models a run can name, in picker order.
async fn list_models(State(state): State<AppState>) -> Json<serde_json::Value> {
    let models: Vec<serde_json::Value> = state
        .integrations
        .models()
        .iter()
        .map(ModelEntry::public)
        .collect();
    Json(json!({ "models": models }))
}

/// What a run gets when it doesn't say: for clients that show the defaults.
async fn defaults(State(state): State<AppState>) -> Json<serde_json::Value> {
    let cfg = &state.cfg;
    let default = state.integrations.default_entry();
    Json(json!({
        "model": default.as_ref().map(|m| m.model.clone()),
        "model_id": default.as_ref().map(|m| m.id.clone()),
        "system": cfg.default_system_prompt,
        "max_turns_cap": cfg.max_turns_cap,
        "plan": cfg.plan,
        "tool_approvals": !cfg.tool_approvals.is_empty(),
        "attachments": {
            "max_count": MAX_ATTACHMENTS,
            "max_bytes": MAX_ATTACHMENT_BYTES,
            "max_total_bytes": MAX_ATTACHMENTS_TOTAL_BYTES,
            "media_types": ALLOWED_ATTACHMENT_TYPES,
        },
    }))
}

fn require_admin(admin: Admin) -> Result<(), AppError> {
    if admin.0 {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "this API key may not manage integrations (see CEDAR_ADMIN_KEYS)",
        ))
    }
}

async fn get_integrations(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(admin)?;
    Ok(Json(state.integrations.status().await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BraveKeyBody {
    api_key: String,
}

async fn put_brave_key(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
    ApiJson(body): ApiJson<BraveKeyBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(admin)?;
    state.integrations.set_brave_key(&body.api_key).await?;
    Ok(Json(state.integrations.status().await?))
}

async fn delete_brave_key(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
) -> Result<StatusCode, AppError> {
    require_admin(admin)?;
    state.integrations.remove_brave_key().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_models(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(admin)?;
    Ok(Json(state.integrations.models_status().await?))
}

async fn list_server_models(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
    ApiJson(query): ApiJson<ServerModelsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(admin)?;
    Ok(Json(state.integrations.server_models(query).await?))
}

async fn put_model(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
    Path(id): Path<String>,
    ApiJson(spec): ApiJson<ModelSpec>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(admin)?;
    Ok(Json(state.integrations.set_model(&id, spec).await?))
}

async fn delete_model(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    require_admin(admin)?;
    state.integrations.remove_model(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizeBody {
    redirect_uri: String,
}

async fn authorize_google(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
    ApiJson(body): ApiJson<AuthorizeBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(admin)?;
    let url = state.integrations.google_authorize(&body.redirect_uri)?;
    Ok(Json(json!({ "url": url })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallbackBody {
    state: String,
    code: String,
}

async fn google_callback(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
    ApiJson(body): ApiJson<CallbackBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(admin)?;
    Ok(Json(
        state
            .integrations
            .google_callback(&body.state, &body.code)
            .await?,
    ))
}

async fn delete_google(
    State(state): State<AppState>,
    Extension(admin): Extension<Admin>,
) -> Result<StatusCode, AppError> {
    require_admin(admin)?;
    state.integrations.remove_google().await?;
    Ok(StatusCode::NO_CONTENT)
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

/// Agree to the plan an interactive run is waiting on: the steps it proposed, or edited ones.
async fn answer_plan(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let run = owned_run(&state, &owner, &id).await?;
    let steps = crate::plan::parse_answer(&body).map_err(AppError::BadRequest)?;
    let answer = json!({ "steps": steps });
    if !state.manager.answer(run.id, "plan", answer.clone()) {
        return Err(AppError::Conflict("the run is not waiting for its plan to be agreed".into()));
    }
    Ok((StatusCode::ACCEPTED, Json(answer)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalDecision {
    decision: String,
}

/// Answer an `approval.required` event: `allow`, `allow_always` (and remember its scope for
/// this API key), or `deny`.
async fn decide_approval(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    Path((id, approval_id)): Path<(String, String)>,
    ApiJson(body): ApiJson<ApprovalDecision>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let run = owned_run(&state, &owner, &id).await?;
    if !matches!(body.decision.as_str(), "allow" | "allow_always" | "deny") {
        return Err(AppError::BadRequest(
            "decision must be allow, allow_always or deny".into(),
        ));
    }
    let answer = json!({ "decision": body.decision });
    if !state
        .manager
        .answer(run.id, &format!("approval:{approval_id}"), answer.clone())
    {
        return Err(AppError::Conflict("the run is not waiting for that approval".into()));
    }
    Ok((StatusCode::ACCEPTED, Json(answer)))
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

/// `GET /v1/events`: the key's finished runs as Server-Sent Events, from after `after` (or the
/// `Last-Event-ID` header) onwards, live. See [`sse::notification_stream`].
async fn stream_notifications(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    ApiQuery(params): ApiQuery<EventsParams>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>> {
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
    sse::notification_stream(state.store.clone(), state.hub.clone(), owner.0, after)
}

// -- compaction -----------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactRequest {
    /// A finished run of the caller's: its stored conversation is compacted.
    run_id: Option<Uuid>,
    /// A plain-text transcript to compact instead (for conversations cedar no longer has).
    transcript: Option<String>,
    model: Option<String>,
}

#[derive(Serialize)]
struct CompactResponse {
    prompt: String,
    model: String,
    model_id: String,
    usage: crate::models::Usage,
}

/// One tool-free model call that turns a conversation into a single self-contained prompt.
async fn compact_conversation(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    ApiJson(req): ApiJson<CompactRequest>,
) -> Result<Json<CompactResponse>, AppError> {
    let (messages, run) = match (req.run_id, req.transcript) {
        (Some(id), None) => {
            let run = owned_run(&state, &owner, &id.to_string()).await?;
            if !run.status.is_terminal() {
                return Err(AppError::Conflict("the run has not finished yet".into()));
            }
            let stored = state.store.list_messages(run.id).await?;
            if stored.is_empty() {
                return Err(AppError::Conflict("the run has no conversation yet".into()));
            }
            (compact::messages_for_run(stored), Some(run))
        }
        (None, Some(transcript)) => {
            if transcript.trim().is_empty() {
                return Err(AppError::BadRequest("transcript must not be empty".into()));
            }
            if transcript.len() > MAX_PROMPT_BYTES {
                return Err(AppError::BadRequest(format!(
                    "transcript is longer than {MAX_PROMPT_BYTES} bytes"
                )));
            }
            (compact::messages_for_transcript(&transcript), None)
        }
        _ => {
            return Err(AppError::BadRequest(
                "give exactly one of run_id and transcript".into(),
            ))
        }
    };
    // Compaction takes no reasoning level: one plain call is enough.
    let model = resolve_model(
        &state.integrations.models(),
        req.model.as_deref(),
        Some(""),
        run.as_ref(),
    )
    .map(|m| ResolvedModel {
        reasoning: None,
        ..m
    })?;
    let llm = state.llm.client_for(&model.id).ok_or_else(|| {
        AppError::Conflict(format!("model {:?} is no longer registered", model.id))
    })?;
    let done = compact::compact(llm.as_ref(), &model.name, model.max_tokens, &messages)
        .await
        .map_err(|e| AppError::BadGateway(format!("could not compact the conversation: {e:#}")))?;
    Ok(Json(CompactResponse {
        prompt: done.prompt,
        model: model.name,
        model_id: model.id,
        usage: done.usage,
    }))
}

// -- workspace files ------------------------------------------------------------------------

#[derive(Serialize)]
struct FileList {
    workspace_id: Uuid,
    #[serde(flatten)]
    listing: WorkspaceListing,
}

/// The caller's run, if its workspace files still exist. Expired workspaces look like missing ones.
async fn owned_live_workspace(
    state: &AppState,
    owner: &Owner,
    raw_id: &str,
) -> Result<Uuid, AppError> {
    let run = owned_run(state, owner, raw_id).await?;
    if !state.store.workspace_is_live(run.workspace_id).await? {
        return Err(AppError::NotFound("the run's workspace no longer exists"));
    }
    Ok(run.workspace_id)
}

/// A missing volume is a 404 (it can expire between the database check and the docker call);
/// anything else is an internal error, logged with its detail.
fn workspace_error(e: anyhow::Error) -> AppError {
    if e.downcast_ref::<WorkspaceMissing>().is_some() {
        AppError::NotFound("the run's workspace no longer exists")
    } else {
        AppError::Internal(e)
    }
}

async fn list_files(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    Path(id): Path<String>,
) -> Result<Json<FileList>, AppError> {
    let workspace_id = owned_live_workspace(&state, &owner, &id).await?;
    let listing = state
        .sandboxes
        .list_workspace_files(workspace_id)
        .await
        .map_err(workspace_error)?;
    Ok(Json(FileList {
        workspace_id,
        listing,
    }))
}

async fn download_file(
    State(state): State<AppState>,
    Extension(owner): Extension<Owner>,
    Path((id, path)): Path<(String, String)>,
) -> Result<Response, AppError> {
    check_workspace_path(&path).map_err(|e| AppError::BadRequest(e.to_string()))?;
    let workspace_id = owned_live_workspace(&state, &owner, &id).await?;
    let file = state
        .sandboxes
        .read_workspace_file(workspace_id, &path, MAX_DOWNLOAD_BYTES)
        .await
        .map_err(workspace_error)?
        .ok_or(AppError::NotFound("no such file in the workspace"))?;
    if file.truncated {
        return Err(AppError::BadRequest(format!(
            "{path} is larger than the {MAX_DOWNLOAD_BYTES} byte download limit"
        )));
    }
    let name = path.rsplit('/').next().unwrap_or(&path);
    let mime = mime_guess::from_path(name)
        .first_raw()
        .unwrap_or("application/octet-stream");
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(mime)),
            (header::CONTENT_DISPOSITION, attachment(name)),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        file.bytes,
    )
        .into_response())
}

/// `attachment; filename="<ascii>"; filename*=UTF-8''<percent-encoded>` for a file name.
fn attachment(name: &str) -> HeaderValue {
    let ascii: String = name
        .chars()
        .map(|c| match c {
            '"' | '\\' => '_',
            c if c.is_ascii_graphic() || c == ' ' => c,
            _ => '_',
        })
        .collect();
    let mut encoded = String::new();
    for b in name.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char)
            }
            _ => encoded.push_str(&format!("%{b:02X}")),
        }
    }
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"
    ))
    .expect("ascii header value")
}

// -- health ---------------------------------------------------------------------------------

async fn healthz() -> &'static str {
    "ok"
}

/// Ready when the database answers and the sandbox runtime (and, if it is on, the browser image)
/// is usable. Details go to the log, not to this unauthenticated endpoint.
async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    let mut checks = vec![
        ("database", state.store.ping().await),
        ("sandbox", state.sandboxes.health().await),
    ];
    if let Some(browsers) = &state.browsers {
        checks.push(("browser", browsers.health().await));
    }
    for (name, result) in &checks {
        if let Err(e) = result {
            tracing::warn!(check = name, error = %format!("{e:#}"), "readiness check failed");
        }
    }
    let ready = checks.iter().all(|(_, r)| r.is_ok());
    let summary: serde_json::Map<String, serde_json::Value> = checks
        .iter()
        .map(|(name, r)| {
            (
                name.to_string(),
                json!(if r.is_ok() { "ok" } else { "failed" }),
            )
        })
        .collect();
    // Informational: a server with no model registered is up, but runs are refused until an
    // admin adds one.
    let models = state.integrations.models().len();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": if ready { "ok" } else { "unavailable" },
            "checks": summary,
            "models": models,
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, media_type: &str, data: &str) -> Attachment {
        Attachment {
            name: name.into(),
            media_type: media_type.into(),
            data: data.into(),
        }
    }

    #[test]
    fn attachments_are_validated() {
        let ok = att("a.png", "image/png", "iVBORw0KGgo=");
        assert!(validate_attachments(&[]).is_ok());
        assert!(
            validate_attachments(&[ok.clone(), att("b.pdf", "application/pdf", "JVBERi0=")])
                .is_ok()
        );

        let message = |atts: &[Attachment]| match validate_attachments(atts) {
            Err(AppError::BadRequest(m)) => m,
            other => panic!("expected a bad request, got {other:?}"),
        };
        assert!(message(&vec![ok.clone(); MAX_ATTACHMENTS + 1]).contains("at most"));
        assert!(message(&[att(" ", "image/png", "aGk=")]).contains("name must not be empty"));
        assert!(message(&[att("a\nb", "image/png", "aGk=")]).contains("name is malformed"));
        assert!(message(&[att("a.txt", "text/plain", "aGk=")]).contains("not supported"));
        assert!(message(&[att("a.png", "image/png", "")]).contains("data is empty"));
        assert!(message(&[att("a.png", "image/png", "not base64!")]).contains("not valid base64"));
        let huge = "A".repeat(base64::encoded_len(MAX_ATTACHMENT_BYTES + 3, true).unwrap());
        assert!(message(&[att("a.png", "image/png", &huge)]).contains("larger than"));
    }
}
