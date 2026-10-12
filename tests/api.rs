mod common;

use axum::http::{Method, StatusCode};
use cedar::models::NewRun;
use cedar::store::Store;
use common::*;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

fn id_of(run: &Value) -> String {
    run["id"].as_str().unwrap().to_string()
}

// -- auth & health ----------------------------------------------------------------------------

#[tokio::test]
async fn requests_without_a_valid_key_are_rejected() {
    let app = TestApp::new(vec![]).await;

    let (status, headers, body) = app.call(Method::GET, "/v1/runs", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers["www-authenticate"], "Bearer");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["error"]["code"], "unauthorized");

    for key in ["wrong", "", "key-"] {
        let (status, _) = app.get("/v1/runs", key).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "key {key:?}");
    }
    // Other auth schemes and a missing token don't count either.
    let req = |value: &'static str| {
        let app = app.clone();
        async move {
            use axum::body::Body;
            use tower::ServiceExt;
            let req = axum::http::Request::builder().uri("/v1/runs").header("authorization", value).body(Body::empty()).unwrap();
            app.router.oneshot(req).await.unwrap().status()
        }
    };
    assert_eq!(req("Basic a2V5LWE6").await, StatusCode::UNAUTHORIZED);
    assert_eq!(req("Bearer").await, StatusCode::UNAUTHORIZED);

    assert_eq!(app.get("/v1/runs", KEY_A).await.0, StatusCode::OK);
    assert_eq!(app.get("/v1/runs", KEY_B).await.0, StatusCode::OK);
}

#[tokio::test]
async fn health_endpoints_are_public_and_readyz_reflects_dependencies() {
    let app = TestApp::new(vec![]).await;
    let (status, _, body) = app.call(Method::GET, "/healthz", None, None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "ok"));

    let (status, body) = app.json(Method::GET, "/readyz", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["checks"], json!({ "database": "ok", "sandbox": "ok" }));

    app.factory.healthy.store(false, Ordering::SeqCst);
    let (status, body) = app.json(Method::GET, "/readyz", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["checks"]["sandbox"], "failed");
    // The detail stays in the log; the public endpoint doesn't echo it.
    assert!(!body.to_string().contains("docker is down"));
}

#[tokio::test]
async fn unknown_routes_get_a_json_404() {
    let app = TestApp::new(vec![]).await;
    let (status, body) = app.json(Method::GET, "/nope", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

// -- the happy path ---------------------------------------------------------------------------

#[tokio::test]
async fn a_run_executes_tools_and_completes() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "bash", json!({ "command": "echo hi" }))),
        Step::Reply(text("all done")),
    ])
    .await;

    let created = app.create_run(KEY_A, "say hi").await;
    assert_eq!(created["status"], "queued");
    assert!(created.get("owner").is_none(), "owner must not leak: {created}");
    let id = id_of(&created);

    let run = app.wait_status(KEY_A, &id, "completed").await;
    assert_eq!(run["final_text"], "all done");
    assert_eq!(run["stop_reason"], "end_turn");
    assert_eq!(run["turns"], 2);
    assert_eq!(run["usage"], json!({ "input_tokens": 20, "output_tokens": 10 }));
    assert!(run["started_at"].is_string() && run["finished_at"].is_string());
    assert_eq!(run["error"], Value::Null);

    // The sandbox ran the command and was torn down afterwards.
    assert_eq!(*app.factory.sandbox(0).commands.lock().unwrap(), ["echo hi"]);
    assert!(app.factory.all_destroyed());

    // The tool result went back to the model, tied to the tool_use id.
    let requests = app.llm.requests.lock().unwrap();
    let last = requests[1]["messages"].as_array().unwrap().last().unwrap().clone();
    drop(requests);
    assert_eq!(last["role"], "user");
    assert_eq!(last["content"][0]["type"], "tool_result");
    assert_eq!(last["content"][0]["tool_use_id"], "t1");
    assert_eq!(last["content"][0]["content"], "ran: echo hi");
    assert!(last["content"][0].get("is_error").is_none());

    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(
        kinds(&events),
        ["run.started", "usage", "tool.call", "tool.result", "usage", "assistant.text", "run.completed"]
    );
    assert_eq!(events.iter().map(|e| e.id).collect::<Vec<_>>(), (1..=7).collect::<Vec<_>>());
    assert_eq!(events[2].data["data"], json!({ "id": "t1", "name": "bash", "input": { "command": "echo hi" } }));
    assert_eq!(events[3].data["data"]["output"], "ran: echo hi");
    assert_eq!(events[6].data["data"]["final_text"], "all done");

    let (_, messages) = app.get(&format!("/v1/runs/{id}/messages"), KEY_A).await;
    let roles: Vec<_> = messages["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
    assert_eq!(messages["messages"][0]["content"], "say hi");
}

