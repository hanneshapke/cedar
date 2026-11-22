//! Saved actions through the API: a recording becomes a draft, the draft an action, and a later
//! run reads the action's steps and has the browser type the password kept with it.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};

const SECRET_KEY: &str = "0101010101010101010101010101010101010101010101010101010101010101";
const WITH_KEY: &[(&str, &str)] = &[("CEDAR_SECRET_KEY", SECRET_KEY), ("BROWSER", "on")];

fn recording() -> Value {
    json!({
        "goal": "Downloading my invoice",
        "duration_ms": 42_000,
        "locale": "en",
        "frames": [
            { "at_ms": 0, "media_type": "image/jpeg", "data": "aGk=" },
            { "at_ms": 9_000, "media_type": "image/jpeg", "data": "aGk=" },
        ],
        "video": { "media_type": "video/webm", "data": "dmlk" },
    })
}

const DRAFT: &str = r#"{"title": "Download the invoice", "summary": "When the user wants last month's invoice",
    "site": "https://example.com/login", "steps": ["Go to https://example.com/login",
    "Type the user name alice into User name", "Enter the saved secret \"password\" into Password", "Click Invoices"],
    "secrets": [{"name": "password", "label": "The portal's password"}], "notes": ""}"#;

fn action_body(password: Option<&str>) -> Value {
    json!({
        "title": "Download the invoice",
        "summary": "When the user wants last month's invoice",
        "site": "example.com",
        "steps": ["Go to https://example.com/login", "Enter the saved secret \"password\" into Password"],
        "secrets": [{ "name": "password", "label": "The portal's password", "value": password }],
        "source": { "kind": "recording", "seconds": 42 },
    })
}

/// The tool result at the end of the messages sent in the model's `request`-th call.
fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"].as_array().unwrap().last().unwrap()["content"][0].clone()
}

#[tokio::test]
async fn a_recording_becomes_a_draft_from_its_frames_and_saves_nothing() {
    let app = TestApp::new(vec![Step::Reply(text(DRAFT))]).await;
    let (status, body) = app.json(Method::POST, "/v1/actions/draft", Some(KEY_A), Some(recording())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["watched"], "frames", "the test model takes images, not video");
    assert_eq!(body["model_id"], TEST_MODEL);
    assert_eq!(body["draft"]["title"], "Download the invoice");
    assert_eq!(body["draft"]["site"], "example.com");
    assert_eq!(body["draft"]["steps"].as_array().unwrap().len(), 4);
    assert_eq!(body["draft"]["secrets"], json!([{ "name": "password", "label": "The portal's password", "saved": false }]));

    // The model got the frames with their times and the request, and no tools to call.
    let sent = &app.llm.requests()[0];
    assert!(sent["tools"].as_array().is_none_or(|t| t.is_empty()));
    let blocks = sent["messages"][0]["content"].as_array().unwrap();
    assert_eq!(blocks[0]["text"], "Frame 1 at 0:00:");
    assert_eq!(blocks[1]["type"], "image");
    assert_eq!(blocks[2]["text"], "Frame 2 at 0:09:");
    let ask = blocks[4]["text"].as_str().unwrap();
    assert!(ask.contains("\"Downloading my invoice\"") && ask.contains("NEVER write out a password"), "{ask}");
    assert!(ask.contains("- fetch_url:"), "the agent's tools are named: {ask}");

    let (_, list) = app.get("/v1/actions", KEY_A).await;
    assert_eq!(list["actions"], json!([]));
    assert_eq!(list["recording"]["frames"], true);
    assert_eq!(list["recording"]["video"], false);
    assert_eq!(list["secrets"], false, "no CEDAR_SECRET_KEY in this app");
}

#[tokio::test]
async fn a_model_that_takes_video_gets_the_video() {
    let app = TestApp::new(vec![Step::Reply(text(DRAFT))]).await;
    let mut entry = app.state.store.get_model(TEST_MODEL).await.unwrap().unwrap();
    entry.video = true;
    app.state.store.upsert_model(&entry).await.unwrap();
    app.state.integrations.reload().await.unwrap();

    let (status, body) = app.json(Method::POST, "/v1/actions/draft", Some(KEY_A), Some(recording())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["watched"], "video");
    let blocks = app.llm.requests()[0]["messages"][0]["content"].as_array().unwrap().clone();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["type"], "video");
    assert_eq!(blocks[0]["source"]["media_type"], "video/webm");
    let (_, list) = app.get("/v1/actions", KEY_A).await;
    assert_eq!(list["recording"]["video"], true);
    let (_, models) = app.get("/v1/models", KEY_A).await;
    assert_eq!(models["models"][0]["video"], true);
}

