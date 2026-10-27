//! End-to-end tests of the chat API against a fake cedar server on a local port.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use cedar_chat::api::{self, AppState};
use cedar_chat::config::Config;
use cedar_chat::store::Store;
use cedar_chat::{notify, schedule};
use tokio_util::sync::CancellationToken;
use chrono::{DateTime, Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt;
use uuid::Uuid;

const CEDAR_KEY: &str = "cedar-key";

#[derive(Default)]
struct FakeCedar {
    runs: Mutex<HashMap<Uuid, Value>>,
    prompts: Mutex<Vec<String>>,
    parents: Mutex<Vec<Option<String>>>,
    deleted: Mutex<Vec<Uuid>>,
    expired: Mutex<Vec<Uuid>>,
    downloads: Mutex<Vec<(Uuid, String)>>,
    compactions: Mutex<Vec<Value>>,
    /// When set, `POST /v1/runs` answers with this status and message instead of a run.
    refuse_runs: Mutex<Option<(StatusCode, &'static str)>>,
    /// Every `POST /v1/runs` body.
    bodies: Mutex<Vec<Value>>,
    /// Admin calls: (route, body).
    admin_calls: Mutex<Vec<(String, Value)>>,
    /// When set, the admin routes refuse the key, as cedar does for a key not in CEDAR_ADMIN_KEYS.
    not_admin: Mutex<bool>,
    /// The key's finished runs, as `GET /v1/events` streams them (see `Harness::finish_with`).
    notifications: Mutex<Vec<Value>>,
    /// The fake Pushover API: every form it received. A token of `bad` is refused.
    pushover: Mutex<Vec<HashMap<String, String>>>,
    /// The fake push service: (endpoint name, headers, body) of every push, and the statuses
    /// it answers with per endpoint name (201 once those run out).
    pushes: Mutex<Vec<(String, HeaderMap, Vec<u8>)>>,
    push_answers: Mutex<HashMap<String, VecDeque<u16>>>,
    /// Answers to runs' plans and approvals: (path, body).
    answers: Mutex<Vec<(String, Value)>>,
    /// The fake Google token endpoint: every form it received. Each gets a new access token.
    google_tokens: Mutex<Vec<HashMap<String, String>>>,
    /// The fake GitHub "latest release": its tag (no release while unset), and how often it was asked.
    release_tag: Mutex<Option<String>>,
    release_lookups: Mutex<u32>,
}

type Shared = Arc<FakeCedar>;

fn authorized(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {CEDAR_KEY}"))
}

fn cedar_error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
}

async fn create_run(State(s): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
    if !authorized(&headers) {
        return cedar_error(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid API key");
    }
    let prompt = body["prompt"].as_str().unwrap().to_string();
    if prompt.contains("TOO MANY") {
        return cedar_error(StatusCode::TOO_MANY_REQUESTS, "too_many_requests", "at most 2 runs");
    }
    if let Some((status, message)) = *s.refuse_runs.lock().unwrap() {
        return cedar_error(status, "refused", message);
    }
    // As cedar does: an unregistered model, or a level the model doesn't offer, is refused.
    let model = body["model"].as_str().unwrap_or("model-a");
    let Some(entry) = registry().into_iter().find(|m| m["id"] == model) else {
        return cedar_error(StatusCode::BAD_REQUEST, "bad_request", &format!("model {model:?} is not registered"));
    };
    if let Some(level) = body["reasoning"].as_str() {
        if !entry["reasoning_levels"].as_array().unwrap().iter().any(|l| l == level) {
            return cedar_error(StatusCode::BAD_REQUEST, "bad_request", &format!("reasoning {level:?} is not offered by model {model:?}"));
        }
    }
    let images = body["attachments"].as_array().is_some_and(|a| a.iter().any(|f| f["media_type"] != "application/pdf"));
    if images && entry["multimodal"] != true {
        return cedar_error(StatusCode::BAD_REQUEST, "bad_request", &format!("model {model:?} does not accept images"));
    }
    s.bodies.lock().unwrap().push(body.clone());
    let parent = body["parent_id"].as_str().map(String::from);
    if let Some(p) = &parent {
        if !s.runs.lock().unwrap().contains_key(&Uuid::parse_str(p).unwrap()) {
            return cedar_error(StatusCode::NOT_FOUND, "not_found", "parent run not found");
        }
    }
    let id = Uuid::new_v4();
    let run = json!({ "id": id, "status": "queued", "prompt": prompt, "model": body["model"], "final_text": null, "error": null });
    s.prompts.lock().unwrap().push(prompt);
    s.parents.lock().unwrap().push(parent);
    s.runs.lock().unwrap().insert(id, run.clone());
    (StatusCode::ACCEPTED, Json(run)).into_response()
}

async fn get_run(State(s): State<Shared>, Path(id): Path<Uuid>) -> axum::response::Response {
    match s.runs.lock().unwrap().get(&id) {
        Some(run) => Json(run.clone()).into_response(),
        None => cedar_error(StatusCode::NOT_FOUND, "not_found", "run not found"),
    }
}

async fn cancel_run(State(s): State<Shared>, Path(id): Path<Uuid>) -> axum::response::Response {
    let mut runs = s.runs.lock().unwrap();
    let run = runs.get_mut(&id).unwrap();
    if run["status"] != "running" && run["status"] != "queued" {
        return cedar_error(StatusCode::CONFLICT, "conflict", "run already finished");
    }
    run["status"] = json!("cancelled");
    (StatusCode::ACCEPTED, Json(run.clone())).into_response()
}

/// cedar answers 409 unless the run waits: here, only runs whose prompt mentions "WAIT" do.
async fn answer(s: Shared, id: Uuid, path: String, body: Value) -> axum::response::Response {
    let waits = s.runs.lock().unwrap().get(&id).is_some_and(|r| r["prompt"].as_str().unwrap_or_default().contains("WAIT"));
    s.answers.lock().unwrap().push((path, body.clone()));
    if !waits {
        return cedar_error(StatusCode::CONFLICT, "conflict", "the run is not waiting for its plan to be agreed");
    }
    (StatusCode::ACCEPTED, Json(body)).into_response()
}

async fn delete_run(State(s): State<Shared>, Path(id): Path<Uuid>) -> StatusCode {
    s.deleted.lock().unwrap().push(id);
    StatusCode::NO_CONTENT
}

/// Compacts a known run's prompt, or a transcript, into "DO: <text>". Unknown runs are a 404.
async fn compact(State(s): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
    if !authorized(&headers) {
        return cedar_error(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid API key");
    }
    s.compactions.lock().unwrap().push(body.clone());
    let text = match body["run_id"].as_str() {
        Some(id) => match s.runs.lock().unwrap().get(&Uuid::parse_str(id).unwrap()) {
            Some(run) => run["prompt"].as_str().unwrap().to_string(),
            None => return cedar_error(StatusCode::NOT_FOUND, "not_found", "run not found"),
        },
        None => body["transcript"].as_str().unwrap().to_string(),
    };
    if text.contains("SILENCE") {
        return cedar_error(StatusCode::BAD_GATEWAY, "bad_gateway", "the model returned no text");
    }
    Json(json!({ "prompt": format!("DO: {text}"), "model": body["model"], "usage": { "input_tokens": 1, "output_tokens": 1 } })).into_response()
}

async fn pushover(State(s): State<Shared>, axum::Form(form): axum::Form<HashMap<String, String>>) -> axum::response::Response {
    let bad = form.get("token").map(String::as_str) == Some("bad");
    s.pushover.lock().unwrap().push(form);
    if bad {
        return (StatusCode::BAD_REQUEST, Json(json!({ "status": 0, "errors": ["application token is invalid"] }))).into_response();
    }
    Json(json!({ "status": 1, "request": "r-1" })).into_response()
}

async fn push_service(State(s): State<Shared>, Path(name): Path<String>, headers: HeaderMap, body: axum::body::Bytes) -> StatusCode {
    s.pushes.lock().unwrap().push((name.clone(), headers, body.to_vec()));
    let status = s.push_answers.lock().unwrap().get_mut(&name).and_then(|q| q.pop_front()).unwrap_or(201);
    StatusCode::from_u16(status).unwrap()
}

/// The fake APNs: records pushes under the device token, answers like `push_service` (200 by
/// default), and gives a 400 Apple's `BadDeviceToken` reason.
async fn apns_service(State(s): State<Shared>, Path(token): Path<String>, headers: HeaderMap, body: axum::body::Bytes) -> axum::response::Response {
    s.pushes.lock().unwrap().push((token.clone(), headers, body.to_vec()));
    let status = s.push_answers.lock().unwrap().get_mut(&token).and_then(|q| q.pop_front()).unwrap_or(200);
    let status = StatusCode::from_u16(status).unwrap();
    let reason = match status.as_u16() {
        200 => return status.into_response(),
        400 => "BadDeviceToken",
        403 => "ExpiredProviderToken",
        410 => "Unregistered",
        _ => "InternalServerError",
    };
    (status, Json(json!({ "reason": reason }))).into_response()
}

/// The fake Google OAuth token endpoint: trades any assertion for `access-N`.
/// GitHub's `releases/latest`, with an ETag that `If-None-Match` is answered with 304 for.
async fn latest_release(State(s): State<Shared>, headers: HeaderMap) -> axum::response::Response {
    *s.release_lookups.lock().unwrap() += 1;
    if headers.get("user-agent").is_none() {
        return (StatusCode::FORBIDDEN, "GitHub wants a User-Agent").into_response();
    }
    let Some(tag) = s.release_tag.lock().unwrap().clone() else {
        return cedar_error(StatusCode::NOT_FOUND, "not_found", "Not Found");
    };
    let etag = format!("\"{tag}\"");
    if headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let v = tag.trim_start_matches('v');
    let asset = |name: String| json!({ "name": name, "browser_download_url": format!("https://example.invalid/{tag}/{name}") });
    let body = json!({
        "tag_name": tag, "html_url": format!("https://example.invalid/releases/{tag}"), "published_at": "2026-10-20T10:00:00Z",
        "draft": false, "prerelease": false,
        "assets": [asset(format!("Cedar-{v}-linux-x64.tar.gz")), asset(format!("Cedar-{v}-linux-arm64.tar.gz")), asset("checksums.txt".into())],
    });
    ([("etag", etag)], Json(body)).into_response()
}

async fn google_token(State(s): State<Shared>, axum::Form(form): axum::Form<HashMap<String, String>>) -> axum::response::Response {
    let mut forms = s.google_tokens.lock().unwrap();
    forms.push(form);
    Json(json!({ "access_token": format!("access-{}", forms.len()), "expires_in": 3599, "token_type": "Bearer" })).into_response()
}

/// The fake FCM: records pushes under the message's token (with the project in the headers as
/// `x-project`), answers like `push_service` (200 by default) with FCM's error bodies.
async fn fcm_service(State(s): State<Shared>, Path((project, action)): Path<(String, String)>, mut headers: HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
    assert_eq!(action, "messages:send");
    let token = body["message"]["token"].as_str().unwrap_or("").to_string();
    headers.insert("x-project", project.parse().unwrap());
    s.pushes.lock().unwrap().push((token.clone(), headers, body.to_string().into_bytes()));
    let status = s.push_answers.lock().unwrap().get_mut(&token).and_then(|q| q.pop_front()).unwrap_or(200);
    let status = StatusCode::from_u16(status).unwrap();
    let (name, code, message) = match status.as_u16() {
        200 => return Json(json!({ "name": format!("projects/{project}/messages/1") })).into_response(),
        400 => ("INVALID_ARGUMENT", "INVALID_ARGUMENT", "The registration token is not a valid FCM registration token"),
        401 => ("UNAUTHENTICATED", "THIRD_PARTY_AUTH_ERROR", "Request had invalid authentication credentials."),
        404 => ("NOT_FOUND", "UNREGISTERED", "Requested entity was not found."),
        _ => ("INTERNAL", "INTERNAL", "Internal error."),
    };
    let details = json!([{ "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError", "errorCode": code }]);
    (status, Json(json!({ "error": { "code": status.as_u16(), "message": message, "status": name, "details": details } }))).into_response()
}

/// A mail server that takes anything: returns its port and the messages it was given.
async fn fake_smtp() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mails = Arc::new(Mutex::new(Vec::new()));
    let store = mails.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let store = store.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = socket.split();
                let mut lines = BufReader::new(reader).lines();
                writer.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                let mut in_data = false;
                let mut mail = String::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    if in_data {
                        if line == "." {
                            in_data = false;
                            store.lock().unwrap().push(std::mem::take(&mut mail));
                            writer.write_all(b"250 queued\r\n").await.unwrap();
                        } else {
                            mail.push_str(&line);
                            mail.push('\n');
                        }
                        continue;
                    }
                    let upper = line.to_ascii_uppercase();
                    let reply: &[u8] = if upper.starts_with("EHLO") {
                        b"250-fake\r\n250 AUTH PLAIN LOGIN\r\n"
                    } else if upper.starts_with("AUTH") {
                        b"235 authenticated\r\n"
                    } else if upper.starts_with("DATA") {
                        in_data = true;
                        b"354 go ahead\r\n"
                    } else if upper.starts_with("QUIT") {
                        writer.write_all(b"221 bye\r\n").await.unwrap();
                        return;
                    } else {
                        b"250 ok\r\n"
                    };
                    writer.write_all(reply).await.unwrap();
                }
            });
        }
    });
    (port, mails)
}

