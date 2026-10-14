//! End-to-end tests of the chat API against a fake cedar server on a local port.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use cedar_chat::api::{self, AppState};
use cedar_chat::config::Config;
use cedar_chat::schedule;
use cedar_chat::store::Store;
use chrono::{DateTime, Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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
    if body["attachments"].as_array().is_some_and(|a| !a.is_empty()) && entry["multimodal"] != true {
        return cedar_error(StatusCode::BAD_REQUEST, "bad_request", &format!("model {model:?} does not accept images or PDFs"));
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
        json!({ "id": "model-a", "label": "Model A", "model": "org/model-a", "reasoning_levels": ["none", "high"], "default_reasoning": null, "multimodal": true, "is_default": true }),
        json!({ "id": "model-b", "label": "Model B", "model": "org/model-b", "reasoning_levels": [], "default_reasoning": null, "multimodal": false, "is_default": false }),
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
            .route("/v1/runs/{id}/events", get(events))
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
            .route("/v1/admin/integrations", get(integrations))
            .route(
                "/v1/admin/integrations/brave",
                axum::routing::put(put_brave).delete(|State(s): State<Shared>| delete_integration(s, "brave")),
            )
            .route("/v1/admin/integrations/google", axum::routing::delete(|State(s): State<Shared>| delete_integration(s, "google")))
            .route("/v1/admin/integrations/google/authorize", post(authorize))
            .route("/v1/admin/integrations/google/callback", post(callback))
            .with_state(cedar.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

        let mut env: HashMap<String, String> =
            [("CEDAR_URL", url.as_str()), ("CEDAR_API_KEY", CEDAR_KEY)].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
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
async fn attachments_are_refused_by_cedar_for_a_model_that_takes_none() {
    let h = Harness::new(&[]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let msgs = format!("/api/conversations/{}/messages", conv["id"].as_str().unwrap());
    // model-b is registered without "accepts images and PDFs": cedar's verdict is relayed.
    let (status, err) = h.json(Method::POST, &msgs, Some(json!({ "content": "look", "model": "model-b", "attachments": [png()] }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("does not accept images or PDFs"), "{err}");
    assert!(h.cedar.bodies.lock().unwrap().is_empty(), "no run was started");
    // An empty list is the same as none.
    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "look", "model": "model-b", "attachments": [] }))).await;
    assert_eq!(status, StatusCode::CREATED);
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

async fn fetch(h: &Harness, uri: &str) -> (StatusCode, HeaderMap, String) {
    h.call_full(Request::builder().uri(uri).body(Body::empty()).unwrap()).await
}

#[tokio::test]
async fn both_uis_are_served() {
    let h = Harness::new(&[]).await;

    for ui in ["v1", "v2"] {
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
            assert!(page.contains("<script src=\"/models.js\">"), "{uri} loads the model registry script");
            assert!(
                page.find("/models.js") < page.find("/schedule.js") && page.find("/models.js") < page.find("/settings.js"),
                "{uri} loads the models before the scripts that fill the pickers from them"
            );
            assert!(page.contains("id=\"reasoning\""), "{uri} has a reasoning picker");
            assert!(page.contains("<script src=\"/schedule.js\">"), "{uri} loads the scheduling script");
            assert!(page.contains("href=\"/schedule.css\""), "{uri} loads the scheduling styles");
            assert!(page.contains("<script src=\"/attach.js\">"), "{uri} loads the attachments script");
            assert!(page.contains("href=\"/attach.css\""), "{uri} loads the attachments styles");
            assert!(page.contains("id=\"settings-dialog\""), "{uri} has the Settings menu");
            assert!(page.contains("id=\"schedules-section\""), "{uri} lists scheduled agents in Settings");
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
        ("/image-view.js", "text/javascript; charset=utf-8", "cedarImages"),
        ("/image-view.css", "text/css; charset=utf-8", ".cedar-image"),
        ("/vendor/leaflet/leaflet.js", "text/javascript; charset=utf-8", "Leaflet 1.9.4"),
        ("/vendor/leaflet/leaflet.css", "text/css; charset=utf-8", ".leaflet-container"),
        ("/models.js", "text/javascript; charset=utf-8", "cedarModels"),
        ("/schedule.js", "text/javascript; charset=utf-8", "cedarSchedule"),
        ("/schedule.css", "text/css; charset=utf-8", ".cedar-sched"),
        ("/attach.js", "text/javascript; charset=utf-8", "cedarAttach"),
        ("/attach.css", "text/css; charset=utf-8", ".cedar-attach"),
    ] {
        let (status, headers, body) = fetch(&h, uri).await;
        assert_eq!((status, headers["content-type"].to_str().unwrap()), (StatusCode::OK, content_type), "{uri}");
        assert!(body.contains(marker), "{uri} is the expected file");
    }

    // The single-UI paths are gone.
    assert_eq!(fetch(&h, "/app.js").await.0, StatusCode::NOT_FOUND);
    assert_eq!(fetch(&h, "/style.css").await.0, StatusCode::NOT_FOUND);
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
    assert_eq!(prompts.last().map(String::as_str), Some("DO: summarize the news"));
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
async fn runs_get_the_default_settings_and_a_conversations_own_tools() {
    let h = Harness::new(&[]).await;

    // Without settings every tool is asked for explicitly, so a continuation can't keep a
    // narrower choice from before.
    let (cid, _) = h.finished_conversation("first", "ok").await;
    let first = h.cedar.bodies.lock().unwrap().last().cloned().unwrap();
    assert_eq!(first["tools"], json!(["*"]));
    assert!(first.get("system").is_none() && first.get("model").is_none());

    let (status, prefs) = h
        .json(
            Method::PUT,
            "/api/settings",
            Some(json!({ "tools": ["weather", "web_search", "weather"], "model": "model-b", "system": "  be brief  ", "max_turns": 7 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(prefs, json!({ "tools": ["weather", "web_search"], "model": "model-b", "system": "be brief", "max_turns": 7 }));
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
    assert!(second["parent_id"].is_string());

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
    let calls = h.cedar.admin_calls.lock().unwrap().clone();
    let routes: Vec<&str> = calls.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(routes, ["get", "put_brave", "put_brave", "delete_brave", "delete_google", "get_models", "put_model:gpu", "put_model:gpu", "delete_model:gpu"]);
    assert_eq!(calls[6].1, spec, "the body goes through as it is");

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

#[tokio::test]
async fn settings_load_without_cedar() {
    let h = Harness::new(&[("CEDAR_URL", "http://127.0.0.1:1")]).await;
    let (status, got) = h.json(Method::GET, "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["defaults"], Value::Null);
    assert_eq!(got["tools"], Value::Null);
}
