//! Runs that leave the choice of model to cedar (the `model_selection` setting is off, as it is
//! unless an admin turns it on): the smallest registered model for everyday requests, the biggest
//! for planning, reviewing, complex tool calls and long tasks.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};

/// A small model by its name; `TEST_MODEL`, whose name has no count, is the biggest.
const SMALL: &str = "qwen3:4b";

fn id_of(run: &Value) -> String {
    run["id"].as_str().unwrap().to_string()
}

/// The model each request so far was sent to.
fn asked(app: &TestApp) -> Vec<String> {
    app.llm
        .requests()
        .iter()
        .map(|r| r["model"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn the_only_model_is_used_for_everything() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "bash", json!({ "command": "echo hi" }))),
        Step::Reply(text("done")),
    ])
    .await;
    let run = app.create_run(KEY_A, "say hi").await;
    assert_eq!(run["model_id"], TEST_MODEL);
    assert_eq!(run["model_auto"], true);
    app.wait_status(KEY_A, &id_of(&run), "completed").await;
    assert_eq!(asked(&app), [TEST_MODEL, TEST_MODEL], "no reply is asked for twice");
}

#[tokio::test]
async fn an_everyday_request_stays_on_the_smallest_model() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "read_file", json!({ "path": "notes.txt" }))),
        Step::Reply(text("it says hi")),
    ])
    .await;
    app.register("small", SMALL, &[]).await;
    let run = app.create_run(KEY_A, "what's in notes.txt?").await;
    assert_eq!(run["model_id"], "small");
    assert_eq!(run["model_auto"], true);
    let done = app.wait_status(KEY_A, &id_of(&run), "completed").await;
    assert_eq!(asked(&app), [SMALL, SMALL], "reading a file is no complex call");
    assert_eq!(done["model_id"], "small");
    assert_eq!(done["turns"], 2);
}

#[tokio::test]
async fn a_complex_tool_call_is_asked_again_of_the_biggest_model() {
    let app = TestApp::new(vec![
        // The smallest model reaches for bash: dropped, and the turn is asked again.
        Step::Reply(tool_use("t0", "bash", json!({ "command": "rm -rf wrong" }))),
        Step::Reply(tool_use("t1", "bash", json!({ "command": "echo hi" }))),
        Step::Reply(text("done")),
    ])
    .await;
    app.register("small", SMALL, &[]).await;
    let run = app.create_run(KEY_A, "say hi").await;
    let id = id_of(&run);
    assert_eq!(run["model_id"], "small");
    let done = app.wait_status(KEY_A, &id, "completed").await;
    assert_eq!(asked(&app), [SMALL, TEST_MODEL, TEST_MODEL], "the run stays with the biggest");
    assert_eq!(done["model_id"], TEST_MODEL, "the run reports the model it went on with");
    assert_eq!(done["model"], TEST_MODEL);
    assert_eq!(done["turns"], 2, "the dropped reply is no turn of its own");
    // Its tokens count all the same.
    assert_eq!(done["usage"], json!({ "input_tokens": 30, "output_tokens": 15 }));

    // Only the biggest model's call ran, and only it is in the conversation.
    let commands = app.factory.sandbox(0).commands.lock().unwrap().clone();
    assert!(commands.iter().any(|c| c == "echo hi"), "{commands:?}");
    assert!(!commands.iter().any(|c| c.contains("wrong")), "{commands:?}");
    let (_, messages) = app.get(&format!("/v1/runs/{id}/messages"), KEY_A).await;
    let stored = messages.to_string();
    assert!(stored.contains("echo hi") && !stored.contains("wrong"), "{stored}");
    // The second request is the first one again, for the other model.
    let requests = app.llm.requests();
    assert_eq!(requests[0]["messages"], requests[1]["messages"]);
}

