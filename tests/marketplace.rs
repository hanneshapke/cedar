//! The community tool marketplace against a local mock registry: browsing, installing with the
//! permissions accepted, refusing tampered releases and name clashes, per-tool secrets, the
//! approval every call of a community tool waits for, the health rules, and uninstalling.

mod common;

use axum::extract::{Path, State};
use axum::http::{Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use common::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const SECRET_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ADMIN: &str = KEY_B;

type Files = Arc<Mutex<HashMap<String, Vec<u8>>>>;

const MANIFEST: &str = r#"{
  "description": "Summarize glucose readings.",
  "version": "1.0.0",
  "author": "Ada",
  "network": ["api.example.com"],
  "secrets": ["GLUCO_TOKEN", {"name": "GLUCO_REGION", "optional": true}],
  "pip": ["requests>=2"],
  "health_data": true
}"#;

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn permissions() -> Value {
    json!({ "network": ["api.example.com"], "secrets": ["GLUCO_TOKEN", "GLUCO_REGION"],
            "pip": ["requests>=2"], "health_data": true })
}

/// The registry's files, served at `/index.json` and `/tools/<name>/<file>`, and the index
/// listing `glucose` with the hashes of what is served (taken when this is called).
fn release(files: &Files, manifest: &str, version: &str) -> Value {
    let mut f = files.lock().unwrap();
    f.insert("glucose/tool.json".into(), manifest.as_bytes().to_vec());
    f.insert("glucose/tool.py".into(), b"print('hi')".to_vec());
    let index = json!({
        "version": 1,
        "tools": [{
            "name": "glucose", "version": version, "summary": "Glucose summary",
            "category": "health", "author": "Ada", "permissions": permissions(),
            "base_url": "tools/glucose/",
            "files": { "tool.json": sha(&f["glucose/tool.json"]), "tool.py": sha(&f["glucose/tool.py"]) }
        }, {
            "name": "Bad Name", "version": "1", "summary": "listed wrong", "base_url": "x/",
            "files": { "tool.json": "00" }
        }]
    });
    f.insert("index.json".into(), index.to_string().into_bytes());
    index
}

async fn registry(files: Files) -> String {
    async fn index(State(f): State<Files>) -> impl IntoResponse {
        f.lock().unwrap()["index.json"].clone()
    }
    async fn file(
        State(f): State<Files>,
        Path((tool, name)): Path<(String, String)>,
    ) -> impl IntoResponse {
        match f.lock().unwrap().get(&format!("{tool}/{name}")) {
            Some(b) => (StatusCode::OK, b.clone()),
            None => (StatusCode::NOT_FOUND, vec![]),
        }
    }
    let app = Router::new()
        .route("/index.json", get(index))
        .route("/tools/{tool}/{name}", get(file))
        .with_state(files);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/index.json")
}

fn tools_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cedar-market-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn app(script: Vec<Step>, dir: &std::path::Path, url: &str) -> TestApp {
    TestApp::with_env(
        script,
        &[
            ("CEDAR_ADMIN_KEYS", ADMIN),
            ("CEDAR_SECRET_KEY", SECRET_KEY),
            ("TOOLS_DIR", dir.to_str().unwrap()),
            ("TOOLS_REGISTRY_URL", url),
        ],
    )
    .await
}

async fn install(app: &TestApp, accept: Value) -> (StatusCode, Value) {
    app.json(
        Method::POST,
        "/v1/admin/marketplace/glucose/install",
        Some(ADMIN),
        Some(json!({ "accept": accept })),
    )
    .await
}