#[tokio::test]
async fn server_tool_blocks_round_trip_unmodified() {
    let search = reply(
        vec![
            json!({ "type": "server_tool_use", "id": "s1", "name": "web_search", "input": { "query": "rust" } }),
            json!({ "type": "web_search_tool_result", "tool_use_id": "s1",
                    "content": [{ "type": "web_search_result", "encrypted_content": "EnCrYpTeD==" }] }),
        ],
        "pause_turn",
    );
    let app = TestApp::new(vec![Step::Reply(search), Step::Reply(text("found it"))]).await;
    let id = id_of(&app.create_run(KEY_A, "search").await);
    app.wait_status(KEY_A, &id, "completed").await;

    let requests = app.llm.requests.lock().unwrap();
    let names: Vec<_> = requests[0]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["bash", "read_file", "write_file", "web_search"]);
    // After pause_turn the assistant turn is resent exactly as received.
    let resent = &requests[1]["messages"][1];
    assert_eq!(resent["role"], "assistant");
    assert_eq!(resent["content"][1]["content"][0]["encrypted_content"], "EnCrYpTeD==");
    drop(requests);

    let events = app.events(KEY_A, &id, None).await;
    assert!(kinds(&events).contains(&"web_search"));
}

#[tokio::test]
async fn tool_errors_and_odd_output_reach_the_model() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "nope", json!({}))),
        Step::Reply(tool_use("t2", "bash", json!({ "command": "fail" }))),
        Step::Reply(tool_use("t3", "bash", json!({ "command": "big" }))),
        Step::Reply(tool_use("t4", "read_file", json!({ "path": "missing.txt" }))),
        Step::Reply(tool_use("t5", "write_file", json!({ "path": "a/b.txt", "content": "hello" }))),
        Step::Reply(tool_use("t6", "read_file", json!({ "path": "a/b.txt" }))),
        Step::Reply(tool_use("t7", "bash", json!({}))),
        Step::Reply(text("done")),
    ])
    .await;
    let id = id_of(&app.create_run_with(KEY_A, json!({ "prompt": "go", "max_turns": 10 })).await);
    app.wait_status(KEY_A, &id, "completed").await;

    let requests = app.llm.requests.lock().unwrap();
    let result = |request: usize| {
        let msgs = requests[request]["messages"].as_array().unwrap();
        msgs.last().unwrap()["content"][0].clone()
    };

    let unknown = result(1);
    assert_eq!(unknown["is_error"], true);
    assert_eq!(unknown["content"], "unknown tool: nope");

    let failed = result(2);
    assert_eq!(failed["content"], "partial\n[stderr]\nboom\n[exit status: 1]");
    assert!(failed.get("is_error").is_none(), "a non-zero exit is output, not a tool error");

    let big = result(3)["content"].as_str().unwrap().to_string();
    assert!(big.len() < 33_000, "output must be capped, got {} bytes", big.len());
    assert!(big.ends_with("\n[truncated 67232 bytes]"), "{}", &big[big.len() - 40..]);

    let missing = result(4);
    assert_eq!(missing["is_error"], true);
    assert!(missing["content"].as_str().unwrap().contains("No such file"));

    assert_eq!(result(5)["content"], "wrote 5 bytes to a/b.txt");
    assert_eq!(result(6)["content"], "hello");
    let bad_input = result(7);
    assert_eq!(bad_input["is_error"], true);
    assert_eq!(bad_input["content"], "missing command");
    drop(requests);

    let events = app.events(KEY_A, &id, None).await;
    let failed_results = events.iter().filter(|e| e.event == "tool.result" && e.data["data"]["is_error"] == true).count();
    assert_eq!(failed_results, 3);
}

// -- failure modes ----------------------------------------------------------------------------