#[tokio::test]
async fn a_task_that_takes_many_turns_goes_on_with_the_biggest_model() {
    let turns = cedar::routing::SMALL_TURNS as usize;
    let mut script: Vec<Step> = (0..=turns)
        .map(|i| Step::Reply(tool_use(&format!("t{i}"), "read_file", json!({ "path": "notes.txt" }))))
        .collect();
    script.push(Step::Reply(text("done")));
    let app = TestApp::new(script).await;
    app.register("small", SMALL, &[]).await;
    let run = app.create_run(KEY_A, "read it again and again").await;
    let done = app.wait_status(KEY_A, &id_of(&run), "completed").await;
    let asked = asked(&app);
    assert_eq!(asked.len(), turns + 2);
    assert!(asked[..turns].iter().all(|m| m == SMALL), "{asked:?}");
    assert!(asked[turns..].iter().all(|m| m == TEST_MODEL), "{asked:?}");
    assert_eq!(done["model_id"], TEST_MODEL);
}

#[tokio::test]
async fn a_run_that_plans_uses_the_biggest_model_throughout() {
    let app = TestApp::new(vec![
        Step::Reply(text(r#"{"steps": ["Look it up", "Write it down"]}"#)),
        Step::Reply(text("done")),
    ])
    .await;
    app.register("small", SMALL, &[]).await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "chart the market", "plan": "on" }))
        .await;
    assert_eq!(run["model_id"], TEST_MODEL, "it starts on the model that plans");
    app.wait_status(KEY_A, &id_of(&run), "completed").await;
    assert_eq!(asked(&app), [TEST_MODEL, TEST_MODEL]);
}

#[tokio::test]
async fn each_model_is_sent_its_own_default_reasoning_level() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t0", "run_python", json!({ "code": "print(1)" }))),
        Step::Reply(text("done")),
    ])
    .await;
    // The smallest model has a default level; `TEST_MODEL` has none.
    let mut small = register_model(&app.state.store, "small", SMALL, &["low", "medium"]).await;
    small.default_reasoning = Some("low".into());
    app.state.store.upsert_model(&small).await.unwrap();
    app.state.llm.set("small", app.llm.clone());
    app.state.integrations.reload().await.unwrap();

    let run = app.create_run(KEY_A, "compute").await;
    assert_eq!(run["reasoning"], "low");
    let done = app.wait_status(KEY_A, &id_of(&run), "completed").await;
    let requests = app.llm.requests();
    assert_eq!(requests[0]["reasoning_effort"], "low");
    assert_eq!(requests[1]["reasoning_effort"], Value::Null, "the biggest model has no default level");
    assert_eq!(done["reasoning"], Value::Null);
}

#[tokio::test]
async fn a_named_model_or_level_is_taken_as_asked() {
    let app = TestApp::new(vec![Step::Repeat(text("ok"))]).await;
    app.register("small", SMALL, &[]).await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "model": TEST_MODEL }))
        .await;
    assert_eq!(run["model_id"], TEST_MODEL);
    assert_eq!(run["model_auto"], false);
    // A level belongs to a model, so naming one leaves the choice with the caller too: the
    // default model, as when users choose.
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "reasoning": "high" }))
        .await;
    assert_eq!(run["model_id"], TEST_MODEL);
    assert_eq!(run["reasoning"], "high");
    assert_eq!(run["model_auto"], false);
}

#[tokio::test]
async fn a_named_small_model_keeps_its_complex_calls() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "bash", json!({ "command": "echo hi" }))),
        Step::Reply(text("done")),
    ])
    .await;
    app.register("small", SMALL, &[]).await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "say hi", "model": "small" }))
        .await;
    let done = app.wait_status(KEY_A, &id_of(&run), "completed").await;
    assert_eq!(asked(&app), [SMALL, SMALL]);
    assert_eq!(done["model_id"], "small");
}

