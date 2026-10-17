//! The HTTP API and the embedded web UI.

use crate::cedar::{self, Attachment, CedarClient, RunOptions};
use crate::config::Config;
use crate::error::AppError;
use crate::prompt::{self, Exchange};
use crate::schedule::{self, Timing};
use crate::store::{AttachmentInfo, Conversation, NewSchedule, Preferences, Schedule, ScheduleRun, Store, Turn};
use chrono::Utc;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Query, Request, State};
use axum::http::header::{self, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 128 * 1024;
const MAX_MESSAGE_BYTES: usize = 32_000;
const MAX_TITLE_CHARS: usize = 200;
const MAX_CONVERSATIONS: u32 = 500;
const MAX_SCHEDULES: i64 = 100;
const MAX_SCHEDULE_RUNS: u32 = 100;
const MAX_TOOLS: usize = 100;
const MAX_SYSTEM_BYTES: usize = 20_000;
const MAX_TURNS: u32 = 1000;
/// Attachments, mirroring cedar's limits: how many per message, how big each and all together
/// (decoded), and the body limit that leaves room for their base64 on the messages route only.
const MAX_ATTACHMENTS: usize = 10;
const MAX_ATTACHMENT_BYTES: usize = 20 << 20;
const MAX_ATTACHMENTS_TOTAL_BYTES: usize = 32 << 20;
const MAX_MESSAGE_BODY_BYTES: usize = 48 << 20;
const MAX_ATTACHMENT_NAME_CHARS: usize = 200;
const ATTACHMENT_TYPES: [&str; 5] = ["image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf"];
/// Where Google sends the browser back after "Connect Google" (under `CHAT_PUBLIC_URL`).
pub const GOOGLE_CALLBACK_PATH: &str = "/api/oauth/google/callback";
const ADMIN_HEADER: &str = "x-admin-token";

/// `/` only redirects to the UI the browser has chosen (see `ui-pref.js`).
const LOADER_HTML: &str = include_str!("../static/index.html");
const UI_PREF_JS: &str = include_str!("../static/ui-pref.js");
/// Shows the images the agent produces in the response; both UIs load it, before the viewer.
const IMAGE_VIEW_JS: &str = include_str!("../static/image-view.js");
const IMAGE_VIEW_CSS: &str = include_str!("../static/image-view.css");
/// Previews Markdown files, PDFs and images; both UIs load it.
const FILE_VIEWER_JS: &str = include_str!("../static/file-viewer.js");
/// Draws the `show_map` tool's result as a map; both UIs load it, and it loads Leaflet on demand.
const MAP_VIEW_JS: &str = include_str!("../static/map-view.js");
const MAP_VIEW_CSS: &str = include_str!("../static/map-view.css");
/// Draws the `show_recipe` tool's result as a recipe card with the steps as a timeline; both UIs load it.
const RECIPE_VIEW_JS: &str = include_str!("../static/recipe-view.js");
const RECIPE_VIEW_CSS: &str = include_str!("../static/recipe-view.css");
/// The Tools, Model and Integrations sections of Settings and the composer's tools menu; both UIs load it.
const MODELS_JS: &str = include_str!("../static/models.js");
const MODELS_CSS: &str = include_str!("../static/models.css");
const SETTINGS_JS: &str = include_str!("../static/settings.js");
const SETTINGS_CSS: &str = include_str!("../static/settings.css");
/// Attaching images and PDFs to a message (button, drag-and-drop, paste); both UIs load it.
const ATTACH_JS: &str = include_str!("../static/attach.js");
const ATTACH_CSS: &str = include_str!("../static/attach.css");
/// Scheduling a conversation as a recurring agent, and the Settings list of schedules; both UIs load it.
const SCHEDULE_JS: &str = include_str!("../static/schedule.js");
const SCHEDULE_CSS: &str = include_str!("../static/schedule.css");
/// Leaflet 1.9.4 (BSD-2-Clause, see `static/vendor/leaflet/LICENSE`), served from here so no
/// third-party code runs in the page. Only the map tiles come from another server.
const LEAFLET_JS: &str = include_str!("../static/vendor/leaflet/leaflet.js");
const LEAFLET_CSS: &str = include_str!("../static/vendor/leaflet/leaflet.css");
/// The logo and icons, shared with the README and the landing page in `docs/assets`. The mark has a
/// light and a dark variant; the pages pick one with `prefers-color-scheme`.
const MARK_SVG: &str = include_str!("../../docs/assets/cedar-mark.svg");
const MARK_ON_DARK_SVG: &str = include_str!("../../docs/assets/cedar-mark-on-dark.svg");
const FAVICON_SVG: &str = include_str!("../../docs/assets/favicon.svg");
const FAVICON_32_PNG: &[u8] = include_bytes!("../../docs/assets/favicon-32.png");
const FAVICON_16_PNG: &[u8] = include_bytes!("../../docs/assets/favicon-16.png");
const APPLE_TOUCH_ICON_PNG: &[u8] = include_bytes!("../../docs/assets/apple-touch-icon.png");

/// One of the two front ends, served under its own prefix.
struct Ui {
    prefix: &'static str,
    index: &'static str,
    js: &'static str,
    css: &'static str,
}

/// "Claude style".
const V1: Ui = Ui {
    prefix: "/v1",
    index: include_str!("../static/v1/index.html"),
    js: include_str!("../static/v1/app.js"),
    css: include_str!("../static/v1/style.css"),
};

/// "Cedar style".
const V2: Ui = Ui {
    prefix: "/v2",
    index: include_str!("../static/v2/index.html"),
    js: include_str!("../static/v2/app.js"),
    css: include_str!("../static/v2/style.css"),
};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub store: Store,
    pub cedar: CedarClient,
    /// SHA-256 of the access token, if one is required.
    token_digest: Option<[u8; 32]>,
    /// SHA-256 of the admin token; `None` hides the Integrations settings.
    admin_digest: Option<[u8; 32]>,
    /// Serializes sending messages, so two tabs can't start overlapping runs in one conversation.
    send_lock: Arc<Mutex<()>>,
}

