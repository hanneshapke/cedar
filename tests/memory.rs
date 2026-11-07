//! Memory: what runs remember about the API key's owner across conversations; the `remember`,
//! `forget` and `recall` tools; the call at the end of a run that notes the facts the user
//! stated, an episode and a procedure; the background consolidation pass; and `/v1/memory`,
//! where people see, change and delete it all.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};
use std::time::Duration;

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

async fn archived(app: &TestApp, key: &str) -> Vec<Value> {
    let (status, body) = app.get("/v1/memory?archived=true", key).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

async fn add(app: &TestApp, key: &str, text: &str) -> (StatusCode, Value) {
    add_with(app, key, json!({ "text": text })).await
}

async fn add_with(app: &TestApp, key: &str, body: Value) -> (StatusCode, Value) {
    app.json(Method::POST, "/v1/memory", Some(key), Some(body))
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

/// The events of kind `kind` in a run, in order.
async fn events_of(app: &TestApp, key: &str, id: &str, kind: &str) -> Vec<Value> {
    app.events(key, id, None)
        .await
        .iter()
        .filter(|e| e.data["type"] == kind)
        .map(|e| e.data["data"].clone())
        .collect()
}

#[tokio::test]
async fn a_remembered_fact_reaches_the_next_conversation() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "m1",
            "remember",
            json!({ "text": "Prefers metric units", "kind": "preference" }),
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
    let names = tool_names(&app, 0);
    for t in ["remember", "forget", "recall"] {
        assert!(names.contains(&t.to_string()), "{names:?}");
    }
    assert!(system(&app, 0).contains("You don't remember anything about this user"));
    assert!(events_of(
        &app,
        KEY_A,
        first["id"].as_str().unwrap(),
        "memory.recalled"
    )
    .await
    .is_empty());

    let results = tool_results(&app, KEY_A, first["id"].as_str().unwrap()).await;
    assert_eq!(results.len(), 1);
    assert!(!results[0].1, "{}", results[0].0);
    let output: Value = serde_json::from_str(&results[0].0).unwrap();
    assert_eq!(output["status"], "added");
    assert_eq!(output["memory"]["id"], "m1");
    assert_eq!(output["memory"]["kind"], "preference");

    let items = memories(&app, KEY_A).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Prefers metric units");
    assert_eq!(items[0]["short_id"], "m1");
    assert_eq!(items[0]["kind"], "preference");
    assert_eq!(items[0]["source"], "user");
    assert_eq!(items[0]["support"], "Prefers metric units");
    assert_eq!(items[0]["run_id"], first["id"]);
    assert!(items[0]["valid_from"].is_string());

    // A new conversation, not a continuation: the fact is in its system prompt, last, and the
    // run says what it was shown.
    let second = run(&app, KEY_A, json!({ "prompt": "How warm is it?" })).await;
    let sys = system(&app, 2);
    assert!(sys.contains("What you remember about this user"), "{sys}");
    assert!(
        sys.contains("About them:\n[m1] Prefers metric units"),
        "{sys}"
    );
    let recalled = events_of(
        &app,
        KEY_A,
        second["id"].as_str().unwrap(),
        "memory.recalled",
    )
    .await;
    assert_eq!(recalled.len(), 1);
    assert_eq!(recalled[0]["items"][0]["short_id"], "m1");
    assert_eq!(recalled[0]["items"][0]["why"], "core");
    assert_eq!(recalled[0]["profile"], false);
    let (_, trace) = app
        .get(
            &format!("/v1/runs/{}/trace", second["id"].as_str().unwrap()),
            KEY_A,
        )
        .await;
    assert_eq!(trace["memory"]["items"][0]["short_id"], "m1");
    let span = &trace["memory_note"];
    let start = span["start"].as_u64().unwrap() as usize;
    assert!(trace["system"].as_str().unwrap()[start..].contains("[m1] Prefers metric units"));
    assert_eq!(memories(&app, KEY_A).await[0]["recall_count"], 1);

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
async fn the_model_can_supersede_forget_and_recall() {
    let app = TestApp::new(vec![]).await;
    let (_, old) = add(&app, KEY_A, "Lives in Bern").await;
    let (_, other) = add(&app, KEY_A, "Has a cat").await;
    assert_eq!(
        (old["short_id"].as_str(), other["short_id"].as_str()),
        (Some("m1"), Some("m2"))
    );

    let app = TestApp::build(
        app.state.store.clone(),
        ScriptedLlm::new(vec![
            Step::Reply(tool_use(
                "m1",
                "remember",
                json!({ "text": "Lives in Zurich", "supersedes": "m1" }),
            )),
            Step::Reply(tool_use("m2", "forget", json!({ "id": "[m2]" }))),
            Step::Reply(tool_use("m3", "forget", json!({ "id": "m2" }))),
            Step::Reply(tool_use(
                "m4",
                "recall",
                json!({ "query": "where does the user live", "history": true }),
            )),
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
    assert!(sys.contains("[m1] Lives in Bern"), "{sys}");
    assert!(sys.contains("Use remember"), "{sys}");

    let results = tool_results(&app, KEY_A, done["id"].as_str().unwrap()).await;
    let superseded: Value = serde_json::from_str(&results[0].0).unwrap();
    assert_eq!(superseded["status"], "superseded");
    assert_eq!(superseded["memory"]["id"], "m3");
    assert!(!results[1].1, "{}", results[1].0);
    // Forgetting it again is an error the model sees.
    assert!(results[2].1);
    assert!(
        results[2].0.contains("no memory with id"),
        "{}",
        results[2].0
    );
    let found: Value = serde_json::from_str(&results[3].0).unwrap();
    let matches = found["matches"].as_array().unwrap();
    assert_eq!(matches[0]["id"], "m3");
    assert_eq!(matches[0]["text"], "Lives in Zurich");
    assert!(
        matches
            .iter()
            .any(|m| m["id"] == "m1" && m["superseded"] == true),
        "{matches:?}"
    );

    let items = memories(&app, KEY_A).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Lives in Zurich");
    // The old city and the cat are history, not gone: in the archive, with the chain.
    let gone = archived(&app, KEY_A).await;
    assert_eq!(gone.len(), 2);
    assert_eq!(gone[0]["superseded_by"], items[0]["id"]);
    assert!(gone[1]["archived_at"].is_string());
    let (status, history) = app
        .get(
            &format!("/v1/memory/{}/history", items[0]["id"].as_str().unwrap()),
            KEY_A,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let versions: Vec<&str> = history["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["text"].as_str().unwrap())
        .collect();
    assert_eq!(versions, ["Lives in Bern", "Lives in Zurich"]);
    let ops: Vec<&str> = history["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["op"].as_str().unwrap())
        .collect();
    assert_eq!(ops, ["add", "add", "supersede"]);
    assert_eq!(history["ops"][2]["actor"], done["id"]);
}

#[tokio::test]
async fn unattended_runs_read_memory_but_cannot_change_it() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("m1", "remember", json!({ "text": "Likes jazz" }))),
        Step::Reply(tool_use("m2", "recall", json!({ "query": "units" }))),
        Step::Reply(text("ok")),
    ])
    .await;
    add(&app, KEY_A, "Prefers metric units").await;
    let done = run(&app, KEY_A, json!({ "prompt": "daily briefing" })).await;
    let sys = system(&app, 0);
    assert!(sys.contains("Prefers metric units"), "{sys}");
    assert!(sys.contains("read-only in this run"), "{sys}");
    let results = tool_results(&app, KEY_A, done["id"].as_str().unwrap()).await;
    assert!(results[0].1);
    assert!(
        results[0].0.contains("read-only in unattended runs"),
        "{}",
        results[0].0
    );
    // Reading is fine.
    assert!(!results[1].1, "{}", results[1].0);
    assert!(results[1].0.contains("Prefers metric units"));
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
    for t in ["remember", "forget", "recall"] {
        assert!(!names.contains(&t.to_string()), "{names:?}");
    }
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
async fn the_api_lists_adds_changes_and_deletes_per_owner() {
    let app = TestApp::with_env(vec![], &[("MEMORY_MAX_ITEMS", "2")]).await;
    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(defaults["memory"]["max_items"], 2);
    assert_eq!(defaults["memory"]["max_chars"], 500);
    assert_eq!(defaults["memory"]["budgets"]["facts"], 2);
    assert_eq!(defaults["memory"]["recall_tokens"], 600);
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
    assert_eq!(group["tools"].as_array().unwrap().len(), 3);

    let (status, a) = add(&app, KEY_A, "  Prefers\n metric units ").await;
    assert_eq!(status, StatusCode::CREATED, "{a}");
    assert_eq!(a["text"], "Prefers metric units");
    assert_eq!(a["kind"], "fact");
    assert_eq!(a["source"], "api");
    assert_eq!(a["run_id"], Value::Null);
    // The same text again comes back as the item it already is, said once more.
    let (status, again) = add(&app, KEY_A, "prefers METRIC units").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["id"], a["id"]);
    assert_eq!(again["mentions"], 2);
    let (status, cat) = add(&app, KEY_A, "Has a cat").await;
    assert_eq!(status, StatusCode::CREATED);
    // Over the budget: the lowest-ranked row is archived, nothing is refused.
    let (status, jazz) = add(&app, KEY_A, "Likes jazz").await;
    assert_eq!(status, StatusCode::CREATED, "{jazz}");
    let items = memories(&app, KEY_A).await;
    let texts: Vec<&str> = items.iter().map(|m| m["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["Prefers metric units", "Likes jazz"]);
    let gone = archived(&app, KEY_A).await;
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0]["id"], cat["id"]);
    let (status, _) = add(&app, KEY_A, " ").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = add(&app, KEY_A, &"x".repeat(501)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = add(&app, KEY_A, "API key: sk-1234567890abcdefghijklmnop").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = add_with(&app, KEY_A, json!({ "text": "x", "kind": "episode" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // Kinds, pins and dates by hand.
    let (status, rule) = add_with(
        &app,
        KEY_A,
        json!({ "text": "Always cite sources", "kind": "instruction" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");
    assert_eq!(rule["pinned"], true);
    let (status, exam) = add_with(&app, KEY_A, json!({ "text": "Has an exam", "kind": "temporary", "valid_until": "2099-01-01T00:00:00Z", "pinned": true })).await;
    assert_eq!(status, StatusCode::CREATED, "{exam}");
    assert!(exam["valid_until"]
        .as_str()
        .unwrap()
        .starts_with("2099-01-01"));
    let (_, list) = app
        .get("/v1/memory?kind=instruction,temporary", KEY_A)
        .await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    let (_, list) = app.get("/v1/memory?q=exam", KEY_A).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["id"], exam["id"]);

    // PATCH: edit, pin, archive, restore; the history keeps every step.
    let uri = format!("/v1/memory/{}", jazz["id"].as_str().unwrap());
    let (status, edited) = app
        .json(
            Method::PATCH,
            &uri,
            Some(KEY_A),
            Some(json!({ "text": "Likes jazz and blues", "pinned": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["text"], "Likes jazz and blues");
    assert_eq!(edited["pinned"], true);
    let (status, item) = app
        .json(
            Method::PATCH,
            &uri,
            Some(KEY_A),
            Some(json!({ "archived": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert!(item["archived_at"].is_string());
    let (status, item) = app
        .json(
            Method::PATCH,
            &uri,
            Some(KEY_A),
            Some(json!({ "archived": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert!(item["archived_at"].is_null());
    let (_, history) = app.get(&format!("{uri}/history"), KEY_A).await;
    let ops: Vec<&str> = history["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["op"].as_str().unwrap())
        .collect();
    assert_eq!(ops, ["add", "update", "pin", "archive", "restore"]);
    assert_eq!(history["ops"][1]["before"], "Likes jazz");
    assert_eq!(history["ops"][1]["actor"], "user");
    let (status, _) = app
        .json(
            Method::PATCH,
            &uri,
            Some(KEY_A),
            Some(json!({ "text": " " })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = app.get(&uri, KEY_A).await;
    assert_eq!(status, StatusCode::OK);

    // The profile.
    let (_, p) = app.get("/v1/memory/profile", KEY_A).await;
    assert_eq!(p["profile"], Value::Null);
    let (status, p) = app
        .json(
            Method::PUT,
            "/v1/memory/profile",
            Some(KEY_A),
            Some(json!({ "text": "Ana, in Zurich." })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{p}");
    assert_eq!(p["profile"]["text"], "Ana, in Zurich.");
    assert_eq!(p["profile"]["user_edited"], true);
    let (_, list) = app.get("/v1/memory", KEY_A).await;
    assert_eq!(list["profile"]["text"], "Ana, in Zurich.");

    // Another key sees none of it, and can't delete it.
    add(&app, KEY_B, "B's fact").await;
    let (_, list) = app.get("/v1/memory", KEY_B).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["profile"], Value::Null);
    let a_uri = format!("/v1/memory/{}", a["id"].as_str().unwrap());
    let (status, _) = app.json(Method::DELETE, &a_uri, Some(KEY_B), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .json(
            Method::PATCH,
            &a_uri,
            Some(KEY_B),
            Some(json!({ "pinned": true })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .json(Method::DELETE, "/v1/memory/not-an-id", Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Export, delete all, import.
    let (status, export) = app.get("/v1/memory/export", KEY_A).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(export["profile"]["text"], "Ana, in Zurich.");
    assert_eq!(export["items"].as_array().unwrap().len(), 5);
    let (status, _) = app.json(Method::DELETE, &a_uri, Some(KEY_A), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .json(Method::DELETE, "/v1/memory", Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(memories(&app, KEY_A).await.is_empty());
    assert!(archived(&app, KEY_A).await.is_empty());
    assert_eq!(memories(&app, KEY_B).await.len(), 1);
    let (status, imported) = app
        .json(Method::POST, "/v1/memory/import", Some(KEY_A), Some(export))
        .await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    // The archived row stays out.
    assert_eq!(imported["added"], 4);
    let (_, list) = app.get("/v1/memory", KEY_A).await;
    assert_eq!(list["profile"]["text"], "Ana, in Zurich.");

    let (status, _) = app.json(Method::GET, "/v1/memory", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_interactive_run_notes_the_facts_the_user_stated() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(text("Nice to meet you, Ana.")),
            // The memory call: three facts in the user's words, one the user never said, one
            // that is a secret.
            Step::Reply(text(
                "{\"remember\": [\
                 {\"text\": \"Is called Ana\", \"kind\": \"fact\", \"support\": \"I'm Ana\"}, \
                 {\"text\": \"Lives in Zurich\", \"support\": \"I live in Zurich\", \"subject\": \"user\", \"relation\": \"home city\"}, \
                 {\"text\": \"Has a cat called Mio\", \"support\": \"my cat Mio\"}, \
                 {\"text\": \"Works at Acme\", \"support\": \"I work at Acme\"}, \
                 {\"text\": \"Her password is hunter2\", \"support\": \"password: hunter2\"}, \
                 {\"text\": \"Likes the weather\"}], \
                 \"topics\": [\"weather\", \"Zurich\"]}",
            )),
            Step::Reply(text("It is 20 °C in Zurich.")),
        ],
        &[("MEMORY_EXTRACT", "on")],
    )
    .await;
    let prompt = "Hi, I'm Ana, I live in Zurich with my cat Mio (password: hunter2 for the wifi). How warm is it?";
    let first = run(
        &app,
        KEY_A,
        json!({ "prompt": prompt, "interactive": true }),
    )
    .await;
    assert_eq!(first["final_text"], "Nice to meet you, Ana.");
    assert_eq!(app.llm.call_count(), 2);
    let call = &app.llm.requests()[1];
    assert_eq!(call["system"], cedar::memory::extract::SYSTEM);
    assert!(call["tools"].as_array().unwrap().is_empty());
    let body = call["messages"][0]["content"].as_str().unwrap();
    assert!(body.starts_with("Nothing is remembered"), "{body}");
    assert!(body.contains("User: Hi, I'm Ana"), "{body}");
    assert!(body.contains("Assistant: Nice to meet you, Ana."), "{body}");
    assert!(body.contains("\"remember\""), "{body}");

    let items = memories(&app, KEY_A).await;
    let texts: Vec<&str> = items.iter().map(|m| m["text"].as_str().unwrap()).collect();
    assert_eq!(
        texts,
        ["Is called Ana", "Lives in Zurich", "Has a cat called Mio"]
    );
    assert!(items.iter().all(|m| m["run_id"] == first["id"]));
    assert_eq!(items[1]["support"], "I live in Zurich");
    assert_eq!(items[1]["relation"], "home city");
    assert_eq!(items[1]["topics"], json!(["weather", "zurich"]));

    let id = first["id"].as_str().unwrap();
    let updated = events_of(&app, KEY_A, id, "memory.updated").await;
    assert_eq!(updated.len(), 1);
    let saved = updated[0]["saved"].as_array().unwrap();
    assert_eq!(saved.len(), 3);
    assert_eq!(saved[0]["status"], "added");
    assert_eq!(saved[0]["memory"]["text"], "Is called Ana");
    assert_eq!(saved[0]["memory"]["id"], items[0]["id"]);
    let skipped = updated[0]["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 3, "{skipped:?}");
    assert!(
        skipped[0]["reason"]
            .as_str()
            .unwrap()
            .contains("not in the user's own words"),
        "{skipped:?}"
    );
    assert!(
        skipped[1]["reason"].as_str().unwrap().contains("password"),
        "{skipped:?}"
    );
    assert!(
        skipped[2]["reason"].as_str().unwrap().contains("no quote"),
        "{skipped:?}"
    );
    assert_eq!(updated[0]["episode"], Value::Null);
    let usage = events_of(&app, KEY_A, id, "usage").await;
    assert_eq!(usage.len(), 2);
    assert_eq!(usage[1]["purpose"], "memory");
    // The event comes after the answer and before the end.
    let kinds: Vec<String> = app
        .events(KEY_A, id, None)
        .await
        .iter()
        .map(|e| e.data["type"].as_str().unwrap().to_string())
        .collect();
    let at = |k: &str| kinds.iter().position(|x| x == k).unwrap();
    assert!(at("assistant.text") < at("memory.updated"));
    assert!(at("memory.updated") < at("run.completed"));

    // The trace keeps the call, in full, as it does the planning call.
    let (status, trace) = app.get(&format!("/v1/runs/{id}/trace"), KEY_A).await;
    assert_eq!(status, StatusCode::OK);
    let calls = trace["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["purpose"], "memory");

    // The next conversation knows.
    run(&app, KEY_A, json!({ "prompt": "How warm is it?" })).await;
    let sys = system(&app, 2);
    assert!(sys.contains("Lives in Zurich"), "{sys}");
}

#[tokio::test]
async fn the_memory_call_supersedes_keyed_facts_and_survives_failing() {
    let app = TestApp::with_env(vec![], &[("MEMORY_EXTRACT", "on")]).await;
    let (_, bern) = add_with(
        &app,
        KEY_A,
        json!({ "text": "Lives in Bern", "subject": "user", "relation": "home city" }),
    )
    .await;
    let app = TestApp::build(
        app.state.store.clone(),
        ScriptedLlm::new(vec![
            Step::Reply(text("Congratulations on the move!")),
            // No supersedes given: the key does it.
            Step::Reply(text(
                "{\"remember\": [{\"text\": \"Lives in Zurich\", \"support\": \"I moved to Zurich\", \"subject\": \"user\", \"relation\": \"home city\"}, \
                 {\"text\": \"lives in zurich\", \"support\": \"moved to Zurich\"}]}",
            )),
            Step::Reply(text("ok")),
            Step::Error("model down"),
            Step::Reply(text("ok")),
            Step::Reply(text("Nothing worth noting.")),
        ]),
        FakeFactory::new(),
        &[("MEMORY_EXTRACT", "on")],
    )
    .await;
    let done = run(
        &app,
        KEY_A,
        json!({ "prompt": "I moved to Zurich!", "interactive": true }),
    )
    .await;
    let body = app.llm.requests()[1]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(body.contains("[m1] (fact) Lives in Bern"), "{body}");
    let items = memories(&app, KEY_A).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Lives in Zurich");
    assert_eq!(items[0]["run_id"], done["id"]);
    let gone = archived(&app, KEY_A).await;
    assert_eq!(gone[0]["id"], bern["id"]);
    assert_eq!(gone[0]["superseded_by"], items[0]["id"]);
    let updated = events_of(&app, KEY_A, done["id"].as_str().unwrap(), "memory.updated").await;
    assert_eq!(updated[0]["saved"].as_array().unwrap().len(), 1);
    assert_eq!(updated[0]["saved"][0]["status"], "superseded");

    // A failed memory call doesn't fail the run, and leaves memory as it was.
    let done = run(
        &app,
        KEY_A,
        json!({ "prompt": "and the weather?", "interactive": true }),
    )
    .await;
    assert_eq!(done["status"], "completed");
    assert!(
        events_of(&app, KEY_A, done["id"].as_str().unwrap(), "memory.updated")
            .await
            .is_empty()
    );
    assert_eq!(memories(&app, KEY_A).await.len(), 1);

    // Nothing usable in the reply: nothing saved, no event.
    let done = run(
        &app,
        KEY_A,
        json!({ "prompt": "thanks", "interactive": true }),
    )
    .await;
    assert!(
        events_of(&app, KEY_A, done["id"].as_str().unwrap(), "memory.updated")
            .await
            .is_empty()
    );
    assert_eq!(app.llm.call_count(), 6);
}

#[tokio::test]
async fn the_memory_call_is_skipped_when_there_is_nothing_to_note() {
    // Unattended runs can't change what is known about the user; neither can a run without the
    // memory group; and MEMORY_EXTRACT=off keeps the old behaviour. None of them makes the
    // extra call, and nor does a run without tools when only experience is on.
    let app = TestApp::with_env(vec![Step::Repeat(text("ok"))], &[("MEMORY_EXTRACT", "on")]).await;
    run(&app, KEY_A, json!({ "prompt": "I'm Ana" })).await;
    assert_eq!(app.llm.call_count(), 1);
    run(
        &app,
        KEY_A,
        json!({ "prompt": "I'm Ana", "interactive": true, "tools": ["web_fetch"] }),
    )
    .await;
    assert_eq!(app.llm.call_count(), 2);
    run(
        &app,
        KEY_A,
        json!({ "prompt": "I'm Ana", "interactive": true }),
    )
    .await;
    assert_eq!(app.llm.call_count(), 4);
    assert!(memories(&app, KEY_A).await.is_empty());

    let app = TestApp::new(vec![Step::Repeat(text("ok"))]).await;
    run(
        &app,
        KEY_A,
        json!({ "prompt": "I'm Ana", "interactive": true }),
    )
    .await;
    assert_eq!(app.llm.call_count(), 1);

    let app = TestApp::with_env(
        vec![Step::Repeat(text("ok"))],
        &[("MEMORY_EXPERIENCE", "on")],
    )
    .await;
    let done = run(&app, KEY_A, json!({ "prompt": "hello" })).await;
    assert_eq!(app.llm.call_count(), 1);
    // Still an episode, written without a call.
    let episodes = events_of(&app, KEY_A, done["id"].as_str().unwrap(), "memory.updated").await;
    assert_eq!(episodes[0]["episode"]["kind"], "episode");
    assert_eq!(episodes[0]["episode"]["text"], "Asked: hello");
    assert_eq!(episodes[0]["episode"]["meta"]["outcome"], "completed");
    assert!(episodes[0]["saved"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_run_with_tools_notes_an_episode_and_a_procedure_and_a_cue_recalls_them() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("c1", "bash", json!({ "command": "python3 primes.py" }))),
            Step::Reply(text("Here are the first ten primes.")),
            // The memory call of an unattended run: no facts asked for, an episode and a procedure.
            Step::Reply(text(
                "{\"remember\": [{\"text\": \"Likes primes\", \"support\": \"primes\"}], \
                 \"episode\": {\"title\": \"Primes script\", \"summary\": \"Ran primes.py and listed the first ten primes.\", \"pitfall\": null}, \
                 \"procedure\": {\"title\": \"Prime lists\", \"summary\": \"Run the existing script rather than writing a new one.\", \"rationale\": \"It is already in the workspace.\", \"pitfall\": \"Don't rewrite it.\"}, \
                 \"topics\": [\"primes\", \"python\"]}",
            )),
            Step::Reply(text("Done again.")),
        ],
        &[("MEMORY_EXPERIENCE", "on")],
    )
    .await;
    let first = run(
        &app,
        KEY_A,
        json!({ "prompt": "Run primes.py and show the output" }),
    )
    .await;
    assert_eq!(app.llm.call_count(), 3);
    let body = app.llm.requests()[2]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!body.contains("\"remember\""), "{body}");
    assert!(body.contains("(called bash"), "{body}");
    assert!(body.contains("The run ended: completed"), "{body}");

    let updated = events_of(&app, KEY_A, first["id"].as_str().unwrap(), "memory.updated").await;
    assert_eq!(updated.len(), 1);
    assert_eq!(
        updated[0]["episode"]["text"],
        "Primes script: Ran primes.py and listed the first ten primes."
    );
    assert_eq!(updated[0]["episode"]["meta"]["tools"], json!(["bash"]));
    assert_eq!(updated[0]["saved"].as_array().unwrap().len(), 1);
    assert_eq!(updated[0]["saved"][0]["memory"]["kind"], "procedure");
    let (_, list) = app.get("/v1/memory?kind=procedure", KEY_A).await;
    let procedure = list["items"][0].clone();
    assert_eq!(
        procedure["text"],
        "Prime lists: Run the existing script rather than writing a new one."
    );
    assert_eq!(procedure["meta"]["pitfall"], "Don't rewrite it.");
    assert_eq!(procedure["source"], "agent");
    // No fact about the user from an unattended run.
    let (_, list) = app.get("/v1/memory?kind=fact", KEY_A).await;
    assert!(list["items"].as_array().unwrap().is_empty());

    // Nothing of it is shown for an unrelated request; a cue brings both back.
    let app = TestApp::build(
        app.state.store.clone(),
        ScriptedLlm::new(vec![
            Step::Reply(text("A poem.")),
            Step::Reply(text("Done again.")),
        ]),
        FakeFactory::new(),
        &[],
    )
    .await;
    let poem = run(&app, KEY_A, json!({ "prompt": "Write a poem about rain" })).await;
    assert!(
        events_of(&app, KEY_A, poem["id"].as_str().unwrap(), "memory.recalled")
            .await
            .is_empty()
    );
    assert!(
        system(&app, 0).contains("You don't remember anything"),
        "{}",
        system(&app, 0)
    );
    let again = run(
        &app,
        KEY_A,
        json!({ "prompt": "Can you run the primes script again like last time?" }),
    )
    .await;
    let sys = system(&app, 1);
    assert!(
        sys.contains("Earlier conversations that may be relevant:\n[m1] "),
        "{sys}"
    );
    assert!(sys.contains("Primes script: Ran primes.py"), "{sys}");
    assert!(
        sys.contains("How similar tasks went well for them:\n[m2] Prime lists"),
        "{sys}"
    );
    assert!(sys.contains("Pitfall: Don't rewrite it."), "{sys}");
    let recalled = events_of(
        &app,
        KEY_A,
        again["id"].as_str().unwrap(),
        "memory.recalled",
    )
    .await;
    assert_eq!(recalled[0]["cues"], json!(["recall"]));
    assert!(recalled[0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["why"] == "cue"));

    // A thumbs down on the run that followed the procedure counts against it; a second one
    // archives it.
    let uri = format!("/v1/runs/{}/feedback", again["id"].as_str().unwrap());
    let (status, body) = app
        .json(
            Method::POST,
            &uri,
            Some(KEY_A),
            Some(json!({ "rating": "down" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rating"], "down");
    assert_eq!(body["archived"].as_array().unwrap().len(), 1);
    assert_eq!(body["archived"][0]["id"], procedure["id"]);
    let (_, run_after) = app
        .get(
            &format!("/v1/runs/{}", again["id"].as_str().unwrap()),
            KEY_A,
        )
        .await;
    assert_eq!(run_after["rating"], "down");
    let (_, list) = app.get("/v1/memory?kind=procedure", KEY_A).await;
    assert!(list["items"].as_array().unwrap().is_empty());
    let (_, list) = app
        .get("/v1/memory?kind=procedure&archived=true", KEY_A)
        .await;
    assert_eq!(list["items"][0]["meta"]["failures"], 1);
    let (status, _) = app
        .json(
            Method::POST,
            "/v1/runs/not-a-run/feedback",
            Some(KEY_A),
            Some(json!({ "rating": "up" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_large_store_is_selected_for_the_request_not_shown_whole() {
    let app = TestApp::new(vec![Step::Repeat(text("ok"))]).await;
    let words = [
        "apples",
        "bicycles",
        "chess",
        "dahlias",
        "espresso",
        "fencing",
        "guitars",
        "hiking",
        "ink",
        "jigsaws",
        "kayaks",
        "lemons",
        "mosaics",
        "noodles",
        "origami",
        "pottery",
        "quilts",
        "roses",
        "sailing",
        "tulips",
        "ukuleles",
        "violins",
        "walnuts",
        "xylophones",
        "yoga",
        "zithers",
        "almonds",
        "basil",
        "cedar",
        "dates",
    ];
    for (i, w) in words.iter().enumerate() {
        add(
            &app,
            KEY_A,
            &format!("Collects {w} on unrelated weekends, item {i}"),
        )
        .await;
    }
    add_with(
        &app,
        KEY_A,
        json!({ "text": "Always answer in German", "kind": "instruction" }),
    )
    .await;
    add(&app, KEY_A, "Has a cat called Mio").await;
    let done = run(
        &app,
        KEY_A,
        json!({ "prompt": "How old might my cat Mio be?" }),
    )
    .await;
    let sys = system(&app, 0);
    assert!(
        sys.contains("About them:\n[m31] (standing instruction) Always answer in German"),
        "{sys}"
    );
    assert!(
        sys.contains("Relevant to this request:\n[m32] Has a cat called Mio"),
        "{sys}"
    );
    assert!(!sys.contains("unrelated weekends"), "{sys}");
    let recalled = events_of(&app, KEY_A, done["id"].as_str().unwrap(), "memory.recalled").await;
    let whys: Vec<&str> = recalled[0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["why"].as_str().unwrap())
        .collect();
    assert_eq!(whys, ["pinned", "match"]);
    let done = run(&app, KEY_A, json!({ "prompt": "Write a limerick" })).await;
    let recalled = events_of(&app, KEY_A, done["id"].as_str().unwrap(), "memory.recalled").await;
    assert_eq!(recalled[0]["items"].as_array().unwrap().len(), 1);
    assert!(!system(&app, 1).contains("Mio"));
}

#[tokio::test]
async fn consolidation_runs_after_enough_runs_and_writes_the_profile() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(text("ok")),
            // The consolidation call.
            Step::Reply(text(
                "{\"profile\": \"Ana lives in Zurich with her cat Mio and prefers tea.\", \
                 \"merge\": [{\"keep\": \"m2\", \"drop\": [\"m1\"]}], \"supersede\": []}",
            )),
            Step::Reply(text("ok")),
        ],
        &[
            ("MEMORY_CONSOLIDATE", "on"),
            ("MEMORY_CONSOLIDATE_AFTER", "1"),
        ],
    )
    .await;
    add(&app, KEY_A, "Likes tea").await;
    add(&app, KEY_A, "Drinks tea every morning").await;
    add(&app, KEY_A, "Has a cat called Mio").await;
    run(&app, KEY_A, json!({ "prompt": "hi" })).await;
    let mut profile = Value::Null;
    for _ in 0..300 {
        let (_, p) = app.get("/v1/memory/profile", KEY_A).await;
        if p["profile"].is_object() {
            profile = p["profile"].clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        profile["text"],
        "Ana lives in Zurich with her cat Mio and prefers tea."
    );
    assert_eq!(profile["user_edited"], false);
    assert_eq!(app.llm.call_count(), 2);
    let body = app.llm.requests()[1]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(body.contains("[m1] (fact, "), "{body}");
    let items = memories(&app, KEY_A).await;
    let texts: Vec<&str> = items.iter().map(|m| m["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["Drinks tea every morning", "Has a cat called Mio"]);
    assert_eq!(items[0]["mentions"], 2);
    let gone = archived(&app, KEY_A).await;
    assert_eq!(gone[0]["text"], "Likes tea");
    assert_eq!(gone[0]["superseded_by"], items[0]["id"]);

    // The next run is shown the profile.
    let done = run(&app, KEY_A, json!({ "prompt": "hello again" })).await;
    let sys = system(&app, 2);
    assert!(sys.contains("Profile: Ana lives in Zurich"), "{sys}");
    let recalled = events_of(&app, KEY_A, done["id"].as_str().unwrap(), "memory.recalled").await;
    assert_eq!(recalled[0]["profile"], true);
}
