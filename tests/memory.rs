//! Memory: what runs remember about the API key's owner across conversations, the `remember` and
//! `forget` tools that change it, and `/v1/memory`, where people see and delete it.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};

/// The tool results of a run, in order: `(output, is_error)`.
async fn tool_results(app: &TestApp, key: &str, id: &str) -> Vec<(String, bool)> {
    app.events(key, id, None)
        .await
        .iter()
        .filter(|e| e.data["type"] == "tool.result")
        .map(|e| {
            let d = &e.data["data"];
            (
                d["output"].as_str().unwrap_or_default().to_string(),
                d["is_error"] == true,
            )
        })
        .collect()
}

/// Run `prompt` to completion and return the run.
async fn run(app: &TestApp, key: &str, body: Value) -> Value {
    let run = app.create_run_with(key, body).await;
    app.wait_status(key, run["id"].as_str().unwrap(), "completed")
        .await
}

async fn memories(app: &TestApp, key: &str) -> Vec<Value> {
    let (status, body) = app.get("/v1/memory", key).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

async fn add(app: &TestApp, key: &str, text: &str) -> (StatusCode, Value) {
    app.json(
        Method::POST,
        "/v1/memory",
        Some(key),
        Some(json!({ "text": text })),
    )
    .await
}

fn system(app: &TestApp, request: usize) -> String {
    app.llm.requests()[request]["system"]
        .as_str()
        .unwrap()
        .to_string()
}

fn tool_names(app: &TestApp, request: usize) -> Vec<String> {
    app.llm.requests()[request]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_remembered_fact_reaches_the_next_conversation() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "m1",
            "remember",
            json!({ "text": "Prefers metric units" }),
        )),
        Step::Reply(text("Noted.")),
        Step::Reply(text("It is 20 °C.")),
    ])
    .await;
    let first = run(
        &app,
        KEY_A,
        json!({ "prompt": "I always want metric units", "interactive": true }),
    )
    .await;
    assert!(tool_names(&app, 0).contains(&"remember".to_string()));
    assert!(system(&app, 0).contains("You don't remember anything about this user"));

    let results = tool_results(&app, KEY_A, first["id"].as_str().unwrap()).await;
    assert_eq!(results.len(), 1);
    assert!(!results[0].1, "{}", results[0].0);
    let output: Value = serde_json::from_str(&results[0].0).unwrap();
    assert_eq!(output["status"], "added");
    assert_eq!(output["memory"]["text"], "Prefers metric units");

    let items = memories(&app, KEY_A).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Prefers metric units");
    assert_eq!(items[0]["run_id"], first["id"]);
    let id = items[0]["id"].as_str().unwrap();

    // A new conversation, not a continuation: the fact is in its system prompt.
    run(&app, KEY_A, json!({ "prompt": "How warm is it?" })).await;
    let sys = system(&app, 2);
    assert!(sys.contains("What you remember about this user"), "{sys}");
    assert!(
        sys.contains(&format!("[{id}] Prefers metric units")),
        "{sys}"
    );

    // Deleting the run keeps the memory, without its source.
    let (status, _) = app
        .json(
            Method::DELETE,
            &format!("/v1/runs/{}", first["id"].as_str().unwrap()),
            Some(KEY_A),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let items = memories(&app, KEY_A).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["run_id"], Value::Null);
}

#[tokio::test]
async fn the_model_can_replace_and_forget() {
    let app = TestApp::new(vec![]).await;
    let (_, old) = add(&app, KEY_A, "Lives in Bern").await;
    let (_, other) = add(&app, KEY_A, "Has a cat").await;
    let old_id = old["id"].as_str().unwrap();
    let other_id = other["id"].as_str().unwrap();

    let app = TestApp::build(
        app.state.store.clone(),
        ScriptedLlm::new(vec![
            Step::Reply(tool_use(
                "m1",
                "remember",
                json!({ "text": "Lives in Zurich", "replaces": old_id }),
            )),
            Step::Reply(tool_use("m2", "forget", json!({ "id": other_id }))),
            Step::Reply(tool_use("m3", "forget", json!({ "id": other_id }))),
            Step::Reply(text("Done.")),
        ]),
        FakeFactory::new(),
        &[],
    )
    .await;
    let done = run(
        &app,
        KEY_A,
        json!({ "prompt": "I moved to Zurich, and the cat is gone", "interactive": true }),
    )
    .await;
    let sys = system(&app, 0);
    assert!(sys.contains(&format!("[{old_id}] Lives in Bern")), "{sys}");
    assert!(sys.contains("Use remember"), "{sys}");

    let results = tool_results(&app, KEY_A, done["id"].as_str().unwrap()).await;
    let replaced: Value = serde_json::from_str(&results[0].0).unwrap();
    assert_eq!(replaced["status"], "replaced");
    assert_eq!(replaced["memory"]["id"], old_id);
    assert!(!results[1].1);
    // Forgetting it again is an error the model sees.
    assert!(results[2].1);
    assert!(
        results[2].0.contains("no memory with id"),
        "{}",
        results[2].0
    );

    let items = memories(&app, KEY_A).await;
    assert_eq!(items.len(), 1);
    assert_eq!(
        (&items[0]["id"], &items[0]["text"]),
        (&json!(old_id), &json!("Lives in Zurich"))
    );
}

