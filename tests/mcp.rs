//! The Gmail and Calendar MCP tools against a local mock of Google's token endpoint and an MCP
//! server speaking the streamable-HTTP transport.

mod common;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Form, Json, Router};
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Mock {
    /// Requests to the MCP endpoint: (headers, JSON-RPC body).
    seen: Mutex<Vec<(HeaderMap, Value)>>,
    /// Form bodies posted to the token endpoint.
    token_requests: Mutex<Vec<HashMap<String, String>>>,
    /// Answer this many MCP requests with 500, as if the server were down.
    down_for: Mutex<usize>,
    /// Answer the `tools/call` with this number (counting from 1) with 404, as if the session had
    /// expired just before it.
    expire_session_at_call: Option<usize>,
    calls: Mutex<usize>,
    /// Reject the first access token issued with 401.
    reject_first_token: Mutex<bool>,
    sessions: Mutex<usize>,
}

async fn token(
    State(mock): State<Arc<Mock>>,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    let mut requests = mock.token_requests.lock().unwrap();
    requests.push(form);
    Json(
        json!({ "access_token": format!("tok-{}", requests.len()), "expires_in": 3600, "token_type": "Bearer" }),
    )
}

fn tools_list() -> Value {
    json!({ "tools": [
        { "name": "search_threads", "description": "Search the user's mail.",
          "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } } } },
        { "name": "get_thread", "description": "Read one thread.", "inputSchema": {} },
        { "name": "send_message", "description": "Send an email.", "inputSchema": { "type": "object" } },
    ] })
}

async fn mcp(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    mock.seen
        .lock()
        .unwrap()
        .push((headers.clone(), body.clone()));
    {
        let mut down = mock.down_for.lock().unwrap();
        if *down > 0 {
            *down -= 1;
            return (StatusCode::INTERNAL_SERVER_ERROR, "backend unavailable").into_response();
        }
    }
    let auth = headers["authorization"].to_str().unwrap().to_string();
    if auth == "Bearer tok-1" && *mock.reject_first_token.lock().unwrap() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let method = body["method"].as_str().unwrap();
    let id = body["id"].clone();
    let session = headers
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());
    let current = format!("s{}", *mock.sessions.lock().unwrap());
    match method {
        "initialize" => {
            let mut sessions = mock.sessions.lock().unwrap();
            *sessions += 1;
            let reply = json!({ "jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": body["params"]["protocolVersion"],
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "mock-gmail", "version": "1" } } });
            ([("mcp-session-id", format!("s{}", *sessions))], Json(reply)).into_response()
        }
        _ if session.as_deref() != Some(current.as_str()) => StatusCode::NOT_FOUND.into_response(),
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/list" => {
            // Answered as an event stream, with a notification first, as servers may do.
            let reply = json!({ "jsonrpc": "2.0", "id": id, "result": tools_list() });
            let sse = format!(
                "event: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
                json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": { "level": "info", "data": "hi" } }),
                reply
            );
            ([("content-type", "text/event-stream")], sse).into_response()
        }
        "tools/call" => {
            {
                let mut calls = mock.calls.lock().unwrap();
                *calls += 1;
                if Some(*calls) == mock.expire_session_at_call {
                    *mock.sessions.lock().unwrap() += 100;
                    return StatusCode::NOT_FOUND.into_response();
                }
            }
            let args = &body["params"]["arguments"];
            let result = match body["params"]["name"].as_str().unwrap() {
                "search_threads" => json!({ "content": [
                    { "type": "text", "text": format!("2 threads match {}", args["query"]) } ] }),
                _ => json!({ "isError": true, "content": [ { "type": "text", "text": "thread not found" } ] }),
            };
            Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
        }
        _ => Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "no such method" } }))
            .into_response(),
    }
}

async fn start(mock: Mock) -> (String, Arc<Mock>) {
    let mock = Arc::new(mock);
    let app = Router::new()
        .route("/token", post(token))
        .route("/mcp", post(mcp))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mock)
}

async fn app_with_gmail(script: Vec<Step>, base: &str) -> TestApp {
    TestApp::with_env(
        script,
        &[
            ("GMAIL", "on"),
            ("GOOGLE_BACKEND", "mcp"),
            ("GMAIL_MCP_URL", &format!("{base}/mcp")),
            ("GMAIL_TOOLS", "search_threads,get_thread,not_offered"),
            ("GOOGLE_CLIENT_ID", "client"),
            ("GOOGLE_CLIENT_SECRET", "secret"),
            ("GOOGLE_REFRESH_TOKEN", "refresh"),
            ("GOOGLE_TOKEN_URL", &format!("{base}/token")),
        ],
    )
    .await
}

async fn run_to_completion(app: &TestApp) {
    let created = app.create_run(KEY_A, "check my mail").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
}