#[tokio::test]
async fn with_model_selection_on_a_run_gets_the_default_model() {
    let app = TestApp::new(vec![
        Step::Reply(text("a")),
        Step::Reply(text("b")),
    ])
    .await;
    app.register("small", SMALL, &[]).await;
    cedar::routing::set_selection(&app.state.store, true).await.unwrap();
    let run = app.create_run(KEY_A, "hi").await;
    assert_eq!(run["model_id"], TEST_MODEL);
    assert_eq!(run["model_auto"], false);
    app.wait_status(KEY_A, &id_of(&run), "completed").await;
    // And a continuation its parent's.
    let first = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "model": "small" }))
        .await;
    app.wait_status(KEY_A, &id_of(&first), "completed").await;
    let next = app
        .create_run_with(KEY_A, json!({ "prompt": "more", "parent_id": id_of(&first) }))
        .await;
    assert_eq!(next["model_id"], "small");

    // Off again, the continuation is cedar's to place.
    cedar::routing::set_selection(&app.state.store, false).await.unwrap();
    let (_, listed) = app.get("/v1/models", KEY_A).await;
    assert_eq!(listed["model_selection"], false);
}

#[tokio::test]
async fn images_go_to_a_model_that_takes_them() {
    let app = TestApp::new(vec![Step::Reply(text("a cat"))]).await;
    // The smallest model takes no images; `TEST_MODEL` does.
    app.register("small", SMALL, &[]).await;
    let png = json!({ "name": "cat.png", "media_type": "image/png",
        "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==" });
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "what is this?", "attachments": [png] }))
        .await;
    assert_eq!(run["model_id"], TEST_MODEL);
    app.wait_status(KEY_A, &id_of(&run), "completed").await;
    assert_eq!(asked(&app), [TEST_MODEL]);
}

#[tokio::test]
async fn compacting_a_conversation_is_for_the_biggest_model() {
    let app = TestApp::new(vec![
        Step::Reply(text("hello")),
        Step::Reply(text("Say hello.")),
    ])
    .await;
    app.register("small", SMALL, &[]).await;
    let run = app.create_run(KEY_A, "say hello").await;
    let id = id_of(&run);
    let done = app.wait_status(KEY_A, &id, "completed").await;
    assert_eq!(done["model_id"], "small");
    let (status, body) = app
        .json(Method::POST, "/v1/compact", Some(KEY_A), Some(json!({ "run_id": id })))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["model_id"], TEST_MODEL);
    assert_eq!(asked(&app), [SMALL, TEST_MODEL]);
}

#[tokio::test]
async fn an_admin_turns_model_selection_on_and_off() {
    let env = [("CEDAR_ADMIN_KEYS", KEY_B), ("CEDAR_SECRET_KEY", "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")];
    let app = TestApp::with_env(vec![], &env).await;
    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(defaults["model_selection"], false, "off unless an admin turns it on");
    let put = |body: Value| app.json(Method::PUT, "/v1/admin/settings", Some(KEY_B), Some(body));

    let (status, settings) = put(json!({ "model_selection": true })).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["model_selection"], true);
    assert_eq!(settings["defaults"]["model_selection"], false);
    assert_eq!(app.get("/v1/defaults", KEY_A).await.1["model_selection"], true);
    assert_eq!(app.get("/v1/models", KEY_A).await.1["model_selection"], true);
    // Other settings leave it alone; `null` and `false` both turn it off.
    let (_, settings) = put(json!({ "run_timeout_secs": 120 })).await;
    assert_eq!(settings["model_selection"], true);
    let (_, settings) = put(json!({ "model_selection": null })).await;
    assert_eq!(settings["model_selection"], false);
    put(json!({ "model_selection": true })).await;
    let (_, settings) = put(json!({ "model_selection": false })).await;
    assert_eq!(settings["model_selection"], false);

    let (status, _) = put(json!({ "model_selection": "yes" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = app
        .json(Method::PUT, "/v1/admin/settings", Some(KEY_A), Some(json!({ "model_selection": true })))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a plain key may not change it");
}