impl AppState {
    pub fn new(cfg: Config, store: Store) -> anyhow::Result<Self> {
        let cedar = CedarClient::new(&cfg.cedar_url, &cfg.cedar_api_key)?;
        let token_digest = cfg.access_token.as_deref().map(|t| Sha256::digest(t.as_bytes()).into());
        let admin_digest = cfg.admin_token.as_deref().map(|t| Sha256::digest(t.as_bytes()).into());
        Ok(Self { cfg: Arc::new(cfg), store, cedar, token_digest, admin_digest, send_lock: Arc::default() })
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/conversations", get(list_conversations).post(create_conversation))
        .route("/conversations/{id}", get(get_conversation).patch(rename_conversation).delete(delete_conversation))
        // Only this route takes attachments, so only it gets the room for them.
        .route("/conversations/{id}/messages", post(send_message).layer(DefaultBodyLimit::max(MAX_MESSAGE_BODY_BYTES)))
        .route("/conversations/{id}/tools", put(set_conversation_tools))
        .route("/conversations/{id}/compact", post(compact_conversation))
        .route("/conversations/{id}/files", get(list_files))
        .route("/conversations/{id}/files/{*path}", get(download_file))
        .route("/turns/{id}/events", get(turn_events))
        .route("/turns/{id}/cancel", post(cancel_turn))
        .route("/schedules", get(list_schedules).post(create_schedule))
        .route("/schedules/{id}", get(get_schedule).patch(update_schedule).delete(delete_schedule))
        .route("/schedules/{id}/run", post(run_schedule_now))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/tools", get(list_tools))
        .route("/models", get(list_models))
        .route("/integrations", get(get_integrations))
        .route("/integrations/brave", put(put_brave_key).delete(delete_brave_key))
        .route("/integrations/models", get(get_models))
        .route("/integrations/models/{id}", put(put_model).delete(delete_model))
        .route("/integrations/google", axum::routing::delete(delete_google))
        .route("/integrations/google/connect", post(connect_google))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        // Public: the page needs it to know whether to ask for a token.
        .route("/config", get(client_config))
        // Public too: Google redirects the browser here, which can't add the token. The single-use
        // `state` cedar issued for an admin's "Connect Google" is what authorizes it.
        .route("/oauth/google/callback", get(google_callback));

    let pages = Router::new()
        .route("/", get(|| async { asset(HTML, LOADER_HTML) }))
        .route("/ui-pref.js", get(|| async { asset(JS, UI_PREF_JS) }))
        .route("/image-view.js", get(|| async { asset(JS, IMAGE_VIEW_JS) }))
        .route("/image-view.css", get(|| async { asset(CSS, IMAGE_VIEW_CSS) }))
        .route("/file-viewer.js", get(|| async { asset(JS, FILE_VIEWER_JS) }))
        .route("/map-view.js", get(|| async { asset(JS, MAP_VIEW_JS) }))
        .route("/map-view.css", get(|| async { asset(CSS, MAP_VIEW_CSS) }))
        .route("/recipe-view.js", get(|| async { asset(JS, RECIPE_VIEW_JS) }))
        .route("/recipe-view.css", get(|| async { asset(CSS, RECIPE_VIEW_CSS) }))
        .route("/models.js", get(|| async { asset(JS, MODELS_JS) }))
        .route("/models.css", get(|| async { asset(CSS, MODELS_CSS) }))
        .route("/settings.js", get(|| async { asset(JS, SETTINGS_JS) }))
        .route("/settings.css", get(|| async { asset(CSS, SETTINGS_CSS) }))
        .route("/attach.js", get(|| async { asset(JS, ATTACH_JS) }))
        .route("/attach.css", get(|| async { asset(CSS, ATTACH_CSS) }))
        .route("/schedule.js", get(|| async { asset(JS, SCHEDULE_JS) }))
        .route("/schedule.css", get(|| async { asset(CSS, SCHEDULE_CSS) }))
        .route("/vendor/leaflet/leaflet.js", get(|| async { asset(JS, LEAFLET_JS) }))
        .route("/vendor/leaflet/leaflet.css", get(|| async { asset(CSS, LEAFLET_CSS) }))
        .route("/assets/cedar-mark.svg", get(|| async { asset(SVG, MARK_SVG) }))
        .route("/assets/cedar-mark-on-dark.svg", get(|| async { asset(SVG, MARK_ON_DARK_SVG) }))
        .route("/assets/favicon.svg", get(|| async { asset(SVG, FAVICON_SVG) }))
        .route("/assets/favicon-32.png", get(|| async { asset(PNG, FAVICON_32_PNG) }))
        .route("/assets/favicon-16.png", get(|| async { asset(PNG, FAVICON_16_PNG) }))
        .route("/assets/apple-touch-icon.png", get(|| async { asset(PNG, APPLE_TOUCH_ICON_PNG) }))
        .route("/healthz", get(|| async { "ok" }));
    with_ui(with_ui(pages, V1), V2)
        .nest("/api", api)
        .fallback(|| async { AppError::NotFound("no such route") })
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

const HTML: &str = "text/html; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const SVG: &str = "image/svg+xml";
const PNG: &str = "image/png";

/// Serve a front end's page at `/vN` and `/vN/`, and its script and stylesheet next to it.
fn with_ui(router: Router<AppState>, ui: Ui) -> Router<AppState> {
    let Ui { prefix, index, js, css } = ui;
    router
        .route(prefix, get(move || async move { asset(HTML, index) }))
        .route(&format!("{prefix}/"), get(move || async move { asset(HTML, index) }))
        .route(&format!("{prefix}/app.js"), get(move || async move { asset(JS, js) }))
        .route(&format!("{prefix}/style.css"), get(move || async move { asset(CSS, css) }))
}

fn asset(content_type: &'static str, body: impl IntoResponse) -> Response {
    ([(CONTENT_TYPE, content_type), (CACHE_CONTROL, "no-cache")], body).into_response()
}

async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Result<Response, AppError> {
    if let Some(expected) = &state.token_digest {
        let presented = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| Sha256::digest(token.trim().as_bytes()));
        match presented {
            Some(digest) if bool::from(digest.as_slice().ct_eq(expected)) => {}
            _ => return Err(AppError::Unauthorized),
        }
    }
    Ok(next.run(req).await)
}

#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
struct ApiJson<T>(T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(AppError))]
struct ApiQuery<T>(T);