#[tokio::test]
async fn a_tool_is_browsed_installed_given_its_secrets_and_uninstalled() {
    let files: Files = Arc::default();
    release(&files, MANIFEST, "1.0.0");
    let url = registry(files.clone()).await;
    let dir = tools_dir();
    let app = app(vec![], &dir, &url).await;

    // Only admins browse; listings that can't be installed are left out.
    let (status, _) = app.get("/v1/admin/marketplace", KEY_A).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, list) = app.get("/v1/admin/marketplace", ADMIN).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["installable"], true);
    let tools = list["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1, "{list}");
    assert_eq!(tools[0]["name"], "glucose");
    assert_eq!(tools[0]["installed"], Value::Null);
    assert_eq!(tools[0]["permissions"], permissions());

    // The admin must accept exactly what the release asks for.
    let mut fewer = permissions();
    fewer["network"] = json!([]);
    let (status, err) = install(&app, fewer).await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert!(!dir.join("glucose").exists());

    let (status, info) = install(&app, permissions()).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    assert_eq!(info["source"], "marketplace");
    assert_eq!(info["version"], "1.0.0");
    assert_eq!(info["installed"]["version"], "1.0.0");
    assert_eq!(info["secrets"][0]["saved"], false);
    assert!(dir.join("glucose/tool.py").is_file());
    assert!(dir.join("glucose/.cedar-install.json").is_file());
    assert_eq!(
        *app.factory.tool_setups.lock().unwrap(),
        [("glucose".to_string(), vec!["requests>=2".to_string()])],
        "its packages went into its own environment"
    );
    let (_, list) = app.get("/v1/admin/marketplace", ADMIN).await;
    assert_eq!(list["tools"][0]["installed"]["version"], "1.0.0");
    assert_eq!(list["tools"][0]["installed"]["update_available"], false);

    // It is now a personal tool runs can use.
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    let custom = catalog["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "custom")
        .unwrap();
    assert_eq!(custom["tools"][0]["name"], "glucose");

    // Secrets: only declared ones, never shown back.
    let secret = |name: &str, value: &str| {
        let app = app.clone();
        let uri = format!("/v1/admin/tools/glucose/secrets/{name}");
        let body = json!({ "value": value });
        async move { app.json(Method::PUT, &uri, Some(ADMIN), Some(body)).await }
    };
    let (status, _) = secret("OTHER", "x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, all) = secret("GLUCO_TOKEN", "tok-123").await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert!(!all.to_string().contains("tok-123"));
    let (_, all) = app.get("/v1/admin/tools", ADMIN).await;
    assert_eq!(all["tools"][0]["secrets"][0]["saved"], true);

    // A new release shows up as an update; installing it replaces the old one.
    release(&files, &MANIFEST.replace("1.0.0", "1.1.0"), "1.1.0");
    let (_, list) = app.get("/v1/admin/marketplace?refresh=true", ADMIN).await;
    assert_eq!(list["tools"][0]["installed"]["update_available"], true);
    let (status, info) = install(&app, permissions()).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    assert_eq!(info["version"], "1.1.0");
    assert_eq!(
        info["secrets"][0]["saved"], true,
        "secrets survive an update"
    );

    // Uninstalling removes the folder, the environment, and the secrets.
    let (status, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/marketplace/glucose",
            Some(ADMIN),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!dir.join("glucose").exists());
    assert_eq!(*app.factory.removed_tool_envs.lock().unwrap(), ["glucose"]);
    let (_, all) = app.get("/v1/admin/tools", ADMIN).await;
    assert_eq!(all["tools"], json!([]));
    let secrets = cedar::secrets::SecretStore::new(
        app.state.store.clone(),
        &hex::decode(SECRET_KEY).unwrap().try_into().unwrap(),
    );
    assert!(secrets
        .names_with_prefix("tool_secret:glucose:")
        .await
        .unwrap()
        .is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn tampered_releases_and_name_clashes_are_refused() {
    let files: Files = Arc::default();
    release(&files, MANIFEST, "1.0.0");
    let url = registry(files.clone()).await;
    let dir = tools_dir();
    let app = app(vec![], &dir, &url).await;

    // A file that doesn't match the index's hash.
    files.lock().unwrap().insert(
        "glucose/tool.py".into(),
        b"import os; os.system('evil')".to_vec(),
    );
    let (status, err) = install(&app, permissions()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{err}");
    assert!(err.to_string().contains("hash"), "{err}");
    assert!(!dir.join("glucose").exists());
    assert!(app.factory.tool_setups.lock().unwrap().is_empty());

    // A manifest asking for more than its listing shows (the index hashes it, so it is served).
    release(
        &files,
        &MANIFEST.replace(
            r#"["api.example.com"]"#,
            r#"["api.example.com", "evil.example"]"#,
        ),
        "1.0.0",
    );
    let (status, err) = install(&app, permissions()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{err}");
    assert!(err.to_string().contains("permissions"), "{err}");

    // A local tool with the name is never replaced, and can't be uninstalled here.
    release(&files, MANIFEST, "1.0.0");
    std::fs::create_dir_all(dir.join("glucose")).unwrap();
    std::fs::write(dir.join("glucose/tool.json"), r#"{"description": "mine"}"#).unwrap();
    std::fs::write(dir.join("glucose/tool.py"), "print('mine')").unwrap();
    app.state.integrations.reload().await.unwrap();
    let (_, list) = app.get("/v1/admin/marketplace", ADMIN).await;
    assert_eq!(list["tools"][0]["conflict"], "a local tool has this name");
    let (status, _) = install(&app, permissions()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        std::fs::read_to_string(dir.join("glucose/tool.py")).unwrap(),
        "print('mine')"
    );
    let (status, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/marketplace/glucose",
            Some(ADMIN),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn without_a_registry_there_is_no_marketplace() {
    let dir = tools_dir();
    let app = TestApp::with_env(
        vec![],
        &[
            ("CEDAR_ADMIN_KEYS", ADMIN),
            ("TOOLS_DIR", dir.to_str().unwrap()),
        ],
    )
    .await;
    let (status, err) = app.get("/v1/admin/marketplace", ADMIN).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(err.to_string().contains("TOOLS_REGISTRY_URL"), "{err}");
    std::fs::remove_dir_all(dir).unwrap();
}

/// Follow a run's events until one of `kind` arrives.
async fn wait_for(app: &TestApp, id: &str, kind: &str) -> SseEvent {
    let mut stream = LiveStream::open(app, KEY_A, id, None).await;
    let found = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(e) = stream.next_event().await {
            if e.data["type"] == kind {
                return Some(e);
            }
        }
        None
    })
    .await;
    found
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("no {kind} event"))
}

#[tokio::test]
async fn a_community_tool_waits_for_approval_gets_its_secrets_and_brings_the_health_rules() {
    let files: Files = Arc::default();
    release(&files, MANIFEST, "1.0.0");
    let url = registry(files.clone()).await;
    let dir = tools_dir();
    let app = app(
        vec![
            Step::Reply(tool_use("t1", "glucose", json!({ "hours": 24 }))),
            Step::Reply(text("Your day looked steady.")),
            Step::Reply(tool_use("t2", "glucose", json!({ "hours": 12 }))),
            Step::Reply(text("Still steady.")),
        ],
        &dir,
        &url,
    )
    .await;
    let (status, _) = install(&app, permissions()).await;
    assert_eq!(status, StatusCode::OK);
    app.json(
        Method::PUT,
        "/v1/admin/tools/glucose/secrets/GLUCO_TOKEN",
        Some(ADMIN),
        Some(json!({ "value": "tok-123" })),
    )
    .await;

    let run = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "how was my day", "interactive": true }),
        )
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    let asked = wait_for(&app, &id, "approval.required").await;
    let approval = &asked.data["data"];
    assert_eq!(approval["rule"], "community");
    assert_eq!(approval["tool"], "glucose");
    assert_eq!(approval["scope"], "glucose 1.0.0");
    let reason = approval["reason"].as_str().unwrap();
    assert!(
        reason.contains("api.example.com") && reason.contains("health data"),
        "{reason}"
    );
    let aid = approval["id"].as_str().unwrap();
    let (status, _) = app
        .json(
            Method::POST,
            &format!("/v1/runs/{id}/approvals/{aid}"),
            Some(KEY_A),
            Some(json!({ "decision": "allow_always" })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    app.wait_status(KEY_A, &id, "completed").await;

    let calls = app.factory.sandbox(0).tool_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].dir, dir.join("glucose"));
    assert!(calls[0].network, "it declares a host");
    assert_eq!(
        calls[0].env,
        [
            ("GLUCO_TOKEN".to_string(), "tok-123".to_string()),
            ("CEDAR_LOCALE".to_string(), "en".to_string()),
            ("CEDAR_LANGUAGE".to_string(), "en".to_string()),
        ],
        "only its own saved secret, and the run's locale"
    );

    // The system prompt carries the health rules and the community-tool warning.
    let system = app.llm.requests()[0]["system"].to_string();
    assert!(system.contains("not medical advice"), "{system}");
    assert!(
        system.contains("never recommend medication or insulin doses"),
        "{system}"
    );
    assert!(system.contains("community tool marketplace"), "{system}");

    // "Always" holds for this release: the next run doesn't ask.
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "and now", "interactive": true }))
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    let events = app.events(KEY_A, &id, None).await;
    assert!(events.iter().all(|e| e.data["type"] != "approval.required"));
    std::fs::remove_dir_all(dir).unwrap();
}
