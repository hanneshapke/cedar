//! Private runs (`"private": true`): nothing about them outlives their conversation. No memory
//! is shown or saved, there are no records tools and no import, no trace of the model calls is
//! kept, and no webhook is called. Every continuation of a private run is private.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};
use std::time::Duration;

/// Run `body` to completion and return the run.
async fn run(app: &TestApp, key: &str, body: Value) -> Value {
    let run = app.create_run_with(key, body).await;
    app.wait_status(key, run["id"].as_str().unwrap(), "completed")
        .await
}

async fn memories(app: &TestApp, key: &str, query: &str) -> Vec<Value> {
    let (status, body) = app.get(&format!("/v1/memory{query}"), key).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

fn tool_names(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn kinds_of(events: &[SseEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| e.data["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_private_run_is_shown_no_memory_and_saves_none() {
    // One reply only: a memory call at the end of the run would have nothing to answer it.
    let app = TestApp::with_env(
        vec![Step::Reply(text("Nice to meet you, Ana."))],
        &[("MEMORY_EXTRACT", "on")],
    )
    .await;
    let (status, _) = app
        .json(
            Method::POST,
            "/v1/memory",
            Some(KEY_A),
            Some(json!({ "text": "Prefers metric units" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let done = run(
        &app,
        KEY_A,
        json!({ "prompt": "Hi, I'm Ana and I live in Zurich.", "interactive": true, "private": true }),
    )
    .await;
    assert_eq!(done["private"], true);
    assert_eq!(done["final_text"], "Nice to meet you, Ana.");

    // Neither memory nor records reached the model: no tools, no note.
    assert_eq!(app.llm.call_count(), 1);
    let request = &app.llm.requests()[0];
    let names = tool_names(request);
    for t in ["remember", "forget", "recall", "records_save", "records_lookup", "records_list"] {
        assert!(!names.contains(&t.to_string()), "{names:?}");
    }
    let sys = request["system"].as_str().unwrap();
    assert!(!sys.contains("Prefers metric units"), "{sys}");
    assert!(!sys.contains("What you remember about this user"), "{sys}");

    // Nothing was noted, not even that the run happened, and what was there was not counted
    // as recalled.
    let id = done["id"].as_str().unwrap();
    let events = kinds_of(&app.events(KEY_A, id, None).await);
    assert!(!events.iter().any(|k| k.starts_with("memory.")), "{events:?}");
    let all = memories(&app, KEY_A, "?kind=fact,preference,instruction,temporary,episode,procedure").await;
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["text"], "Prefers metric units");
    assert_eq!(all[0]["recall_count"], 0);
}

#[tokio::test]
async fn a_private_run_that_fails_leaves_no_episode() {
    let app = TestApp::with_env(
        vec![Step::Error("the model is down"), Step::Error("the model is down")],
        &[("MEMORY_EXPERIENCE", "on")],
    )
    .await;
    let episodes = "?kind=episode,procedure,fact";
    let created = app
        .create_run_with(KEY_A, json!({ "prompt": "Plan my secret trip", "private": true }))
        .await;
    app.wait_status(KEY_A, created["id"].as_str().unwrap(), "failed")
        .await;
    // The bookkeeping after a run is not awaited by the status; give it a moment.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let all = memories(&app, KEY_A, episodes).await;
    assert!(all.is_empty(), "{all:?}");

    // The same run, not private, is noted as an episode: that is what was left out above.
    let created = app
        .create_run_with(KEY_A, json!({ "prompt": "Plan my trip" }))
        .await;
    app.wait_status(KEY_A, created["id"].as_str().unwrap(), "failed")
        .await;
    let mut all = Vec::new();
    for _ in 0..200 {
        all = memories(&app, KEY_A, episodes).await;
        if !all.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["kind"], "episode");
}

#[tokio::test]
async fn a_private_run_keeps_no_trace() {
    let app = TestApp::new(vec![Step::Repeat(text("ok"))]).await;
    let private = run(&app, KEY_A, json!({ "prompt": "between us", "private": true })).await;
    let id = private["id"].as_str().unwrap();
    let (status, body) = app.get(&format!("/v1/runs/{id}/trace"), KEY_A).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Nothing was written that a trace could be built from.
    let run_id = uuid::Uuid::parse_str(id).unwrap();
    assert!(app.state.store.list_llm_calls(run_id).await.unwrap().is_empty());
    assert_eq!(app.state.store.sent_prompt(run_id).await.unwrap(), (None, None));

    // The conversation itself is still there: a continuation needs it.
    let (status, body) = app.get(&format!("/v1/runs/{id}/messages"), KEY_A).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["messages"].as_array().unwrap().len(), 2);

    // A run that isn't private keeps its trace, as before.
    let open = run(&app, KEY_A, json!({ "prompt": "for the record" })).await;
    let (status, trace) = app
        .get(&format!("/v1/runs/{}/trace", open["id"].as_str().unwrap()), KEY_A)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(trace["calls"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_private_run_imports_no_attachments_into_records() {
    use base64::Engine;
    let app = TestApp::new(vec![Step::Repeat(text("ok"))]).await;
    let csv = json!({ "name": "Running Log.csv", "media_type": "text/csv",
        "data": base64::engine::general_purpose::STANDARD.encode("Date,km\n2026-11-01,5.2\n") });
    let done = run(
        &app,
        KEY_A,
        json!({ "prompt": "my runs", "attachments": [csv], "import_attachments": true, "private": true }),
    )
    .await;
    assert_eq!(done["import_attachments"], false);
    let (_, body) = app.get("/v1/records", KEY_A).await;
    assert!(body["sets"].as_array().unwrap().is_empty(), "{body}");
}

#[tokio::test]
async fn a_continuation_of_a_private_run_is_private_whatever_it_asks_for() {
    let app = TestApp::new(vec![Step::Repeat(text("ok"))]).await;
    let first = run(&app, KEY_A, json!({ "prompt": "between us", "private": true })).await;
    let second = run(
        &app,
        KEY_A,
        json!({ "prompt": "and now openly", "parent_id": first["id"], "private": false }),
    )
    .await;
    assert_eq!(second["private"], true);
    let (status, _) = app
        .get(&format!("/v1/runs/{}/trace", second["id"].as_str().unwrap()), KEY_A)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The other way round is fine: an open conversation can go private from some message on.
    let open = run(&app, KEY_A, json!({ "prompt": "openly" })).await;
    assert_eq!(open["private"], false);
    let later = run(
        &app,
        KEY_A,
        json!({ "prompt": "from here on, between us", "parent_id": open["id"], "private": true }),
    )
    .await;
    assert_eq!(later["private"], true);
}

#[tokio::test]
async fn a_private_run_calls_no_webhook() {
    let app = TestApp::with_env(
        vec![Step::Repeat(text("ok"))],
        &[("WEBHOOK_ALLOW_PRIVATE", "on"), ("WEBHOOK_RETRY_SECS", "0")],
    )
    .await;
    // Asking for both is a mistake, said at once.
    let (status, body) = app
        .json(
            Method::POST,
            "/v1/runs",
            Some(KEY_A),
            Some(json!({ "prompt": "hi", "private": true, "callback_url": "http://127.0.0.1:9/hook" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("private"), "{body}");

    // A continuation keeps its parent's webhook, unless it is private.
    let open = run(
        &app,
        KEY_A,
        json!({ "prompt": "hi", "callback_url": "http://127.0.0.1:9/hook" }),
    )
    .await;
    let private = run(
        &app,
        KEY_A,
        json!({ "prompt": "between us", "parent_id": open["id"], "private": true }),
    )
    .await;
    assert_eq!(private["callback_url"], Value::Null);
    let id = uuid::Uuid::parse_str(private["id"].as_str().unwrap()).unwrap();
    // The run is still announced on the key's own event stream, with nothing to deliver.
    let (attempts, delivered, _) = app.state.store.delivery_status(id).await.unwrap().unwrap();
    assert_eq!((attempts, delivered), (0, None));
    assert!(app.state.store.due_deliveries(10).await.unwrap().iter().all(|d| d.notification.run_id != id));
}
