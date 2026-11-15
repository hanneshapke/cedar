//! Runs on a model with a small context window: requests sized to fit, a reply cut off by the
//! window tried again with less, and a run that still doesn't fit ending as failed.

mod common;

use cedar::llm::LlmResponse;
use cedar::models::Usage;
use common::*;
use serde_json::{json, Value};

/// A reply that stopped at its limit before it said anything, as a full window gives.
fn cut_off(input_tokens: u64, output_tokens: u64) -> LlmResponse {
    LlmResponse {
        content: vec![],
        stop_reason: Some("max_tokens".into()),
        usage: Usage { input_tokens, output_tokens },
        reasoning: Some("Let me think about all of this at length".into()),
    }
}

fn id_of(run: &Value) -> String {
    run["id"].as_str().unwrap().to_string()
}

/// Give the test model a context window.
async fn set_window(app: &TestApp, window: Option<u32>) {
    let mut entry = app.state.store.get_model(TEST_MODEL).await.unwrap().unwrap();
    entry.context_window = window;
    app.state.store.upsert_model(&entry).await.unwrap();
    app.state.integrations.reload().await.unwrap();
}

#[tokio::test]
async fn a_small_window_shapes_the_request() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "bash", json!({ "command": "big" }))),
        Step::Reply(text("done")),
    ])
    .await;
    set_window(&app, Some(16_384)).await;
    let id = id_of(&app.create_run(KEY_A, "Summarize today's AI news into a PDF").await);
    let run = app.wait_status(KEY_A, &id, "completed").await;
    assert_eq!(run["final_text"], "done");

    let requests = app.llm.requests();
    let first = &requests[0];
    // The reply gets what the window leaves, not the registered 16384.
    let max = first["max_tokens"].as_u64().unwrap();
    assert!(max < 16_384, "{max}");
    let system = first["system"].as_str().unwrap();
    assert!(system.contains("context window is small"), "{system}");
    // The date heads cedar's note, right after the client's own prompt.
    let date = system.find("Today is ").expect("the date is given");
    assert!(date < system.find("The sandbox").unwrap(), "{system}");
    // Only the tools the request seems to need are offered.
    let names: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"generate_pdf") && names.contains(&"bash"), "{names:?}");
    assert!(!names.contains(&"weather_forecast") && !names.contains(&"show_recipe"), "{names:?}");
    // The tool's output was held to a share of the window, not the server's 32 KiB.
    let last: &Value = requests[1]["messages"].as_array().unwrap().last().unwrap();
    let output = last["content"][0]["content"].as_str().unwrap_or_else(|| panic!("{last}"));
    assert!(output.len() < 16 * 1024, "{}", output.len());
}

#[tokio::test]
async fn a_large_window_leaves_the_request_alone() {
    let app = TestApp::new(vec![Step::Reply(text("done"))]).await;
    set_window(&app, Some(200_000)).await;
    let id = id_of(&app.create_run(KEY_A, "What's the weather in Portland?").await);
    app.wait_status(KEY_A, &id, "completed").await;
    let first = &app.llm.requests()[0];
    assert_eq!(first["max_tokens"], 16_384);
    let system = first["system"].as_str().unwrap();
    assert!(!system.contains("context window is small"));
    let names: Vec<&str> = first["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"show_recipe"), "every tool is offered: {names:?}");
}

#[tokio::test]
async fn a_reply_cut_off_by_the_window_is_tried_again() {
    // No window is registered: the cut-off reply shows it (31555 + 1213 = 32768).
    let app = TestApp::new(vec![Step::Reply(cut_off(31_555, 1_213)), Step::Reply(text("done"))]).await;
    let id = id_of(&app.create_run(KEY_A, "hello").await);
    let run = app.wait_status(KEY_A, &id, "completed").await;
    assert_eq!(run["final_text"], "done");
    let requests = app.llm.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["max_tokens"], 16_384);
    // The empty reply is not part of the conversation.
    let messages = requests[1]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{messages:?}");
}

