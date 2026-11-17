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

// -- the user's own computer (COMPUTER_RUNTIME=native) ---------------------------------------

/// Follow a run's events until the `n`-th `approval.required` (from 1) arrives.
async fn approval(app: &TestApp, id: &str, n: usize) -> Value {
    let mut stream = LiveStream::open(app, KEY_A, id, None).await;
    let found = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut seen = 0;
        while let Some(e) = stream.next_event().await {
            if e.data["type"] == "approval.required" {
                seen += 1;
                if seen == n {
                    return Some(e.data["data"].clone());
                }
            }
        }
        None
    })
    .await;
    found.ok().flatten().unwrap_or_else(|| panic!("no approval #{n}"))
}

async fn decide(app: &TestApp, id: &str, approval: &Value, decision: &str) {
    let uri = format!("/v1/runs/{id}/approvals/{}", approval["id"].as_str().unwrap());
    let (status, _) = app
        .json(Method::POST, &uri, Some(KEY_A), Some(json!({ "decision": decision })))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

/// An app whose runs get the user's own computer: the `computer_runtime` setting says so.
async fn native_app(calls: Vec<Value>, env: &[(&str, &str)]) -> TestApp {
    let app = TestApp::with_env(script(calls), env).await;
    let native = Some(cedar::config::ComputerRuntime::Native);
    cedar::computer::set_runtime(&app.state.store, native).await.unwrap();
    app
}

#[tokio::test]
async fn the_users_own_computer_asks_before_every_call_until_always() {
    let app = native_app(
        vec![
            json!({ "action": "screenshot" }),
            json!({ "action": "click", "x": 200, "y": 150, "count": 2 }),
            json!({ "action": "type", "text": "hello" }),
            json!({ "action": "key", "keys": "ctrl+s" }),
        ],
        COMPUTER_ON,
    )
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "rename the file", "interactive": true }))
        .await;
    let id = run["id"].as_str().unwrap().to_string();

    // Looking is asked about too: the screenshot shows whatever is on the screen.
    let first = approval(&app, &id, 1).await;
    assert_eq!(first["tool"], "computer");
    assert_eq!(first["rule"], "computer");
    assert_eq!(first["scope"], "the rest of this reply");
    let reason = first["reason"].as_str().unwrap();
    assert!(reason.starts_with("Look at your screen on this computer."), "{reason}");
    assert!(app.computers.created.lock().unwrap().is_empty(), "nothing before the OK");
    decide(&app, &id, &first, "allow").await;

    // Once is once.
    let second = approval(&app, &id, 2).await;
    let reason = second["reason"].as_str().unwrap();
    assert!(reason.starts_with("Double-click at (200, 150) on this computer."), "{reason}");
    decide(&app, &id, &second, "deny").await;

    // "Always" covers the rest of this run: the last call runs without a card.
    let third = approval(&app, &id, 3).await;
    assert!(third["reason"].as_str().unwrap().starts_with("Type \"hello\""), "{third}");
    decide(&app, &id, &third, "allow_always").await;
    app.wait_status(KEY_A, &id, "completed").await;

    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(events.iter().filter(|e| e.event == "approval.required").count(), 3);
    assert_eq!(
        *app.computers.computer(0).requests.lock().unwrap(),
        [
            json!({ "action": "screenshot" }),
            json!({ "action": "type", "text": "hello" }),
            json!({ "action": "key", "keys": "ctrl+s" }),
        ],
        "the declined click never reached the computer"
    );
    let declined = tool_result(&app, 2);
    assert_eq!(declined["is_error"], true, "{declined}");
    assert!(declined["content"].as_str().unwrap().contains("declined"), "{declined}");

    // Nothing is remembered for the key: the next conversation is asked again.
    let again = app
        .create_run_with(KEY_A, json!({ "prompt": "look again", "interactive": true }))
        .await;
    assert!(app.state.store.grants(KEY_A, "computer").await.unwrap().is_empty());
    let (status, _) = app
        .json(Method::POST, &format!("/v1/runs/{}/cancel", again["id"].as_str().unwrap()), Some(KEY_A), None)
        .await;
    assert!(status == StatusCode::ACCEPTED || status == StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_run_nobody_answers_for_gets_no_computer_of_the_users_own() {
    let app = native_app(vec![json!({ "action": "screenshot" })], COMPUTER_ON).await;
    let created = app.create_run(KEY_A, "tidy my desktop").await;
    app.wait_status(KEY_A, created["id"].as_str().unwrap(), "completed")
        .await;
    let request = &app.llm.requests()[0];
    assert!(!offered(request).contains(&"computer"), "{:?}", offered(request));
    assert_eq!(tool_result(&app, 1)["content"], "unknown tool: computer");
    assert!(app.computers.created.lock().unwrap().is_empty());

    // A container is a sandbox, so an unattended run may have one.
    let (sandboxed, _) = run_calls(vec![json!({ "action": "screenshot" })], COMPUTER_ON).await;
    assert!(offered(&sandboxed.llm.requests()[0]).contains(&"computer"));
}

#[tokio::test]
async fn with_the_rule_off_the_users_own_computer_is_not_asked_about() {
    let app = native_app(
        vec![json!({ "action": "screenshot" })],
        &[("COMPUTER", "on"), ("TOOL_APPROVALS", "deletes")],
    )
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "look", "interactive": true }))
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    let events = app.events(KEY_A, &id, None).await;
    assert!(events.iter().all(|e| e.event != "approval.required"));
    assert_eq!(app.computers.computer(0).requests.lock().unwrap().len(), 1);
}

