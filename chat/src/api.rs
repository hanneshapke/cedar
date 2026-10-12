//! The HTTP API and the embedded web UI.

use crate::cedar::{self, CedarClient};
use crate::config::Config;
use crate::error::AppError;
use crate::prompt::{self, Exchange};
use crate::schedule::{self, Timing};
use crate::store::{Conversation, NewSchedule, Schedule, ScheduleRun, Store, Turn};
use chrono::Utc;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Request, State};
use axum::http::header::{self, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
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

/// `/` only redirects to the UI the browser has chosen (see `ui-pref.js`).
const LOADER_HTML: &str = include_str!("../static/index.html");
const UI_PREF_JS: &str = include_str!("../static/ui-pref.js");
/// Previews Markdown files and PDFs; both UIs load it.
const FILE_VIEWER_JS: &str = include_str!("../static/file-viewer.js");
/// Draws the `show_map` tool's result as a map; both UIs load it, and it loads Leaflet on demand.
const MAP_VIEW_JS: &str = include_str!("../static/map-view.js");
const MAP_VIEW_CSS: &str = include_str!("../static/map-view.css");
/// Scheduling a conversation as a recurring agent, and the Settings list of schedules; both UIs load it.
const SCHEDULE_JS: &str = include_str!("../static/schedule.js");
const SCHEDULE_CSS: &str = include_str!("../static/schedule.css");
/// Leaflet 1.9.4 (BSD-2-Clause, see `static/vendor/leaflet/LICENSE`), served from here so no
/// third-party code runs in the page. Only the map tiles come from another server.
const LEAFLET_JS: &str = include_str!("../static/vendor/leaflet/leaflet.js");
const LEAFLET_CSS: &str = include_str!("../static/vendor/leaflet/leaflet.css");

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
    /// Serializes sending messages, so two tabs can't start overlapping runs in one conversation.
    send_lock: Arc<Mutex<()>>,
}

