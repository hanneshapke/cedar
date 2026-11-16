//! The `computer` tool through the API, with a fake computer factory: when desktops start and
//! stop, what reaches them, and how their screenshots get to the model.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

const COMPUTER_ON: &[(&str, &str)] = &[("COMPUTER", "on")];

/// The tool result at the end of the messages sent in the model's `request`-th call.
fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

fn script(calls: Vec<Value>) -> Vec<Step> {
    let mut script: Vec<Step> = calls
        .into_iter()
        .enumerate()
        .map(|(i, input)| Step::Reply(tool_use(&format!("t{i}"), "computer", input)))
        .collect();
    script.push(Step::Reply(text("done")));
    script
}

/// Run one `computer` call per entry, then return the app and the run's id once it completed.
async fn run_calls(calls: Vec<Value>, env: &[(&str, &str)]) -> (TestApp, String) {
    let app = TestApp::with_env(script(calls), env).await;
    let created = app.create_run(KEY_A, "fill in the spreadsheet").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    (app, id)
}

fn screenshot() -> Value {
    json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": FAKE_SCREENSHOT } })
}

#[tokio::test]
async fn one_computer_serves_the_whole_run_and_is_removed_with_it() {
    let (app, id) = run_calls(
        vec![
            json!({ "action": "screenshot" }),
            json!({ "action": "click", "x": 200, "y": 150, "count": 2 }),
            json!({ "action": "type", "text": "42\n" }),
        ],
        COMPUTER_ON,
    )
    .await;

    let created = app.computers.created.lock().unwrap().clone();
    assert_eq!(created.len(), 1, "started once, on first use");
    let (run_id, workspace_id, _) = &created[0];
    assert_eq!(run_id.to_string(), id);
    let (_, run) = app.get(&format!("/v1/runs/{id}"), KEY_A).await;
    assert_eq!(
        Some(workspace_id.to_string().as_str()),
        run["workspace_id"].as_str(),
        "it works on the conversation's files: {run}"
    );
    assert_eq!(
        *app.computers.computer(0).requests.lock().unwrap(),
        [
            json!({ "action": "screenshot" }),
            json!({ "action": "click", "x": 200, "y": 150, "button": "left", "count": 2 }),
            json!({ "action": "type", "text": "42\n" }),
        ]
    );
    assert_eq!(*app.computers.destroyed.lock().unwrap(), [*run_id]);
}

#[tokio::test]
async fn the_screenshot_reaches_the_model_with_the_result() {
    let (app, id) = run_calls(
        vec![json!({ "action": "click", "x": 200, "y": 150 })],
        COMPUTER_ON,
    )
    .await;

    let result = tool_result(&app, 1);
    assert!(result.get("is_error").is_none(), "{result}");
    assert_eq!(
        result["content"],
        json!([
            { "type": "text", "text": "The screen after the action is in the screenshot (1280x800 pixels). The pointer is at (200, 150). The window with the keyboard is \"Untitled 1 - LibreOffice Calc\"." },
            screenshot(),
        ])
    );

    // The event says there is an image without carrying it; the conversation has it, and the
    // trace its size.
    let events = app.events(KEY_A, &id, None).await;
    let event = events.iter().find(|e| e.event == "tool.result").unwrap();
    assert_eq!(event.data["data"]["image"], true, "{}", event.data);
    assert!(!event.data.to_string().contains(FAKE_SCREENSHOT));
    let (_, messages) = app.get(&format!("/v1/runs/{id}/messages"), KEY_A).await;
    assert!(messages.to_string().contains(FAKE_SCREENSHOT), "{messages}");
    let (_, trace) = app.get(&format!("/v1/runs/{id}/trace"), KEY_A).await;
    assert!(!trace.to_string().contains(FAKE_SCREENSHOT));
    assert!(trace.to_string().contains("\"bytes\":6"), "{trace}");
}