#[tokio::test]
async fn unattended_runs_read_memory_but_cannot_change_it() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("m1", "remember", json!({ "text": "Likes jazz" }))),
        Step::Reply(text("ok")),
    ])
    .await;
    add(&app, KEY_A, "Prefers metric units").await;
    let done = run(&app, KEY_A, json!({ "prompt": "daily briefing" })).await;
    let sys = system(&app, 0);
    assert!(sys.contains("Prefers metric units"), "{sys}");
    assert!(sys.contains("Memory is read-only in this run"), "{sys}");
    let results = tool_results(&app, KEY_A, done["id"].as_str().unwrap()).await;
    assert!(results[0].1);
    assert!(
        results[0].0.contains("read-only in unattended runs"),
        "{}",
        results[0].0
    );
    assert_eq!(memories(&app, KEY_A).await.len(), 1);
}

#[tokio::test]
async fn without_the_memory_group_nothing_is_read_or_offered() {
    let app = TestApp::new(vec![Step::Reply(text("ok"))]).await;
    add(&app, KEY_A, "Prefers metric units").await;
    run(
        &app,
        KEY_A,
        json!({ "prompt": "hi", "interactive": true, "tools": ["web_fetch"] }),
    )
    .await;
    let names = tool_names(&app, 0);
    assert!(!names.contains(&"remember".to_string()), "{names:?}");
    assert!(!names.contains(&"forget".to_string()), "{names:?}");
    let sys = system(&app, 0);
    assert!(!sys.contains("Prefers metric units"), "{sys}");
    assert!(!sys.contains("remember"), "{sys}");
}

#[tokio::test]
async fn memory_off_removes_the_tools_and_the_note() {
    let app = TestApp::with_env(vec![Step::Reply(text("ok"))], &[("MEMORY", "off")]).await;
    let (status, body) = add(&app, KEY_A, "x").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    run(&app, KEY_A, json!({ "prompt": "hi", "interactive": true })).await;
    assert!(!tool_names(&app, 0).contains(&"remember".to_string()));
    assert!(!system(&app, 0).contains("remember"));

    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(defaults["memory"], Value::Null);
    let (_, tools) = app.get("/v1/tools", KEY_A).await;
    let group = tools["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "memory")
        .unwrap()
        .clone();
    assert_eq!(group["available"], false);
    assert_eq!(group["reason"], "Turned off on the server (MEMORY=off)");
    // What was saved before can still be listed and deleted.
    assert_eq!(memories(&app, KEY_A).await.len(), 0);
}

#[tokio::test]
async fn the_api_lists_adds_and_deletes_per_owner() {
    let app = TestApp::with_env(vec![], &[("MEMORY_MAX_ITEMS", "2")]).await;
    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(
        defaults["memory"],
        json!({ "max_items": 2, "max_chars": 500 })
    );
    let (_, tools) = app.get("/v1/tools", KEY_A).await;
    let group = tools["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "memory")
        .unwrap()
        .clone();
    assert_eq!(group["label"], "Memory");
    assert_eq!(group["available"], true);

    let (status, a) = add(&app, KEY_A, "  Prefers\n metric units ").await;
    assert_eq!(status, StatusCode::CREATED, "{a}");
    assert_eq!(a["text"], "Prefers metric units");
    assert_eq!(a["run_id"], Value::Null);
    // The same text again comes back as the item it already is.
    let (status, again) = add(&app, KEY_A, "prefers METRIC units").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["id"], a["id"]);
    let (status, _) = add(&app, KEY_A, "Has a cat").await;
    assert_eq!(status, StatusCode::CREATED);
    // Full.
    let (status, body) = add(&app, KEY_A, "Likes jazz").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("memory is full"));
    let (status, _) = add(&app, KEY_A, " ").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = add(&app, KEY_A, &"x".repeat(501)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Another key sees none of it, and can't delete it.
    add(&app, KEY_B, "B's fact").await;
    let (_, list) = app.get("/v1/memory", KEY_B).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["limits"], json!({ "max_items": 2, "max_chars": 500 }));
    let a_uri = format!("/v1/memory/{}", a["id"].as_str().unwrap());
    let (status, _) = app.json(Method::DELETE, &a_uri, Some(KEY_B), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .json(Method::DELETE, "/v1/memory/not-an-id", Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = app.json(Method::DELETE, &a_uri, Some(KEY_A), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let items = memories(&app, KEY_A).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Has a cat");

    let (status, _) = app
        .json(Method::DELETE, "/v1/memory", Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(memories(&app, KEY_A).await.is_empty());
    assert_eq!(memories(&app, KEY_B).await.len(), 1);

    let (status, _) = app.json(Method::GET, "/v1/memory", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