// -- choosing the runtime in the settings (`computer_runtime`) -------------------------------

const ADMIN: &[(&str, &str)] = &[("COMPUTER", "on"), ("CEDAR_ADMIN_KEYS", "key-a")];

async fn settings(app: &TestApp, body: Value) -> (StatusCode, Value) {
    app.json(Method::PUT, "/v1/admin/settings", Some(KEY_A), Some(body)).await
}

#[tokio::test]
async fn an_admin_chooses_what_the_computer_tool_works() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t0", "computer", json!({ "action": "screenshot" }))),
            Step::Reply(text("done")),
            Step::Reply(text("done")),
        ],
        ADMIN,
    )
    .await;
    let (status, shown) = app.get("/v1/admin/settings", KEY_A).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shown["computer_runtime"], Value::Null, "{shown}");
    assert_eq!(shown["defaults"]["computer_runtime"], "docker");
    assert_eq!(shown["computer_enabled"], true);
    assert_eq!(shown["native_computer_supported"], true);

    let (status, saved) = settings(&app, json!({ "computer_runtime": "native" })).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["computer_runtime"], "native");

    // From the next run: the tool is described as the user's own computer, has no `open`, and
    // its calls wait for an OK.
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "look", "interactive": true }))
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    let asked = approval(&app, &id, 1).await;
    assert_eq!(asked["rule"], "computer");
    decide(&app, &id, &asked, "allow").await;
    app.wait_status(KEY_A, &id, "completed").await;
    let request = &app.llm.requests()[0];
    let tool = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "computer")
        .unwrap();
    assert!(tool["description"].as_str().unwrap().contains("their real computer"), "{tool}");
    assert!(!tool.to_string().contains("\"open\""), "{tool}");

    // The default is saved as no setting; `null` goes back to it too.
    let (_, back) = settings(&app, json!({ "computer_runtime": "docker" })).await;
    assert_eq!(back["computer_runtime"], Value::Null, "{back}");
    settings(&app, json!({ "computer_runtime": "native" })).await;
    let (_, reset) = settings(&app, json!({ "computer_runtime": null })).await;
    assert_eq!(reset["computer_runtime"], Value::Null);
    let run = app.create_run(KEY_A, "nothing to do").await;
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed").await;
    let sandboxed = app.llm.requests()[2]["tools"].to_string();
    assert!(sandboxed.contains("Work a desktop computer"), "back in the container");

    // Other settings are left alone by a change of this one, and it by theirs.
    settings(&app, json!({ "computer_runtime": "native" })).await;
    let (_, other) = settings(&app, json!({ "max_image_side": 1024 })).await;
    assert_eq!((&other["computer_runtime"], &other["max_image_side"]), (&json!("native"), &json!(1024)));
}

#[tokio::test]
async fn a_runtime_that_cannot_work_is_refused() {
    let app = TestApp::with_env(vec![], ADMIN).await;
    let (status, body) = settings(&app, json!({ "computer_runtime": "vm" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("\"docker\" or \"native\""), "{body}");

    // The image is missing: switching to the container would fail every call after it.
    app.computers.unhealthy.store(true, Ordering::SeqCst);
    let (status, body) = settings(&app, json!({ "computer_runtime": "docker" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("can't run here yet"), "{body}");
    let (_, shown) = app.get("/v1/admin/settings", KEY_A).await;
    assert_eq!(shown["computer_runtime"], Value::Null, "nothing was saved");

    // With the tool off there is nothing to choose; only an admin may choose at all.
    let off = TestApp::with_env(vec![], &[("CEDAR_ADMIN_KEYS", "key-a")]).await;
    let (status, body) = settings(&off, json!({ "computer_runtime": "native" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("COMPUTER=off"), "{body}");
    let (_, shown) = off.get("/v1/admin/settings", KEY_A).await;
    assert_eq!(shown["computer_enabled"], false);
    let (status, _) = app
        .json(Method::PUT, "/v1/admin/settings", Some(KEY_B), Some(json!({ "computer_runtime": "native" })))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