fn parse_id(raw: &str, what: &'static str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw).map_err(|_| AppError::NotFound(what))
}

async fn find_conversation(state: &AppState, raw_id: &str) -> Result<Conversation, AppError> {
    let id = parse_id(raw_id, "conversation not found")?;
    state.store.get_conversation(id).await?.ok_or(AppError::NotFound("conversation not found"))
}

async fn find_turn(state: &AppState, raw_id: &str) -> Result<Turn, AppError> {
    let id = parse_id(raw_id, "turn not found")?;
    state.store.get_turn(id).await?.ok_or(AppError::NotFound("turn not found"))
}

/// Bring a turn's status up to date with its cedar run, until the run is over.
async fn refresh(state: &AppState, turn: Turn) -> Result<Turn, AppError> {
    if cedar::is_terminal(&turn.status) {
        return Ok(turn);
    }
    let (status, final_text, error) = match state.cedar.get_run(turn.run_id).await? {
        Some(run) => (run.status, run.final_text, run.error),
        None => ("failed".to_string(), None, Some("the run no longer exists on the cedar server".to_string())),
    };
    if status != turn.status {
        state.store.update_turn(turn.id, &status, final_text.as_deref(), error.as_deref()).await?;
    }
    Ok(Turn { status, final_text, error, ..turn })
}

/// Refresh every unfinished turn. If cedar can't be reached, keep the last known state.
async fn refresh_all(state: &AppState, turns: Vec<Turn>) -> Vec<Turn> {
    let mut out = Vec::with_capacity(turns.len());
    for turn in turns {
        out.push(match refresh(state, turn.clone()).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(turn_id = %turn.id, error = %e, "could not refresh turn");
                turn
            }
        });
    }
    out
}

// -- config ---------------------------------------------------------------------------------

async fn client_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "auth_required": state.token_digest.is_some(),
        "admin_enabled": state.admin_digest.is_some(),
    }))
}

// -- conversations --------------------------------------------------------------------------

#[derive(Serialize)]
struct ConversationList {
    conversations: Vec<Conversation>,
}

async fn list_conversations(State(state): State<AppState>) -> Result<Json<ConversationList>, AppError> {
    Ok(Json(ConversationList { conversations: state.store.list_conversations(MAX_CONVERSATIONS).await? }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleBody {
    title: Option<String>,
}

fn clean_title(title: Option<String>) -> Result<Option<String>, AppError> {
    match title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) if t.chars().count() > MAX_TITLE_CHARS => {
            Err(AppError::BadRequest(format!("title is longer than {MAX_TITLE_CHARS} characters")))
        }
        other => Ok(other),
    }
}

async fn create_conversation(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TitleBody>,
) -> Result<(StatusCode, Json<Conversation>), AppError> {
    let title = clean_title(body.title)?;
    Ok((StatusCode::CREATED, Json(state.store.create_conversation(title.as_deref()).await?)))
}

#[derive(Serialize)]
struct ConversationDetail {
    conversation: Conversation,
    turns: Vec<Turn>,
}

async fn get_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ConversationDetail>, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    let turns = refresh_all(&state, state.store.list_turns(conversation.id).await?).await;
    Ok(Json(ConversationDetail { conversation, turns }))
}

async fn rename_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<TitleBody>,
) -> Result<Json<Conversation>, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    let title = clean_title(body.title)?.ok_or_else(|| AppError::BadRequest("title must not be empty".into()))?;
    state.store.rename_conversation(conversation.id, &title).await?;
    Ok(Json(Conversation { title: Some(title), ..conversation }))
}

