//! `ChatCompletionsClient` against a local mock of an OpenAI-compatible server.

mod common;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use cedar::api::{self, AppState};
use cedar::llm::{ChatCompletionsClient, LlmClient, MessageRequest};
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
        .route("/v1/chat/completions", post(handler))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, mock)
}

fn ok_body() -> Value {
    json!({ "choices": [{ "message": { "role": "assistant", "content": "hi" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 7, "completion_tokens": 2, "total_tokens": 9 } })
}

/// The error shape vLLM uses (OpenAI nests this under `error`).
fn api_error(message: &str) -> Value {
    json!({ "object": "error", "message": message, "type": "InternalServerError", "code": 500 })
}

async fn call(client: &ChatCompletionsClient) -> anyhow::Result<cedar::llm::LlmResponse> {
    let messages = [json!({ "role": "user", "content": "hello" })];
    let tools = [json!({ "name": "bash", "description": "run a command",
                         "input_schema": { "type": "object" } })];
    client
        .create_message(&MessageRequest {
            model: "test-model",
            max_tokens: 123,
            reasoning_effort: None,
            system: "be nice",
            tools: &tools,
            messages: &messages,
        })
        .await
}

fn client(url: String) -> ChatCompletionsClient {
    ChatCompletionsClient::new(url, Some("sk-secret".into()))
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
    assert_eq!(headers["authorization"], "Bearer sk-secret");
    assert!(headers.get("x-api-key").is_none());
    assert_eq!(
        *body,
        json!({ "model": "test-model", "max_tokens": 123,
                "messages": [{ "role": "system", "content": "be nice" },
                             { "role": "user", "content": "hello" }],
                "tools": [{ "type": "function", "function": {
                    "name": "bash", "description": "run a command",
                    "parameters": { "type": "object" } } }] })
    );
}

#[tokio::test]
async fn images_and_pdfs_are_sent_as_data_url_parts() {
    let (url, mock) = start(vec![(200, ok_body(), None)]).await;
    let messages = [json!({ "role": "user", "content": [
        { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "aGk=" }, "title": "a.png" },
        { "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": "cGRm" }, "title": "b.pdf" },
        { "type": "text", "text": "what are these?" },
    ] })];
    client(url)
        .create_message(&MessageRequest {
            model: "test-model",
            max_tokens: 10,
            reasoning_effort: None,
            system: "",
            tools: &[],
            messages: &messages,
        })
        .await
        .unwrap();
    let seen = mock.seen.lock().unwrap();
    assert_eq!(
        seen[0].1["messages"],
        json!([{ "role": "user", "content": [
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,aGk=" } },
            { "type": "file", "file": { "filename": "b.pdf", "file_data": "data:application/pdf;base64,cGRm" } },
            { "type": "text", "text": "what are these?" },
        ] }])
    );
}

#[tokio::test]
async fn no_authorization_header_without_an_api_key() {
    let (url, mock) = start(vec![(200, ok_body(), None)]).await;
    let client = ChatCompletionsClient::new(url, None).unwrap();
    call(&client).await.unwrap();
    assert!(mock.seen.lock().unwrap()[0]
        .0
        .get("authorization")
        .is_none());
}

#[tokio::test]
async fn a_tool_call_reply_comes_back_as_tool_use_blocks() {
    let reply = json!({ "choices": [{ "finish_reason": "tool_calls", "message": {
        "role": "assistant", "content": null,
        "tool_calls": [{ "id": "chatcmpl-tool-1", "type": "function",
                         "function": { "name": "bash", "arguments": "{\"command\": \"ls\"}" } }] } }],
        "usage": { "prompt_tokens": 30, "completion_tokens": 9 } });
    let (url, _mock) = start(vec![(200, reply, None)]).await;
    let resp = call(&client(url)).await.unwrap();
    assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(
        resp.content,
        [
            json!({ "type": "tool_use", "id": "chatcmpl-tool-1", "name": "bash",
                 "input": { "command": "ls" } })
        ]
    );
}