impl AppState {
    pub fn new(cfg: Config, store: Store) -> anyhow::Result<Self> {
        let cedar = CedarClient::new(&cfg.cedar_url, &cfg.cedar_api_key)?;
        let token_digest = cfg.access_token.as_deref().map(|t| Sha256::digest(t.as_bytes()).into());
        Ok(Self { cfg: Arc::new(cfg), store, cedar, token_digest, send_lock: Arc::default() })
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/conversations", get(list_conversations).post(create_conversation))
        .route("/conversations/{id}", get(get_conversation).patch(rename_conversation).delete(delete_conversation))
        .route("/conversations/{id}/messages", post(send_message))
        .route("/conversations/{id}/compact", post(compact_conversation))
        .route("/conversations/{id}/files", get(list_files))
        .route("/conversations/{id}/files/{*path}", get(download_file))
        .route("/turns/{id}/events", get(turn_events))
        .route("/turns/{id}/cancel", post(cancel_turn))
        .route("/schedules", get(list_schedules).post(create_schedule))
        .route("/schedules/{id}", get(get_schedule).patch(update_schedule).delete(delete_schedule))
        .route("/schedules/{id}/run", post(run_schedule_now))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        // Public: the page needs it to know whether to ask for a token.
        .route("/config", get(client_config));

    let pages = Router::new()
        .route("/", get(|| async { asset(HTML, LOADER_HTML) }))
        .route("/ui-pref.js", get(|| async { asset(JS, UI_PREF_JS) }))
        .route("/file-viewer.js", get(|| async { asset(JS, FILE_VIEWER_JS) }))
        .route("/map-view.js", get(|| async { asset(JS, MAP_VIEW_JS) }))
        .route("/map-view.css", get(|| async { asset(CSS, MAP_VIEW_CSS) }))
        .route("/schedule.js", get(|| async { asset(JS, SCHEDULE_JS) }))
        .route("/schedule.css", get(|| async { asset(CSS, SCHEDULE_CSS) }))
        .route("/vendor/leaflet/leaflet.js", get(|| async { asset(JS, LEAFLET_JS) }))
        .route("/vendor/leaflet/leaflet.css", get(|| async { asset(CSS, LEAFLET_CSS) }))
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

/// Serve a front end's page at `/vN` and `/vN/`, and its script and stylesheet next to it.
fn with_ui(router: Router<AppState>, ui: Ui) -> Router<AppState> {
    let Ui { prefix, index, js, css } = ui;
    router
        .route(prefix, get(move || async move { asset(HTML, index) }))
        .route(&format!("{prefix}/"), get(move || async move { asset(HTML, index) }))
        .route(&format!("{prefix}/app.js"), get(move || async move { asset(JS, js) }))
        .route(&format!("{prefix}/style.css"), get(move || async move { asset(CSS, css) }))
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
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
    Json(json!({ "auth_required": state.token_digest.is_some(), "models": state.cfg.models }))
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
    model: Option<String>,
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

/// A model from the request: `None` (or blank) for cedar's default, else one of the offered ones.
fn clean_model(state: &AppState, model: Option<&str>) -> Result<Option<String>, AppError> {
    match model.map(str::trim).filter(|m| !m.is_empty()) {
        None => Ok(None),
        Some(m) if state.cfg.models.iter().any(|allowed| allowed == m) => Ok(Some(m.to_string())),
        Some(m) => Err(AppError::BadRequest(format!("model {m:?} is not offered"))),
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
    let model = clean_model(&state, body.model.as_deref())?;
    let model = model.as_deref();

    let _guard = state.send_lock.lock().await;
    let conversation = find_conversation(&state, &id).await?;
    let turns = settled_turns(&state, &conversation).await?;

    // Continue the previous run, so the agent keeps the whole conversation (tool calls included)
    // and the files in its sandbox workspace.
    let run = match turns.last() {
        None => state.cedar.create_run(&content, model, None).await?,
        Some(last) => match state.cedar.create_run(&content, model, Some(last.run_id)).await {
            // cedar no longer has the previous run (deleted, or a reset database). Start over,
            // carrying what this service remembers of the conversation in the prompt.
            Err(AppError::Upstream { status: StatusCode::NOT_FOUND, .. }) => {
                tracing::warn!(run_id = %last.run_id, "previous run is gone from cedar; resending history as text");
                let run_prompt = prompt::compose(&exchanges(&turns), &content, state.cfg.history_max_bytes);
                state.cedar.create_run(&run_prompt, model, None).await?
            }
            other => other?,
        },
    };
    let turn = state.store.insert_turn(conversation.id, run.id, &content, &run.status, model).await?;
    Ok((StatusCode::CREATED, Json(turn)))
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
    let model = clean_model(&state, conversation.model.as_deref()).ok().flatten();
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
    /// Blank or absent means cedar's default model.
    model: Option<String>,
    frequency: Option<String>,
    weekday: Option<u8>,
    time: Option<String>,
    timezone: Option<String>,
    enabled: Option<bool>,
    /// The chat the schedule was made from.
    conversation_id: Option<Uuid>,
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
    let model = clean_model(&state, body.model.as_deref())?;
    let missing = |what: &str| AppError::BadRequest(format!("{what} is required"));
    let frequency = body.frequency.ok_or_else(|| missing("frequency"))?;
    let time = body.time.ok_or_else(|| missing("time"))?;
    let timezone = body.timezone.ok_or_else(|| missing("timezone"))?;
    let timing = Timing::parse(&frequency, body.weekday, &time, &timezone).map_err(AppError::BadRequest)?;
    if state.store.count_schedules().await? >= MAX_SCHEDULES {
        return Err(AppError::Conflict(format!("at most {MAX_SCHEDULES} schedules; delete one first")));
    }
    let schedule = state
        .store
        .create_schedule(&NewSchedule {
            title: &title,
            prompt: &prompt,
            model: model.as_deref(),
            source_conversation_id: body.conversation_id,
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
    if let Some(model) = &body.model {
        schedule.model = clean_model(&state, Some(model))?;
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