#[tokio::test]
async fn exhausting_max_turns_fails_the_run() {
    let looping = tool_use("t", "bash", json!({ "command": "echo again" }));
    let app = TestApp::new(vec![Step::Repeat(looping)]).await;
    let id = id_of(&app.create_run_with(KEY_A, json!({ "prompt": "loop", "max_turns": 2 })).await);

    let run = app.wait_status(KEY_A, &id, "failed").await;
    assert_eq!(run["error"], "run did not finish within 2 turns");
    assert_eq!(run["turns"], 2);
    assert_eq!(app.llm.call_count(), 2);
    assert!(app.factory.all_destroyed());

    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(events.last().unwrap().event, "run.failed");
}

#[tokio::test]
async fn llm_errors_fail_the_run_with_the_message() {
    let app = TestApp::new(vec![Step::Error("Anthropic API error 400: bad model")]).await;
    let id = id_of(&app.create_run(KEY_A, "x").await);
    let run = app.wait_status(KEY_A, &id, "failed").await;
    assert_eq!(run["error"], "Anthropic API error 400: bad model");
    assert!(app.factory.all_destroyed());
}

#[tokio::test]
async fn a_broken_sandbox_fails_the_run_instead_of_confusing_the_model() {
    let app = TestApp::new(vec![Step::Reply(tool_use("t", "bash", json!({ "command": "fatal" })))]).await;
    let id = id_of(&app.create_run(KEY_A, "x").await);
    let run = app.wait_status(KEY_A, &id, "failed").await;
    assert_eq!(run["error"], "sandbox failure: wedged");
    assert_eq!(app.llm.call_count(), 1, "the model must not be asked to recover from a dead sandbox");
}

#[tokio::test]
async fn a_panicking_run_is_still_finalized_and_cleaned_up() {
    let app = TestApp::new(vec![Step::Panic]).await;
    let id = id_of(&app.create_run(KEY_A, "x").await);
    let run = app.wait_status(KEY_A, &id, "failed").await;
    assert_eq!(run["error"], "internal error");
    assert!(app.factory.all_destroyed());
    // Streams close rather than hang.
    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(events.last().unwrap().event, "run.failed");
}

#[tokio::test]
async fn runs_that_exceed_the_time_limit_fail() {
    let app = TestApp::with_env(vec![Step::Block], &[("RUN_TIMEOUT_SECS", "1")]).await;
    let id = id_of(&app.create_run(KEY_A, "x").await);
    let run = app.wait_status(KEY_A, &id, "failed").await;
    assert_eq!(run["error"], "run exceeded the 1s time limit");
    assert!(app.factory.all_destroyed());
}

// -- cancellation -----------------------------------------------------------------------------

