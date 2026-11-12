//! `POST /v1/tools/{name}/call`: one tool called outside a run, for clients that watch something
//! and start a run only when it changed. Gmail and Calendar calls are in `tests/google_api.rs`,
//! which has the mock of Google's APIs.

mod common;

use axum::http::{header, Method, StatusCode};
use axum::routing::get;
use axum::Router;
use common::*;
use serde_json::{json, Value};
use std::path::PathBuf;
use uuid::Uuid;

async fn start() -> String {
    let big: Vec<Value> = (0..4000)
        .map(|i| json!({ "n": i, "text": "padding to make this long" }))
        .collect();
    let big = json!({ "items": big }).to_string();
    assert!(big.len() > 100_000, "longer than a run's output cap");
    let app = Router::new()
        .route(
            "/price.json",
            get(|| async { ([(header::CONTENT_TYPE, "application/json")], r#"{"price": {"eur": 41.5}}"#) }),
        )
        .route(
            "/page",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    "<html><head><script>nonce('x1')</script></head><body><h1>In stock</h1></body></html>",
                )
            }),
        )
        .route(
            "/plain",
            get(|| async { ([(header::CONTENT_TYPE, "text/plain")], " 17\n") }),
        )
        .route(
            "/big.json",
            get(move || {
                let big = big.clone();
                async move { ([(header::CONTENT_TYPE, "application/json")], big) }
            }),
        )
        .route(
            "/huge",
            get(|| async { ([(header::CONTENT_TYPE, "text/plain")], "x".repeat(2 << 20)) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn call(app: &TestApp, name: &str, input: Value) -> (StatusCode, Value) {
    app.json(
        Method::POST,
        &format!("/v1/tools/{name}/call"),
        Some(KEY_A),
        Some(json!({ "input": input })),
    )
    .await
}

fn tools_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cedar-poll-{}", Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("air")).unwrap();
    std::fs::write(
        dir.join("air/tool.json"),
        r#"{ "description": "Air quality.", "input_schema": { "type": "object" } }"#,
    )
    .unwrap();
    std::fs::write(dir.join("air/tool.py"), "print('{}')").unwrap();
    dir
}

#[tokio::test]
async fn only_pollable_tools_can_be_called() {
    let app = TestApp::new(vec![]).await;
    // The catalog says which tools can be, and that web addresses can.
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    let tools: Vec<&Value> = catalog["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["tools"].as_array().unwrap())
        .collect();
    let pollable = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap()["pollable"].clone();
    assert_eq!(pollable("weather_forecast"), true);
    assert_eq!(pollable("fetch_url"), false);
    assert_eq!(catalog["sources"][0]["name"], "url");

    for name in ["bash", "write_file", "fetch_url", "download_file", "remember", "records_save"] {
        let (status, body) = call(&app, name, json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
        assert!(body["error"]["message"].as_str().unwrap().contains("outside a run"));
    }
    let (status, _) = call(&app, "no_such_tool", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .json(Method::POST, "/v1/tools/url/call", None, Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = app
        .json(Method::POST, "/v1/tools/url/call", Some(KEY_A), Some(json!({ "input": [1] })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nothing ran: no sandbox, no model call.
    assert!(app.factory.created.lock().unwrap().is_empty());
    assert_eq!(app.llm.call_count(), 0);
}

#[tokio::test]
async fn a_url_poll_returns_the_body_and_refuses_private_addresses() {
    let base = start().await;
    let app = TestApp::with_env(vec![], &[("WEB_FETCH_ALLOW_PRIVATE", "on")]).await;

    let (status, body) = call(&app, "url", json!({ "url": format!("{base}/price.json") })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_error"], false);
    assert_eq!(body["json"]["price"]["eur"], 41.5);
    assert_eq!(body["truncated"], false);

    // A page is reduced to its text, so markup that changes per load is no change.
    let (_, body) = call(&app, "url", json!({ "url": format!("{base}/page"), "as": "text" })).await;
    assert_eq!(body["output"], "# In stock", "{body}");
    let (_, body) = call(&app, "url", json!({ "url": format!("{base}/plain") })).await;
    assert_eq!(body["output"], "17");
    assert_eq!(body["json"], 17);

    // Asked for JSON, it has to be JSON; too large is an error rather than a cut-off value.
    let (_, body) = call(&app, "url", json!({ "url": format!("{base}/page"), "as": "json" })).await;
    assert_eq!(body["is_error"], true);
    assert!(body["output"].as_str().unwrap().contains("did not return JSON"), "{body}");
    let (_, body) = call(&app, "url", json!({ "url": format!("{base}/huge") })).await;
    assert_eq!(body["is_error"], true);
    assert!(body["output"].as_str().unwrap().contains("larger than"), "{body}");
    let (_, body) = call(&app, "url", json!({})).await;
    assert_eq!(body["is_error"], true);

    // Without the test's exemption, an address on this machine is refused before any request.
    let guarded = TestApp::new(vec![]).await;
    let (status, body) = call(&guarded, "url", json!({ "url": format!("{base}/price.json") })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["is_error"], true, "{body}");
    let (_, body) = call(&guarded, "url", json!({ "url": "http://169.254.169.254/latest" })).await;
    assert_eq!(body["is_error"], true, "{body}");

    // With web fetching off there is no such source.
    let off = TestApp::with_env(vec![], &[("WEB_FETCH", "off")]).await;
    let (status, _) = call(&off, "url", json!({ "url": format!("{base}/price.json") })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, catalog) = off.get("/v1/tools", KEY_A).await;
    assert_eq!(catalog["sources"], json!([]));
}

#[tokio::test]
async fn long_json_is_not_cut_into_something_that_no_longer_parses() {
    let base = start().await;
    let app = TestApp::with_env(vec![], &[("WEB_FETCH_ALLOW_PRIVATE", "on")]).await;
    let (_, body) = call(&app, "url", json!({ "url": format!("{base}/big.json") })).await;
    assert_eq!(body["is_error"], false, "{}", body["output"].as_str().unwrap_or("").len());
    assert_eq!(body["json"]["items"].as_array().unwrap().len(), 4000);
    assert_eq!(body["json"]["items"][3999]["n"], 3999);
}

#[tokio::test]
async fn a_personal_tool_gets_a_sandbox_and_workspace_for_the_one_call() {
    let dir = tools_dir();
    let app = TestApp::with_env(vec![], &[("TOOLS_DIR", dir.to_str().unwrap())]).await;
    let (status, body) = call(&app, "air", json!({ "city": "Berlin" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_error"], false, "{body}");
    assert!(body["output"].as_str().unwrap().starts_with("tool air: {\"city\":\"Berlin\"}"), "{body}");

    let created = app.factory.created.lock().unwrap().clone();
    assert_eq!(created.len(), 1);
    let sandbox = &created[0].1;
    assert_eq!(sandbox.tool_calls.lock().unwrap().len(), 1);
    assert!(sandbox.destroyed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(app.factory.removed_workspaces.lock().unwrap().len(), 1);
    assert_eq!(app.llm.call_count(), 0);

    // A sandbox that can't be made is the call's error, and leaves nothing behind.
    app.factory.fail_create.store(true, std::sync::atomic::Ordering::SeqCst);
    let (status, body) = call(&app, "air", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["is_error"], true, "{body}");
}

#[tokio::test]
async fn calls_are_recorded_for_the_key_that_made_them() {
    let base = start().await;
    let app = TestApp::with_env(vec![], &[("WEB_FETCH_ALLOW_PRIVATE", "on")]).await;
    call(&app, "url", json!({ "url": format!("{base}/plain") })).await;
    call(&app, "url", json!({ "url": format!("{base}/missing") })).await;
    call(&app, "bash", json!({})).await; // refused: not a call

    let owner = app.state.auth.authenticate(KEY_A).unwrap();
    let calls = app.state.store.list_tool_calls(&owner.0, 10).await.unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0].tool, "url");
    assert!(calls[0].error.as_deref().unwrap().contains("404"), "{calls:?}");
    assert_eq!(calls[1].error, None);
    let other = app.state.auth.authenticate(KEY_B).unwrap();
    assert!(app.state.store.list_tool_calls(&other.0, 10).await.unwrap().is_empty());
}
