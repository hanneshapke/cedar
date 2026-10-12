//! `AnthropicClient` against a local mock of the Messages API.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use cedar::llm::{AnthropicClient, LlmClient, MessageRequest};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Mock {
    /// (status, body, retry-after); the last entry repeats once the queue is down to one.
    responses: Mutex<VecDeque<(u16, Value, Option<&'static str>)>>,
    seen: Mutex<Vec<(HeaderMap, Value)>>,
}

async fn handler(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    mock.seen.lock().unwrap().push((headers, body));
    let mut queue = mock.responses.lock().unwrap();
    let (status, body, retry_after) = if queue.len() > 1 {
        queue.pop_front().unwrap()
    } else {
        queue.front().cloned().unwrap()
    };
    let mut resp_headers = HeaderMap::new();
    if let Some(v) = retry_after {
        resp_headers.insert("retry-after", v.parse().unwrap());
    }
    (
        StatusCode::from_u16(status).unwrap(),
        resp_headers,
        Json(body),
    )
}

async fn start(responses: Vec<(u16, Value, Option<&'static str>)>) -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock {
        responses: Mutex::new(responses.into()),
        seen: Mutex::new(vec![]),
    });
    let app = Router::new()
        .route("/v1/messages", post(handler))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, mock)
}

fn ok_body() -> Value {
    json!({ "content": [{ "type": "text", "text": "hi" }], "stop_reason": "end_turn",
            "usage": { "input_tokens": 7, "output_tokens": 2 } })
}

fn api_error(message: &str) -> Value {
    json!({ "type": "error", "error": { "type": "api_error", "message": message } })
}

async fn call(client: &AnthropicClient) -> anyhow::Result<cedar::llm::LlmResponse> {
    let messages = [json!({ "role": "user", "content": "hello" })];
    let tools = [json!({ "name": "bash" })];
    client
        .create_message(&MessageRequest {
            model: "claude-test",
            max_tokens: 123,
            system: "be nice",
            tools: &tools,
            messages: &messages,
        })
        .await
}

fn client(url: String) -> AnthropicClient {
    AnthropicClient::new("sk-secret".into(), url)
        .unwrap()
        .with_retry(4, Duration::from_millis(5))
}

#[tokio::test]
async fn sends_the_expected_request_and_parses_the_reply() {
    let (url, mock) = start(vec![(200, ok_body(), None)]).await;
    let resp = call(&client(url)).await.unwrap();
    assert_eq!(resp.content[0]["text"], "hi");
    assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!((resp.usage.input_tokens, resp.usage.output_tokens), (7, 2));

    let seen = mock.seen.lock().unwrap();
    let (headers, body) = &seen[0];
    assert_eq!(headers["x-api-key"], "sk-secret");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(
        *body,
        json!({ "model": "claude-test", "max_tokens": 123, "system": "be nice",
                "tools": [{ "name": "bash" }], "messages": [{ "role": "user", "content": "hello" }] })
    );
}

#[tokio::test]
async fn retries_overload_and_rate_limit_errors_then_succeeds() {
    let (url, mock) = start(vec![
        (529, api_error("overloaded"), None),
        (429, api_error("slow down"), Some("0")),
        (500, api_error("oops"), None),
        (200, ok_body(), None),
    ])
    .await;
    let resp = call(&client(url)).await.unwrap();
    assert_eq!(resp.content[0]["text"], "hi");
    assert_eq!(mock.seen.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn client_errors_are_not_retried() {
    let (url, mock) = start(vec![
        (400, api_error("model: not found"), None),
        (200, ok_body(), None),
    ])
    .await;
    let err = call(&client(url)).await.unwrap_err().to_string();
    assert!(
        err.contains("400") && err.contains("model: not found"),
        "{err}"
    );
    assert_eq!(mock.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn gives_up_after_the_configured_attempts() {
    let (url, mock) = start(vec![(503, api_error("unavailable"), None)]).await;
    let err = call(&client(url).with_retry(3, Duration::from_millis(1)))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("503") && err.contains("unavailable"), "{err}");
    assert_eq!(mock.seen.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn connection_failures_are_retried_and_then_reported() {
    // Nothing is listening on this port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let started = std::time::Instant::now();
    let err = call(&client(url).with_retry(3, Duration::from_millis(20)))
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("sending API request"),
        "{err:#}"
    );
    // Two backoffs (20ms + 40ms) happened between three attempts.
    assert!(started.elapsed() >= Duration::from_millis(60));
}