async fn delete_conversation(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    let mut turns = Vec::new();
    for turn in state.store.list_turns(conversation.id).await? {
        turns.push(refresh(&state, turn).await?);
    }
    if turns.iter().any(|t| !cedar::is_terminal(&t.status)) {
        return Err(AppError::Conflict("a response is still being generated; stop it first".into()));
    }
    for turn in &turns {
        // Best effort: the conversation is gone either way.
        if let Err(e) = state.cedar.delete_run(turn.run_id).await {
            tracing::warn!(run_id = %turn.run_id, error = %e, "could not delete cedar run");
        }
    }
    state.store.delete_conversation(conversation.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// -- messages -------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageBody {
    content: String,
    /// A registered model's id. Absent: the default model from Settings.
    model: Option<String>,
    /// One of the model's reasoning levels. Absent: the model's default.
    reasoning: Option<String>,
    /// PDFs for any model; images for one registered as accepting them.
    attachments: Option<Vec<Attachment>>,
}

/// The files sent with a message: checked the way cedar checks them (count, name, type, base64,
/// sizes) so a mistake is reported before anything is sent, and summarized for the turn. The
/// size comes from the base64 length; cedar does the real decoding.
fn clean_attachments(attachments: Vec<Attachment>) -> Result<(Vec<Attachment>, Vec<AttachmentInfo>), AppError> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(AppError::BadRequest(format!("at most {MAX_ATTACHMENTS} files per message")));
    }
    let mut infos = Vec::with_capacity(attachments.len());
    let mut total = 0usize;
    for (i, a) in attachments.iter().enumerate() {
        let which = |what: &str| AppError::BadRequest(format!("file {} ({:?}): {what}", i + 1, a.name));
        let name = a.name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest(format!("file {}: name must not be empty", i + 1)));
        }
        if name.chars().count() > MAX_ATTACHMENT_NAME_CHARS || name.chars().any(char::is_control) {
            return Err(which("name is malformed"));
        }
        if !ATTACHMENT_TYPES.contains(&a.media_type.as_str()) {
            return Err(which("only PNG, JPEG, GIF, WebP and PDF files can be attached"));
        }
        let data = a.data.as_bytes();
        if data.is_empty() {
            return Err(which("the file is empty"));
        }
        if data.len() % 4 != 0 || !data.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')) {
            return Err(which("data is not valid base64"));
        }
        let padding = data.iter().rev().take_while(|b| **b == b'=').count();
        let size = data.len() / 4 * 3 - padding;
        if size > MAX_ATTACHMENT_BYTES {
            return Err(which(&format!("larger than {} MiB", MAX_ATTACHMENT_BYTES >> 20)));
        }
        total += size;
        if total > MAX_ATTACHMENTS_TOTAL_BYTES {
            return Err(AppError::BadRequest(format!("the files add up to more than {} MiB", MAX_ATTACHMENTS_TOTAL_BYTES >> 20)));
        }
        infos.push(AttachmentInfo { name: name.to_string(), media_type: a.media_type.clone(), size: size as u64 });
    }
    Ok((attachments, infos))
}

/// A user message or a schedule's prompt: trimmed, non-empty, and not too long for cedar.
fn clean_message(what: &str, content: &str) -> Result<String, AppError> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::BadRequest(format!("{what} must not be empty")));
    }
    if content.len() > MAX_MESSAGE_BYTES {
        return Err(AppError::BadRequest(format!("{what} is longer than {MAX_MESSAGE_BYTES} bytes")));
    }
    Ok(content.to_string())
}

/// Blank means absent, for the model and reasoning fields.
fn blank_is_none(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(String::from)
}

/// The registered model `model` names (by id, or by its name on the server), if any.
fn find_model<'a>(models: &'a [Value], model: &str) -> Option<&'a Value> {
    models.iter().find(|m| m["id"] == model).or_else(|| models.iter().find(|m| m["model"] == model))
}

/// A model and reasoning level to save (for the defaults or a schedule), checked against
/// cedar's registry so a stale choice can't break every later run: the model must be
/// registered, and the level one the model (or, without a model, the default one) offers.
async fn clean_model_choice(
    state: &AppState,
    model: Option<&str>,
    reasoning: Option<&str>,
) -> Result<(Option<String>, Option<String>), AppError> {
    let model = blank_is_none(model);
    let reasoning = blank_is_none(reasoning);
    if model.is_none() && reasoning.is_none() {
        return Ok((None, None));
    }
    let models = state.cedar.models().await?;
    let entry = match &model {
        Some(m) => find_model(&models, m).ok_or_else(|| AppError::BadRequest(format!("model {m:?} is not registered")))?,
        None => models
            .iter()
            .find(|m| m["is_default"] == true)
            .ok_or_else(|| AppError::BadRequest("no model is registered yet".into()))?,
    };
    if let Some(level) = &reasoning {
        let offered = entry["reasoning_levels"].as_array().is_some_and(|ls| ls.iter().any(|l| l == level));
        if !offered {
            return Err(AppError::BadRequest(format!("model {:?} offers no reasoning level {level:?}", entry["id"])));
        }
    }
    Ok((model.map(|_| entry["id"].as_str().unwrap_or_default().to_string()), reasoning))
}

/// A saved default model, if cedar still has it. `None` lets cedar use its own default.
async fn still_registered(state: &AppState, model: Option<&str>) -> Option<String> {
    let model = model?;
    match state.cedar.models().await {
        Ok(models) => find_model(&models, model).map(|m| m["id"].as_str().unwrap_or(model).to_string()),
        // Can't tell: let cedar decide.
        Err(_) => Some(model.to_string()),
    }
}

/// The conversation's turns, brought up to date. `Err(Conflict)` if one is still going.
async fn settled_turns(state: &AppState, conversation: &Conversation) -> Result<Vec<Turn>, AppError> {
    let mut turns = Vec::new();
    for turn in state.store.list_turns(conversation.id).await? {
        turns.push(refresh(state, turn).await?);
    }
    if turns.iter().any(|t| !cedar::is_terminal(&t.status)) {
        return Err(AppError::Conflict("wait for the current response to finish, or stop it".into()));
    }
    Ok(turns)
}

/// The finished exchanges of a conversation, for resending as text.
fn exchanges(turns: &[Turn]) -> Vec<Exchange<'_>> {
    turns
        .iter()
        .filter(|t| t.status == "completed")
        .filter_map(|t| Some(Exchange { user: &t.prompt, assistant: t.final_text.as_deref()? }))
        .collect()
}