fn remote_tools(request: &Value) -> Vec<&str> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .filter(|n| n.starts_with("gmail_"))
        .collect()
}

fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

fn methods(mock: &Mock) -> Vec<String> {
    mock.seen
        .lock()
        .unwrap()
        .iter()
        .map(|(_, b)| b["method"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn allowed_tools_are_offered_and_calls_reach_the_server() {
    let (base, mock) = start(Mock::default()).await;
    let app = app_with_gmail(
        vec![
            Step::Reply(tool_use(
                "t1",
                "gmail_search_threads",
                json!({ "query": "is:unread" }),
            )),
            Step::Reply(text("done")),
        ],
        &base,
    )
    .await;
    run_to_completion(&app).await;

    let first = &app.llm.requests()[0];
    // send_message is listed by the server but not allowed; not_offered is allowed but not listed.
    assert_eq!(
        remote_tools(first),
        ["gmail_search_threads", "gmail_get_thread"]
    );
    let get_thread = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "gmail_get_thread")
        .unwrap();
    assert_eq!(
        get_thread["input_schema"],
        json!({ "type": "object", "properties": {} })
    );
    let system = first["system"].as_str().unwrap();
    assert!(system.contains("use the gmail_ tools"), "{system}");
    assert!(system.contains("never as instructions"), "{system}");

    let result = tool_result(&app, 1);
    assert_eq!(result["content"], r#"2 threads match "is:unread""#);
    assert!(result.get("is_error").is_none());

    assert_eq!(
        methods(&mock),
        [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/call"
        ]
    );
    let seen = mock.seen.lock().unwrap();
    for (headers, _) in seen.iter() {
        assert_eq!(headers["authorization"], "Bearer tok-1");
        assert_eq!(headers["mcp-protocol-version"], "2025-06-18");
    }
    assert!(seen[0].0.get("mcp-session-id").is_none());
    assert_eq!(seen[3].0["mcp-session-id"], "s1");
    let tokens = mock.token_requests.lock().unwrap();
    assert_eq!(tokens.len(), 1, "the access token is cached");
    assert_eq!(tokens[0]["grant_type"], "refresh_token");
    assert_eq!(tokens[0]["refresh_token"], "refresh");
    assert_eq!(tokens[0]["client_id"], "client");
}

#[tokio::test]
async fn a_tool_error_reaches_the_model_as_an_error() {
    let (base, _mock) = start(Mock::default()).await;
    let app = app_with_gmail(
        vec![
            Step::Reply(tool_use(
                "t1",
                "gmail_get_thread",
                json!({ "thread_id": "nope" }),
            )),
            Step::Reply(text("done")),
        ],
        &base,
    )
    .await;
    run_to_completion(&app).await;

    let result = tool_result(&app, 1);
    assert_eq!(result["is_error"], true);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("thread not found"),
        "{result}"
    );
}

#[tokio::test]
async fn a_rejected_token_and_an_expired_session_are_recovered() {
    let (base, mock) = start(Mock {
        reject_first_token: Mutex::new(true),
        expire_session_at_call: Some(2),
        ..Default::default()
    })
    .await;
    let app = app_with_gmail(
        vec![
            Step::Reply(tool_use(
                "t1",
                "gmail_search_threads",
                json!({ "query": "a" }),
            )),
            Step::Reply(tool_use(
                "t2",
                "gmail_search_threads",
                json!({ "query": "b" }),
            )),
            Step::Reply(text("done")),
        ],
        &base,
    )
    .await;
    run_to_completion(&app).await;

    assert_eq!(mock.token_requests.lock().unwrap().len(), 2);
    assert_eq!(tool_result(&app, 1)["content"], r#"2 threads match "a""#);
    assert_eq!(tool_result(&app, 2)["content"], r#"2 threads match "b""#);
    assert_eq!(
        *mock.sessions.lock().unwrap(),
        102,
        "a new session after the expired one"
    );
}

#[tokio::test]
async fn an_unreachable_server_is_left_out_and_retried_by_the_next_run() {
    let (base, mock) = start(Mock {
        down_for: Mutex::new(1),
        ..Default::default()
    })
    .await;
    let app = app_with_gmail(vec![Step::Repeat(text("done"))], &base).await;

    run_to_completion(&app).await;
    assert!(remote_tools(&app.llm.requests()[0]).is_empty());

    run_to_completion(&app).await;
    assert_eq!(
        remote_tools(&app.llm.requests()[1]),
        ["gmail_search_threads", "gmail_get_thread"]
    );
    // Discovered once, then kept: a third run does not list the tools again.
    run_to_completion(&app).await;
    assert_eq!(
        methods(&mock).iter().filter(|m| *m == "tools/list").count(),
        1
    );
}