#[tokio::test]
async fn retries_overload_and_rate_limit_errors_then_succeeds() {
    let (url, mock) = start(vec![
        (503, api_error("overloaded"), None),
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

fn tool_call_reply(id: &str, command: &str) -> Value {
    json!({ "choices": [{ "finish_reason": "tool_calls", "message": {
        "role": "assistant", "content": null,
        "tool_calls": [{ "id": id, "type": "function",
                         "function": { "name": "bash",
                                       "arguments": json!({ "command": command }).to_string() } }] } }],
        "usage": { "prompt_tokens": 20, "completion_tokens": 5 } })
}

fn text_reply(text: &str) -> Value {
    json!({ "choices": [{ "finish_reason": "stop", "message": { "role": "assistant", "content": text } }],
            "usage": { "prompt_tokens": 25, "completion_tokens": 3 } })
}

fn roles(request: &Value) -> Vec<&str> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect()
}

/// The whole path: agent loop -> real client -> mock server, over a tool call and a continuation.
#[tokio::test]
async fn the_agent_loop_drives_a_chat_completions_server() {
    let (url, mock) = start(vec![
        (200, tool_call_reply("call-1", "echo hi"), None),
        (200, text_reply("done"), None),
        (200, text_reply("again"), None),
    ])
    .await;
    let llm = Arc::new(ChatCompletionsClient::new(url, None).unwrap());
    let clients = Arc::new(cedar::llm::LlmRouter::new());
    clients.set(common::TEST_MODEL, llm);
    let store = cedar::store::Store::connect_memory().await.unwrap();
    common::register_model(&store, common::TEST_MODEL, common::TEST_MODEL, &[]).await;
    let factory = common::FakeFactory::new();
    let state = AppState::new(common::test_config(&[]), store, clients, factory.clone()).unwrap();
    state.integrations.reload().await.unwrap();
    let app = common::TestApp {
        router: api::router(state.clone()),
        state,
        llm: common::ScriptedLlm::new(vec![]), // unused: the real client is wired in above
        factory,
        browsers: Default::default(), // unused: the browser is off
    };

    let first = app.create_run(common::KEY_A, "say hi").await;
    let first_id = first["id"].as_str().unwrap().to_string();
    let run = app.wait_status(common::KEY_A, &first_id, "completed").await;
    assert_eq!(run["final_text"], "done");
    assert_eq!(run["stop_reason"], "end_turn");

    let second = app
        .create_run_with(
            common::KEY_A,
            json!({ "prompt": "once more", "parent_id": first_id }),
        )
        .await;
    let second_id = second["id"].as_str().unwrap().to_string();
    let run = app
        .wait_status(common::KEY_A, &second_id, "completed")
        .await;
    assert_eq!(run["final_text"], "again");

    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(roles(&seen[0].1), ["system", "user"]);
    // The seven sandbox tools plus fetch_url, download_file, weather_forecast, show_map,
    // show_recipe, plan_trip, and ask_user.
    assert_eq!(seen[0].1["tools"].as_array().unwrap().len(), 14);
    // The tool call goes back as an assistant message plus a tool message with its output.
    assert_eq!(roles(&seen[1].1), ["system", "user", "assistant", "tool"]);
    let messages = seen[1].1["messages"].as_array().unwrap();
    let call = &messages[2]["tool_calls"][0];
    assert_eq!(call["id"], "call-1");
    assert_eq!(call["function"]["name"], "bash");
    assert_eq!(call["function"]["arguments"], "{\"command\":\"echo hi\"}");
    assert_eq!(messages[3]["tool_call_id"], "call-1");
    assert!(messages[3]["content"]
        .as_str()
        .unwrap()
        .contains("ran: echo hi"));
    // The continuation carries the earlier exchange, then the new prompt.
    assert_eq!(
        roles(&seen[2].1),
        ["system", "user", "assistant", "tool", "assistant", "user"]
    );
    assert_eq!(seen[2].1["messages"][5]["content"], "once more");
}