async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<MessageBody>,
) -> Result<(StatusCode, Json<Turn>), AppError> {
    let content = clean_message("message", &body.content)?;
    // The model and level go to cedar as given: it checks them against its registry.
    let requested = blank_is_none(body.model.as_deref());
    let reasoning = blank_is_none(body.reasoning.as_deref());
    // Whether the model takes them is cedar's call (its registry says), relayed as it answers.
    let (attachments, infos) = clean_attachments(body.attachments.unwrap_or_default())?;
    let prefs = state.store.preferences().await?;

    let _guard = state.send_lock.lock().await;
    let conversation = find_conversation(&state, &id).await?;
    let turns = settled_turns(&state, &conversation).await?;
    let settings = RunSettings::resolve(&state, &prefs, requested, reasoning, conversation.tools.clone()).await;
    let model = settings.model.as_deref();
    let options = |parent| RunOptions { attachments: Some(&attachments), ..settings.options(parent) };

    // Continue the previous run, so the agent keeps the whole conversation (tool calls and
    // earlier attachments included) and the files in its sandbox workspace.
    let run = match turns.last() {
        None => state.cedar.create_run(&content, &options(None)).await?,
        Some(last) => match state.cedar.create_run(&content, &options(Some(last.run_id))).await {
            // cedar no longer has the previous run (deleted, or a reset database). Start over,
            // carrying what this service remembers of the conversation in the prompt. Earlier
            // attachments went with the run; only this message's come along.
            Err(AppError::Upstream { status: StatusCode::NOT_FOUND, .. }) => {
                tracing::warn!(run_id = %last.run_id, "previous run is gone from cedar; resending history as text");
                let run_prompt = prompt::compose(&exchanges(&turns), &content, state.cfg.history_max_bytes);
                state.cedar.create_run(&run_prompt, &options(None)).await?
            }
            other => other?,
        },
    };
    let turn = state
        .store
        .insert_turn(conversation.id, run.id, &content, &run.status, model, settings.reasoning.as_deref(), &infos)
        .await?;
    Ok((StatusCode::CREATED, Json(turn)))
}

// -- settings and tools ---------------------------------------------------------------------

/// How a new run is started, from the Settings defaults and a conversation's or schedule's own
/// tools.
pub struct RunSettings {
    pub model: Option<String>,
    /// The reasoning level asked for; `None` leaves the model's default to cedar.
    pub reasoning: Option<String>,
    pub system: Option<String>,
    pub max_turns: Option<u32>,
    /// `*` when every tool is meant, so a continuation doesn't keep its parent's narrower choice.
    pub tools: Vec<String>,
}

impl RunSettings {
    /// `model`, `reasoning` and `tools` win over the defaults. A default model that is no longer
    /// registered is dropped (cedar then uses its own default).
    pub async fn resolve(
        state: &AppState,
        prefs: &Preferences,
        model: Option<String>,
        reasoning: Option<String>,
        tools: Option<Vec<String>>,
    ) -> Self {
        let model = match model {
            Some(m) => Some(m),
            None => still_registered(state, prefs.model.as_deref()).await,
        };
        let tools = tools.or_else(|| prefs.tools.clone()).unwrap_or_else(|| vec!["*".to_string()]);
        Self { model, reasoning, system: prefs.system.clone(), max_turns: prefs.max_turns, tools }
    }

    pub fn options(&self, parent: Option<Uuid>) -> RunOptions<'_> {
        RunOptions {
            model: self.model.as_deref(),
            reasoning: self.reasoning.as_deref(),
            system: self.system.as_deref(),
            max_turns: self.max_turns,
            tools: Some(&self.tools),
            parent,
            attachments: None,
        }
    }
}

/// A tool selection from the client, checked against what cedar offers so a typo can't break
/// every later message. Groups are accepted even when cedar has no tools in them right now.
async fn clean_tools(state: &AppState, tools: Option<Vec<String>>) -> Result<Option<Vec<String>>, AppError> {
    let Some(mut tools) = tools else { return Ok(None) };
    if tools.len() > MAX_TOOLS {
        return Err(AppError::BadRequest(format!("tools may name at most {MAX_TOOLS} entries")));
    }
    tools.sort();
    tools.dedup();
    let catalog = state.cedar.tools().await?;
    let groups = catalog["groups"].as_array().cloned().unwrap_or_default();
    let known = |name: &str| {
        name == "*"
            || groups.iter().any(|g| {
                g["id"] == name
                    || g["tools"].as_array().is_some_and(|ts| ts.iter().any(|t| t["name"] == name))
                    // MCP servers list their tools only once a run has connected to them.
                    || g["discovered"] == false && name.strip_prefix(g["id"].as_str().unwrap_or("-")).is_some_and(|r| r.starts_with('_'))
            })
    };
    if let Some(bad) = tools.iter().find(|t| !known(t)) {
        return Err(AppError::BadRequest(format!("cedar offers no tool or tool group {bad:?}")));
    }
    Ok(Some(tools))
}

/// The saved defaults, plus cedar's own (`defaults`: its model, system prompt and turn limit, or
/// `null` if cedar can't be reached) so the page can show what an unset field means.
async fn get_settings(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let prefs = state.store.preferences().await?;
    let defaults = match state.cedar.defaults().await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "could not read cedar's defaults");
            Value::Null
        }
    };
    let mut body = serde_json::to_value(prefs).map_err(anyhow::Error::from)?;
    body["defaults"] = defaults;
    Ok(Json(body))
}

async fn put_settings(State(state): State<AppState>, ApiJson(body): ApiJson<SettingsBody>) -> Result<Json<Preferences>, AppError> {
    let system = body.system.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if system.as_ref().is_some_and(|s| s.len() > MAX_SYSTEM_BYTES) {
        return Err(AppError::BadRequest(format!("system is longer than {MAX_SYSTEM_BYTES} bytes")));
    }
    if body.max_turns.is_some_and(|n| n == 0 || n > MAX_TURNS) {
        return Err(AppError::BadRequest(format!("max_turns must be between 1 and {MAX_TURNS}")));
    }
    let (model, _) = clean_model_choice(&state, body.model.as_deref(), None).await?;
    let prefs = Preferences {
        tools: clean_tools(&state, body.tools).await?,
        model,
        system,
        max_turns: body.max_turns,
    };
    state.store.set_preferences(&prefs).await?;
    Ok(Json(prefs))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsBody {
    tools: Option<Vec<String>>,
    model: Option<String>,
    system: Option<String>,
    max_turns: Option<u32>,
}

/// cedar's tool groups, for the Tools settings and the composer's tools menu.
async fn list_tools(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.cedar.tools().await?))
}

