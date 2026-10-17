//! The `web_search` tool against a local mock of the Brave Search API.

mod common;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use common::*;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

struct Mock {
    /// (status, body); the last entry repeats once the queue is down to one.
    responses: Mutex<VecDeque<(u16, Value)>>,
    seen: Mutex<Vec<(HeaderMap, HashMap<String, String>)>>,
}

async fn handler(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    mock.seen.lock().unwrap().push((headers, params));
    let mut queue = mock.responses.lock().unwrap();
    let (status, body) = if queue.len() > 1 {
        queue.pop_front().unwrap()
    } else {
        queue.front().cloned().unwrap()
    };
    (StatusCode::from_u16(status).unwrap(), Json(body))
}

async fn start(responses: Vec<(u16, Value)>) -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock {
        responses: Mutex::new(responses.into()),
        seen: Mutex::new(vec![]),
    });
    let app = Router::new()
        .route("/res/v1/web/search", get(handler))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/res/v1/web/search",
        listener.local_addr().unwrap()
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, mock)
}

fn results() -> Value {
    json!({ "web": { "results": [
        { "title": "Rust <strong>1.90</strong>", "url": "https://blog.rust-lang.org/1.90",
          "description": "Release notes", "age": "3 days ago" },
        { "title": "Rust", "url": "https://rust-lang.org" },
    ] } })
}

const FORMATTED: &str =
    "1. Rust 1.90\n   https://blog.rust-lang.org/1.90 (3 days ago)\n   Release notes\n\n\
                         2. Rust\n   https://rust-lang.org";

async fn app_with_search(script: Vec<Step>, url: &str) -> TestApp {
    TestApp::with_env(
        script,
        &[("BRAVE_API_KEY", "brave-key"), ("BRAVE_API_BASE_URL", url)],
    )
    .await
}

/// The tool result at the end of the messages sent in the model's `request`-th call.
fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

async fn run_to_completion(app: &TestApp) {
    let created = app.create_run(KEY_A, "look it up").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
}

fn tool_names(request: &Value) -> Vec<&str> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_search_reaches_brave_and_the_results_reach_the_model() {
    let (url, mock) = start(vec![(200, results())]).await;
    let app = app_with_search(
        vec![
            Step::Reply(tool_use(
                "t1",
                "web_search",
                json!({ "query": "rust release", "count": 2, "freshness": "pw" }),
            )),
            Step::Reply(text("done")),
        ],
        &url,
    )
    .await;
    run_to_completion(&app).await;

    assert_eq!(
        tool_names(&app.llm.requests()[0]),
        [
            "bash",
            "read_file",
            "write_file",
            "run_python",
            "read_document",
            "generate_pdf",
            "generate_chart",
            "web_search",
            "fetch_url",
            "download_file",
            "weather_forecast",
            "show_map",
            "show_recipe",
            "plan_trip",
            "compare_stocks",
            "ask_user"
        ]
    );

    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (headers, params) = &seen[0];
    assert_eq!(headers["x-subscription-token"], "brave-key");
    assert_eq!(headers["accept"], "application/json");
    assert_eq!(params["q"], "rust release");
    assert_eq!(params["count"], "2");
    assert_eq!(params["freshness"], "pw");

    let result = tool_result(&app, 1);
    assert_eq!(result["tool_use_id"], "t1");
    assert_eq!(result["content"], FORMATTED);
    assert!(result.get("is_error").is_none());
}

#[tokio::test]
async fn defaults_apply_and_freshness_is_omitted_when_unset() {
    let (url, mock) = start(vec![(200, results())]).await;
    let app = app_with_search(
        vec![
            Step::Reply(tool_use("t1", "web_search", json!({ "query": "rust" }))),
            Step::Reply(text("done")),
        ],
        &url,
    )
    .await;
    run_to_completion(&app).await;

    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen[0].1["count"], "5");
    assert!(!seen[0].1.contains_key("freshness"));
}