#[tokio::test]
async fn a_running_run_can_be_cancelled() {
    let app = TestApp::new(vec![Step::Block]).await;
    let id = id_of(&app.create_run(KEY_A, "x").await);
    app.wait_status(KEY_A, &id, "running").await;

    let (status, _) = app.json(Method::POST, &format!("/v1/runs/{id}/cancel"), Some(KEY_A), None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    app.wait_status(KEY_A, &id, "cancelled").await;
    assert!(app.factory.all_destroyed());

    let events = app.events(KEY_A, &id, None).await;
    let last = events.last().unwrap();
    assert_eq!((last.event.as_str(), &last.data["data"]["reason"]), ("run.cancelled", &json!("cancelled by client")));

    // Cancelling again is a conflict, not an error in disguise.
    let (status, body) = app.json(Method::POST, &format!("/v1/runs/{id}/cancel"), Some(KEY_A), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "conflict");
}

#[tokio::test]
async fn a_queued_run_can_be_cancelled_without_ever_starting() {
    let app = TestApp::with_env(vec![Step::Block], &[("MAX_CONCURRENT_RUNS", "1")]).await;
    let first = id_of(&app.create_run(KEY_A, "first").await);
    app.wait_status(KEY_A, &first, "running").await;
    let second = id_of(&app.create_run(KEY_A, "second").await);

    // The only slot is taken, so the second run waits in the queue.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_, queued) = app.get(&format!("/v1/runs/{second}"), KEY_A).await;
    assert_eq!(queued["status"], "queued");

    app.json(Method::POST, &format!("/v1/runs/{second}/cancel"), Some(KEY_A), None).await;
    let run = app.wait_status(KEY_A, &second, "cancelled").await;
    assert_eq!(run["started_at"], Value::Null);
    assert_eq!(app.llm.call_count(), 1, "the queued run never reached the model");
    assert_eq!(app.factory.created.lock().unwrap().len(), 1, "and never got a sandbox");
    assert_eq!(app.get(&format!("/v1/runs/{first}"), KEY_A).await.1["status"], "running");
}

#[tokio::test]
async fn shutdown_cancels_active_runs_and_drains() {
    let app = TestApp::new(vec![Step::Block]).await;
    let id = id_of(&app.create_run(KEY_A, "x").await);
    app.wait_status(KEY_A, &id, "running").await;

    app.state.manager.begin_shutdown();
    app.state.manager.drain(Duration::from_secs(5)).await;

    let run = app.get(&format!("/v1/runs/{id}"), KEY_A).await.1;
    assert_eq!(run["status"], "cancelled");
    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(events.last().unwrap().data["data"]["reason"], "server shutting down");
    assert!(app.factory.all_destroyed());
}

// -- limits & validation ----------------------------------------------------------------------

#[tokio::test]
async fn active_runs_per_key_are_capped() {
    let app = TestApp::with_env(
        vec![Step::Block, Step::Block, Step::Reply(text("ok"))],
        &[("MAX_ACTIVE_RUNS_PER_KEY", "1")],
    )
    .await;
    let first = id_of(&app.create_run(KEY_A, "one").await);

    let (status, body) = app.json(Method::POST, "/v1/runs", Some(KEY_A), Some(json!({ "prompt": "two" }))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["code"], "too_many_requests");

    // The cap is per key, and it frees up when a run ends.
    app.create_run(KEY_B, "other key").await;
    app.json(Method::POST, &format!("/v1/runs/{first}/cancel"), Some(KEY_A), None).await;
    app.wait_status(KEY_A, &first, "cancelled").await;
    app.create_run(KEY_A, "now it fits").await;
}

#[tokio::test]
async fn invalid_requests_are_rejected_with_json_errors() {
    let app = TestApp::with_env(vec![], &[("ALLOWED_MODELS", "claude-haiku-5-5")]).await;
    let post = |body: Value| {
        let app = app.clone();
        async move { app.json(Method::POST, "/v1/runs", Some(KEY_A), Some(body)).await }
    };

    for (body, needle) in [
        (json!({ "prompt": "   " }), "prompt must not be empty"),
        (json!({ "prompt": "x", "max_turns": 0 }), "max_turns"),
        (json!({ "prompt": "x", "model": "gpt-4" }), "not allowed"),
        (json!({ "prompt": "x", "bogus": 1 }), "bogus"),
        (json!({ "nothing": "here" }), "prompt"),
        (json!({ "prompt": "x".repeat(100_001) }), "prompt is longer"),
        (json!({ "prompt": "x", "system": "s".repeat(20_001) }), "system is longer"),
    ] {
        let (status, resp) = post(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
        assert_eq!(resp["error"]["code"], "bad_request");
        assert!(resp["error"]["message"].as_str().unwrap().contains(needle), "{resp} should mention {needle}");
    }

    // A missing body / content type is a client error reported in the same JSON format.
    let (status, _, text) = app.call(Method::POST, "/v1/runs", Some(KEY_A), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["error"]["code"], "bad_request");

    let (status, resp) = post(json!({ "prompt": "x".repeat(300_000) })).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(resp["error"]["code"], "payload_too_large");
}

#[tokio::test]
async fn defaults_and_clamps_are_applied() {
    let app = TestApp::with_env(
        vec![Step::Reply(text("a")), Step::Reply(text("b")), Step::Reply(text("c"))],
        &[("ALLOWED_MODELS", "claude-haiku-5-5")],
    )
    .await;

    let plain = app.create_run(KEY_A, "x").await;
    assert_eq!(plain["model"], "claude-sonnet-5-5");
    assert_eq!(plain["max_turns"], 10);
    assert!(plain["system"].as_str().unwrap().contains("/workspace"));

    let custom = app
        .create_run_with(KEY_A, json!({ "prompt": "x", "model": "claude-haiku-5-5", "system": "be brief", "max_turns": 999 }))
        .await;
    assert_eq!(custom["model"], "claude-haiku-5-5");
    assert_eq!(custom["system"], "be brief");
    assert_eq!(custom["max_turns"], 10, "max_turns is clamped to the server cap");

    // The default model is always allowed, even when it isn't on the allow-list.
    app.create_run_with(KEY_A, json!({ "prompt": "x", "model": "claude-sonnet-5-5" })).await;

    app.wait_status(KEY_A, &id_of(&custom), "completed").await;
    let requests = app.llm.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|r| r["model"] == "claude-haiku-5-5" && r["system"] == "be brief"));
}