/// The models cedar has registered, for the pickers.
async fn list_models(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({ "models": state.cedar.models().await? })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolsBody {
    /// `null` returns the conversation to the default tools.
    tools: Option<Vec<String>>,
}

async fn set_conversation_tools(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ToolsBody>,
) -> Result<Json<Conversation>, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    let tools = clean_tools(&state, body.tools).await?;
    state.store.set_conversation_tools(conversation.id, tools.as_deref()).await?;
    Ok(Json(Conversation { tools, ..conversation }))
}

// -- integrations (admin) -------------------------------------------------------------------

/// The Integrations settings need the admin token (`X-Admin-Token`) on top of the access token.
fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let Some(expected) = &state.admin_digest else {
        return Err(AppError::Forbidden("integrations are managed in cedar's environment here (CHAT_ADMIN_TOKEN is not set)".into()));
    };
    let presented = headers.get(ADMIN_HEADER).and_then(|v| v.to_str().ok()).map(|t| Sha256::digest(t.trim().as_bytes()));
    match presented {
        Some(digest) if bool::from(digest.as_slice().ct_eq(expected)) => Ok(()),
        _ => Err(AppError::Forbidden("wrong or missing admin token".into())),
    }
}

fn redirect_uri(state: &AppState) -> String {
    format!("{}{GOOGLE_CALLBACK_PATH}", state.cfg.public_url)
}

async fn get_integrations(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    let mut status = state.cedar.integrations().await?;
    // To register with the Google OAuth client.
    status["google"]["redirect_uri"] = json!(redirect_uri(&state));
    Ok(Json(status))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BraveKeyBody {
    api_key: String,
}

async fn put_brave_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<BraveKeyBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.set_brave_key(&body.api_key).await?))
}

async fn get_models(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.models_admin().await?))
}

/// Register a model or change one. The body is cedar's (`{base_url, model, label?, api_key?,
/// reasoning_levels?, default_reasoning?, is_default?}`) and goes through as it is; cedar
/// checks it and the server it names.
async fn put_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<Value>,
) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    if !body.is_object() {
        return Err(AppError::BadRequest("the body must be a JSON object".into()));
    }
    Ok(Json(state.cedar.set_model(&id, &body).await?))
}