#[tokio::test]
async fn a_rate_limited_search_is_retried_once() {
    let (url, mock) = start(vec![
        (429, json!({ "error": { "detail": "rate limited" } })),
        (200, results()),
    ])
    .await;
    let app = app_with_search(
        vec![
            Step::Reply(tool_use("t1", "web_search", json!({ "query": "rust" }))),
            Step::Reply(text("done")),
        ],
        &url,
    )
    .await;
    run_to_completion(&app).await;

    assert_eq!(mock.seen.lock().unwrap().len(), 2);
    let result = tool_result(&app, 1);
    assert_eq!(result["content"], FORMATTED);
    assert!(result.get("is_error").is_none());
}

#[tokio::test]
async fn persistent_rate_limiting_is_reported_to_the_model() {
    let (url, mock) = start(vec![(429, json!({}))]).await;
    let app = app_with_search(
        vec![
            Step::Reply(tool_use("t1", "web_search", json!({ "query": "rust" }))),
            Step::Reply(text("done")),
        ],
        &url,
    )
    .await;
    run_to_completion(&app).await;

    assert_eq!(mock.seen.lock().unwrap().len(), 2, "one try plus one retry");
    let result = tool_result(&app, 1);
    assert_eq!(result["is_error"], true);
    assert!(result["content"].as_str().unwrap().contains("rate limited"));
}

#[tokio::test]
async fn a_rejected_key_is_an_error_the_model_sees_without_the_key() {
    let (url, _mock) = start(vec![(
        401,
        json!({ "error": { "detail": "Invalid subscription token" } }),
    )])
    .await;
    let app = app_with_search(
        vec![
            Step::Reply(tool_use("t1", "web_search", json!({ "query": "rust" }))),
            Step::Reply(text("done")),
        ],
        &url,
    )
    .await;
    run_to_completion(&app).await;

    let result = tool_result(&app, 1);
    assert_eq!(result["is_error"], true);
    let content = result["content"].as_str().unwrap();
    assert!(content.contains("rejected the API key"), "{content}");
    assert!(!content.contains("brave-key"), "{content}");
}

#[tokio::test]
async fn other_api_errors_carry_brave_s_detail() {
    let (url, _mock) = start(vec![(
        422,
        json!({ "error": { "code": "VALIDATION", "detail": "query too long" } }),
    )])
    .await;
    let app = app_with_search(
        vec![
            Step::Reply(tool_use("t1", "web_search", json!({ "query": "rust" }))),
            Step::Reply(text("done")),
        ],
        &url,
    )
    .await;
    run_to_completion(&app).await;

    let result = tool_result(&app, 1);
    assert_eq!(result["is_error"], true);
    let content = result["content"].as_str().unwrap();
    assert!(
        content.contains("422") && content.contains("query too long"),
        "{content}"
    );
}

#[tokio::test]
async fn bad_input_never_calls_brave() {
    let (url, mock) = start(vec![(200, results())]).await;
    let app = app_with_search(
        vec![
            Step::Reply(tool_use("t1", "web_search", json!({ "query": "  " }))),
            Step::Reply(text("done")),
        ],
        &url,
    )
    .await;
    run_to_completion(&app).await;

    assert!(mock.seen.lock().unwrap().is_empty());
    let result = tool_result(&app, 1);
    assert_eq!(result["is_error"], true);
    assert_eq!(result["content"], "missing query");
}

#[tokio::test]
async fn without_a_brave_key_the_tool_is_not_offered() {
    let app = TestApp::new(vec![Step::Reply(text("hi"))]).await;
    run_to_completion(&app).await;

    assert_eq!(
        tool_names(&app.llm.requests()[0]),
        [
            "bash",
            "read_file",
            "write_file",
            "run_python",
            "read_document",
            "generate_pdf",
            "generate_chart",
            "fetch_url",
            "download_file",
            "weather_forecast",
            "show_map",
            "show_recipe",
            "plan_trip",
            "compare_stocks",
            "ask_user"
        ]
    );
}