#[tokio::test]
async fn runs_that_never_use_it_start_no_computer() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t0", "bash", json!({ "command": "ls" }))),
            Step::Reply(text("done")),
        ],
        COMPUTER_ON,
    )
    .await;
    let created = app.create_run(KEY_A, "list the files").await;
    app.wait_status(KEY_A, created["id"].as_str().unwrap(), "completed")
        .await;
    assert!(app.computers.created.lock().unwrap().is_empty());
    assert!(app.computers.destroyed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn mistakes_go_back_to_the_model_as_errors() {
    let (app, _) = run_calls(
        vec![
            json!({ "action": "click", "x": 10 }),
            json!({ "action": "click", "x": 13, "y": 10 }),
            json!({ "action": "open", "path": "../../etc/passwd" }),
        ],
        COMPUTER_ON,
    )
    .await;

    for (call, expected) in [
        (1, "click needs `y`"),
        (2, "the pointer could not be moved"),
        (3, "`..`"),
    ] {
        let result = tool_result(&app, call);
        assert_eq!(result["is_error"], true, "{result}");
        let content = result["content"].as_str().unwrap();
        assert!(content.contains(expected), "call {call}: {content}");
    }
    // Only the request that passed cedar's checks reached the computer.
    assert_eq!(
        *app.computers.computer(0).requests.lock().unwrap(),
        [json!({ "action": "click", "x": 13, "y": 10, "button": "left", "count": 1 })]
    );
}

fn offered(request: &Value) -> Vec<&str> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn the_computer_is_offered_and_explained_when_on() {
    let (app, _) = run_calls(vec![], COMPUTER_ON).await;
    let request = &app.llm.requests()[0];
    assert!(offered(request).contains(&"computer"), "{:?}", offered(request));
    let system = request["system"].as_str().unwrap();
    assert!(system.contains("use the computer tool"), "{system}");

    let (_, tools) = app.get("/v1/tools", KEY_A).await;
    let group = tools["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "computer")
        .unwrap();
    assert_eq!(group["label"], "Computer use");
    assert_eq!(group["tools"][0]["name"], "computer");
}

#[tokio::test]
async fn without_the_setting_there_is_no_computer_tool() {
    let (app, _) = run_calls(vec![json!({ "action": "screenshot" })], &[]).await;
    let request = &app.llm.requests()[0];
    assert!(!offered(request).contains(&"computer"));
    assert!(!request["system"].as_str().unwrap().contains("computer tool"));
    assert_eq!(tool_result(&app, 1)["content"], "unknown tool: computer");
    assert!(app.computers.created.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_model_that_reads_no_images_is_not_offered_it() {
    let app = TestApp::with_env(script(vec![json!({ "action": "screenshot" })]), COMPUTER_ON).await;
    // Only `TEST_MODEL` is registered as taking images.
    app.register("text-only", "text-only", &[]).await;
    let created = app
        .create_run_with(KEY_A, json!({ "prompt": "open the spreadsheet", "model": "text-only" }))
        .await;
    app.wait_status(KEY_A, created["id"].as_str().unwrap(), "completed")
        .await;
    let request = &app.llm.requests()[0];
    assert!(!offered(request).contains(&"computer"), "{:?}", offered(request));
    assert!(!request["system"].as_str().unwrap().contains("computer tool"));
    assert_eq!(tool_result(&app, 1)["content"], "unknown tool: computer");
    assert!(app.computers.created.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_smaller_image_limit_scales_the_screenshot_and_the_positions() {
    let app = TestApp::with_env(
        script(vec![json!({ "action": "click", "x": 320, "y": 200 })]),
        COMPUTER_ON,
    )
    .await;
    cedar::images::set_max_side(&app.state.store, Some(640))
        .await
        .unwrap();
    let created = app.create_run(KEY_A, "click the middle").await;
    app.wait_status(KEY_A, created["id"].as_str().unwrap(), "completed")
        .await;
    assert_eq!(
        *app.computers.computer(0).requests.lock().unwrap(),
        [json!({ "action": "click", "x": 641, "y": 401, "button": "left", "count": 1,
                 "width": 640, "height": 400 })]
    );
    let text = tool_result(&app, 1)["content"][0]["text"].as_str().unwrap().to_string();
    assert!(text.contains("(640x400 pixels)"), "{text}");
    assert!(text.contains("The pointer is at (320, 200)."), "{text}");
}

#[tokio::test]
async fn only_the_newest_screenshots_are_sent_again() {
    let (app, id) = run_calls(vec![json!({ "action": "screenshot" }); 6], COMPUTER_ON).await;
    // The request after the sixth screenshot: the three oldest are a note, the rest images.
    let last = app.llm.requests()[6]["messages"].clone();
    let results: Vec<&Value> = last
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| &m["content"][0])
        .filter(|b| b["type"] == "tool_result")
        .collect();
    assert_eq!(results.len(), 6);
    for old in &results[..3] {
        let content = old["content"].as_str().unwrap();
        assert!(content.contains("no longer shown"), "{content}");
    }
    for new in &results[3..] {
        assert_eq!(new["content"][1], screenshot());
    }
    // What is stored keeps every one.
    let (_, messages) = app.get(&format!("/v1/runs/{id}/messages"), KEY_A).await;
    assert_eq!(messages.to_string().matches(FAKE_SCREENSHOT).count(), 6);
}

#[tokio::test]
async fn a_cancelled_run_still_removes_its_computer() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "computer", json!({ "action": "screenshot" }))),
            Step::Block,
        ],
        COMPUTER_ON,
    )
    .await;
    let created = app.create_run(KEY_A, "look at the screen").await;
    let id = created["id"].as_str().unwrap().to_string();
    // Wait until the computer has been used and the model is thinking again.
    for _ in 0..200 {
        if app.llm.call_count() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(app.computers.created.lock().unwrap().len(), 1);
    let (status, _) = app
        .json(Method::POST, &format!("/v1/runs/{id}/cancel"), Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    app.wait_status(KEY_A, &id, "cancelled").await;
    assert_eq!(app.computers.destroyed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn readiness_includes_the_computer_image_when_it_is_on() {
    let app = TestApp::with_env(vec![], COMPUTER_ON).await;
    let (status, body) = app.json(Method::GET, "/readyz", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["checks"]["computer"], "ok", "{body}");

    app.computers.unhealthy.store(true, Ordering::SeqCst);
    let (status, body) = app.json(Method::GET, "/readyz", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["checks"]["computer"], "failed", "{body}");

    let off = TestApp::new(vec![]).await;
    let (_, body) = off.json(Method::GET, "/readyz", None, None).await;
    assert!(body["checks"].get("computer").is_none(), "{body}");
}