#[tokio::test]
async fn a_run_that_still_does_not_fit_fails() {
    let app = TestApp::new(vec![
        Step::Reply(cut_off(31_555, 1_213)),
        Step::Reply(cut_off(31_555, 1_213)),
    ])
    .await;
    set_window(&app, Some(32_768)).await;
    let id = id_of(&app.create_run(KEY_A, "hello").await);
    let run = app.wait_status(KEY_A, &id, "failed").await;
    let error = run["error"].as_str().unwrap_or_default();
    assert!(error.contains("context window"), "{run}");
    assert_eq!(app.llm.call_count(), 2);
}

#[tokio::test]
async fn a_reply_that_spent_its_own_limit_fails_without_a_retry() {
    // It asked for 16384 and used all of it: the window had room, the reasoning ran long.
    let app = TestApp::new(vec![Step::Reply(cut_off(2_000, 16_384))]).await;
    let id = id_of(&app.create_run(KEY_A, "hello").await);
    let run = app.wait_status(KEY_A, &id, "failed").await;
    let error = run["error"].as_str().unwrap_or_default();
    assert!(error.contains("reply limit of 16384"), "{run}");
    assert_eq!(app.llm.call_count(), 1);
}

#[tokio::test]
async fn a_growing_conversation_is_cut_down_to_fit() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "bash", json!({ "command": "big" }))),
        Step::Reply(tool_use("t2", "bash", json!({ "command": "big" }))),
        Step::Reply(tool_use("t3", "bash", json!({ "command": "big" }))),
        Step::Reply(tool_use("t4", "bash", json!({ "command": "big" }))),
        Step::Reply(text("done")),
    ])
    .await;
    set_window(&app, Some(16_384)).await;
    let id = id_of(&app.create_run(KEY_A, "Summarize today's AI news into a PDF").await);
    app.wait_status(KEY_A, &id, "completed").await;
    for (i, req) in app.llm.requests().iter().enumerate() {
        let bytes = req["system"].as_str().unwrap().len()
            + req["tools"].to_string().len()
            + req["messages"].as_array().unwrap().iter().map(|m| m.to_string().len()).sum::<usize>();
        let max = req["max_tokens"].as_u64().unwrap() as usize;
        // At three bytes a token, the prompt and the reply it may take stay within the window.
        assert!(bytes / 3 + max <= 16_384, "request {i}: {bytes} bytes and {max} tokens for the reply");
    }
    let events = app.events(KEY_A, &id, None).await;
    assert!(events.iter().any(|e| e.event == "context.trimmed"), "older results were cut down");
}

/// A server that answers every request with 429, as a rate-limited API does; its address.
async fn rate_limited() -> String {
    use axum::{http::StatusCode, routing::get, Router};
    let app = Router::new().route("/", get(|| async { (StatusCode::TOO_MANY_REQUESTS, "slow down") }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/")
}

#[tokio::test]
async fn a_tool_that_keeps_failing_is_not_called_a_third_time() {
    let limited = json!({ "url": rate_limited().await });
    let app = TestApp::with_env(
        vec![
            Step::Reply(reply(
                vec![
                    json!({ "type": "tool_use", "id": "a", "name": "fetch_url", "input": limited }),
                    json!({ "type": "tool_use", "id": "b", "name": "bash", "input": { "command": "echo hi" } }),
                ],
                "tool_use",
            )),
            Step::Reply(tool_use("c", "fetch_url", limited.clone())),
            Step::Reply(tool_use("d", "fetch_url", limited.clone())),
            Step::Reply(text("done")),
        ],
        &[("WEB_FETCH_ALLOW_PRIVATE", "on")],
    )
    .await;
    let id = id_of(&app.create_run(KEY_A, "read the page").await);
    app.wait_status(KEY_A, &id, "completed").await;
    let requests = app.llm.requests();
    // Results come back in the order of the calls.
    let first: Vec<&str> = requests[1]["messages"][2]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["tool_use_id"].as_str())
        .collect();
    assert_eq!(first, ["a", "b"]);
    let third = requests[3]["messages"].as_array().unwrap().last().unwrap()["content"][0].clone();
    let text = third["content"].as_str().unwrap();
    assert!(text.starts_with("not called: fetch_url failed 2 times"), "{text}");
}