/// cedar's per-key stream of finished runs: the notifications after `after`, then the end of the
/// stream (cedar would keep it open; the chat server reconnects from its cursor either way).
async fn notifications(State(s): State<Shared>, headers: HeaderMap, axum::extract::RawQuery(q): axum::extract::RawQuery) -> axum::response::Response {
    if !authorized(&headers) {
        return cedar_error(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid API key");
    }
    let after: i64 = q.as_deref().and_then(|q| q.strip_prefix("after=")).and_then(|n| n.parse().ok()).unwrap_or(0);
    let mut body = String::from(": keep-alive\n\n");
    for n in s.notifications.lock().unwrap().iter().filter(|n| n["id"].as_i64().unwrap() > after) {
        body.push_str(&format!("id: {}\nevent: {}\ndata: {n}\n\n", n["id"], n["type"].as_str().unwrap()));
    }
    ([("content-type", "text/event-stream")], body).into_response()
}

async fn events(Path(id): Path<Uuid>, axum::extract::RawQuery(q): axum::extract::RawQuery) -> impl IntoResponse {
    let body = format!(
        "id: 1\nevent: assistant.text\ndata: {{\"seq\":1,\"type\":\"assistant.text\",\"data\":{{\"text\":\"for {id} {}\"}}}}\n\n\
         id: 2\nevent: run.completed\ndata: {{\"seq\":2,\"type\":\"run.completed\",\"data\":{{}}}}\n\n",
        q.unwrap_or_default()
    );
    ([("content-type", "text/event-stream")], body)
}

/// Every run's workspace holds `out/report.pdf`, unless the run has been marked `expired`.
async fn list_files(State(s): State<Shared>, Path(id): Path<Uuid>) -> axum::response::Response {
    if !s.runs.lock().unwrap().contains_key(&id) {
        return cedar_error(StatusCode::NOT_FOUND, "not_found", "run not found");
    }
    if s.expired.lock().unwrap().contains(&id) {
        return cedar_error(StatusCode::NOT_FOUND, "not_found", "the run's workspace no longer exists");
    }
    Json(json!({
        "workspace_id": id,
        "files": [{ "path": "out/report.pdf", "size": 4, "modified_at": "2026-10-11T12:00:00Z" }],
        "truncated": false,
    }))
    .into_response()
}

async fn download_file(State(s): State<Shared>, Path((id, path)): Path<(Uuid, String)>) -> axum::response::Response {
    s.downloads.lock().unwrap().push((id, path.clone()));
    if path != "out/report.pdf" {
        return cedar_error(StatusCode::NOT_FOUND, "not_found", "no such file in the workspace");
    }
    (
        [
            ("content-type", "application/pdf"),
            ("content-disposition", "attachment; filename=\"report.pdf\"; filename*=UTF-8''report.pdf"),
        ],
        "%PDF",
    )
        .into_response()
}

/// What the fake cedar has registered: `model-a` (the default, with reasoning levels) and `model-b`.
fn registry() -> Vec<Value> {
    vec![
        json!({ "id": "model-a", "label": "Model A", "model": "org/model-a", "reasoning_levels": ["none", "high"], "default_reasoning": null, "multimodal": true, "max_tokens": 16384, "is_default": true }),
        json!({ "id": "model-b", "label": "Model B", "model": "org/model-b", "reasoning_levels": [], "default_reasoning": null, "multimodal": false, "max_tokens": 16384, "is_default": false }),
    ]
}

async fn models() -> Json<Value> {
    Json(json!({ "models": registry() }))
}

async fn tools() -> Json<Value> {
    Json(json!({ "groups": [
        { "id": "web_search", "label": "Web search", "available": false, "reason": "Needs a Brave Search API key", "tools": [] },
        { "id": "weather", "label": "Weather", "available": true, "tools": [{ "name": "weather_forecast", "description": "d", "writes": false }] },
        { "id": "gmail", "label": "Gmail", "available": true, "discovered": false, "tools": [] },
    ] }))
}

async fn admin(s: &Shared, route: &str, body: Value) -> Option<axum::response::Response> {
    s.admin_calls.lock().unwrap().push((route.to_string(), body));
    (*s.not_admin.lock().unwrap()).then(|| cedar_error(StatusCode::FORBIDDEN, "forbidden", "not an admin key"))
}

async fn integrations(State(s): State<Shared>) -> axum::response::Response {
    if let Some(refused) = admin(&s, "get", Value::Null).await {
        return refused;
    }
    Json(json!({ "storage": true, "brave": { "configured": false, "source": null }, "google": { "enabled": true, "configured": false } })).into_response()
}

async fn put_brave(State(s): State<Shared>, Json(body): Json<Value>) -> axum::response::Response {
    if let Some(refused) = admin(&s, "put_brave", body.clone()).await {
        return refused;
    }
    if body["api_key"] == "bad" {
        return cedar_error(StatusCode::BAD_REQUEST, "bad_request", "Brave Search rejected the API key (401 Unauthorized)");
    }
    Json(json!({ "brave": { "configured": true, "source": "ui", "last4": "1234" } })).into_response()
}

async fn delete_integration(s: Shared, which: &str) -> axum::response::Response {
    if let Some(refused) = admin(&s, &format!("delete_{which}"), Value::Null).await {
        return refused;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn authorize(State(s): State<Shared>, Json(body): Json<Value>) -> axum::response::Response {
    if let Some(refused) = admin(&s, "authorize", body.clone()).await {
        return refused;
    }
    Json(json!({ "url": format!("https://accounts.example/auth?state=st-1&redirect_uri={}", body["redirect_uri"].as_str().unwrap()) })).into_response()
}

async fn callback(State(s): State<Shared>, Json(body): Json<Value>) -> axum::response::Response {
    if let Some(refused) = admin(&s, "callback", body.clone()).await {
        return refused;
    }
    if body["state"] != "st-1" {
        return cedar_error(StatusCode::BAD_REQUEST, "bad_request", "this Google sign-in link is unknown, used, or expired; start again");
    }
    Json(json!({ "account": "ana@example.com" })).into_response()
}

struct Harness {
    app: Router,
    state: AppState,
    cedar: Shared,
}

impl Harness {
    async fn new(extra: &[(&str, &str)]) -> Self {
        let cedar = Shared::default();
        let fake = Router::new()
            .route("/v1/runs", post(create_run))
            .route("/v1/runs/{id}", get(get_run).delete(delete_run))
            .route("/v1/runs/{id}/cancel", post(cancel_run))
            .route(
                "/v1/runs/{id}/plan",
                post(|State(s): State<Shared>, Path(id): Path<Uuid>, Json(b): Json<Value>| answer(s, id, "plan".into(), b)),
            )
            .route(
                "/v1/runs/{id}/approvals/{aid}",
                post(|State(s): State<Shared>, Path((id, aid)): Path<(Uuid, String)>, Json(b): Json<Value>| {
                    answer(s, id, format!("approvals/{aid}"), b)
                }),
            )
            .route("/v1/runs/{id}/events", get(events))
            .route("/v1/events", get(notifications))
            .route("/v1/runs/{id}/files", get(list_files))
            .route("/v1/runs/{id}/files/{*path}", get(download_file))
            .route("/v1/compact", post(compact))
            .route("/v1/tools", get(tools))
            .route("/v1/models", get(models))
            .route("/v1/defaults", get(|| async { Json(json!({ "model": "org/model-a", "model_id": "model-a", "system": "You are helpful.", "max_turns_cap": 25 })) }))
            .route(
                "/v1/admin/models",
                get(|State(s): State<Shared>| async move {
                    if let Some(refused) = admin(&s, "get_models", Value::Null).await {
                        return refused;
                    }
                    Json(json!({ "models": registry() })).into_response()
                }),
            )
            .route(
                "/v1/admin/models/{id}",
                axum::routing::put(|State(s): State<Shared>, Path(id): Path<String>, Json(body): Json<Value>| async move {
                    if let Some(refused) = admin(&s, &format!("put_model:{id}"), body.clone()).await {
                        return refused;
                    }
                    if body["base_url"] == "http://127.0.0.1:1" {
                        return cedar_error(StatusCode::BAD_REQUEST, "bad_request", "could not reach the model server");
                    }
                    Json(json!({ "id": id, "base_url": body["base_url"], "model": body["model"], "api_key_set": body.get("api_key").is_some(), "listed_models": ["m-1"] })).into_response()
                })
                .delete(|State(s): State<Shared>, Path(id): Path<String>| async move { delete_integration(s, &format!("model:{id}")).await }),
            )
            .route(
                "/v1/admin/settings",
                get(|State(s): State<Shared>| async move {
                    if let Some(refused) = admin(&s, "get_settings", Value::Null).await {
                        return refused;
                    }
                    Json(json!({ "max_image_side": null, "defaults": { "max_image_side": 1568 } })).into_response()
                })
                .put(|State(s): State<Shared>, Json(body): Json<Value>| async move {
                    if let Some(refused) = admin(&s, "put_settings", body.clone()).await {
                        return refused;
                    }
                    if body["max_image_side"] == 1 {
                        return cedar_error(StatusCode::BAD_REQUEST, "bad_request", "max_image_side must be between 256 and 8192");
                    }
                    Json(json!({ "max_image_side": body["max_image_side"], "defaults": { "max_image_side": 1568 } })).into_response()
                }),
            )
            .route(
                "/v1/admin/server-models",
                post(|State(s): State<Shared>, Json(body): Json<Value>| async move {
                    if let Some(refused) = admin(&s, "server_models", body.clone()).await {
                        return refused;
                    }
                    if body["base_url"] == "http://127.0.0.1:1" {
                        return cedar_error(StatusCode::BAD_REQUEST, "bad_request", "could not reach the model server");
                    }
                    Json(json!({ "base_url": body["base_url"], "models": ["m-1", "m-2"] })).into_response()
                }),
            )
            .route("/v1/admin/integrations", get(integrations))
            .route(
                "/v1/admin/integrations/brave",
                axum::routing::put(put_brave).delete(|State(s): State<Shared>| delete_integration(s, "brave")),
            )
            .route("/v1/admin/integrations/google", axum::routing::delete(|State(s): State<Shared>| delete_integration(s, "google")))
            .route("/v1/admin/integrations/google/authorize", post(authorize))
            .route("/v1/admin/integrations/google/callback", post(callback))
            .route(
                "/v1/admin/marketplace/{name}/install",
                post(|State(s): State<Shared>, Path(name): Path<String>, Json(body): Json<Value>| async move {
                    if let Some(refused) = admin(&s, &format!("install_{name}"), body.clone()).await {
                        return refused;
                    }
                    Json(json!({ "name": name, "source": "marketplace" })).into_response()
                }),
            )
            .route(
                "/v1/admin/tools/{name}/secrets/{secret}",
                axum::routing::put(|State(s): State<Shared>, Path((name, secret)): Path<(String, String)>, Json(body): Json<Value>| async move {
                    if let Some(refused) = admin(&s, &format!("secret_{name}_{secret}"), body.clone()).await {
                        return refused;
                    }
                    Json(json!({ "tools": [] })).into_response()
                }),
            )
            // Not cedar, but on the same port: the services the chat's notifications go to.
            .route("/pushover", post(pushover))
            .route("/push/{name}", post(push_service))
            .route("/apns/3/device/{token}", post(apns_service))
            .route("/fcm/v1/projects/{project}/{action}", post(fcm_service))
            .route("/google/token", post(google_token))
            .route("/github/releases/latest", get(latest_release))
            .with_state(cedar.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

        let pushover_url = format!("{url}/pushover");
        let apns_url = format!("{url}/apns");
        let fcm_url = format!("{url}/fcm");
        let google_token_url = format!("{url}/google/token");
        let update_url = format!("{url}/github/releases/latest");
        let mut env: HashMap<String, String> = [
            ("CEDAR_URL", url.as_str()),
            ("CEDAR_API_KEY", CEDAR_KEY),
            ("CHAT_PUSHOVER_URL", pushover_url.as_str()),
            ("CHAT_APNS_URL", apns_url.as_str()),
            ("CHAT_FCM_URL", fcm_url.as_str()),
            ("CHAT_GOOGLE_TOKEN_URL", google_token_url.as_str()),
            ("CHAT_UPDATE_URL", update_url.as_str()),
        ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for (k, v) in extra {
            env.insert(k.to_string(), v.to_string());
        }
        let cfg = Config::from_lookup(|k| env.get(k).cloned()).unwrap();
        let state = AppState::new(cfg, Store::connect_memory().await.unwrap()).unwrap();
        Self { app: api::router(state.clone()), state, cedar }
    }

    /// A conversation with one finished exchange; returns its id and the run's id.
    async fn finished_conversation(&self, prompt: &str, answer: &str) -> (String, String) {
        let (_, conv) = self.json(Method::POST, "/api/conversations", Some(json!({}))).await;
        let cid = conv["id"].as_str().unwrap().to_string();
        let (_, turn) = self.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": prompt }))).await;
        let run_id = turn["run_id"].as_str().unwrap().to_string();
        self.finish(&run_id, "completed", Some(answer));
        (cid, run_id)
    }

    async fn call(&self, method: Method, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, String) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let (status, _, body) = self.call_full(req).await;
        (status, body)
    }

    async fn call_full(&self, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        (parts.status, parts.headers, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn json(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, text) = self.call(method, uri, None, body).await;
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    fn finish(&self, run_id: &str, status: &str, text: Option<&str>) {
        let id = Uuid::parse_str(run_id).unwrap();
        let mut runs = self.cedar.runs.lock().unwrap();
        let run = runs.get_mut(&id).unwrap();
        run["status"] = json!(status);
        run["final_text"] = json!(text);
    }

    /// End a run the way cedar does: its state, and a notification on the key's event stream.
    fn finish_with(&self, run_id: &str, status: &str, text: Option<&str>, stop_reason: Option<&str>) -> i64 {
        self.finish(run_id, status, text);
        let mut runs = self.cedar.runs.lock().unwrap();
        let run = runs.get_mut(&Uuid::parse_str(run_id).unwrap()).unwrap();
        run["stop_reason"] = json!(stop_reason);
        if status == "failed" {
            run["error"] = json!("it broke");
        }
        drop(runs);
        self.notify(run_id, status, stop_reason)
    }

    /// A notification for a run, as cedar's stream carries it; returns its id.
    fn notify(&self, run_id: &str, status: &str, stop_reason: Option<&str>) -> i64 {
        let mut notifications = self.cedar.notifications.lock().unwrap();
        let id = notifications.len() as i64 + 1;
        notifications.push(json!({
            "id": id, "type": format!("run.{status}"), "run_id": run_id, "seq": 2, "status": status,
            "stop_reason": stop_reason, "error": if status == "failed" { json!("it broke") } else { Value::Null },
            "parent_id": null, "finished_at": "2026-10-17T09:30:00Z",
        }));
        id
    }

    /// Follow cedar's stream once, from the saved cursor, until the fake closes it.
    async fn sync(&self) -> usize {
        notify::sync(&self.state, &CancellationToken::new()).await.unwrap()
    }

    /// The fake services' base URL (the same port as the fake cedar).
    fn base_url(&self) -> String {
        self.state.cfg.cedar_url.clone()
    }

    /// Open `GET /api/events` and return its body, to read notices from.
    async fn notice_stream(&self) -> Body {
        let req = Request::builder().uri("/api/events").body(Body::empty()).unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        resp.into_body()
    }
}

/// The next notice on an `/api/events` body, within a second.
async fn next_notice(body: &mut Body) -> Value {
    let mut buf = String::new();
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), body.frame()).await.expect("a notice in time").unwrap().unwrap();
        if let Ok(data) = frame.into_data() {
            buf.push_str(std::str::from_utf8(&data).unwrap());
        }
        if let Some(end) = buf.find("\n\n") {
            let block: String = buf.drain(..end + 2).collect();
            let data: String = block.lines().filter_map(|l| l.strip_prefix("data:")).map(|l| l.trim_start()).collect();
            if !data.is_empty() {
                return serde_json::from_str(&data).unwrap();
            }
        }
    }
}

#[tokio::test]
async fn follow_up_messages_continue_the_previous_run() {
    let h = Harness::new(&[]).await;
    let (status, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let cid = conv["id"].as_str().unwrap();
    assert!(conv["title"].is_null());

    let (status, t1) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "What is 2+2?" }))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(t1["status"], "queued");
    assert_eq!(h.cedar.prompts.lock().unwrap()[0], "What is 2+2?");

    // A second message while the first is still going is refused.
    let (status, _) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "hurry" }))).await;
    assert_eq!(status, StatusCode::CONFLICT);

    h.finish(t1["run_id"].as_str().unwrap(), "completed", Some("It is 4."));
    let (status, _) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "Times 3?" }))).await;
    assert_eq!(status, StatusCode::CREATED);
    // cedar carries the conversation and workspace; the prompt is just the new message.
    assert_eq!(h.cedar.prompts.lock().unwrap()[1], "Times 3?");
    assert_eq!(h.cedar.parents.lock().unwrap()[1].as_deref(), t1["run_id"].as_str());
    assert_eq!(h.cedar.parents.lock().unwrap()[0], None);

    let (_, detail) = h.json(Method::GET, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(detail["conversation"]["title"], "What is 2+2?");
    let turns = detail["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0]["status"], "completed");
    assert_eq!(turns[0]["final_text"], "It is 4.");
    assert_eq!(turns[1]["prompt"], "Times 3?");

    let (_, list) = h.json(Method::GET, "/api/conversations", None).await;
    assert_eq!(list["conversations"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn history_is_resent_as_text_when_the_previous_run_is_gone() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let msgs = format!("/api/conversations/{}/messages", conv["id"].as_str().unwrap());
    let (_, t1) = h.json(Method::POST, &msgs, Some(json!({ "content": "What is 2+2?" }))).await;
    let run_id = t1["run_id"].as_str().unwrap();
    h.finish(run_id, "completed", Some("It is 4."));
    // Refresh while cedar still has the run, then lose it.
    h.json(Method::GET, &format!("/api/conversations/{}", conv["id"].as_str().unwrap()), None).await;
    h.cedar.runs.lock().unwrap().remove(&Uuid::parse_str(run_id).unwrap());

    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "Times 3?" }))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(h.cedar.parents.lock().unwrap().last().unwrap(), &None);
    let prompt = h.cedar.prompts.lock().unwrap().last().unwrap().clone();
    assert!(prompt.contains("<user>\nWhat is 2+2?\n</user>"));
    assert!(prompt.contains("<assistant>\nIt is 4.\n</assistant>"));
    assert!(prompt.ends_with("Times 3?"));
}

// -- attachments ------------------------------------------------------------------------------

fn png() -> Value {
    json!({ "name": "dot.png", "media_type": "image/png", "data": "iVBORw0KGgo=" })
}

#[tokio::test]
async fn attachments_are_forwarded_to_a_model_that_takes_them() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let cid = conv["id"].as_str().unwrap();
    let msgs = format!("/api/conversations/{cid}/messages");
    // Well over the 128 KiB that every other route allows.
    let big = json!({ "name": "scan.pdf", "media_type": "application/pdf", "data": "A".repeat(400_000) });
    let (status, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "what are these?", "attachments": [png(), big] }))).await;
    assert_eq!(status, StatusCode::CREATED, "{turn}");
    let sent = h.cedar.bodies.lock().unwrap().last().cloned().unwrap();
    assert_eq!(sent["prompt"], "what are these?");
    assert_eq!(sent["attachments"], json!([png(), big]));
    let expected = json!([
        { "name": "dot.png", "media_type": "image/png", "size": 8 },
        { "name": "scan.pdf", "media_type": "application/pdf", "size": 300_000 },
    ]);
    assert_eq!(turn["attachments"], expected, "the turn remembers the files, not their bytes");
    let (_, detail) = h.json(Method::GET, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(detail["turns"][0]["attachments"], expected);

    // A follow-up without files sends none, and the turn says so.
    h.finish(turn["run_id"].as_str().unwrap(), "completed", Some("a dot and a scan"));
    let (status, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "thanks" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{turn}");
    assert!(h.cedar.bodies.lock().unwrap().last().unwrap().get("attachments").is_none());
    assert_eq!(turn["attachments"], json!([]));
}

#[tokio::test]
async fn images_are_refused_by_cedar_for_a_model_that_takes_none() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let msgs = format!("/api/conversations/{}/messages", conv["id"].as_str().unwrap());
    // model-b is registered without "accepts images": cedar's verdict is relayed.
    let (status, err) = h.json(Method::POST, &msgs, Some(json!({ "content": "look", "model": "model-b", "attachments": [png()] }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("does not accept images"), "{err}");
    assert!(h.cedar.bodies.lock().unwrap().is_empty(), "no run was started");
    // An empty list is the same as none.
    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "look", "model": "model-b", "attachments": [] }))).await;
    assert_eq!(status, StatusCode::CREATED);
    // A PDF goes to any model.
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let msgs = format!("/api/conversations/{}/messages", conv["id"].as_str().unwrap());
    let pdf = json!({ "name": "scan.pdf", "media_type": "application/pdf", "data": "JVBERi0xLjQ=" });
    let (status, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "read", "model": "model-b", "attachments": [pdf] }))).await;
    assert_eq!(status, StatusCode::CREATED, "{turn}");
    assert_eq!(h.cedar.bodies.lock().unwrap().last().unwrap()["attachments"][0]["name"], "scan.pdf");
}

#[tokio::test]
async fn attachment_messages_are_validated() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let msgs = format!("/api/conversations/{}/messages", conv["id"].as_str().unwrap());
    let file = |name: &str, media_type: &str, data: &str| json!({ "name": name, "media_type": media_type, "data": data });
    for (attachments, needle) in [
        (json!(vec![png(); 11]), "at most 10"),
        (json!([file("  ", "image/png", "aGk=")]), "name must not be empty"),
        (json!([file("a.txt", "text/plain", "aGk=")]), "only PNG"),
        (json!([file("a.png", "image/png", "")]), "empty"),
        (json!([file("a.png", "image/png", "aGk")]), "not valid base64"),
        (json!([file("a.png", "image/png", "a Gk=")]), "not valid base64"),
        (json!([file("a.png", "image/png", &"A".repeat(28 << 20))]), "larger than 20 MiB"),
        (json!([{ "name": "a.png", "media_type": "image/png", "data": "aGk=", "size": 3 }]), "size"),
    ] {
        let (status, err) = h.json(Method::POST, &msgs, Some(json!({ "content": "look", "attachments": attachments }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
        assert!(err["error"]["message"].as_str().unwrap().contains(needle), "{err} should mention {needle}");
    }
    // Over the route's own body limit.
    let (status, err) = h.json(Method::POST, &msgs, Some(json!({ "content": "look", "attachments": [file("a.png", "image/png", &"A".repeat(49 << 20))] }))).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{err}");
    assert_eq!(err["error"]["code"], "payload_too_large");
    assert!(h.cedar.bodies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn attachments_survive_the_history_resend_fallback() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let msgs = format!("/api/conversations/{}/messages", conv["id"].as_str().unwrap());
    let (_, t1) = h.json(Method::POST, &msgs, Some(json!({ "content": "What is 2+2?" }))).await;
    let run_id = t1["run_id"].as_str().unwrap();
    h.finish(run_id, "completed", Some("It is 4."));
    h.json(Method::GET, &format!("/api/conversations/{}", conv["id"].as_str().unwrap()), None).await;
    h.cedar.runs.lock().unwrap().remove(&Uuid::parse_str(run_id).unwrap());

    let (status, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "And this?", "attachments": [png()] }))).await;
    assert_eq!(status, StatusCode::CREATED, "{turn}");
    let sent = h.cedar.bodies.lock().unwrap().last().cloned().unwrap();
    assert!(sent.get("parent_id").is_none());
    assert!(sent["prompt"].as_str().unwrap().contains("<user>\nWhat is 2+2?\n</user>"));
    assert_eq!(sent["attachments"], json!([png()]));
    assert_eq!(turn["attachments"][0]["name"], "dot.png");
}