#[tokio::test]
async fn a_recording_nothing_can_watch_or_read_is_refused() {
    let app = TestApp::new(vec![Step::Reply(text("I see nothing."))]).await;
    let (status, body) = app.json(Method::POST, "/v1/actions/draft", Some(KEY_A), Some(json!({ "goal": "x" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("send the recording"));

    // Only a video, and no model that takes one.
    let mut only_video = recording();
    only_video["frames"] = json!([]);
    let (status, body) = app.json(Method::POST, "/v1/actions/draft", Some(KEY_A), Some(only_video)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"].as_str().unwrap().contains("send the recording's frames too"), "{body}");

    // A reply that is no draft is the model's failure, not the caller's.
    let (status, body) = app.json(Method::POST, "/v1/actions/draft", Some(KEY_A), Some(recording())).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body["error"]["message"].as_str().unwrap().contains("could not turn the recording into steps"));
    assert_eq!(app.llm.call_count(), 1);
}

#[tokio::test]
async fn actions_belong_to_their_owner_and_never_show_a_password() {
    let app = TestApp::with_env(vec![], WITH_KEY).await;
    let (status, saved) = app.json(Method::POST, "/v1/actions", Some(KEY_A), Some(action_body(Some("hunter22")))).await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["name"], "download_the_invoice");
    assert_eq!(saved["secrets"], json!([{ "name": "password", "label": "The portal's password", "saved": true }]));
    assert_eq!(saved["source"], json!({ "kind": "recording", "seconds": 42 }));
    assert!(!saved.to_string().contains("hunter22"));
    let id = saved["id"].as_str().unwrap();

    let (_, list) = app.get("/v1/actions", KEY_A).await;
    assert_eq!(list["actions"].as_array().unwrap().len(), 1);
    assert_eq!(list["secrets"], true);
    assert!(!list.to_string().contains("hunter22"));
    let (_, other) = app.get("/v1/actions", KEY_B).await;
    assert_eq!(other["actions"], json!([]));
    let (status, _) = app.get(&format!("/v1/actions/{id}"), KEY_B).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app.json(Method::DELETE, &format!("/v1/actions/{id}"), Some(KEY_B), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // An edit keeps the saved password unless it sends another or drops the secret.
    let mut edit = action_body(None);
    edit["title"] = json!("Fetch the invoice");
    let (status, edited) = app.json(Method::PUT, &format!("/v1/actions/{id}"), Some(KEY_A), Some(edit)).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!((edited["name"].as_str(), edited["title"].as_str()), (Some("download_the_invoice"), Some("Fetch the invoice")));
    assert_eq!(edited["secrets"][0]["saved"], true);

    let (status, body) = app.json(Method::POST, "/v1/actions", Some(KEY_A), Some(json!({ "title": "x", "steps": [] }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("at least one step"));

    let (status, _) = app.json(Method::DELETE, &format!("/v1/actions/{id}"), Some(KEY_A), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM secrets WHERE name LIKE 'action_secret:%'")
        .fetch_one(app.state.store.pool())
        .await
        .unwrap();
    assert_eq!(left, 0, "the password goes with the action");
}

#[tokio::test]
async fn a_password_is_not_stored_without_a_key_to_encrypt_it() {
    let app = TestApp::new(vec![]).await;
    let (status, body) = app.json(Method::POST, "/v1/actions", Some(KEY_A), Some(action_body(Some("hunter22")))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"].as_str().unwrap().contains("CEDAR_SECRET_KEY"));
    let (status, saved) = app.json(Method::POST, "/v1/actions", Some(KEY_A), Some(action_body(None))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(saved["secrets"][0]["saved"], false);
}

#[tokio::test]
async fn a_run_is_told_of_its_owners_actions_reads_one_and_types_its_password_unseen() {
    let script = vec![
        Step::Reply(tool_use("t0", "saved_actions", json!({ "name": "download_the_invoice" }))),
        Step::Reply(tool_use("t1", "browser", json!({ "action": "type", "ref": 1, "secret": "download_the_invoice/password", "submit": true }))),
        Step::Reply(text("done")),
    ];
    let app = TestApp::with_env(script, WITH_KEY).await;
    let (status, _) = app.json(Method::POST, "/v1/actions", Some(KEY_A), Some(action_body(Some("hunter22")))).await;
    assert_eq!(status, StatusCode::CREATED);

    let created = app.create_run(KEY_A, "get my invoice").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;

    // The prompt names the action; the tool hands over its steps and the name of the secret.
    let requests = app.llm.requests();
    let system = requests[0]["system"].as_str().unwrap();
    assert!(system.contains("- download_the_invoice: Download the invoice (When the user wants last month's invoice), on example.com"), "{system}");
    let steps: Value = serde_json::from_str(tool_result(&app, 1)["content"].as_str().unwrap()).unwrap();
    assert_eq!(steps["steps"][0], "Go to https://example.com/login");
    assert_eq!(steps["secrets"], json!([{ "secret": "download_the_invoice/password", "label": "The portal's password", "saved": true }]));

    // The browser was asked where it is, then got the password; the fake page echoes the
    // request, and the model still never reads the value.
    let sent = app.browsers.browser(0).requests.lock().unwrap().clone();
    assert_eq!(sent, [
        json!({ "action": "snapshot" }),
        json!({ "action": "type", "ref": 1, "submit": true, "text": "hunter22" }),
    ]);
    let typed = tool_result(&app, 2);
    assert!(typed.get("is_error").is_none(), "{typed}");
    assert!(typed["content"].as_str().unwrap().contains("\"text\":\"[saved secret]\""), "{typed}");
    for request in &requests {
        assert!(!request.to_string().contains("hunter22"), "the model was sent the password");
    }
    let (_, messages) = app.get(&format!("/v1/runs/{id}/messages"), KEY_A).await;
    assert!(!messages.to_string().contains("hunter22"), "the conversation kept the password");
    let (_, trace) = app.get(&format!("/v1/runs/{id}/trace"), KEY_A).await;
    assert!(!trace.to_string().contains("hunter22"), "the trace kept the password");

    let (_, list) = app.get("/v1/actions", KEY_A).await;
    assert_eq!(list["actions"][0]["uses"], 1);

    // Someone else's run knows nothing of it.
    let other = TestApp::build(
        app.state.store.clone(),
        ScriptedLlm::new(vec![
            Step::Reply(tool_use("t0", "browser", json!({ "action": "type", "ref": 1, "secret": "download_the_invoice/password" }))),
            Step::Reply(text("done")),
        ]),
        FakeFactory::new(),
        WITH_KEY,
    )
    .await;
    let created = other.create_run(KEY_B, "get my invoice").await;
    other.wait_status(KEY_B, created["id"].as_str().unwrap(), "completed").await;
    assert!(!other.llm.requests()[0]["system"].as_str().unwrap().contains("download_the_invoice"));
    let refused = tool_result(&other, 1);
    assert_eq!(refused["is_error"], true);
    assert!(refused["content"].as_str().unwrap().contains("no saved action"), "{refused}");
}

#[tokio::test]
async fn a_password_is_not_typed_on_another_site() {
    let script = vec![
        Step::Reply(tool_use("t0", "browser", json!({ "action": "type", "ref": 1, "secret": "download_the_invoice" }))),
        Step::Reply(tool_use("t1", "browser", json!({ "action": "click", "ref": 1, "secret": "download_the_invoice" }))),
        Step::Reply(tool_use("t2", "browser", json!({ "action": "type", "ref": 1, "text": "x", "secret": "download_the_invoice" }))),
        Step::Reply(text("done")),
    ];
    let app = TestApp::with_env(script, WITH_KEY).await;
    // Saved for another site than the one the (fake) browser is on.
    let mut body = action_body(Some("hunter22"));
    body["site"] = json!("portal.example.org");
    let (status, _) = app.json(Method::POST, "/v1/actions", Some(KEY_A), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED);

    let created = app.create_run(KEY_A, "get my invoice").await;
    app.wait_status(KEY_A, created["id"].as_str().unwrap(), "completed").await;

    let refused = tool_result(&app, 1);
    assert_eq!(refused["is_error"], true);
    assert!(refused["content"].as_str().unwrap().contains("only typed on portal.example.org"), "{refused}");
    assert!(tool_result(&app, 2)["content"].as_str().unwrap().contains("only goes with action \"type\""));
    assert!(tool_result(&app, 3)["content"].as_str().unwrap().contains("either text or secret"));
    // Only the look at where the browser is reached it.
    assert_eq!(*app.browsers.browser(0).requests.lock().unwrap(), [json!({ "action": "snapshot" })]);
}

#[tokio::test]
async fn with_actions_off_runs_get_no_tool_but_the_api_still_lists_and_deletes() {
    let app = TestApp::with_env(vec![Step::Reply(text("done"))], &[("ACTIONS", "off")]).await;
    let (status, saved) = app.json(Method::POST, "/v1/actions", Some(KEY_A), Some(action_body(None))).await;
    assert_eq!(status, StatusCode::CREATED);
    let created = app.create_run(KEY_A, "hello").await;
    app.wait_status(KEY_A, created["id"].as_str().unwrap(), "completed").await;
    let request = &app.llm.requests()[0];
    assert!(!request["system"].as_str().unwrap().contains("Saved actions"));
    assert!(request["tools"].as_array().unwrap().iter().all(|t| t["name"] != "saved_actions"));
    let (_, list) = app.get("/v1/actions", KEY_A).await;
    assert_eq!(list["enabled"], false);
    let (status, _) = app.json(Method::DELETE, &format!("/v1/actions/{}", saved["id"].as_str().unwrap()), Some(KEY_A), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