// -- ownership, listing, deletion -------------------------------------------------------------

#[tokio::test]
async fn runs_are_private_to_the_key_that_created_them() {
    let app = TestApp::new(vec![Step::Reply(text("secret"))]).await;
    let id = id_of(&app.create_run(KEY_A, "mine").await);
    app.wait_status(KEY_A, &id, "completed").await;

    for (method, path) in [
        (Method::GET, format!("/v1/runs/{id}")),
        (Method::GET, format!("/v1/runs/{id}/messages")),
        (Method::GET, format!("/v1/runs/{id}/events")),
        (Method::POST, format!("/v1/runs/{id}/cancel")),
        (Method::DELETE, format!("/v1/runs/{id}")),
    ] {
        let (status, body) = app.json(method.clone(), &path, Some(KEY_B), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        assert_eq!(body["error"]["code"], "not_found");
    }
    assert_eq!(app.get("/v1/runs", KEY_B).await.1["runs"], json!([]));
    assert_eq!(app.get("/v1/runs", KEY_A).await.1["runs"].as_array().unwrap().len(), 1);

    // Malformed ids look like missing runs too.
    assert_eq!(app.get("/v1/runs/not-a-uuid", KEY_A).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn listing_paginates_newest_first_and_filters_by_status() {
    let app = TestApp::new(vec![Step::Reply(text("1")), Step::Reply(text("2")), Step::Reply(text("3"))]).await;
    let mut ids = Vec::new();
    for i in 0..3 {
        let id = id_of(&app.create_run(KEY_A, &format!("run {i}")).await);
        app.wait_status(KEY_A, &id, "completed").await;
        ids.push(id);
    }

    let (_, page1) = app.get("/v1/runs?limit=2", KEY_A).await;
    let got: Vec<_> = page1["runs"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(got, [ids[2].clone(), ids[1].clone()]);
    let cursor = page1["next_before"].as_str().unwrap();
    assert_eq!(cursor, ids[1]);

    let (_, page2) = app.get(&format!("/v1/runs?limit=2&before={cursor}"), KEY_A).await;
    assert_eq!(page2["runs"].as_array().unwrap().len(), 1);
    assert_eq!(page2["runs"][0]["id"], ids[0]);
    assert_eq!(page2["next_before"], Value::Null);

    assert_eq!(app.get("/v1/runs?status=completed", KEY_A).await.1["runs"].as_array().unwrap().len(), 3);
    assert_eq!(app.get("/v1/runs?status=failed", KEY_A).await.1["runs"].as_array().unwrap().len(), 0);
    assert_eq!(app.get("/v1/runs?status=bogus", KEY_A).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(app.get("/v1/runs?before=nope", KEY_A).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(app.get("/v1/runs?limit=abc", KEY_A).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn only_finished_runs_can_be_deleted() {
    let app = TestApp::new(vec![Step::Block]).await;
    let id = id_of(&app.create_run(KEY_A, "x").await);
    app.wait_status(KEY_A, &id, "running").await;

    let path = format!("/v1/runs/{id}");
    let (status, body) = app.json(Method::DELETE, &path, Some(KEY_A), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    app.json(Method::POST, &format!("{path}/cancel"), Some(KEY_A), None).await;
    app.wait_status(KEY_A, &id, "cancelled").await;
    assert_eq!(app.json(Method::DELETE, &path, Some(KEY_A), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(app.get(&path, KEY_A).await.0, StatusCode::NOT_FOUND);
    assert_eq!(app.state.store.events_after(id.parse().unwrap(), 0, 10).await.unwrap().len(), 0);
}

// -- event streaming --------------------------------------------------------------------------

#[tokio::test]
async fn event_streams_resume_after_the_last_seen_id() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "bash", json!({ "command": "echo hi" }))),
        Step::Reply(text("done")),
    ])
    .await;
    let id = id_of(&app.create_run(KEY_A, "x").await);
    app.wait_status(KEY_A, &id, "completed").await;

    let all = app.events(KEY_A, &id, None).await;
    assert_eq!(all.len(), 7);

    let resumed = app.events(KEY_A, &id, Some(3)).await;
    assert_eq!(resumed.iter().map(|e| e.id).collect::<Vec<_>>(), [4, 5, 6, 7]);

    // `?after=` works for clients that can't set headers.
    let (status, _, body) = app.call(Method::GET, &format!("/v1/runs/{id}/events?after=5"), Some(KEY_A), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(parse_sse(&body).iter().map(|e| e.id).collect::<Vec<_>>(), [6, 7]);

    // Nothing left to send: the stream closes immediately instead of hanging on a finished run.
    let done = tokio::time::timeout(Duration::from_secs(2), app.events(KEY_A, &id, Some(7))).await.unwrap();
    assert!(done.is_empty());
}

#[tokio::test]
async fn event_streams_follow_a_live_run_in_order() {
    let gate = Arc::new(Semaphore::new(0));
    let app = TestApp::new(vec![
        Step::Gate(gate.clone(), tool_use("t1", "bash", json!({ "command": "echo hi" }))),
        Step::Reply(text("done")),
    ])
    .await;
    let id = id_of(&app.create_run(KEY_A, "x").await);
    app.wait_status(KEY_A, &id, "running").await;

    let reader = tokio::spawn({
        let (app, id) = (app.clone(), id.clone());
        async move { app.events(KEY_A, &id, None).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!reader.is_finished(), "the stream must stay open while the run is in progress");

    gate.add_permits(1);
    let events = tokio::time::timeout(Duration::from_secs(5), reader).await.unwrap().unwrap();
    assert_eq!(
        kinds(&events),
        ["run.started", "usage", "tool.call", "tool.result", "usage", "assistant.text", "run.completed"]
    );
    assert_eq!(events.iter().map(|e| e.id).collect::<Vec<_>>(), (1..=7).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_stream_opened_before_the_run_is_picked_up_still_sees_everything() {
    // Hold the only slot so the run is queued when the stream opens.
    let app = TestApp::with_env(vec![Step::Block, Step::Reply(text("late"))], &[("MAX_CONCURRENT_RUNS", "1")]).await;
    let blocker = id_of(&app.create_run(KEY_A, "blocker").await);
    app.wait_status(KEY_A, &blocker, "running").await;
    let queued = id_of(&app.create_run(KEY_A, "queued").await);

    let reader = tokio::spawn({
        let (app, id) = (app.clone(), queued.clone());
        async move { app.events(KEY_A, &id, None).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    app.json(Method::POST, &format!("/v1/runs/{blocker}/cancel"), Some(KEY_A), None).await;

    let events = tokio::time::timeout(Duration::from_secs(5), reader).await.unwrap().unwrap();
    assert_eq!(kinds(&events), ["run.started", "usage", "assistant.text", "run.completed"]);
}

// -- startup recovery -------------------------------------------------------------------------

#[tokio::test]
async fn runs_left_over_from_a_previous_process_are_failed_on_startup() {
    let store = Store::connect_memory().await.unwrap();
    let new = |prompt: &str| NewRun {
        owner: cedar::auth::AuthKeys::new(&[KEY_A.into()]).authenticate(KEY_A).unwrap().0,
        prompt: prompt.into(),
        system: "s".into(),
        model: "m".into(),
        max_turns: 3,
    };
    let running = store.insert_run_limited(&new("was running"), 10).await.unwrap().unwrap();
    store.mark_running(running.id).await.unwrap();
    let queued = store.insert_run_limited(&new("was queued"), 10).await.unwrap().unwrap();

    let app = TestApp::build(store, ScriptedLlm::new(vec![]), FakeFactory::new(), &[]);
    app.state.manager.recover().await.unwrap();

    for id in [running.id, queued.id] {
        let run = app.get(&format!("/v1/runs/{id}"), KEY_A).await.1;
        assert_eq!(run["status"], "failed");
        assert_eq!(run["error"], "server restarted before the run finished");
        let events = app.events(KEY_A, &id.to_string(), None).await;
        assert_eq!(kinds(&events), ["run.failed"]);
    }
    assert_eq!(app.factory.cleanups.load(Ordering::SeqCst), 1, "leftover containers are swept too");
    // And the failed runs no longer count against the key's active-run cap.
    assert_eq!(app.state.store.count_active(&running.owner).await.unwrap(), 0);
}