#[tokio::test]
async fn events_are_relayed_from_cedar() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let cid = conv["id"].as_str().unwrap();
    let (_, turn) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "hi" }))).await;
    let (status, body) = h.call(Method::GET, &format!("/api/turns/{}/events?after=3", turn["id"].as_str().unwrap()), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(&format!("for {} after=3", turn["run_id"].as_str().unwrap())));
    assert!(body.contains("event: run.completed"));

    let (status, _) = h.call(Method::GET, &format!("/api/turns/{}/events", Uuid::new_v4()), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cancel_rename_and_delete() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({ "title": "  Mine  " }))).await;
    assert_eq!(conv["title"], "Mine");
    let cid = conv["id"].as_str().unwrap();
    let (_, turn) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "long job" }))).await;
    let tid = turn["id"].as_str().unwrap();

    let (status, _) = h.json(Method::DELETE, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "active conversations can't be deleted");

    let (status, _) = h.json(Method::POST, &format!("/api/turns/{tid}/cancel"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, _) = h.json(Method::POST, &format!("/api/turns/{tid}/cancel"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, renamed) = h.json(Method::PATCH, &format!("/api/conversations/{cid}"), Some(json!({ "title": "Renamed" }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["title"], "Renamed");
    let (status, _) = h.json(Method::PATCH, &format!("/api/conversations/{cid}"), Some(json!({ "title": " " }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = h.json(Method::DELETE, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(h.cedar.deleted.lock().unwrap().len(), 1);
    let (status, _) = h.json(Method::GET, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn validation_and_upstream_errors() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let msgs = format!("/api/conversations/{}/messages", conv["id"].as_str().unwrap());

    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "   " }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // cedar's verdict on the model and the level is relayed as it is.
    let (status, err) = h.json(Method::POST, &msgs, Some(json!({ "content": "hi", "model": "model-z" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("not registered"), "{err}");
    let (status, err) = h.json(Method::POST, &msgs, Some(json!({ "content": "hi", "reasoning": "max" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("not offered"), "{err}");
    let (status, err) = h.json(Method::POST, &msgs, Some(json!({ "content": "TOO MANY" }))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(err["error"]["message"], "at most 2 runs");
    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "hi", "model": "model-b" }))).await;
    assert_eq!(status, StatusCode::CREATED);

    let (_, cfg) = h.json(Method::GET, "/api/config", None).await;
    assert_eq!(cfg, json!({ "auth_required": false, "admin_enabled": false }));
    // The pickers take cedar's registry, relayed.
    let (status, listed) = h.json(Method::GET, "/api/models", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["models"][0]["id"], "model-a");
    assert_eq!(listed["models"][1]["reasoning_levels"], json!([]));
}

#[tokio::test]
async fn a_wrong_cedar_key_is_not_reported_as_the_browsers_fault() {
    let h = Harness::new(&[("CEDAR_API_KEY", "wrong")]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let (status, err) =
        h.json(Method::POST, &format!("/api/conversations/{}/messages", conv["id"].as_str().unwrap()), Some(json!({ "content": "hi" }))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(err["error"]["message"], "cedar rejected CEDAR_API_KEY");
}

#[tokio::test]
async fn access_token_guards_the_api_but_not_the_page() {
    let h = Harness::new(&[("CHAT_ACCESS_TOKEN", "letmein")]).await;
    assert_eq!(h.call(Method::GET, "/api/conversations", None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(h.call(Method::GET, "/api/conversations", Some("nope"), None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(h.call(Method::GET, "/api/conversations", Some("letmein"), None).await.0, StatusCode::OK);

    let (status, cfg) = h.call(Method::GET, "/api/config", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cfg.contains("\"auth_required\":true"));
    let (status, page) = h.call(Method::GET, "/", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("<script src=\"/ui-pref.js\">"));
}

#[tokio::test]
async fn pairing_waits_for_https() {
    let h = Harness::new(&[("CHAT_ACCESS_TOKEN", "letmein"), ("CHAT_TAILSCALE_CLI", "/nonexistent/tailscale")]).await;
    assert_eq!(h.call(Method::GET, "/api/pairing", None, None).await.0, StatusCode::UNAUTHORIZED);
    let (status, body) = h.call(Method::GET, "/api/pairing", Some("letmein"), None).await;
    assert_eq!(status, StatusCode::OK);
    let p: Value = serde_json::from_str(&body).unwrap();
    // CHAT_PUBLIC_URL is the plain-http default, which the iOS app can't use.
    assert_eq!(p["server"], Value::Null);
    assert_eq!(p["link"], Value::Null);
    assert_eq!(p["qr_svg"], Value::Null);
    assert_eq!(p["on_tailnet"], false);
    assert_eq!(p["access_token"], "letmein");
    assert_eq!(p["platform"], std::env::consts::OS);
    assert_eq!(p["tailscale"]["state"], "unavailable");
    assert!(p["tailscale"]["detail"].as_str().unwrap().contains("/nonexistent/tailscale"));
}

/// A stand-in for the tailscale CLI that prints `status`, and fails like a logged-out one when told.
#[cfg(unix)]
fn fake_tailscale(name: &str, status: &str, exit: i32) -> String {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("cedar-chat-ts-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("tailscale");
    std::fs::write(&bin, format!("#!/bin/sh\n[ \"$1 $2\" = \"status --json\" ] || exit 9\ncat <<'EOF'\n{status}\nEOF\nexit {exit}\n")).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin.to_string_lossy().into_owned()
}

#[cfg(unix)]
#[tokio::test]
async fn pairing_on_the_tailnet_gives_the_code() {
    let ready = r#"{"BackendState": "Running", "Self": {"DNSName": "box.tail1234.ts.net."}, "CertDomains": ["box.tail1234.ts.net"]}"#;
    let cli = fake_tailscale("ready", ready, 0);
    let h = Harness::new(&[("CHAT_ACCESS_TOKEN", "tok&en"), ("CHAT_PUBLIC_URL", "https://box.tail1234.ts.net"), ("CHAT_TAILSCALE_CLI", &cli)]).await;
    let (status, body) = h.call(Method::GET, "/api/pairing", Some("tok&en"), None).await;
    assert_eq!(status, StatusCode::OK);
    let p: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(p["server"], "https://box.tail1234.ts.net");
    assert_eq!(p["on_tailnet"], true);
    assert_eq!(p["tailscale"]["state"], "ready");
    assert_eq!(p["tailscale"]["name"], "box.tail1234.ts.net");
    // The link the iOS app parses (ConnectionCode.swift), with the token encoded.
    assert_eq!(p["link"], "cedar://connect?server=https%3A%2F%2Fbox.tail1234.ts.net&token=tok%26en");
    assert!(p["qr_svg"].as_str().unwrap().starts_with("<svg"));

    // A logged-out Tailscale prints its status and exits non-zero; the page still learns why.
    let cli = fake_tailscale("logged-out", r#"{"BackendState": "NeedsLogin", "Self": {"DNSName": ""}}"#, 1);
    let h = Harness::new(&[("CHAT_PUBLIC_URL", "https://chat.example:8443"), ("CHAT_TAILSCALE_CLI", &cli)]).await;
    let (_, p) = h.json(Method::GET, "/api/pairing", None).await;
    assert_eq!(p["tailscale"]["state"], "logged_out");
    assert!(p["tailscale"]["detail"].as_str().unwrap().contains("not logged in"));
    // The user's own HTTPS address works too, just not as a tailnet one; without a token the
    // code carries an empty one.
    assert_eq!(p["server"], "https://chat.example:8443");
    assert_eq!(p["on_tailnet"], false);
    assert_eq!(p["access_token"], Value::Null);
    assert_eq!(p["link"], "cedar://connect?server=https%3A%2F%2Fchat.example%3A8443&token=");
}

async fn fetch(h: &Harness, uri: &str) -> (StatusCode, HeaderMap, String) {
    h.call_full(Request::builder().uri(uri).body(Body::empty()).unwrap()).await
}

#[tokio::test]
async fn both_uis_are_served() {
    let h = Harness::new(&[]).await;

    for ui in ["v1", "v2", "v3", "v4"] {
        // The page is reachable with and without the trailing slash and loads its own assets.
        for uri in [format!("/{ui}"), format!("/{ui}/")] {
            let (status, headers, page) = fetch(&h, &uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert_eq!(headers["content-type"], "text/html; charset=utf-8");
            assert!(page.contains(&format!("data-ui=\"{ui}\"")), "{uri} names itself");
            assert!(page.contains(&format!("<script src=\"/{ui}/app.js\">")));
            assert!(page.contains(&format!("href=\"/{ui}/style.css\"")));
            assert!(page.contains("<script src=\"/ui-pref.js\">"));
            assert!(page.contains("<script src=\"/file-viewer.js\">"), "{uri} loads the file viewer");
            assert!(page.contains("<script src=\"/image-view.js\">"), "{uri} loads the image view");
            assert!(page.contains("href=\"/image-view.css\""), "{uri} loads the image styles");
            assert!(
                page.find("/image-view.js") < page.find("/file-viewer.js"),
                "{uri} loads the image view before the viewer, which asks it which files are images"
            );
            assert!(page.contains("<script src=\"/map-view.js\">"), "{uri} loads the map view");
            assert!(page.contains("href=\"/map-view.css\""), "{uri} loads the map styles");
            assert!(page.contains("<script src=\"/recipe-view.js\">"), "{uri} loads the recipe view");
            assert!(page.contains("href=\"/recipe-view.css\""), "{uri} loads the recipe styles");
            assert!(page.contains("<script src=\"/clarify-view.js\">"), "{uri} loads the question form");
            assert!(page.contains("href=\"/clarify-view.css\""), "{uri} loads the question form styles");
            assert!(page.find("/map-view.js") < page.find("/transit-view.js"), "{uri} loads the trip view after the map view it draws with");
            assert!(page.contains("href=\"/transit-view.css\""), "{uri} loads the trip styles");
            assert!(page.contains("<script src=\"/stock-view.js\">"), "{uri} loads the stock view");
            assert!(page.contains("href=\"/stock-view.css\""), "{uri} loads the stock styles");
            assert!(page.contains("<script src=\"/chart-view.js\">"), "{uri} loads the chart view");
            assert!(page.contains("href=\"/chart-view.css\""), "{uri} loads the chart styles");
            assert!(page.contains("<script src=\"/models.js\">"), "{uri} loads the model registry script");
            assert!(
                page.find("/models.js") < page.find("/schedule.js") && page.find("/models.js") < page.find("src=\"/settings.js\""),
                "{uri} loads the models before the scripts that fill the pickers from them"
            );
            assert!(
                page.find("src=\"/i18n.js\"") < page.find("src=\"/i18n/common.js\"")
                    && page.find("src=\"/i18n/common.js\"") < page.find("src=\"/ui-pref.js\""),
                "{uri} loads the translations before every script that shows text"
            );
            assert!(
                page.find("src=\"/settings-shell.js\"") < page.find("src=\"/language.js\""),
                "{uri} loads the language picker after the Settings frame it registers with"
            );
            assert!(page.contains("id=\"reasoning\""), "{uri} has a reasoning picker");
            assert!(page.contains("href=\"/models.css\""), "{uri} loads the send menu styles");
            assert!(page.contains("<script src=\"/schedule.js\">"), "{uri} loads the scheduling script");
            assert!(page.contains("href=\"/schedule.css\""), "{uri} loads the scheduling styles");
            assert!(page.contains("<script src=\"/star.js\">"), "{uri} loads the starring script");
            assert!(page.contains("href=\"/star.css\""), "{uri} loads the starring styles");
            assert!(page.contains("data-starred") && page.contains("data-star-toggle"), "{uri} has the Starred list and the star button");
            assert!(page.contains("<script src=\"/attach.js\">"), "{uri} loads the attachments script");
            assert!(page.contains("href=\"/attach.css\""), "{uri} loads the attachments styles");
            assert!(page.contains("<script src=\"/notify.js\">"), "{uri} loads the inbox script");
            assert!(page.contains("href=\"/notify.css\""), "{uri} loads the inbox styles");
            assert!(page.contains("id=\"inbox-btn\""), "{uri} has the inbox button");
            assert!(page.contains("id=\"notify-section\""), "{uri} has the Notifications settings");
            assert!(page.contains("<script src=\"/pairing.js\">"), "{uri} loads the iPhone settings");
            assert!(page.contains("id=\"iphone-section\""), "{uri} has the iPhone settings");
            assert!(page.find("/settings-shell.js") < page.find("/pairing.js"), "{uri} loads the Phone page after the frame it registers with");
            assert!(page.contains("id=\"settings-dialog\""), "{uri} has the Settings menu");
            assert!(page.contains("<script src=\"/settings-shell.js\">"), "{uri} loads the Settings dialog's frame");
            assert!(
                page.find("/ui-pref.js") < page.find("/settings-shell.js") && page.find("/settings-shell.js") < page.find("src=\"/settings.js\""),
                "{uri} loads the Settings frame after the UI choice and before the pages that register with it"
            );
            assert!(page.contains("id=\"schedules-section\""), "{uri} lists scheduled agents in the sidebar");
            assert!(page.contains("id=\"schedule-btn\""), "{uri} has a Schedule button");
        }
        let (status, headers, _) = fetch(&h, &format!("/{ui}/app.js")).await;
        assert_eq!((status, headers["content-type"].to_str().unwrap()), (StatusCode::OK, "text/javascript; charset=utf-8"));
        let (status, headers, _) = fetch(&h, &format!("/{ui}/style.css")).await;
        assert_eq!((status, headers["content-type"].to_str().unwrap()), (StatusCode::OK, "text/css; charset=utf-8"));
    }

    // The shared script holds the saved choice; `/` only sends the browser to the UI it names.
    let (status, headers, script) = fetch(&h, "/ui-pref.js").await;
    assert_eq!((status, headers["content-type"].to_str().unwrap()), (StatusCode::OK, "text/javascript; charset=utf-8"));
    assert!(script.contains("cedar-chat-ui"));

    // So does the file viewer, which both pages load before their own script.
    let (status, headers, script) = fetch(&h, "/file-viewer.js").await;
    assert_eq!((status, headers["content-type"].to_str().unwrap()), (StatusCode::OK, "text/javascript; charset=utf-8"));
    assert!(script.contains("cedarViewer"));

    // So does the map view for show_map results, with the Leaflet it loads on demand served from here.
    for (uri, content_type, marker) in [
        ("/map-view.js", "text/javascript; charset=utf-8", "cedarMap"),
        ("/map-view.css", "text/css; charset=utf-8", ".cedar-map"),
        ("/recipe-view.js", "text/javascript; charset=utf-8", "cedarRecipe"),
        ("/recipe-view.css", "text/css; charset=utf-8", ".cedar-recipe"),
        ("/clarify-view.js", "text/javascript; charset=utf-8", "cedarClarify"),
        ("/clarify-view.css", "text/css; charset=utf-8", ".cedar-clarify"),
        ("/transit-view.js", "text/javascript; charset=utf-8", "cedarTransit"),
        ("/transit-view.css", "text/css; charset=utf-8", ".cedar-trip"),
        ("/stock-view.js", "text/javascript; charset=utf-8", "cedarStocks"),
        ("/stock-view.css", "text/css; charset=utf-8", ".cedar-stocks"),
        ("/chart-view.js", "text/javascript; charset=utf-8", "cedarChart"),
        ("/chart-view.css", "text/css; charset=utf-8", ".cedar-chart"),
        ("/image-view.js", "text/javascript; charset=utf-8", "cedarImages"),
        ("/image-view.css", "text/css; charset=utf-8", ".cedar-image"),
        ("/vendor/leaflet/leaflet.js", "text/javascript; charset=utf-8", "Leaflet 1.9.4"),
        ("/vendor/leaflet/leaflet.css", "text/css; charset=utf-8", ".leaflet-container"),
        ("/models.js", "text/javascript; charset=utf-8", "cedarModels"),
        ("/models.css", "text/css; charset=utf-8", ".cedar-send-menu"),
        ("/schedule.js", "text/javascript; charset=utf-8", "cedarSchedule"),
        ("/schedule.css", "text/css; charset=utf-8", ".cedar-sched"),
        ("/star.js", "text/javascript; charset=utf-8", "cedarStar"),
        ("/star.css", "text/css; charset=utf-8", ".cedar-star"),
        ("/settings-shell.js", "text/javascript; charset=utf-8", "cedarSettingsShell"),
        ("/settings.js", "text/javascript; charset=utf-8", "cedarSettings"),
        ("/settings.css", "text/css; charset=utf-8", ".settings-dialog"),
        ("/attach.js", "text/javascript; charset=utf-8", "cedarAttach"),
        ("/attach.css", "text/css; charset=utf-8", ".cedar-attach"),
        ("/notify.js", "text/javascript; charset=utf-8", "cedarNotify"),
        ("/pairing.js", "text/javascript; charset=utf-8", "cedarPairing"),
        ("/notify.css", "text/css; charset=utf-8", ".cedar-inbox"),
        ("/sw.js", "text/javascript; charset=utf-8", "showNotification"),
    ] {
        let (status, headers, body) = fetch(&h, uri).await;
        assert_eq!((status, headers["content-type"].to_str().unwrap()), (StatusCode::OK, content_type), "{uri}");
        assert!(body.contains(marker), "{uri} is the expected file");
    }

    // The single-UI paths are gone.
    assert_eq!(fetch(&h, "/app.js").await.0, StatusCode::NOT_FOUND);
    assert_eq!(fetch(&h, "/style.css").await.0, StatusCode::NOT_FOUND);
}

/// The logo and icons are embedded from `docs/assets`; both pages link them, light and dark.
#[tokio::test]
async fn the_logo_and_icons_are_served_and_linked() {
    let h = Harness::new(&[]).await;
    let served = [
        ("/assets/cedar-mark.svg", "image/svg+xml", b"<svg".as_slice()),
        ("/assets/cedar-mark-on-dark.svg", "image/svg+xml", b"<svg"),
        ("/assets/favicon.svg", "image/svg+xml", b"<svg"),
        ("/assets/favicon-32.png", "image/png", b"\x89PNG"),
        ("/assets/favicon-16.png", "image/png", b"\x89PNG"),
        ("/assets/apple-touch-icon.png", "image/png", b"\x89PNG"),
    ];
    for (uri, content_type, magic) in served {
        let resp = h.app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        assert_eq!(resp.headers()["content-type"], content_type, "{uri}");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(bytes.starts_with(magic), "{uri} is the expected file");
    }

    for ui in ["v1", "v2"] {
        let (_, _, page) = fetch(&h, &format!("/{ui}/")).await;
        assert!(page.contains("rel=\"icon\" href=\"/assets/favicon.svg\""), "{ui} has an icon");
        assert!(page.contains("rel=\"apple-touch-icon\" href=\"/assets/apple-touch-icon.png\""), "{ui} has a touch icon");
        assert!(page.contains("srcset=\"/assets/cedar-mark-on-dark.svg\""), "{ui} swaps the mark in dark mode");
        assert!(page.contains("src=\"/assets/cedar-mark.svg\""), "{ui} shows the mark");
        // Every asset a page or script points at is one the server has.
        let (_, _, script) = fetch(&h, &format!("/{ui}/app.js")).await;
        for text in [&page, &script] {
            for (at, _) in text.match_indices("/assets/") {
                let path: String = text[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || "/.-_".contains(*c)).collect();
                assert!(served.iter().any(|(uri, ..)| *uri == path), "{ui} links {path}, which isn't served");
            }
        }
    }
}

#[tokio::test]
async fn workspace_files_are_listed_and_downloaded_through_the_latest_run() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let cid = conv["id"].as_str().unwrap().to_string();
    let files_url = format!("/api/conversations/{cid}/files");

    // No turns yet: an empty listing, not an error.
    let (status, listing) = h.json(Method::GET, &files_url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing, json!({ "workspace_id": null, "files": [], "truncated": false }));
    let (status, _) = h.call(Method::GET, &format!("{files_url}/out/report.pdf"), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let msgs = format!("/api/conversations/{cid}/messages");
    let (_, t1) = h.json(Method::POST, &msgs, Some(json!({ "content": "make a pdf" }))).await;
    h.finish(t1["run_id"].as_str().unwrap(), "completed", Some("done"));
    let (_, t2) = h.json(Method::POST, &msgs, Some(json!({ "content": "and a chart" }))).await;

    let (status, listing) = h.json(Method::GET, &files_url, None).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(listing["workspace_id"], t2["run_id"], "the latest run's workspace is used");
    assert_eq!(listing["files"][0]["path"], "out/report.pdf");

    let req = Request::builder().uri(format!("{files_url}/out/report.pdf")).body(Body::empty()).unwrap();
    let (status, headers, body) = h.call_full(req).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, "%PDF");
    assert_eq!(headers["content-type"], "application/pdf");
    assert_eq!(headers["content-disposition"], "attachment; filename=\"report.pdf\"; filename*=UTF-8''report.pdf");
    assert_eq!(headers["cache-control"], "no-store");
    let t2_run = Uuid::parse_str(t2["run_id"].as_str().unwrap()).unwrap();
    assert_eq!(*h.cedar.downloads.lock().unwrap(), [(t2_run, "out/report.pdf".to_string())]);

    // Names are passed on percent-encoded and decoded by cedar; cedar's errors are relayed.
    let (status, err) = h.json(Method::GET, &format!("{files_url}/it's%20gone.txt"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["message"], "no such file in the workspace");
    assert_eq!(h.cedar.downloads.lock().unwrap().last().unwrap().1, "it's gone.txt");

    h.cedar.expired.lock().unwrap().push(t2_run);
    let (status, err) = h.json(Method::GET, &files_url, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["message"], "the run's workspace no longer exists");

    // Like every other API route, files need the access token when one is configured.
    let guarded = Harness::new(&[("CHAT_ACCESS_TOKEN", "letmein")]).await;
    let (status, _) = guarded.call(Method::GET, &files_url, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// -- scheduling ---------------------------------------------------------------------------------

fn schedule_body(cid: &str) -> Value {
    json!({
        "title": "Morning digest", "prompt": "DO: summarize the news", "frequency": "weekdays",
        "time": "09:00", "timezone": "Europe/Berlin", "conversation_id": cid,
    })
}

fn ts(v: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(v.as_str().unwrap()).unwrap().with_timezone(&Utc)
}

#[tokio::test]
async fn a_conversation_is_compacted_through_its_latest_run_or_its_history() {
    let h = Harness::new(&[]).await;
    let (_, empty) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let (status, _) = h.json(Method::POST, &format!("/api/conversations/{}/compact", empty["id"].as_str().unwrap()), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing to compact yet");

    let (cid, run1) = h.finished_conversation("What is 2+2?", "It is 4.").await;
    let msgs = format!("/api/conversations/{cid}/messages");
    let (_, t2) = h.json(Method::POST, &msgs, Some(json!({ "content": "Times 3?", "model": "model-a" }))).await;
    let run2 = t2["run_id"].as_str().unwrap().to_string();
    let compact = format!("/api/conversations/{cid}/compact");
    let (status, _) = h.json(Method::POST, &compact, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "not while a response is being generated");
    h.finish(&run2, "completed", Some("12."));

    let (status, body) = h.json(Method::POST, &compact, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "prompt": "DO: Times 3?", "source": "run" }));
    let sent = h.cedar.compactions.lock().unwrap().last().unwrap().clone();
    assert_eq!(sent, json!({ "run_id": run2, "model": "model-a" }), "the latest run, with the conversation's model");
    // A model cedar no longer has is left to cedar's default.
    h.state.store.insert_turn(Uuid::parse_str(&cid).unwrap(), Uuid::parse_str(&run2).unwrap(), "x", "completed", Some("model-gone"), None, &[]).await.unwrap();
    let (status, _) = h.json(Method::POST, &compact, None).await;
    assert_eq!(status, StatusCode::OK);
    let sent = h.cedar.compactions.lock().unwrap().last().unwrap().clone();
    assert_eq!(sent.get("model"), None);

    // When cedar has lost the run, the remembered exchanges are compacted instead.
    h.cedar.runs.lock().unwrap().remove(&Uuid::parse_str(&run2).unwrap());
    let (status, body) = h.json(Method::POST, &compact, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "history");
    let prompt = body["prompt"].as_str().unwrap();
    assert!(prompt.starts_with("DO: <user>\nWhat is 2+2?\n</user>\n<assistant>\nIt is 4.\n</assistant>"), "{prompt}");
    assert!(prompt.contains("Times 3?") && prompt.ends_with("12.\n</assistant>"), "{prompt}");
    let _ = run1;

    // cedar's failure to compact is relayed.
    let (cid, _) = h.finished_conversation("SILENCE please", "ok").await;
    let (status, body) = h.json(Method::POST, &format!("/api/conversations/{cid}/compact"), None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
}

#[tokio::test]
async fn schedules_are_validated_created_updated_and_deleted() {
    let h = Harness::new(&[]).await;
    let (cid, _) = h.finished_conversation("hi", "hello").await;
    let post = |body: Value| h.json(Method::POST, "/api/schedules", Some(body));

    for (bad, why) in [
        (json!({ "prompt": "x", "frequency": "daily", "time": "09:00", "timezone": "UTC" }), "title"),
        (json!({ "title": "t", "prompt": "  ", "frequency": "daily", "time": "09:00", "timezone": "UTC" }), "prompt"),
        (json!({ "title": "t", "prompt": "x", "frequency": "hourly", "time": "09:00", "timezone": "UTC" }), "frequency"),
        (json!({ "title": "t", "prompt": "x", "frequency": "weekly", "time": "09:00", "timezone": "UTC" }), "weekday"),
        (json!({ "title": "t", "prompt": "x", "frequency": "daily", "time": "9 am", "timezone": "UTC" }), "time"),
        (json!({ "title": "t", "prompt": "x", "frequency": "daily", "time": "09:00", "timezone": "Mars/Base" }), "time zone"),
        (json!({ "title": "t", "prompt": "x", "frequency": "daily", "time": "09:00", "timezone": "UTC", "model": "model-z" }), "model"),
        (json!({ "title": "t", "prompt": "x", "frequency": "daily", "time": "09:00", "timezone": "UTC", "model": "model-b", "reasoning": "high" }), "reasoning level"),
        (json!({ "title": "t", "prompt": "x", "frequency": "daily", "time": "09:00", "timezone": "UTC", "reasoning": "max" }), "reasoning level"),
        (json!({ "title": "t", "prompt": "x", "frequency": "daily", "time": "09:00" }), "timezone"),
    ] {
        let (status, err) = post(bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {err}");
        assert!(err["error"]["message"].as_str().unwrap().contains(why), "{why}: {err}");
    }

    let before = Utc::now();
    let (status, s) = post(schedule_body(&cid)).await;
    assert_eq!(status, StatusCode::CREATED, "{s}");
    let sid = s["id"].as_str().unwrap().to_string();
    assert_eq!(s["title"], "Morning digest");
    assert_eq!(s["frequency"], "weekdays");
    assert_eq!(s["weekday"], Value::Null);
    assert_eq!(s["time_of_day"], "09:00");
    assert_eq!(s["timezone"], "Europe/Berlin");
    assert_eq!(s["enabled"], true);
    assert_eq!(s["model"], Value::Null);
    assert_eq!(s["reasoning"], Value::Null);
    assert_eq!(s["source_conversation_id"], cid);
    assert_eq!(s["last_run_at"], Value::Null);
    let next = ts(&s["next_run_at"]);
    assert!(next > before && next < before + Duration::days(4), "{next}");

    let (_, list) = h.json(Method::GET, "/api/schedules", None).await;
    assert_eq!(list["schedules"].as_array().unwrap().len(), 1);
    assert_eq!(list["schedules"][0]["id"], sid);
    assert_eq!(list["schedules"][0]["last_run"], Value::Null);

    // Updating the timing recomputes the next run; a weekly schedule needs its weekday.
    let patch_url = format!("/api/schedules/{sid}");
    let patch = |body: Value| h.json(Method::PATCH, &patch_url, Some(body));
    let (status, err) = patch(json!({ "frequency": "weekly" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    let (status, s) = patch(json!({ "frequency": "weekly", "weekday": 6, "time": "23:00", "model": "model-a", "reasoning": "high", "title": " Sunday digest " })).await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!((s["frequency"].as_str(), s["weekday"].as_u64(), s["time_of_day"].as_str()), (Some("weekly"), Some(6), Some("23:00")));
    assert_eq!(s["model"], "model-a");
    assert_eq!(s["reasoning"], "high");
    // A level is checked against the schedule's model, even when only the level changes.
    let (status, err) = patch(json!({ "reasoning": "max" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    let (status, err) = patch(json!({ "model": "model-b" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "model-b offers no level high: {err}");
    let (_, s) = patch(json!({ "model": "model-b", "reasoning": "" })).await;
    assert_eq!((s["model"].as_str(), s["reasoning"].as_str()), (Some("model-b"), None));
    let (_, s) = patch(json!({ "model": "model-a", "reasoning": "high" })).await;
    assert_eq!(s["reasoning"], "high");
    assert_eq!(s["title"], "Sunday digest");
    let next = ts(&s["next_run_at"]);
    assert_eq!(next.with_timezone(&chrono_tz::Europe::Berlin).format("%a %H:%M").to_string(), "Sun 23:00");

    // Pausing keeps the schedule; resuming skips what was missed.
    let (_, s) = patch(json!({ "enabled": false })).await;
    assert_eq!(s["enabled"], false);
    let stored = h.state.store.get_schedule(Uuid::parse_str(&sid).unwrap()).await.unwrap().unwrap();
    assert!(!stored.enabled);
    let (_, s) = patch(json!({ "enabled": true })).await;
    assert_eq!(s["enabled"], true);
    assert!(ts(&s["next_run_at"]) > Utc::now());

    // The prompt can be edited; a blank model goes back to the default.
    let (_, s) = patch(json!({ "prompt": " DO: something else ", "model": "", "reasoning": "" })).await;
    assert_eq!((s["prompt"].as_str(), s["model"].as_str(), s["reasoning"].as_str()), (Some("DO: something else"), None, None));

    let (status, detail) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["schedule"]["id"], sid);
    assert_eq!(detail["runs"], json!([]));

    let (status, _) = h.json(Method::DELETE, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = h.json(Method::PATCH, &format!("/api/schedules/{}", Uuid::new_v4()), Some(json!({ "enabled": true }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn due_schedules_start_a_run_in_a_hidden_conversation() {
    let h = Harness::new(&[]).await;
    let (cid, _) = h.finished_conversation("hi", "hello").await;
    let mut body = schedule_body(&cid);
    body["model"] = json!("model-a");
    body["reasoning"] = json!("high");
    let (_, s) = h.json(Method::POST, "/api/schedules", Some(body)).await;
    let sid = Uuid::parse_str(s["id"].as_str().unwrap()).unwrap();

    // Not due yet: nothing happens.
    assert_eq!(schedule::tick(&h.state, Utc::now()).await.unwrap(), 0);

    // Due (the process was down over the slot, say): exactly one run, and the next slot is in the future.
    let due = ts(&s["next_run_at"]);
    let later = due + Duration::hours(3);
    assert_eq!(schedule::tick(&h.state, later).await.unwrap(), 1);
    assert_eq!(schedule::tick(&h.state, later).await.unwrap(), 0, "a slot fires once");
    let prompts = h.cedar.prompts.lock().unwrap().clone();
    // cedar is told nobody is there to answer questions; the conversation keeps the prompt as written.
    let sent = prompts.last().unwrap();
    assert!(sent.starts_with("DO: summarize the news\n\n"), "{sent}");
    assert!(sent.contains("nobody is there to answer questions"), "{sent}");
    assert_eq!(h.cedar.parents.lock().unwrap().last().unwrap(), &None, "a fresh run, not a continuation");
    let stored = h.state.store.get_schedule(sid).await.unwrap().unwrap();
    assert!(stored.next_run_at > Utc::now(), "{}", stored.next_run_at);
    assert!(stored.last_run_at.is_some());

    let (status, detail) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let runs = detail["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(ts(&run["due_at"]), due);
    assert_eq!(run["manual"], false);
    assert_eq!(run["error"], Value::Null);
    assert_eq!(run["turn"]["status"], "queued");
    assert_eq!(run["turn"]["prompt"], "DO: summarize the news");
    let run_cid = run["conversation_id"].as_str().unwrap().to_string();
    assert_eq!(run["turn"]["conversation_id"], run_cid);

    // The run's conversation opens like any other, but is not in the main list.
    let (status, conv) = h.json(Method::GET, &format!("/api/conversations/{run_cid}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(conv["conversation"]["schedule_id"], sid.to_string());
    assert_eq!(conv["conversation"]["model"], "model-a");
    assert_eq!(conv["conversation"]["reasoning"], "high");
    let sent = h.cedar.bodies.lock().unwrap().last().cloned().unwrap();
    assert_eq!((sent["model"].as_str(), sent["reasoning"].as_str()), (Some("model-a"), Some("high")));
    // No one is there to answer a scheduled run: it neither plans nor waits.
    assert!(sent.get("interactive").is_none() && sent.get("plan").is_none(), "{sent}");
    let title = conv["conversation"]["title"].as_str().unwrap();
    assert!(title.starts_with("Morning digest · "), "{title}");
    assert_eq!(conv["turns"].as_array().unwrap().len(), 1);
    let (_, list) = h.json(Method::GET, "/api/conversations", None).await;
    let listed: Vec<&str> = list["conversations"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(listed, [cid.as_str()]);

    // Once the run finishes, the schedule list shows its result.
    h.finish(run["turn"]["run_id"].as_str().unwrap(), "completed", Some("Top story: …"));
    let (_, list) = h.json(Method::GET, "/api/schedules", None).await;
    assert_eq!(list["schedules"][0]["last_run"]["turn"]["status"], "completed");
    assert_eq!(list["schedules"][0]["last_run"]["turn"]["final_text"], "Top story: …");

    // Deleting the schedule deletes the runs' conversations and their cedar runs.
    let (status, _) = h.json(Method::DELETE, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(h.cedar.deleted.lock().unwrap().len(), 1);
    let (status, _) = h.json(Method::GET, &format!("/api/conversations/{run_cid}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_now_starts_a_run_without_moving_the_schedule() {
    let h = Harness::new(&[]).await;
    let (cid, _) = h.finished_conversation("hi", "hello").await;
    let (_, s) = h.json(Method::POST, "/api/schedules", Some(schedule_body(&cid))).await;
    let sid = s["id"].as_str().unwrap().to_string();
    let run_now = format!("/api/schedules/{sid}/run");

    let (status, run) = h.json(Method::POST, &run_now, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    assert_eq!(run["manual"], true);
    assert_eq!(run["turn"]["status"], "queued");
    let (status, err) = h.json(Method::POST, &run_now, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    let (status, err) = h.json(Method::DELETE, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");

    let (_, after) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(after["schedule"]["next_run_at"], s["next_run_at"], "the next scheduled run is unchanged");
    assert!(after["schedule"]["last_run_at"].is_string());

    h.finish(run["turn"]["run_id"].as_str().unwrap(), "failed", None);
    let (status, _) = h.json(Method::POST, &run_now, None).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // A cedar refusal is reported and leaves nothing behind.
    *h.cedar.refuse_runs.lock().unwrap() = Some((StatusCode::TOO_MANY_REQUESTS, "busy"));
    h.finish(after["runs"][0]["turn"]["run_id"].as_str().unwrap(), "completed", Some("x"));
    let (_, latest) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
    h.finish(latest["runs"][0]["turn"]["run_id"].as_str().unwrap(), "completed", Some("x"));
    let (status, err) = h.json(Method::POST, &run_now, None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{err}");
    let (_, latest) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(latest["runs"].as_array().unwrap().len(), 2);
    assert_eq!(h.state.store.list_conversations(100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn chats_and_scheduled_runs_can_be_starred() {
    let h = Harness::new(&[]).await;
    let (cid, _) = h.finished_conversation("hi", "hello").await;
    let (_, other) = h.json(Method::POST, "/api/conversations", Some(json!({ "title": "Other" }))).await;
    let other = other["id"].as_str().unwrap().to_string();
    let (_, s) = h.json(Method::POST, "/api/schedules", Some(schedule_body(&cid))).await;
    let sid = s["id"].as_str().unwrap().to_string();
    let (_, run) = h.json(Method::POST, &format!("/api/schedules/{sid}/run"), None).await;
    let run_cid = run["conversation_id"].as_str().unwrap().to_string();

    let (status, list) = h.json(Method::GET, "/api/starred", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["conversations"], json!([]));
    let (_, conv) = h.json(Method::GET, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(conv["conversation"]["starred_at"], Value::Null);

    let (status, starred) = h.json(Method::PUT, &format!("/api/conversations/{cid}/star"), None).await;
    assert_eq!(status, StatusCode::OK, "{starred}");
    let first = starred["starred_at"].clone();
    assert!(first.is_string(), "{starred}");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let (status, starred) = h.json(Method::PUT, &format!("/api/conversations/{run_cid}/star"), None).await;
    assert_eq!(status, StatusCode::OK, "{starred}");
    assert_eq!(starred["schedule_id"], sid);
    // Starring again keeps the first time, so the list's order doesn't change.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let (_, again) = h.json(Method::PUT, &format!("/api/conversations/{cid}/star"), None).await;
    assert_eq!(again["starred_at"], first);

    // Most recently starred first; the scheduled run carries its schedule's title, and stays out
    // of the main list.
    let (_, list) = h.json(Method::GET, "/api/starred", None).await;
    let items = list["conversations"].as_array().unwrap();
    let ids: Vec<&str> = items.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [run_cid.as_str(), cid.as_str()]);
    assert_eq!(items[0]["schedule_title"], "Morning digest");
    assert_eq!(items[1]["schedule_title"], Value::Null);
    assert_eq!(items[1]["title"], "hi");
    let (_, main) = h.json(Method::GET, "/api/conversations", None).await;
    let listed: Vec<&str> = main["conversations"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(listed, [other.as_str(), cid.as_str()]);
    assert!(main["conversations"][1]["starred_at"].is_string(), "the main list marks it starred too");

    // Unstarring is idempotent too.
    for _ in 0..2 {
        let (status, unstarred) = h.json(Method::DELETE, &format!("/api/conversations/{cid}/star"), None).await;
        assert_eq!(status, StatusCode::OK, "{unstarred}");
        assert_eq!(unstarred["starred_at"], Value::Null);
    }
    let (_, list) = h.json(Method::GET, "/api/starred", None).await;
    assert_eq!(list["conversations"].as_array().unwrap().len(), 1);

    let (status, _) = h.json(Method::PUT, &format!("/api/conversations/{}/star", Uuid::new_v4()), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = h.json(Method::DELETE, "/api/conversations/nope/star", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_busy_cedar_is_retried_for_an_hour_then_the_slot_is_recorded_as_failed() {
    let h = Harness::new(&[]).await;
    let (cid, _) = h.finished_conversation("hi", "hello").await;
    let (_, s) = h.json(Method::POST, "/api/schedules", Some(schedule_body(&cid))).await;
    let sid = Uuid::parse_str(s["id"].as_str().unwrap()).unwrap();
    let due = ts(&s["next_run_at"]);
    *h.cedar.refuse_runs.lock().unwrap() = Some((StatusCode::TOO_MANY_REQUESTS, "busy"));

    // Within the grace period the slot is kept for the next tick.
    assert_eq!(schedule::tick(&h.state, due + Duration::minutes(5)).await.unwrap(), 0);
    let stored = h.state.store.get_schedule(sid).await.unwrap().unwrap();
    assert_eq!(stored.next_run_at, due);
    assert!(h.state.store.list_schedule_runs(sid, 10).await.unwrap().is_empty());
    assert!(h.state.store.list_schedule_turns(sid).await.unwrap().is_empty());

    // Past it, the failure is recorded and the schedule moves on.
    assert_eq!(schedule::tick(&h.state, due + Duration::hours(2)).await.unwrap(), 0);
    let stored = h.state.store.get_schedule(sid).await.unwrap().unwrap();
    assert!(stored.next_run_at > due + Duration::hours(2));
    let (_, detail) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(detail["runs"].as_array().unwrap().len(), 1);
    assert_eq!(detail["runs"][0]["error"], "busy");
    assert_eq!(detail["runs"][0]["turn"], Value::Null);
    assert_eq!(detail["runs"][0]["conversation_id"], Value::Null);

    // A validation error (not cedar being busy) is never retried.
    *h.cedar.refuse_runs.lock().unwrap() = Some((StatusCode::BAD_REQUEST, "model not allowed"));
    h.state.store.mark_fired(sid, Utc::now(), Some(Utc::now() - Duration::minutes(1))).await.unwrap();
    assert_eq!(schedule::tick(&h.state, Utc::now()).await.unwrap(), 0);
    let (_, detail) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
    assert_eq!(detail["runs"].as_array().unwrap().len(), 2);
    assert!(h.state.store.get_schedule(sid).await.unwrap().unwrap().next_run_at > Utc::now());
}

// -- settings, tools, integrations ------------------------------------------------------------

#[tokio::test]
async fn the_language_goes_with_messages_schedules_and_notifications() {
    let h = Harness::new(&[]).await;
    let (cid, _) = h.finished_conversation("first", "ok").await;
    let first = h.cedar.bodies.lock().unwrap().last().cloned().unwrap();
    assert!(first.get("locale").is_none(), "no language saved: cedar's default");

    // Saving the language keeps the other settings, and other settings keep the language.
    h.json(Method::PUT, "/api/settings", Some(json!({ "system": "be brief" }))).await;
    let (status, prefs) = h.json(Method::PUT, "/api/settings/locale", Some(json!({ "locale": " de-CH " }))).await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!((prefs["locale"].as_str(), prefs["system"].as_str()), (Some("de-CH"), Some("be brief")));
    let (_, prefs) = h.json(Method::PUT, "/api/settings", Some(json!({ "system": "kurz" }))).await;
    assert_eq!(prefs["locale"], "de-CH");
    let (status, _) = h.json(Method::PUT, "/api/settings/locale", Some(json!({ "locale": "de CH" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, got) = h.json(Method::GET, "/api/settings", None).await;
    assert_eq!(got["locale"], "de-CH");

    // A message without a language of its own uses the saved one; with one, its own.
    let msgs = format!("/api/conversations/{cid}/messages");
    let (_, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "zweite" }))).await;
    h.finish(turn["run_id"].as_str().unwrap(), "completed", Some("ok"));
    assert_eq!(h.cedar.bodies.lock().unwrap().last().unwrap()["locale"], "de-CH");
    let (status, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "troisième", "locale": "fr-CA" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{turn}");
    h.finish(turn["run_id"].as_str().unwrap(), "completed", Some("ok"));
    assert_eq!(h.cedar.bodies.lock().unwrap().last().unwrap()["locale"], "fr-CA");
    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "x", "locale": "-" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // null goes back to cedar's default.
    let (_, prefs) = h.json(Method::PUT, "/api/settings/locale", Some(json!({ "locale": null }))).await;
    assert_eq!(prefs["locale"], Value::Null);
}

#[tokio::test]
async fn runs_get_the_default_settings_and_a_conversations_own_tools() {
    let h = Harness::new(&[]).await;

    // Without settings every tool is asked for explicitly, so a continuation can't keep a
    // narrower choice from before.
    let (cid, _) = h.finished_conversation("first", "ok").await;
    let first = h.cedar.bodies.lock().unwrap().last().cloned().unwrap();
    assert_eq!(first["tools"], json!(["*"]));
    assert!(first.get("system").is_none() && first.get("model").is_none());
    // Someone is reading a message's run, so it may ask; it plans when the prompt calls for it.
    assert_eq!(first["interactive"], true);
    assert_eq!(first["plan"], "auto");

    let (status, prefs) = h
        .json(
            Method::PUT,
            "/api/settings",
            Some(json!({ "tools": ["weather", "web_search", "weather"], "model": "model-b", "system": "  be brief  ", "max_turns": 7, "plan": " Off " })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(prefs, json!({ "tools": ["weather", "web_search"], "model": "model-b", "system": "be brief", "max_turns": 7, "plan": "off", "locale": null }));
    let (status, _) = h.json(Method::PUT, "/api/settings", Some(json!({ "plan": "sometimes" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, got) = h.json(Method::GET, "/api/settings", None).await;
    assert_eq!(got["tools"], prefs["tools"]);
    assert_eq!(got["system"], "be brief");
    // cedar's own defaults come along, so the page can show what an unset field means.
    assert_eq!(got["defaults"]["system"], "You are helpful.");
    assert_eq!(got["defaults"]["max_turns_cap"], 25);

    let msgs = format!("/api/conversations/{cid}/messages");
    let (status, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "second" }))).await;
    assert_eq!(status, StatusCode::CREATED);
    h.finish(turn["run_id"].as_str().unwrap(), "completed", Some("ok"));
    let second = h.cedar.bodies.lock().unwrap().last().cloned().unwrap();
    assert_eq!(second["tools"], json!(["weather", "web_search"]));
    assert_eq!(second["model"], "model-b");
    assert_eq!(second["system"], "be brief");
    assert_eq!(second["max_turns"], 7);
    assert_eq!(second["plan"], "off");
    assert!(second["parent_id"].is_string());

    // A message can choose its own plan mode (the phone apps' setting); a made-up one is refused.
    let (status, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "third", "plan": " Tool " }))).await;
    assert_eq!(status, StatusCode::CREATED, "{turn}");
    h.finish(turn["run_id"].as_str().unwrap(), "completed", Some("ok"));
    assert_eq!(h.cedar.bodies.lock().unwrap().last().unwrap()["plan"], "tool");
    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "fourth", "plan": "maybe" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, prefs) = h.json(Method::PUT, "/api/settings", Some(json!({ "plan": "tool" }))).await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(prefs["plan"], "tool");
    let (status, _) = h.json(Method::PUT, "/api/settings", Some(json!({ "plan": "off" }))).await;
    assert_eq!(status, StatusCode::OK);

    // The conversation's own tools win; an explicit model too.
    let (status, conv) =
        h.json(Method::PUT, &format!("/api/conversations/{cid}/tools"), Some(json!({ "tools": ["gmail_search_threads"] }))).await;
    assert_eq!(status, StatusCode::OK, "{conv}");
    assert_eq!(conv["tools"], json!(["gmail_search_threads"]));
    let (_, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "third", "model": "model-a", "reasoning": "high" }))).await;
    h.finish(turn["run_id"].as_str().unwrap(), "completed", Some("ok"));
    let third = h.cedar.bodies.lock().unwrap().last().cloned().unwrap();
    assert_eq!(third["tools"], json!(["gmail_search_threads"]));
    assert_eq!(third["model"], "model-a");
    assert_eq!(third["reasoning"], "high");
    let (_, detail) = h.json(Method::GET, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(detail["conversation"]["tools"], json!(["gmail_search_threads"]));
    assert_eq!(detail["conversation"]["model"], "model-a", "remembered for the picker");
    assert_eq!(detail["conversation"]["reasoning"], "high");

    // A saved default model cedar no longer has is dropped, so messages still go through.
    h.state.store.set_preferences(&cedar_chat::store::Preferences { model: Some("model-gone".into()), ..Default::default() }).await.unwrap();
    let (status, turn) = h.json(Method::POST, &msgs, Some(json!({ "content": "fourth" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{turn}");
    h.finish(turn["run_id"].as_str().unwrap(), "completed", Some("ok"));
    assert_eq!(h.cedar.bodies.lock().unwrap().last().unwrap().get("model"), None);

    // Back to the defaults.
    let (_, conv) = h.json(Method::PUT, &format!("/api/conversations/{cid}/tools"), Some(json!({ "tools": null }))).await;
    assert_eq!(conv["tools"], Value::Null);
}

#[tokio::test]
async fn settings_are_validated() {
    let h = Harness::new(&[]).await;
    for (body, needle) in [
        (json!({ "tools": ["nope"] }), "no tool or tool group"),
        (json!({ "tools": ["gmail"], "model": "model-z" }), "not registered"),
        (json!({ "max_turns": 0 }), "max_turns"),
        (json!({ "system": "x".repeat(20_001) }), "system"),
        (json!({ "colour": "red" }), "unknown field"),
        (json!({ "multimodal": true }), "unknown field"),
    ] {
        let (status, err) = h.json(Method::PUT, "/api/settings", Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(err["error"]["message"].as_str().unwrap().contains(needle), "{body}: {err}");
    }
    let (status, _) = h.json(Method::PUT, "/api/conversations/00000000-0000-0000-0000-000000000000/tools", Some(json!({ "tools": [] }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, tools) = h.json(Method::GET, "/api/tools", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tools["groups"][1]["id"], "weather");
}

#[tokio::test]
async fn schedules_keep_their_conversations_tools() {
    let h = Harness::new(&[]).await;
    let (cid, _) = h.finished_conversation("weather please", "sunny").await;
    h.json(Method::PUT, &format!("/api/conversations/{cid}/tools"), Some(json!({ "tools": ["weather"] }))).await;
    let (status, schedule) = h.json(Method::POST, "/api/schedules", Some(schedule_body(&cid))).await;
    assert_eq!(status, StatusCode::CREATED, "{schedule}");
    assert_eq!(schedule["tools"], json!(["weather"]));
    let sid = schedule["id"].as_str().unwrap();

    let (status, _) = h.json(Method::POST, &format!("/api/schedules/{sid}/run"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(h.cedar.bodies.lock().unwrap().last().unwrap()["tools"], json!(["weather"]));

    // `null` hands the schedule back to the defaults; an absent field leaves it alone.
    let (_, updated) = h.json(Method::PATCH, &format!("/api/schedules/{sid}"), Some(json!({ "title": "Forecast" }))).await;
    assert_eq!(updated["tools"], json!(["weather"]));
    let (_, updated) = h.json(Method::PATCH, &format!("/api/schedules/{sid}"), Some(json!({ "tools": null }))).await;
    assert_eq!(updated["tools"], Value::Null);
    let (status, _) = h.json(Method::PATCH, &format!("/api/schedules/{sid}"), Some(json!({ "tools": ["bogus"] }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

async fn admin_call(h: &Harness, method: Method, uri: &str, admin: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri).header("authorization", "Bearer access");
    if let Some(a) = admin {
        req = req.header("x-admin-token", a);
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let (status, _, text) = h.call_full(req).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[tokio::test]
async fn integrations_need_the_admin_token() {
    let h = Harness::new(&[("CHAT_ACCESS_TOKEN", "access")]).await;
    let (status, err) = admin_call(&h, Method::GET, "/api/integrations", Some("anything"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(err["error"]["message"].as_str().unwrap().contains("CHAT_ADMIN_TOKEN"));

    let h = Harness::new(&[("CHAT_ACCESS_TOKEN", "access"), ("CHAT_ADMIN_TOKEN", "admin"), ("CHAT_PUBLIC_URL", "https://chat.example")]).await;
    let (_, cfg) = h.json(Method::GET, "/api/config", None).await;
    assert_eq!(cfg["admin_enabled"], true);
    for admin in [None, Some("wrong")] {
        let (status, _) = admin_call(&h, Method::GET, "/api/integrations", admin, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    // The access token is still needed too.
    let (status, _) = h.call(Method::GET, "/api/integrations", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(h.cedar.admin_calls.lock().unwrap().is_empty());

    let (status, status_body) = admin_call(&h, Method::GET, "/api/integrations", Some("admin"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status_body["google"]["redirect_uri"], "https://chat.example/api/oauth/google/callback");

    let (status, err) = admin_call(&h, Method::PUT, "/api/integrations/brave", Some("admin"), Some(json!({ "api_key": "bad" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("rejected"));
    let (status, saved) = admin_call(&h, Method::PUT, "/api/integrations/brave", Some("admin"), Some(json!({ "api_key": "k-1234" }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["brave"]["last4"], "1234");
    let (status, _) = admin_call(&h, Method::DELETE, "/api/integrations/brave", Some("admin"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = admin_call(&h, Method::DELETE, "/api/integrations/google", Some("admin"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, listed) = admin_call(&h, Method::GET, "/api/integrations/models", Some("admin"), None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["models"][0]["id"], "model-a");
    let query = json!({ "base_url": "http://gpu:8000", "api_key": "k" });
    let (status, found) = admin_call(&h, Method::POST, "/api/integrations/server-models", Some("admin"), Some(query.clone())).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["models"], json!(["m-1", "m-2"]));
    let (status, err) = admin_call(&h, Method::POST, "/api/integrations/server-models", Some("admin"), Some(json!({ "base_url": "http://127.0.0.1:1" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("could not reach"));
    let (status, _) = admin_call(&h, Method::POST, "/api/integrations/server-models", None, Some(query.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let spec = json!({ "base_url": "http://gpu:8000", "api_key": "k", "model": "m-1", "label": "GPU", "reasoning_levels": ["none", "high"], "is_default": true });
    let (status, saved) = admin_call(&h, Method::PUT, "/api/integrations/models/gpu", Some("admin"), Some(spec.clone())).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["listed_models"], json!(["m-1"]));
    // cedar's verdict on the server is relayed; a body that isn't an object is refused here.
    let (status, err) = admin_call(&h, Method::PUT, "/api/integrations/models/gpu", Some("admin"), Some(json!({ "base_url": "http://127.0.0.1:1", "model": "m-1" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("could not reach"));
    let (status, _) = admin_call(&h, Method::PUT, "/api/integrations/models/gpu", Some("admin"), Some(json!([1]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = admin_call(&h, Method::DELETE, "/api/integrations/models/gpu", Some("admin"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = admin_call(&h, Method::PUT, "/api/integrations/models/gpu", None, Some(json!({ "base_url": "http://x", "model": "m" }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // cedar's server-wide settings (the max image size) go through as they are.
    let (status, got) = admin_call(&h, Method::GET, "/api/integrations/settings", Some("admin"), None).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["defaults"]["max_image_side"], 1568);
    let (status, saved) = admin_call(&h, Method::PUT, "/api/integrations/settings", Some("admin"), Some(json!({ "max_image_side": 1024 }))).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["max_image_side"], 1024);
    let (status, err) = admin_call(&h, Method::PUT, "/api/integrations/settings", Some("admin"), Some(json!({ "max_image_side": 1 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("between 256 and 8192"));
    let (status, _) = admin_call(&h, Method::PUT, "/api/integrations/settings", None, Some(json!({ "max_image_side": 1024 }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let calls = h.cedar.admin_calls.lock().unwrap().clone();
    let routes: Vec<&str> = calls.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(routes, ["get", "put_brave", "put_brave", "delete_brave", "delete_google", "get_models", "server_models", "server_models", "put_model:gpu", "put_model:gpu", "delete_model:gpu", "get_settings", "put_settings", "put_settings"]);
    assert_eq!(calls[6].1, query, "the body goes through as it is");
    assert_eq!(calls[8].1, spec, "the body goes through as it is");

    let (status, connect) = admin_call(&h, Method::POST, "/api/integrations/google/connect", Some("admin"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(connect["url"].as_str().unwrap().ends_with("redirect_uri=https://chat.example/api/oauth/google/callback"));

    // cedar refusing chat's key is explained, not passed on as the browser's fault.
    *h.cedar.not_admin.lock().unwrap() = true;
    let (status, err) = admin_call(&h, Method::GET, "/api/integrations", Some("admin"), None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(err["error"]["message"].as_str().unwrap().contains("CEDAR_ADMIN_KEYS"));
}

#[tokio::test]
async fn the_google_callback_needs_no_token_and_redirects_to_the_page() {
    let h = Harness::new(&[("CHAT_ACCESS_TOKEN", "access"), ("CHAT_ADMIN_TOKEN", "admin")]).await;
    let get = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();

    let (status, headers, _) = h.call_full(get("/api/oauth/google/callback?state=st-1&code=c0de")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/?integrations=connected");
    let (route, body) = h.cedar.admin_calls.lock().unwrap().last().cloned().unwrap();
    assert_eq!(route, "callback");
    assert_eq!(body, json!({ "state": "st-1", "code": "c0de" }));

    let (_, headers, _) = h.call_full(get("/api/oauth/google/callback?state=forged&code=c")).await;
    let location = headers["location"].to_str().unwrap();
    assert!(location.starts_with("/?integrations=error&message="), "{location}");
    assert!(location.contains("expired"), "{location}");

    let (_, headers, _) = h.call_full(get("/api/oauth/google/callback?error=access_denied&state=st-1")).await;
    assert!(headers["location"].to_str().unwrap().contains("access_denied"));
    let (_, headers, _) = h.call_full(get("/api/oauth/google/callback")).await;
    assert!(headers["location"].to_str().unwrap().contains("incomplete"));
}

// -- being told when a response finishes -------------------------------------------------------

#[tokio::test]
async fn finished_runs_are_learned_from_cedars_event_stream() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let cid = conv["id"].as_str().unwrap().to_string();
    assert_eq!((&conv["unread"], &conv["needs_answer"]), (&json!(false), &json!(false)));
    let (_, turn) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "hi" }))).await;
    let tid = turn["id"].as_str().unwrap().to_string();
    let run_id = turn["run_id"].as_str().unwrap().to_string();
    assert_eq!((&turn["seen"], &turn["finished_at"]), (&json!(false), &Value::Null));

    // Nothing has finished: the stream has nothing, the inbox is empty.
    assert_eq!(h.sync().await, 0);
    let (_, inbox) = h.json(Method::GET, "/api/inbox", None).await;
    assert_eq!(inbox, json!({ "items": [], "unread": 0 }));

    // The run ends; cedar's stream says so; the turn is brought up to date and the tabs told.
    let mut notices = h.notice_stream().await;
    let nid = h.finish_with(&run_id, "completed", Some("hello"), Some("end_turn"));
    assert_eq!(h.sync().await, 1);
    let notice = next_notice(&mut notices).await;
    assert_eq!(notice["type"], "turn.finished");
    assert_eq!(notice["turn_id"], tid);
    assert_eq!(notice["conversation_id"], cid);
    assert_eq!(notice["status"], "completed");
    assert_eq!(notice["stop_reason"], "end_turn");
    assert_eq!(notice["needs_answer"], false);
    assert_eq!(notice["title"], "hi");
    assert_eq!(notice["schedule_id"], Value::Null);
    assert_eq!(h.state.store.notifications_cursor().await.unwrap(), nid);

    let (_, detail) = h.json(Method::GET, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(detail["conversation"]["unread"], true);
    assert_eq!(detail["turns"][0]["status"], "completed");
    assert_eq!(detail["turns"][0]["final_text"], "hello");
    assert_eq!(detail["turns"][0]["stop_reason"], "end_turn");
    assert!(detail["turns"][0]["finished_at"].is_string());
    assert_eq!(detail["turns"][0]["seen"], false);
    let (_, list) = h.json(Method::GET, "/api/conversations", None).await;
    assert_eq!(list["conversations"][0]["unread"], true);
    let (_, inbox) = h.json(Method::GET, "/api/inbox", None).await;
    assert_eq!(inbox["unread"], 1);
    assert_eq!(inbox["items"][0]["turn_id"], tid);
    assert_eq!(inbox["items"][0]["conversation_id"], cid);
    assert_eq!(inbox["items"][0]["title"], "hi");
    assert_eq!(inbox["items"][0]["status"], "completed");
    assert_eq!(inbox["items"][0]["needs_answer"], false);

    // Looking at the conversation clears it; the stream has nothing new after the cursor.
    let (status, seen) = h.json(Method::POST, &format!("/api/conversations/{cid}/seen"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen["unread"], false);
    let (_, inbox) = h.json(Method::GET, "/api/inbox", None).await;
    assert_eq!(inbox, json!({ "items": [], "unread": 0 }));
    let (_, detail) = h.json(Method::GET, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(detail["turns"][0]["seen"], true);
    assert_eq!(h.sync().await, 0);
    assert_eq!(h.state.store.notifications_cursor().await.unwrap(), nid);

    // A failure is announced too, and a run cedar reports that this service never started is skipped.
    let (_, turn) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "more" }))).await;
    let run_id = turn["run_id"].as_str().unwrap().to_string();
    h.notify(&Uuid::new_v4().to_string(), "completed", Some("end_turn"));
    let nid = h.finish_with(&run_id, "failed", None, None);
    assert_eq!(h.sync().await, 2);
    let notice = next_notice(&mut notices).await;
    assert_eq!((notice["status"].as_str(), notice["error"].as_str()), (Some("failed"), Some("it broke")));
    assert_eq!(h.state.store.notifications_cursor().await.unwrap(), nid);
    let (_, inbox) = h.json(Method::GET, "/api/inbox", None).await;
    assert_eq!(inbox["unread"], 1);
    assert_eq!(inbox["items"][0]["status"], "failed");

    let (status, _) = h.json(Method::POST, &format!("/api/conversations/{}/seen", Uuid::new_v4()), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_question_leaves_the_conversation_waiting_for_an_answer() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let cid = conv["id"].as_str().unwrap().to_string();
    let (_, turn) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "do it" }))).await;
    let run_id = turn["run_id"].as_str().unwrap().to_string();
    let mut notices = h.notice_stream().await;
    h.finish_with(&run_id, "completed", Some("Which one?"), Some("ask_user"));
    h.sync().await;
    assert_eq!(next_notice(&mut notices).await["needs_answer"], true);
    let (_, inbox) = h.json(Method::GET, "/api/inbox", None).await;
    assert_eq!(inbox["items"][0]["needs_answer"], true);
    let (_, list) = h.json(Method::GET, "/api/conversations", None).await;
    assert_eq!((&list["conversations"][0]["unread"], &list["conversations"][0]["needs_answer"]), (&json!(true), &json!(true)));

    // Seen, but still waiting; the answer settles it.
    let (_, seen) = h.json(Method::POST, &format!("/api/conversations/{cid}/seen"), None).await;
    assert_eq!((&seen["unread"], &seen["needs_answer"]), (&json!(false), &json!(true)));
    h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "the first" }))).await;
    let (_, detail) = h.json(Method::GET, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!(detail["conversation"]["needs_answer"], false);
}

#[tokio::test]
async fn a_scheduled_run_that_finishes_lands_in_the_inbox() {
    let h = Harness::new(&[]).await;
    let (cid, _) = h.finished_conversation("hi", "hello").await;
    let (_, s) = h.json(Method::POST, "/api/schedules", Some(schedule_body(&cid))).await;
    let sid = s["id"].as_str().unwrap().to_string();
    let due = ts(&s["next_run_at"]);
    assert_eq!(schedule::tick(&h.state, due + Duration::hours(1)).await.unwrap(), 1);
    let (_, detail) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
    let run = &detail["runs"][0];
    let run_cid = run["conversation_id"].as_str().unwrap().to_string();

    let mut notices = h.notice_stream().await;
    h.finish_with(run["turn"]["run_id"].as_str().unwrap(), "completed", Some("Top story"), Some("end_turn"));
    h.sync().await;
    let notice = next_notice(&mut notices).await;
    assert_eq!(notice["schedule_id"], sid);
    assert_eq!(notice["conversation_id"], run_cid);
    // Out of the conversation list, in the inbox.
    let (_, list) = h.json(Method::GET, "/api/conversations", None).await;
    assert_eq!(list["conversations"].as_array().unwrap().len(), 1);
    let (_, inbox) = h.json(Method::GET, "/api/inbox", None).await;
    assert_eq!(inbox["unread"], 1);
    assert_eq!(inbox["items"][0]["schedule_id"], sid);
    assert_eq!(inbox["items"][0]["conversation_id"], run_cid);
    assert!(inbox["items"][0]["title"].as_str().unwrap().starts_with("Morning digest"));
    h.json(Method::POST, &format!("/api/conversations/{run_cid}/seen"), None).await;
    let (_, inbox) = h.json(Method::GET, "/api/inbox", None).await;
    assert_eq!(inbox["unread"], 0);
}

#[tokio::test]
async fn a_page_that_watched_the_response_end_marks_it_seen_before_the_notice_arrives() {
    // The page's own stream shows the end first; it calls `seen`, which refreshes the turn from
    // cedar. The notification that comes later must not make the response unread again.
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let cid = conv["id"].as_str().unwrap().to_string();
    let (_, turn) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "hi" }))).await;
    let run_id = turn["run_id"].as_str().unwrap().to_string();
    h.finish_with(&run_id, "completed", Some("hello"), Some("end_turn"));
    let (_, seen) = h.json(Method::POST, &format!("/api/conversations/{cid}/seen"), None).await;
    assert_eq!(seen["unread"], false);
    assert_eq!(h.sync().await, 1);
    let (_, inbox) = h.json(Method::GET, "/api/inbox", None).await;
    assert_eq!(inbox["unread"], 0);
    let (_, detail) = h.json(Method::GET, &format!("/api/conversations/{cid}"), None).await;
    assert_eq!((&detail["conversation"]["unread"], &detail["turns"][0]["seen"]), (&json!(false), &json!(true)));
}

// -- out of the app: Pushover, email, browser push ---------------------------------------------

/// A conversation whose one run ended as `status`; returns its turn id.
async fn ended_turn(h: &Harness, title: &str, status: &str, stop_reason: Option<&str>) -> Uuid {
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({ "title": title }))).await;
    let cid = conv["id"].as_str().unwrap();
    let (_, turn) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "do it" }))).await;
    h.finish_with(turn["run_id"].as_str().unwrap(), status, Some("The answer is 42."), stop_reason);
    h.sync().await;
    Uuid::parse_str(turn["id"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn notification_settings_are_saved_without_leaking_secrets() {
    let h = Harness::new(&[]).await;
    let (status, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!((got["default_method"].as_str(), got["scope"].as_str()), (Some("none"), Some("attention")));
    assert_eq!(got["configured"], json!([]));
    assert_eq!(got["pushover"]["configured"], false);
    let key = got["browser"]["public_key"].as_str().expect("the push keys are made on first read");
    assert!(key.len() > 80 && !key.contains('='), "an uncompressed P-256 key, base64url: {key}");
    assert_eq!(got["browser"]["subscriptions"], json!([]));
    assert_eq!(got["ios"], json!({ "configured": false, "team_id": "", "key_id": "", "key_set": false, "topic": "dev.cedar.Cedar", "environment": "production", "devices": [] }));
    assert_eq!(got["last_delivery"], Value::Null);

    // A method that can't send is refused.
    let (status, err) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "ios" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("team id"), "{err}");
    let (status, err) = h.json(Method::PUT, "/api/notifications", Some(json!({ "ios": { "private_key": "not a key" } }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains(".p8"), "{err}");
    let (status, err) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "pushover" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("Pushover needs"), "{err}");
    let (status, _) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "email" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "browser" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no browser is subscribed");
    let (status, _) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "pigeon" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = h.json(Method::PUT, "/api/notifications", Some(json!({ "email": { "from": "not an address" } }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let body = json!({
        "default_method": "pushover", "scope": "all",
        "pushover": { "token": "app-token-1", "user": "user-key-wxyz" },
        "email": { "host": "smtp.example", "security": "starttls", "username": "me", "password": "hunter2", "from": "Cedar <cedar@example.com>", "to": "me@example.com" },
        "ios": { "team_id": "TEAM42", "key_id": "KEY42", "private_key": "-----BEGIN PRIVATE KEY-----\nP8SECRET\n-----END PRIVATE KEY-----", "environment": "sandbox" },
        "contact": "ops@example.com",
    });
    let (status, got) = h.json(Method::PUT, "/api/notifications", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!((got["default_method"].as_str(), got["scope"].as_str()), (Some("pushover"), Some("all")));
    assert_eq!(got["configured"], json!(["pushover", "email"]), "both are set up, whichever is the default; iOS waits for a phone");
    assert_eq!(got["ios"]["configured"], true);
    assert_eq!((got["ios"]["key_set"].as_bool(), got["ios"]["environment"].as_str(), got["ios"]["key_id"].as_str()), (Some(true), Some("sandbox"), Some("KEY42")));
    assert_eq!(got["pushover"], json!({ "configured": true, "token_set": true, "user_set": true, "user_last4": "wxyz" }));
    assert_eq!(got["email"]["password_set"], true);
    assert_eq!(got["email"]["host"], "smtp.example");
    assert_eq!(got["email"]["security"], "starttls");
    assert_eq!(got["browser"]["contact"], "ops@example.com");
    let text = got.to_string();
    assert!(!text.contains("app-token-1") && !text.contains("user-key-wxyz") && !text.contains("hunter2") && !text.contains("P8SECRET"), "{text}");

    // Leaving a secret out keeps it; an empty one clears it.
    let (_, got) = h.json(Method::PUT, "/api/notifications", Some(json!({ "pushover": { "token": "app-token-2" } }))).await;
    assert_eq!(got["pushover"]["user_last4"], "wxyz");
    let (_, got) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "none", "pushover": { "token": "", "user": "" } }))).await;
    assert_eq!(got["pushover"]["configured"], false);
    let (status, _) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "none" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pushover_messages_go_out_and_refusals_are_reported() {
    let h = Harness::new(&[]).await;
    let (_, _) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "pushover", "pushover": { "token": "app-token", "user": "user-key" } }))).await;
    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "pushover" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["ok"], true);
    let sent = h.cedar.pushover.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0]["token"].as_str(), sent[0]["user"].as_str()), ("app-token", "user-key"));
    assert!(sent[0]["title"].contains("test"));
    assert!(sent[0]["url"].starts_with("http://127.0.0.1:3000/"), "{}", sent[0]["url"]);
    let (_, settings) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!((&settings["last_delivery"]["ok"], &settings["last_delivery"]["test"]), (&json!(true), &json!(true)));
    assert_eq!(settings["last_delivery"]["method"], "pushover");

    h.json(Method::PUT, "/api/notifications", Some(json!({ "pushover": { "token": "bad" } }))).await;
    let (status, err) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "pushover" }))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(err["error"]["message"].as_str().unwrap().contains("application token is invalid"), "{err}");
    let (_, settings) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(settings["last_delivery"]["ok"], false);
    assert!(settings["last_delivery"]["detail"].as_str().unwrap().contains("invalid"));
}

#[tokio::test]
async fn email_goes_out_over_smtp() {
    let (port, mails) = fake_smtp().await;
    let h = Harness::new(&[]).await;
    let body = json!({
        "default_method": "email",
        "email": { "host": "127.0.0.1", "port": port, "security": "none", "username": "me", "password": "pw", "from": "Cedar <cedar@example.com>", "to": "Me <me@example.com>" },
    });
    let (status, got) = h.json(Method::PUT, "/api/notifications", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "email" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let sent = mails.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Subject: Cedar: test notification"), "{}", sent[0]);
    assert!(sent[0].contains("From: \"Cedar\" <cedar@example.com>") || sent[0].contains("From: Cedar <cedar@example.com>"), "{}", sent[0]);
    assert!(sent[0].contains("http://127.0.0.1:3000/"), "{}", sent[0]);

    // Nothing listening: the error names the problem.
    h.json(Method::PUT, "/api/notifications", Some(json!({ "email": { "port": 1 } }))).await;
    let (status, err) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "email" }))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(err["error"]["message"].as_str().unwrap().contains("mail server"), "{err}");
}

#[tokio::test]
async fn the_mac_app_is_a_method_once_it_has_connected() {
    let h = Harness::new(&[]).await;
    let (status, _) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "desktop" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no app has connected yet");
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(got["desktop"], json!({ "configured": false, "connected_at": null, "listening": false }));

    // The app connects: the method is set up and, nothing having been chosen, the default.
    let req = Request::builder().uri("/api/notifications/desktop/events").body(Body::empty()).unwrap();
    let resp = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let mut stream = resp.into_body();
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(got["default_method"], "desktop");
    assert_eq!(got["configured"], json!(["desktop"]));
    assert_eq!((&got["desktop"]["configured"], &got["desktop"]["listening"]), (&json!(true), &json!(true)));

    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "desktop" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let sent = next_notice(&mut stream).await;
    assert_eq!(sent["type"], "notification");
    assert_eq!(sent["title"], "Cedar: test notification");
    assert_eq!(sent["tag"], "test");

    let failed = ended_turn(&h, "Broken", "failed", None).await;
    let outcome = cedar_chat::channels::dispatch(&h.state, failed).await.unwrap().expect("sent");
    assert!(outcome.is_ok(), "{outcome:?}");
    let sent = next_notice(&mut stream).await;
    assert_eq!(sent["title"], "Broken: failed");
    let cid = h.state.store.get_turn(failed).await.unwrap().unwrap().conversation_id;
    assert!(sent["url"].as_str().unwrap().ends_with(&format!("/#/c/{cid}")), "{sent}");

    // The app quits: still a method to pick, but a send says nobody is listening.
    drop(stream);
    let (status, err) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "desktop" }))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(err["error"]["message"].as_str().unwrap().contains("not listening"), "{err}");
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!((&got["configured"], &got["desktop"]["listening"]), (&json!(["desktop"]), &json!(false)));

    // A later connection leaves a chosen default alone.
    h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "none" }))).await;
    let req = Request::builder().uri("/api/notifications/desktop/events").body(Body::empty()).unwrap();
    let _stream = h.app.clone().oneshot(req).await.unwrap().into_body();
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(got["default_method"], "none");
}

#[tokio::test]
async fn finished_runs_are_sent_by_the_chosen_method_unless_looked_at() {
    let h = Harness::new(&[]).await;
    // Nothing is chosen: nothing goes out.
    let plain = ended_turn(&h, "Plain", "completed", Some("end_turn")).await;
    assert_eq!(cedar_chat::channels::dispatch(&h.state, plain).await.unwrap(), None);

    h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "pushover", "pushover": { "token": "t", "user": "u" } }))).await;
    // The default scope: a plain completion is not worth a push, a failure and a question are.
    assert_eq!(cedar_chat::channels::dispatch(&h.state, plain).await.unwrap(), None);
    let failed = ended_turn(&h, "Broken", "failed", None).await;
    let outcome = cedar_chat::channels::dispatch(&h.state, failed).await.unwrap().expect("sent");
    assert!(outcome.is_ok(), "{outcome:?}");
    let question = ended_turn(&h, "Lisbon", "completed", Some("ask_user")).await;
    assert!(cedar_chat::channels::dispatch(&h.state, question).await.unwrap().is_some());
    let sent = h.cedar.pushover.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["title"], "Broken: failed");
    assert!(sent[0]["message"].contains("it broke"), "{}", sent[0]["message"]);
    assert_eq!(sent[1]["title"], "Lisbon: needs your answer");
    assert!(sent[1]["message"].contains("The answer is 42."));
    assert!(sent[1]["url"].ends_with(&format!("/#/c/{}", h.state.store.get_turn(question).await.unwrap().unwrap().conversation_id)));
    let (_, settings) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(settings["last_delivery"]["test"], false);

    // Looked at meanwhile: not sent. The wider scope sends plain completions too.
    let seen = ended_turn(&h, "Seen", "failed", None).await;
    let cid = h.state.store.get_turn(seen).await.unwrap().unwrap().conversation_id;
    h.json(Method::POST, &format!("/api/conversations/{cid}/seen"), None).await;
    assert_eq!(cedar_chat::channels::dispatch(&h.state, seen).await.unwrap(), None);
    h.json(Method::PUT, "/api/notifications", Some(json!({ "scope": "all" }))).await;
    assert!(cedar_chat::channels::dispatch(&h.state, plain).await.unwrap().is_some());
    assert_eq!(h.cedar.pushover.lock().unwrap().last().unwrap()["title"], "Plain: finished");

    // A scheduled run counts as needing attention; a stopped one never goes out.
    h.json(Method::PUT, "/api/notifications", Some(json!({ "scope": "attention" }))).await;
    let (cid, _) = h.finished_conversation("hi", "hello").await;
    let (_, s) = h.json(Method::POST, "/api/schedules", Some(schedule_body(&cid))).await;
    let due = ts(&s["next_run_at"]);
    schedule::tick(&h.state, due + Duration::hours(1)).await.unwrap();
    let (_, detail) = h.json(Method::GET, &format!("/api/schedules/{}", s["id"].as_str().unwrap()), None).await;
    let run = &detail["runs"][0];
    h.finish_with(run["turn"]["run_id"].as_str().unwrap(), "completed", Some("Top story"), Some("end_turn"));
    h.sync().await;
    let turn_id = Uuid::parse_str(run["turn"]["id"].as_str().unwrap()).unwrap();
    assert!(cedar_chat::channels::dispatch(&h.state, turn_id).await.unwrap().is_some());
    let last = h.cedar.pushover.lock().unwrap().last().unwrap().clone();
    assert!(last["title"].starts_with("Morning digest"), "{}", last["title"]);
    assert!(last["message"].starts_with("A scheduled run"), "{}", last["message"]);
    let stopped = ended_turn(&h, "Stopped", "cancelled", None).await;
    assert_eq!(cedar_chat::channels::dispatch(&h.state, stopped).await.unwrap(), None);
}

#[tokio::test]
async fn a_scheduled_agent_picks_its_own_method() {
    let (port, mails) = fake_smtp().await;
    let h = Harness::new(&[]).await;
    let (cid, _) = h.finished_conversation("hi", "hello").await;

    // Only a method that is set up can be picked; the default needs nothing.
    let mut body = schedule_body(&cid);
    body["notify"] = json!("email");
    let (status, err) = h.json(Method::POST, "/api/schedules", Some(body.clone())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("SMTP"), "{err}");
    body["notify"] = json!("pigeon");
    assert_eq!(h.json(Method::POST, "/api/schedules", Some(body.clone())).await.0, StatusCode::BAD_REQUEST);
    body["notify"] = json!(null);
    let (status, s) = h.json(Method::POST, "/api/schedules", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{s}");
    assert_eq!(s["notify"], Value::Null);
    let sid = s["id"].as_str().unwrap().to_string();

    // Email set up, but Pushover is the default: the schedule picks email for itself.
    let settings = json!({
        "default_method": "pushover", "pushover": { "token": "t", "user": "u" },
        "email": { "host": "127.0.0.1", "port": port, "security": "none", "from": "cedar@example.com", "to": "me@example.com" },
    });
    let (status, got) = h.json(Method::PUT, "/api/notifications", Some(settings)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["configured"], json!(["pushover", "email"]));
    let (status, s) = h.json(Method::PATCH, &format!("/api/schedules/{sid}"), Some(json!({ "notify": "email" }))).await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(s["notify"], "email");
    let (_, list) = h.json(Method::GET, "/api/schedules", None).await;
    assert_eq!(list["schedules"][0]["notify"], "email");

    // Its run finishes: the mail goes, Pushover stays quiet.
    async fn fire(h: &Harness, sid: &str) -> Uuid {
        let (_, s) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
        let due = ts(&s["schedule"]["next_run_at"]);
        schedule::tick(&h.state, due + Duration::hours(1)).await.unwrap();
        let (_, detail) = h.json(Method::GET, &format!("/api/schedules/{sid}"), None).await;
        let run = detail["runs"][0].clone();
        h.finish_with(run["turn"]["run_id"].as_str().unwrap(), "completed", Some("Top story"), Some("end_turn"));
        h.sync().await;
        Uuid::parse_str(run["turn"]["id"].as_str().unwrap()).unwrap()
    }
    let turn = fire(&h, &sid).await;
    let outcome = cedar_chat::channels::dispatch(&h.state, turn).await.unwrap().expect("sent");
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(mails.lock().unwrap().len(), 1);
    assert!(mails.lock().unwrap()[0].contains("Subject: Morning digest"), "{}", mails.lock().unwrap()[0]);
    assert!(h.cedar.pushover.lock().unwrap().is_empty());

    // "Not at all" keeps it quiet even though the default would send; back to the default and it goes by Pushover.
    h.json(Method::PATCH, &format!("/api/schedules/{sid}"), Some(json!({ "notify": "none" }))).await;
    let turn = fire(&h, &sid).await;
    assert_eq!(cedar_chat::channels::dispatch(&h.state, turn).await.unwrap(), None);
    let (_, s) = h.json(Method::PATCH, &format!("/api/schedules/{sid}"), Some(json!({ "notify": null }))).await;
    assert_eq!(s["notify"], Value::Null);
    let turn = fire(&h, &sid).await;
    assert!(cedar_chat::channels::dispatch(&h.state, turn).await.unwrap().is_some());
    assert_eq!(h.cedar.pushover.lock().unwrap().len(), 1);
    assert_eq!(mails.lock().unwrap().len(), 1);
}

/// The receiver keys of RFC 8291's example: a valid P-256 point and a 16-byte secret.
const P256DH: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";

#[tokio::test]
async fn browser_push_reaches_every_subscribed_browser_and_drops_dead_ones() {
    let h = Harness::new(&[]).await;
    let subscribe = |name: &str, agent: &str| {
        let h = &h;
        let endpoint = format!("{}/push/{name}", h.base_url());
        let body = json!({ "endpoint": endpoint, "expirationTime": null, "keys": { "p256dh": P256DH, "auth": AUTH } });
        let agent = agent.to_string();
        async move {
            let req = Request::builder()
                .method(Method::POST)
                .uri("/api/notifications/push/subscriptions")
                .header("content-type", "application/json")
                .header("user-agent", agent)
                .body(Body::from(body.to_string()))
                .unwrap();
            let (status, _, text) = h.call_full(req).await;
            assert_eq!(status, StatusCode::OK, "{text}");
            serde_json::from_str::<Value>(&text).unwrap()
        }
    };
    let got = subscribe("a", "Mozilla/5.0 (X11; Linux) Firefox/130.0").await;
    assert!(got["subscription_id"].is_string());
    let got = subscribe("b", "Mozilla/5.0 (Macintosh) Chrome/129.0").await;
    let subs = got["browser"]["subscriptions"].as_array().unwrap();
    assert_eq!(subs.len(), 2);
    assert!(subs[0]["user_agent"].as_str().unwrap().contains("Firefox"));
    assert!(!got["browser"]["subscriptions"].to_string().contains("push/a"), "endpoints and keys stay on the server");
    // Subscribing again from the same browser is one subscription.
    subscribe("a", "Mozilla/5.0 (X11; Linux) Firefox/131.0").await;
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(got["browser"]["subscriptions"].as_array().unwrap().len(), 2);
    let (status, _) = h.json(Method::POST, "/api/notifications/push/subscriptions", Some(json!({ "endpoint": "ftp://x", "keys": { "p256dh": P256DH, "auth": AUTH } }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, got) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "browser", "contact": "ops@example.com" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "browser" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert!(got["detail"].as_str().unwrap().contains("2 browsers"), "{got}");
    let pushes = h.cedar.pushes.lock().unwrap().clone();
    assert_eq!(pushes.len(), 2);
    let mut names: Vec<&str> = pushes.iter().map(|(n, _, _)| n.as_str()).collect();
    names.sort();
    assert_eq!(names, ["a", "b"]);
    let (_, headers, body) = &pushes[0];
    let auth = headers["authorization"].to_str().unwrap();
    assert!(auth.starts_with("vapid t=") && auth.contains("k="), "{auth}");
    assert_eq!(headers["content-encoding"], "aes128gcm");
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert_eq!(headers["ttl"], "86400");
    assert_eq!(headers["urgency"], "high");
    assert!(body.len() > 100, "an encrypted payload with its header: {} bytes", body.len());
    assert!(!String::from_utf8_lossy(body).contains("test notification"), "the payload is encrypted");
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert!(got["browser"]["subscriptions"][0]["last_used_at"].is_string());

    // A browser the push service no longer knows is dropped; the other still gets it.
    h.cedar.push_answers.lock().unwrap().insert("b".into(), [410].into());
    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "browser" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert!(got["detail"].as_str().unwrap().contains("1 browser,") && got["detail"].as_str().unwrap().contains("no longer subscribed"), "{got}");
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    let subs = got["browser"]["subscriptions"].as_array().unwrap();
    assert_eq!(subs.len(), 1);
    assert!(subs[0]["user_agent"].as_str().unwrap().contains("Firefox"));

    // A finished run goes to the browser too; a push service failure is reported, not fatal.
    let failed = ended_turn(&h, "Broken", "failed", None).await;
    let outcome = cedar_chat::channels::dispatch(&h.state, failed).await.unwrap().expect("sent");
    assert!(outcome.is_ok(), "{outcome:?}");
    h.cedar.push_answers.lock().unwrap().insert("a".into(), [500].into());
    let (status, err) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "browser" }))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(err["error"]["message"].as_str().unwrap().contains("500"), "{err}");

    // The browser turns push off, or the user removes a browser from the list.
    let (status, got) = h.json(Method::DELETE, "/api/notifications/push/subscriptions", Some(json!({ "endpoint": format!("{}/push/a", h.base_url()) }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["browser"]["subscriptions"], json!([]));
    let got = subscribe("c", "Safari/605").await;
    let id = got["subscription_id"].as_str().unwrap().to_string();
    let (status, got) = h.json(Method::DELETE, &format!("/api/notifications/push/subscriptions/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["browser"]["subscriptions"], json!([]));
    let (status, _) = h.json(Method::DELETE, &format!("/api/notifications/push/subscriptions/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // With no browser left the method can't stay "browser".
    let (status, _) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "browser" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// A device token as the iOS app reads it from APNs: 32 bytes, hex.
fn device_token(fill: char) -> String {
    std::iter::repeat_n(fill, 64).collect()
}

#[tokio::test]
async fn iphone_push_reaches_every_registered_phone_and_drops_dead_ones() {
    let h = Harness::new(&[]).await;
    let register = |token: String, name: Option<&str>| {
        let h = &h;
        let body = json!({ "token": token, "name": name });
        async move {
            let (status, got) = h.json(Method::POST, "/api/notifications/apns/devices", Some(body)).await;
            assert_eq!(status, StatusCode::OK, "{got}");
            got
        }
    };
    let got = register(device_token('a'), Some("Hannes's iPhone")).await;
    assert!(got["device_id"].is_string());
    let got = register(device_token('b'), None).await;
    let devices = got["ios"]["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0]["name"], "Hannes's iPhone");
    assert!(!got.to_string().contains(&device_token('a')), "tokens stay on the server");
    // Registering again from the same phone (the token in upper case, as some code hands it over) is one device.
    register(device_token('a').to_ascii_uppercase(), Some("iPhone 16")).await;
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(got["ios"]["devices"].as_array().unwrap().len(), 2);
    assert_eq!(got["ios"]["devices"][0]["name"], "iPhone 16");
    let (status, _) = h.json(Method::POST, "/api/notifications/apns/devices", Some(json!({ "token": "not hex" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Without the key the method can't be chosen, even with phones registered.
    let (status, err) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "ios" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("team id"), "{err}");
    let key = jwt_simple::algorithms::ES256KeyPair::generate().to_pem().unwrap();
    let body = json!({ "default_method": "ios", "ios": { "team_id": "TEAM42", "key_id": "KEY42", "private_key": key, "topic": "dev.cedar.Cedar" } });
    let (status, got) = h.json(Method::PUT, "/api/notifications", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["configured"], json!(["ios"]));

    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "ios" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert!(got["detail"].as_str().unwrap().contains("2 iPhones"), "{got}");
    let pushes = h.cedar.pushes.lock().unwrap().clone();
    assert_eq!(pushes.len(), 2);
    let mut tokens: Vec<&str> = pushes.iter().map(|(n, _, _)| n.as_str()).collect();
    tokens.sort();
    assert_eq!(tokens, [device_token('a'), device_token('b')]);
    let (_, headers, body) = &pushes[0];
    let auth = headers["authorization"].to_str().unwrap();
    assert!(auth.starts_with("bearer ") && auth.matches('.').count() == 2, "a JWT: {auth}");
    assert_eq!(headers["apns-topic"], "dev.cedar.Cedar");
    assert_eq!(headers["apns-push-type"], "alert");
    assert_eq!(headers["apns-priority"], "10");
    assert_eq!(headers["apns-collapse-id"], "test");
    let payload: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(payload["aps"]["alert"]["title"], "Cedar: test notification");
    assert_eq!(payload["aps"]["sound"], "default");
    assert!(payload["url"].as_str().unwrap().ends_with('/'));
    assert_eq!(payload["conversation_id"], Value::Null);
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert!(got["ios"]["devices"][0]["last_used_at"].is_string());
    assert_eq!(got["last_delivery"]["method"], "ios");

    // A phone Apple no longer knows is dropped; the other still gets it.
    h.cedar.push_answers.lock().unwrap().insert(device_token('b'), [400].into());
    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "ios" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert!(got["detail"].as_str().unwrap().contains("1 iPhone,") && got["detail"].as_str().unwrap().contains("no longer registered"), "{got}");
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(got["ios"]["devices"].as_array().unwrap().len(), 1);

    // An expired provider token is signed again and the push retried, once.
    h.cedar.push_answers.lock().unwrap().insert(device_token('a'), [403].into());
    h.cedar.pushes.lock().unwrap().clear();
    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "ios" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let pushes = h.cedar.pushes.lock().unwrap().clone();
    assert_eq!(pushes.len(), 2, "the refused push and its retry");
    assert_ne!(pushes[0].1["authorization"], pushes[1].1["authorization"], "with a new token");

    // A finished run goes to the phone with the conversation to open; a question is wanted
    // under the default scope.
    h.cedar.pushes.lock().unwrap().clear();
    let asked = ended_turn(&h, "Trip", "completed", Some("ask_user")).await;
    let outcome = cedar_chat::channels::dispatch(&h.state, asked).await.unwrap().expect("sent");
    assert!(outcome.is_ok(), "{outcome:?}");
    let pushes = h.cedar.pushes.lock().unwrap().clone();
    assert_eq!(pushes.len(), 1);
    let payload: Value = serde_json::from_slice(&pushes[0].2).unwrap();
    assert_eq!(payload["aps"]["alert"]["title"], "Trip: needs your answer");
    let cid = payload["conversation_id"].as_str().expect("the conversation to open");
    assert_eq!(payload["aps"]["thread-id"], cid);
    assert!(payload["url"].as_str().unwrap().ends_with(&format!("/#/c/{cid}")));
    assert_eq!(pushes[0].1["apns-collapse-id"].to_str().unwrap(), asked.to_string());
    // An APNs failure is reported, not fatal.
    h.cedar.push_answers.lock().unwrap().insert(device_token('a'), [500].into());
    let (status, err) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "ios" }))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(err["error"]["message"].as_str().unwrap().contains("500"), "{err}");

    // The app turns push off, or the user removes a phone from the list.
    let (status, got) = h.json(Method::DELETE, "/api/notifications/apns/devices", Some(json!({ "token": device_token('a') }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["ios"]["devices"], json!([]));
    let got = register(device_token('c'), Some("iPad")).await;
    let id = got["device_id"].as_str().unwrap().to_string();
    let (status, got) = h.json(Method::DELETE, &format!("/api/notifications/apns/devices/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["ios"]["devices"], json!([]));
    let (status, _) = h.json(Method::DELETE, &format!("/api/notifications/apns/devices/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // With no phone left the method can't stay "ios".
    let (status, _) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "ios" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // A schedule can't pick it either.
    let (status, err) = h.json(Method::POST, "/api/schedules", Some(json!({ "title": "t", "prompt": "p", "frequency": "daily", "time": "09:00", "timezone": "UTC", "notify": "ios" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("no iPhone"), "{err}");
}

/// A Firebase service account's JSON file, with a new RSA key.
fn service_account() -> String {
    let key = openssl::pkey::PKey::from_rsa(openssl::rsa::Rsa::generate(2048).unwrap()).unwrap();
    let pem = String::from_utf8(key.private_key_to_pem_pkcs8().unwrap()).unwrap();
    json!({ "type": "service_account", "project_id": "cedar-test", "private_key_id": "kid-1", "private_key": pem, "client_email": "fcm@cedar-test.iam.gserviceaccount.com" }).to_string()
}

#[tokio::test]
async fn android_push_goes_through_fcm_and_unifiedpush_and_drops_dead_phones() {
    let h = Harness::new(&[]).await;
    let register = |body: Value| {
        let h = &h;
        async move { h.json(Method::POST, "/api/notifications/android/devices", Some(body)).await }
    };
    // Registration is checked: a known transport, and for UnifiedPush an https endpoint.
    let (status, err) = register(json!({ "transport": "carrier-pigeon", "token": "x" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("fcm or unifiedpush"), "{err}");
    let (status, err) = register(json!({ "transport": "unifiedpush", "token": "http://ntfy.example/upabc" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("only https endpoints"), "{err}");
    let (status, _) = register(json!({ "transport": "unifiedpush", "token": "not a url" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = register(json!({ "transport": "fcm", "token": "has spaces in it" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = register(json!({ "transport": "fcm", "token": "x".repeat(5000) })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, got) = register(json!({ "transport": "fcm", "token": "fcm-a", "name": "Pixel 8" })).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["device"]["transport"], "fcm");
    assert_eq!(got["device"]["name"], "Pixel 8");
    assert_eq!(got["device"]["id"], got["device_id"]);
    assert!(got["device"]["created_at"].is_string() && got["device"]["last_used_at"].is_null(), "{got}");
    assert!(got["device"].get("token").is_none() && !got.to_string().contains("fcm-a"), "tokens stay on the server");
    assert_eq!(got["android"]["devices"].as_array().unwrap().len(), 1);
    // The same phone again is one device.
    let (_, got) = register(json!({ "transport": "fcm", "token": "fcm-a", "name": "Pixel 9" })).await;
    assert_eq!(got["android"]["devices"].as_array().unwrap().len(), 1);
    assert_eq!(got["android"]["devices"][0]["name"], "Pixel 9");
    // A phone without Google's services, through ntfy. Its endpoint is https in real life; the
    // fake here is plain http, so it goes into the store directly.
    let ntfy = format!("{}/push/ntfy-1", h.base_url());
    h.state.store.add_android_device("unifiedpush", &ntfy, Some("GrapheneOS")).await.unwrap();
    register(json!({ "transport": "fcm", "token": "fcm-b" })).await;

    // FCM phones need the key; the UnifiedPush one doesn't.
    let (status, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["android"]["fcm_configured"], false);
    assert_eq!(got["android"]["configured"], true, "the UnifiedPush phone works without a key");
    let (status, err) = h.json(Method::PUT, "/api/notifications", Some(json!({ "android": { "service_account": "{\"project_id\":\"p\"}" } }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"]["message"].as_str().unwrap().contains("service-account key"), "{err}");
    let account = service_account();
    let (status, got) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "android", "android": { "service_account": account } }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["android"]["fcm_configured"], true);
    assert_eq!(got["android"]["key_set"], true);
    assert_eq!(got["android"]["project_id"], "cedar-test");
    assert!(!got.to_string().contains("PRIVATE KEY"), "the key is never sent back");
    assert_eq!(got["configured"], json!(["android"]));

    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "android" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert!(got["detail"].as_str().unwrap().contains("sent to 3 Android phones"), "{got}");
    // One access token, from a signed assertion, for both FCM pushes.
    let forms = h.cedar.google_tokens.lock().unwrap().clone();
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0]["grant_type"], "urn:ietf:params:oauth:grant-type:jwt-bearer");
    assert_eq!(forms[0]["assertion"].matches('.').count(), 2);
    let pushes = h.cedar.pushes.lock().unwrap().clone();
    assert_eq!(pushes.len(), 3);
    let fcm: Vec<_> = pushes.iter().filter(|(n, _, _)| n.starts_with("fcm-")).collect();
    assert_eq!(fcm.len(), 2);
    let (_, headers, body) = fcm[0];
    assert_eq!(headers["authorization"], "Bearer access-1");
    assert_eq!(headers["x-project"], "cedar-test");
    let body: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(body["message"]["data"], json!({ "kind": "test", "conversation_id": "", "tag": "test" }));
    assert_eq!(body["message"]["android"], json!({ "priority": "high", "collapse_key": "test" }));
    assert!(body["message"].get("notification").is_none(), "data only");
    let (_, headers, body) = pushes.iter().find(|(n, _, _)| n == "ntfy-1").expect("the ntfy push");
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(headers["ttl"], "86400");
    assert_eq!(headers["urgency"], "high");
    let payload: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(payload, json!({ "kind": "test", "conversation_id": "", "tag": "test" }));
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert!(got["android"]["devices"].as_array().unwrap().iter().all(|d| d["last_used_at"].is_string()), "{got}");
    assert_eq!(got["last_delivery"]["method"], "android");

    // A finished run: which conversation and turn, never what was said.
    h.cedar.pushes.lock().unwrap().clear();
    let turn = ended_turn(&h, "Secret plans", "completed", Some("ask_user")).await;
    let outcome = cedar_chat::channels::dispatch(&h.state, turn).await.unwrap().expect("sent");
    assert!(outcome.is_ok(), "{outcome:?}");
    let pushes = h.cedar.pushes.lock().unwrap().clone();
    assert_eq!(pushes.len(), 3);
    for (_, _, body) in &pushes {
        let text = String::from_utf8_lossy(body);
        assert!(!text.contains("Secret plans") && !text.contains("The answer is") && !text.contains("title") && !text.contains("body"), "{text}");
    }
    let payload: Value = serde_json::from_slice(&pushes.iter().find(|(n, _, _)| n == "ntfy-1").unwrap().2).unwrap();
    assert_eq!(payload["kind"], "turn");
    assert_eq!(payload["tag"], turn.to_string());
    assert!(Uuid::parse_str(payload["conversation_id"].as_str().unwrap()).is_ok(), "{payload}");
    assert_eq!(h.cedar.google_tokens.lock().unwrap().len(), 1, "the access token is reused");

    // FCM's 401 gets a new access token and one retry.
    h.cedar.push_answers.lock().unwrap().insert("fcm-a".into(), [401].into());
    h.cedar.pushes.lock().unwrap().clear();
    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "android" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert!(got["detail"].as_str().unwrap().contains("sent to 3"), "{got}");
    assert_eq!(h.cedar.google_tokens.lock().unwrap().len(), 2);

    // Dead phones are dropped: FCM's UNREGISTERED (404), a token FCM calls invalid (400), and
    // a UnifiedPush endpoint that is gone (410).
    h.cedar.push_answers.lock().unwrap().insert("fcm-a".into(), [404].into());
    h.cedar.push_answers.lock().unwrap().insert("ntfy-1".into(), [410].into());
    let (status, got) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "android" }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["detail"], "sent to 1 Android phone, 2 no longer registered");
    let (_, got) = h.json(Method::GET, "/api/notifications", None).await;
    assert_eq!(got["android"]["devices"].as_array().unwrap().len(), 1);
    h.cedar.push_answers.lock().unwrap().insert("fcm-b".into(), [500].into());
    let (status, err) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "android" }))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "a server error keeps the phone");
    assert!(err["error"]["message"].as_str().unwrap().contains("FCM answered 500"), "{err}");
    h.cedar.push_answers.lock().unwrap().insert("fcm-b".into(), [400].into());
    let (status, err) = h.json(Method::POST, "/api/notifications/test", Some(json!({ "method": "android" }))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(err["error"]["message"].as_str().unwrap().contains("every registered Android phone has gone"), "{err}");

    // The app turns push off, or the user removes a phone from the list.
    let (_, got) = register(json!({ "transport": "unifiedpush", "token": "https://ntfy.example/upxyz?up=1", "name": "Fairphone" })).await;
    let id = got["device_id"].as_str().unwrap().to_string();
    register(json!({ "transport": "fcm", "token": "fcm-c" })).await;
    let (status, got) = h.json(Method::DELETE, "/api/notifications/android/devices", Some(json!({ "token": "fcm-c" }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["android"]["devices"].as_array().unwrap().len(), 1);
    let (status, got) = h.json(Method::DELETE, &format!("/api/notifications/android/devices/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["android"]["devices"], json!([]));
    let (status, _) = h.json(Method::DELETE, &format!("/api/notifications/android/devices/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // With no phone left the method can't be picked, by default or by a schedule.
    let (status, _) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "android" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, err) = h.json(Method::POST, "/api/schedules", Some(json!({ "title": "t", "prompt": "p", "frequency": "daily", "time": "09:00", "timezone": "UTC", "notify": "android" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("no Android phone"), "{err}");
    // An empty key clears it.
    let (status, got) = h.json(Method::PUT, "/api/notifications", Some(json!({ "default_method": "none", "android": { "service_account": "" } }))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!((got["android"]["key_set"].as_bool(), got["android"]["project_id"].as_str()), (Some(false), Some("")));
}

#[tokio::test]
async fn an_unreachable_cedar_fails_the_sync_without_moving_the_cursor() {
    let h = Harness::new(&[("CEDAR_URL", "http://127.0.0.1:1")]).await;
    assert!(notify::sync(&h.state, &CancellationToken::new()).await.is_err());
    assert_eq!(h.state.store.notifications_cursor().await.unwrap(), 0);
}

#[tokio::test]
async fn settings_load_without_cedar() {
    let h = Harness::new(&[("CEDAR_URL", "http://127.0.0.1:1")]).await;
    let (status, got) = h.json(Method::GET, "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["defaults"], Value::Null);
    assert_eq!(got["tools"], Value::Null);
}

// -- plans and approvals ----------------------------------------------------------------------

#[tokio::test]
async fn plans_and_approvals_are_answered_through_the_turn() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let cid = conv["id"].as_str().unwrap();
    let (_, turn) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "WAIT for me" }))).await;
    let tid = turn["id"].as_str().unwrap();
    let run_id = turn["run_id"].as_str().unwrap();

    let steps = json!({ "steps": [{ "id": "1", "title": "Search" }] });
    let (status, body) = h.json(Method::POST, &format!("/api/turns/{tid}/plan"), Some(steps.clone())).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let (status, _) = h.json(Method::POST, &format!("/api/turns/{tid}/approvals/a1"), Some(json!({ "decision": "allow_always" }))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, _) = h.json(Method::POST, &format!("/api/turns/{tid}/approvals/a1"), Some(json!({ "decision": "yes" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        *h.cedar.answers.lock().unwrap(),
        [("plan".to_string(), steps), ("approvals/a1".to_string(), json!({ "decision": "allow_always" }))]
    );

    // cedar's refusal comes through, so the page can say the run moved on.
    h.finish(run_id, "completed", Some("ok"));
    let (_, turn) = h.json(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": "go on" }))).await;
    let tid = turn["id"].as_str().unwrap();
    let (status, body) = h.json(Method::POST, &format!("/api/turns/{tid}/plan"), Some(json!({ "steps": ["x"] }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("not waiting"), "{body}");
    let (status, _) = h.json(Method::POST, "/api/turns/00000000-0000-0000-0000-000000000000/plan", Some(json!({ "steps": [] }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn community_tools_pass_through_to_cedar_with_checked_names() {
    let h = Harness::new(&[
        ("CHAT_ACCESS_TOKEN", "access"),
        ("CHAT_ADMIN_TOKEN", "admin"),
    ])
    .await;
    let accept =
        json!({ "accept": { "network": [], "secrets": [], "pip": [], "health_data": false } });
    let (status, _) = admin_call(
        &h,
        Method::POST,
        "/api/integrations/marketplace/glucose/install",
        None,
        Some(accept.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = admin_call(
        &h,
        Method::POST,
        "/api/integrations/marketplace/glucose/install",
        Some("admin"),
        Some(accept.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "marketplace");

    // Names that cedar's URLs couldn't take never reach it.
    for bad in ["Glucose", "a..b", "x%2F..%2Fy"] {
        let uri = format!("/api/integrations/marketplace/{bad}/install");
        let (status, _) =
            admin_call(&h, Method::POST, &uri, Some("admin"), Some(accept.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, _) = admin_call(
        &h,
        Method::PUT,
        "/api/integrations/tools/glucose/secrets/TOKEN",
        Some("admin"),
        Some(json!({ "value": "v" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin_call(
        &h,
        Method::PUT,
        "/api/integrations/tools/glucose/secrets/token",
        Some("admin"),
        Some(json!({ "value": "v" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let calls: Vec<String> = h
        .cedar
        .admin_calls
        .lock()
        .unwrap()
        .iter()
        .map(|(r, _)| r.clone())
        .collect();
    assert_eq!(calls, ["install_glucose", "secret_glucose_TOKEN"]);
    assert_eq!(h.cedar.admin_calls.lock().unwrap()[0].1, accept);
}

// -- updates ------------------------------------------------------------------------------------

#[tokio::test]
async fn a_newer_release_is_offered_and_handed_to_the_updater() {
    let dir = std::env::temp_dir().join(format!("cedar-chat-update-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let h = Harness::new(&[("CHAT_ADMIN_TOKEN", "adm"), ("CHAT_UPDATE_DIR", dir.to_str().unwrap())]).await;
    let current = env!("CARGO_PKG_VERSION");

    // Nothing known yet: no banner, nothing to install.
    let (status, body) = h.json(Method::GET, "/api/update", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["current"], current);
    assert_eq!(body["available"], false);
    assert_eq!(body["can_install"], true);
    assert_eq!(body["install"]["state"], "idle");
    assert_eq!(admin_call(&h, Method::POST, "/api/update", Some("adm"), None).await.0, StatusCode::CONFLICT);

    // The same release as this build is not an update.
    *h.cedar.release_tag.lock().unwrap() = Some(format!("v{current}"));
    let (_, body) = h.json(Method::POST, "/api/update/check", None).await;
    assert_eq!(body["latest"]["version"], current);
    assert_eq!(body["available"], false);
    assert!(body["commands"].is_null());

    // A newer one is. "Check now" right after the last check answers from memory.
    *h.cedar.release_tag.lock().unwrap() = Some("v999.0.0".into());
    let (_, body) = h.json(Method::POST, "/api/update/check", None).await;
    assert_eq!(body["available"], false);
    assert_eq!(*h.cedar.release_lookups.lock().unwrap(), 1);
    cedar_chat::update::check(&h.state, true).await.unwrap();
    let (_, body) = h.json(Method::GET, "/api/update", None).await;
    assert_eq!(body["available"], true);
    assert_eq!(body["latest"]["version"], "999.0.0");
    assert_eq!(body["latest"]["notes_url"], "https://example.invalid/releases/v999.0.0");
    assert_eq!(body["commands"], "sudo cedar-update 999.0.0");
    // Asked again, GitHub says it is unchanged (304) and the release is kept.
    cedar_chat::update::check(&h.state, true).await.unwrap();
    assert_eq!(h.json(Method::GET, "/api/update", None).await.1["latest"]["version"], "999.0.0");

    // Only an admin starts the install, and then the request is waiting for the unit.
    assert_eq!(admin_call(&h, Method::POST, "/api/update", None, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(admin_call(&h, Method::POST, "/api/update", Some("wrong"), None).await.0, StatusCode::FORBIDDEN);
    let (status, body) = admin_call(&h, Method::POST, "/api/update", Some("adm"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["install"]["state"], "requested");
    assert_eq!(body["install"]["version"], "999.0.0");
    let request: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("request")).unwrap()).unwrap();
    assert_eq!(request["version"], "999.0.0");
    assert_eq!(admin_call(&h, Method::POST, "/api/update", Some("adm"), None).await.0, StatusCode::CONFLICT);

    // The unit takes the request and reports its progress; a failure can be tried again.
    std::fs::remove_file(dir.join("request")).unwrap();
    std::fs::write(dir.join("status.json"), r#"{"state":"verifying","version":"999.0.0"}"#).unwrap();
    assert_eq!(h.json(Method::GET, "/api/update", None).await.1["install"]["state"], "verifying");
    assert_eq!(admin_call(&h, Method::POST, "/api/update", Some("adm"), None).await.0, StatusCode::CONFLICT);
    std::fs::write(dir.join("status.json"), r#"{"state":"failed","version":"999.0.0","detail":"checksum mismatch"}"#).unwrap();
    let (_, body) = h.json(Method::GET, "/api/update", None).await;
    assert_eq!((body["install"]["state"].as_str(), body["install"]["detail"].as_str()), (Some("failed"), Some("checksum mismatch")));
    assert_eq!(admin_call(&h, Method::POST, "/api/update", Some("adm"), None).await.0, StatusCode::ACCEPTED);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn without_the_updater_the_banner_shows_the_commands() {
    let h = Harness::new(&[("CHAT_ADMIN_TOKEN", "adm")]).await;
    *h.cedar.release_tag.lock().unwrap() = Some("v999.0.0".into());
    cedar_chat::update::check(&h.state, true).await.unwrap();
    let (_, body) = h.json(Method::GET, "/api/update", None).await;
    assert_eq!(body["available"], true);
    assert_eq!(body["can_install"], false);
    if cfg!(target_os = "linux") && matches!(std::env::consts::ARCH, "x86_64" | "aarch64") {
        let commands = body["commands"].as_str().unwrap();
        assert!(commands.contains("sha256sum -c --ignore-missing checksums.txt"), "{commands}");
        assert!(commands.contains("sudo systemctl restart cedar cedar-chat"), "{commands}");
    }
    let (status, body) = admin_call(&h, Method::POST, "/api/update", Some("adm"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"]["message"].as_str().unwrap().contains("CHAT_UPDATE_DIR"));
}

#[tokio::test]
async fn the_update_status_needs_the_access_token() {
    let h = Harness::new(&[("CHAT_ACCESS_TOKEN", "tok")]).await;
    assert_eq!(h.call(Method::GET, "/api/update", None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(h.call(Method::GET, "/api/update", Some("tok"), None).await.0, StatusCode::OK);
    let (status, page) = h.call(Method::GET, "/update.js", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("cedarUpdate"));
}
