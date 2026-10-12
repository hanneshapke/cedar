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

struct Harness {
    app: Router,
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
        Self { app: api::router(state), cedar }
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
    let h = Harness::new(&[("CHAT_MODELS", "model-a,model-b")]).await;
    let (_, conv) = h.json(Method::POST, "/api/conversations", Some(json!({}))).await;
    let msgs = format!("/api/conversations/{}/messages", conv["id"].as_str().unwrap());

    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "   " }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "hi", "model": "model-z" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, err) = h.json(Method::POST, &msgs, Some(json!({ "content": "TOO MANY" }))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(err["error"]["message"], "at most 2 runs");
    let (status, _) = h.json(Method::POST, &msgs, Some(json!({ "content": "hi", "model": "model-b" }))).await;
    assert_eq!(status, StatusCode::CREATED);

    let (_, cfg) = h.json(Method::GET, "/api/config", None).await;
    assert_eq!(cfg, json!({ "auth_required": false, "models": ["model-a", "model-b"] }));
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
            assert!(page.contains("<script src=\"/map-view.js\">"), "{uri} loads the map view");
            assert!(page.contains("href=\"/map-view.css\""), "{uri} loads the map styles");
            assert!(page.contains("id=\"settings-dialog\""), "{uri} has the Settings menu");
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
        ("/vendor/leaflet/leaflet.js", "text/javascript; charset=utf-8", "Leaflet 1.9.4"),
        ("/vendor/leaflet/leaflet.css", "text/css; charset=utf-8", ".leaflet-container"),
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