async fn delete_model(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_model(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_brave_key(State(state): State<AppState>, headers: HeaderMap) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_brave_key().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn connect_google(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    let url = state.cedar.google_authorize(&redirect_uri(&state)).await?;
    Ok(Json(json!({ "url": url })))
}

async fn delete_google(State(state): State<AppState>, headers: HeaderMap) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_google().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Google sends the browser here after "Connect Google". Hand the code to cedar, then go back to
/// the page, which reports the outcome from the query string.
async fn google_callback(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> Redirect {
    let outcome = match (params.get("state"), params.get("code"), params.get("error")) {
        (_, _, Some(error)) => Err(format!("Google sign-in was not completed ({error})")),
        (Some(s), Some(code), None) => state.cedar.google_callback(s, code).await.map_err(|e| e.to_string()),
        _ => Err("the sign-in response from Google is incomplete".to_string()),
    };
    let mut url = reqwest::Url::parse("http://local/").expect("static URL");
    match outcome {
        Ok(_) => url.query_pairs_mut().append_pair("integrations", "connected"),
        Err(message) => {
            tracing::warn!(error = %message, "Connect Google failed");
            url.query_pairs_mut().append_pair("integrations", "error").append_pair("message", &message)
        }
    };
    Redirect::to(&format!("/?{}", url.query().unwrap_or_default()))
}

// -- compaction -----------------------------------------------------------------------------

#[derive(Serialize)]
struct Compacted {
    /// One self-contained prompt that stands for the whole conversation.
    prompt: String,
    /// `run` when cedar compacted the stored run (tool calls included); `history` when the run
    /// was gone and the exchanges this service remembers were used instead.
    source: &'static str,
}

/// Have the model turn the conversation so far into one prompt, for scheduling it.
async fn compact_conversation(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Compacted>, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    let turns = settled_turns(&state, &conversation).await?;
    let last = turns
        .iter()
        .rev()
        .find(|t| t.status == "completed")
        .ok_or(AppError::NotFound("the conversation has no finished response to schedule yet"))?;
    // The conversation's model, if cedar still has it; else cedar's default.
    let model = still_registered(&state, conversation.model.as_deref()).await;
    match state.cedar.compact_run(last.run_id, model.as_deref()).await {
        Ok(prompt) => Ok(Json(Compacted { prompt, source: "run" })),
        Err(AppError::Upstream { status: StatusCode::NOT_FOUND, .. }) => {
            tracing::warn!(run_id = %last.run_id, "run is gone from cedar; compacting the remembered exchanges");
            let transcript = prompt::transcript(&exchanges(&turns), state.cfg.history_max_bytes);
            let prompt = state.cedar.compact_transcript(&transcript, model.as_deref()).await?;
            Ok(Json(Compacted { prompt, source: "history" }))
        }
        Err(e) => Err(e),
    }
}

// -- schedules ------------------------------------------------------------------------------

/// Fields of a schedule as the client sends them. Creating needs all of them (but `model`,
/// `weekday`, `conversation_id` and `enabled`); updating takes any subset.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ScheduleBody {
    title: Option<String>,
    prompt: Option<String>,
    /// A registered model's id. Blank or absent means cedar's default model.
    model: Option<String>,
    /// One of the model's reasoning levels. Blank or absent means the model's default.
    reasoning: Option<String>,
    frequency: Option<String>,
    weekday: Option<u8>,
    time: Option<String>,
    timezone: Option<String>,
    enabled: Option<bool>,
    /// The chat the schedule was made from.
    conversation_id: Option<Uuid>,
    /// The tools its runs use; `null` means the defaults. When creating, absent means the
    /// conversation's own tools, if it has any.
    #[serde(default, deserialize_with = "nullable")]
    tools: Option<Option<Vec<String>>>,
}

/// Tells an absent field (`None`) from an explicit `null` (`Some(None)`).
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// A run record with the turn that carries its cedar run, if it started.
#[derive(Serialize)]
struct RunDetail {
    #[serde(flatten)]
    run: ScheduleRun,
    turn: Option<Turn>,
}

#[derive(Serialize)]
struct ScheduleSummary {
    #[serde(flatten)]
    schedule: Schedule,
    last_run: Option<RunDetail>,
}

#[derive(Serialize)]
struct ScheduleDetail {
    schedule: Schedule,
    runs: Vec<RunDetail>,
}

#[derive(Serialize)]
struct ScheduleList {
    schedules: Vec<ScheduleSummary>,
}

async fn find_schedule(state: &AppState, raw_id: &str) -> Result<Schedule, AppError> {
    let id = parse_id(raw_id, "schedule not found")?;
    state.store.get_schedule(id).await?.ok_or(AppError::NotFound("schedule not found"))
}

/// Attach each run's turn, refreshed from cedar while it is unfinished.
async fn with_turns(state: &AppState, runs: Vec<ScheduleRun>) -> Result<Vec<RunDetail>, AppError> {
    let mut out = Vec::with_capacity(runs.len());
    for run in runs {
        let turn = match run.turn_id {
            Some(id) => state.store.get_turn(id).await?,
            None => None,
        };
        let turn = match turn {
            Some(t) => refresh_all(state, vec![t]).await.pop(),
            None => None,
        };
        out.push(RunDetail { run, turn });
    }
    Ok(out)
}

async fn list_schedules(State(state): State<AppState>) -> Result<Json<ScheduleList>, AppError> {
    let mut schedules = Vec::new();
    for schedule in state.store.list_schedules().await? {
        let last = state.store.list_schedule_runs(schedule.id, 1).await?;
        let last_run = with_turns(&state, last).await?.pop();
        schedules.push(ScheduleSummary { schedule, last_run });
    }
    Ok(Json(ScheduleList { schedules }))
}

async fn create_schedule(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ScheduleBody>,
) -> Result<(StatusCode, Json<Schedule>), AppError> {
    let title = clean_title(body.title)?.ok_or_else(|| AppError::BadRequest("title must not be empty".into()))?;
    let prompt = clean_message("prompt", body.prompt.as_deref().unwrap_or_default())?;
    let (model, reasoning) = clean_model_choice(&state, body.model.as_deref(), body.reasoning.as_deref()).await?;
    let missing = |what: &str| AppError::BadRequest(format!("{what} is required"));
    let frequency = body.frequency.ok_or_else(|| missing("frequency"))?;
    let time = body.time.ok_or_else(|| missing("time"))?;
    let timezone = body.timezone.ok_or_else(|| missing("timezone"))?;
    let timing = Timing::parse(&frequency, body.weekday, &time, &timezone).map_err(AppError::BadRequest)?;
    if state.store.count_schedules().await? >= MAX_SCHEDULES {
        return Err(AppError::Conflict(format!("at most {MAX_SCHEDULES} schedules; delete one first")));
    }
    let tools = match body.tools {
        Some(tools) => clean_tools(&state, tools).await?,
        None => match body.conversation_id {
            Some(cid) => state.store.get_conversation(cid).await?.and_then(|c| c.tools),
            None => None,
        },
    };
    let schedule = state
        .store
        .create_schedule(&NewSchedule {
            title: &title,
            prompt: &prompt,
            model: model.as_deref(),
            reasoning: reasoning.as_deref(),
            source_conversation_id: body.conversation_id,
            tools: tools.as_deref(),
            frequency: timing.frequency.as_str(),
            weekday: timing.weekday.map(|d| d.num_days_from_monday() as u8),
            time_of_day: &timing.time.format("%H:%M").to_string(),
            timezone: timing.tz.name(),
            next_run_at: timing.next_after(Utc::now()),
        })
        .await?;
    Ok((StatusCode::CREATED, Json(schedule)))
}

async fn get_schedule(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<ScheduleDetail>, AppError> {
    let schedule = find_schedule(&state, &id).await?;
    let runs = with_turns(&state, state.store.list_schedule_runs(schedule.id, MAX_SCHEDULE_RUNS).await?).await?;
    Ok(Json(ScheduleDetail { schedule, runs }))
}

async fn update_schedule(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ScheduleBody>,
) -> Result<Json<Schedule>, AppError> {
    let mut schedule = find_schedule(&state, &id).await?;
    if let Some(title) = body.title {
        schedule.title = clean_title(Some(title))?.ok_or_else(|| AppError::BadRequest("title must not be empty".into()))?;
    }
    if let Some(prompt) = &body.prompt {
        schedule.prompt = clean_message("prompt", prompt)?;
    }
    if body.model.is_some() || body.reasoning.is_some() {
        let model = body.model.as_deref().or(schedule.model.as_deref());
        let reasoning = body.reasoning.as_deref().or(schedule.reasoning.as_deref());
        (schedule.model, schedule.reasoning) = clean_model_choice(&state, model, reasoning).await?;
    }
    if let Some(tools) = body.tools {
        schedule.tools = clean_tools(&state, tools).await?;
    }
    let retime = body.frequency.is_some() || body.weekday.is_some() || body.time.is_some() || body.timezone.is_some();
    if retime {
        let timing = Timing::parse(
            body.frequency.as_deref().unwrap_or(&schedule.frequency),
            body.weekday.or(schedule.weekday),
            body.time.as_deref().unwrap_or(&schedule.time_of_day),
            body.timezone.as_deref().unwrap_or(&schedule.timezone),
        )
        .map_err(AppError::BadRequest)?;
        schedule.frequency = timing.frequency.as_str().to_string();
        schedule.weekday = timing.weekday.map(|d| d.num_days_from_monday() as u8);
        schedule.time_of_day = timing.time.format("%H:%M").to_string();
        schedule.timezone = timing.tz.name().to_string();
        schedule.next_run_at = timing.next_after(Utc::now());
    }
    if let Some(enabled) = body.enabled {
        if enabled && !schedule.enabled && !retime {
            // Resuming: don't fire for the slots missed while paused.
            let timing = Timing::of(&schedule).map_err(AppError::BadRequest)?;
            schedule.next_run_at = timing.next_after(Utc::now());
        }
        schedule.enabled = enabled;
    }
    if !state.store.update_schedule(&schedule).await? {
        return Err(AppError::NotFound("schedule not found"));
    }
    schedule.updated_at = Utc::now();
    Ok(Json(schedule))
}

async fn delete_schedule(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    let schedule = find_schedule(&state, &id).await?;
    let turns = refresh_all(&state, state.store.list_schedule_turns(schedule.id).await?).await;
    if turns.iter().any(|t| !cedar::is_terminal(&t.status)) {
        return Err(AppError::Conflict("a run of this schedule is still going; stop it first".into()));
    }
    for turn in &turns {
        // Best effort, as for a conversation: the schedule is gone either way.
        if let Err(e) = state.cedar.delete_run(turn.run_id).await {
            tracing::warn!(run_id = %turn.run_id, error = %e, "could not delete cedar run");
        }
    }
    state.store.delete_schedule(schedule.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Start a run right now, outside the schedule. Doesn't move the next scheduled run.
async fn run_schedule_now(State(state): State<AppState>, Path(id): Path<String>) -> Result<(StatusCode, Json<RunDetail>), AppError> {
    let schedule = find_schedule(&state, &id).await?;
    let newest = state.store.list_schedule_runs(schedule.id, 1).await?;
    if let Some(last) = with_turns(&state, newest).await?.pop() {
        if last.turn.as_ref().is_some_and(|t| !cedar::is_terminal(&t.status)) {
            return Err(AppError::Conflict("the previous run of this schedule is still going".into()));
        }
    }
    let now = Utc::now();
    let run = schedule::fire(&state, &schedule, now, now, true).await?;
    let detail = with_turns(&state, vec![run]).await?.pop().expect("one run");
    Ok((StatusCode::ACCEPTED, Json(detail)))
}

// -- workspace files ------------------------------------------------------------------------

/// The run whose workspace holds the conversation's files: the latest one, since every turn
/// continues the previous run and keeps its workspace. `None` for a conversation with no turns.
async fn latest_run(state: &AppState, raw_id: &str) -> Result<Option<Uuid>, AppError> {
    let conversation = find_conversation(state, raw_id).await?;
    Ok(state.store.list_turns(conversation.id).await?.last().map(|t| t.run_id))
}

async fn list_files(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, AppError> {
    match latest_run(&state, &id).await? {
        Some(run_id) => Ok(Json(state.cedar.list_files(run_id).await?)),
        None => Ok(Json(json!({ "workspace_id": null, "files": [], "truncated": false }))),
    }
}

/// Relay a file download from cedar, keeping the headers that describe the file.
async fn download_file(State(state): State<AppState>, Path((id, path)): Path<(String, String)>) -> Result<Response, AppError> {
    let run_id = latest_run(&state, &id).await?.ok_or(AppError::NotFound("no such file in the workspace"))?;
    let upstream = state.cedar.download_file(run_id, &path).await?;
    let mut headers = axum::http::HeaderMap::new();
    for name in [CONTENT_TYPE, header::CONTENT_LENGTH, header::CONTENT_DISPOSITION] {
        if let Some(value) = upstream.headers().get(&name) {
            headers.insert(name, value.clone());
        }
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, Body::from_stream(upstream.bytes_stream())).into_response())
}

#[derive(Deserialize)]
struct EventsParams {
    after: Option<i64>,
}

/// Relay the cedar event stream for a turn's run: replay from `after`, then follow it live.
async fn turn_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiQuery(params): ApiQuery<EventsParams>,
) -> Result<Response, AppError> {
    let turn = find_turn(&state, &id).await?;
    let upstream = state.cedar.events(turn.run_id, params.after.unwrap_or(0).max(0)).await?;
    Ok((
        [(CONTENT_TYPE, "text/event-stream"), (CACHE_CONTROL, "no-cache")],
        [("x-accel-buffering", "no")],
        Body::from_stream(upstream.bytes_stream()),
    )
        .into_response())
}

async fn cancel_turn(State(state): State<AppState>, Path(id): Path<String>) -> Result<(StatusCode, Json<Turn>), AppError> {
    let turn = refresh(&state, find_turn(&state, &id).await?).await?;
    if cedar::is_terminal(&turn.status) {
        return Err(AppError::Conflict(format!("the response already {}", turn.status)));
    }
    state.cedar.cancel_run(turn.run_id).await?;
    Ok((StatusCode::ACCEPTED, Json(turn)))
}
