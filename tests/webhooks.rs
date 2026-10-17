//! Notifications when a run ends: webhooks to a run's `callback_url`, and the key's event
//! stream at `GET /v1/events`.

mod common;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::routing::post;
use axum::Router;
use cedar::models::NewRun;
use cedar::store::Store;
use cedar::webhooks::hmac_sha256;
use common::*;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A webhook endpoint on a local port that records what it receives and answers with the
/// statuses it was given (200 once those run out).
#[derive(Default)]
struct Hook {
    /// Each delivery's headers and body, with the raw bytes the signature covers.
    received: Mutex<Vec<(HeaderMap, Value, Vec<u8>)>>,
    answers: Mutex<VecDeque<u16>>,
}

async fn receive(State(hook): State<Arc<Hook>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    hook.received.lock().unwrap().push((headers, value, body.to_vec()));
    let status = hook.answers.lock().unwrap().pop_front().unwrap_or(200);
    StatusCode::from_u16(status).unwrap()
}

impl Hook {
    async fn serve(answers: &[u16]) -> (Arc<Self>, String) {
        let hook = Arc::new(Hook {
            answers: Mutex::new(answers.iter().copied().collect()),
            ..Default::default()
        });
        let app = Router::new()
            .route("/hook", post(receive))
            .with_state(hook.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (hook, url)
    }

    fn count(&self) -> usize {
        self.received.lock().unwrap().len()
    }

    /// Wait until `n` deliveries have arrived, panicking after a few seconds.
    async fn wait_for(&self, n: usize) -> Vec<(HeaderMap, Value, Vec<u8>)> {
        for _ in 0..500 {
            if self.count() >= n {
                return self.received.lock().unwrap().clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("only {} of {n} deliveries arrived", self.count());
    }
}

/// Loopback endpoints need `WEBHOOK_ALLOW_PRIVATE`; retries happen at once.
const LOCAL: &[(&str, &str)] = &[("WEBHOOK_ALLOW_PRIVATE", "on"), ("WEBHOOK_RETRY_SECS", "0")];

fn id_of(run: &Value) -> String {
    run["id"].as_str().unwrap().to_string()
}

/// Poll the delivery bookkeeping until the attempt count stops at `attempts`.
async fn wait_attempts(store: &Store, run_id: &str, attempts: i64) -> (i64, bool, Option<String>) {
    let id = uuid::Uuid::parse_str(run_id).unwrap();
    for _ in 0..500 {
        if let Some((n, delivered, error)) = store.delivery_status(id).await.unwrap() {
            if n >= attempts {
                return (n, delivered.is_some(), error);
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("delivery of {run_id} never reached {attempts} attempts");
}

#[tokio::test]
async fn a_finished_run_is_posted_to_its_callback_url_with_a_signature() {
    let (hook, url) = Hook::serve(&[]).await;
    let app = TestApp::with_env(vec![Step::Reply(text("done"))], LOCAL).await;
    let run = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "hi", "callback_url": url, "callback_secret": "s3cret" }),
        )
        .await;
    assert_eq!(run["callback_url"], url);
    assert!(run.get("callback_secret").is_none(), "the secret must not be returned: {run}");
    let id = id_of(&run);
    let finished = app.wait_status(KEY_A, &id, "completed").await;
    assert_eq!(finished["callback_url"], url);

    let received = hook.wait_for(1).await;
    let (headers, body, raw) = &received[0];
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(headers["x-cedar-event"], "run.completed");
    assert_eq!(headers["x-cedar-run"], id);
    assert!(headers["user-agent"].to_str().unwrap().starts_with("cedar/"));
    assert_eq!(body["type"], "run.completed");
    assert_eq!(body["run_id"], id);
    assert_eq!(body["status"], "completed");
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["error"], Value::Null);
    assert_eq!(body["parent_id"], Value::Null);
    assert_eq!(body["seq"], 4, "the terminal event's seq: started, usage, text, completed");
    assert!(body["finished_at"].is_string());
    assert_eq!(headers["x-cedar-delivery"], body["id"].as_i64().unwrap().to_string());
    // Nothing the client could learn from the run itself is in the payload.
    assert!(body.get("final_text").is_none() && body.get("prompt").is_none(), "{body}");

    // The signature covers the timestamp and the raw body.
    let signature = headers["x-cedar-signature"].to_str().unwrap();
    let (t, v1) = signature.split_once(',').unwrap();
    let t = t.strip_prefix("t=").unwrap();
    let v1 = v1.strip_prefix("v1=").unwrap();
    let now = chrono::Utc::now().timestamp();
    assert!((now - t.parse::<i64>().unwrap()).abs() < 60, "{signature}");
    let mut signed = format!("{t}.").into_bytes();
    signed.extend_from_slice(raw);
    assert_eq!(v1, hex::encode(hmac_sha256(b"s3cret", &signed)));

    let (attempts, delivered, error) = wait_attempts(&app.state.store, &id, 1).await;
    assert_eq!((attempts, delivered, error), (1, true, None));
    // Once delivered, that's it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(hook.count(), 1);

    // The key's event stream has the same notification.
    let (status, body) = app.get("/v1/runs", KEY_A).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["runs"][0]["callback_url"], url);
}

#[tokio::test]
async fn runs_without_a_secret_are_posted_unsigned() {
    let (hook, url) = Hook::serve(&[]).await;
    let app = TestApp::with_env(vec![Step::Error("model is down")], LOCAL).await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "callback_url": url }))
        .await;
    app.wait_status(KEY_A, &id_of(&run), "failed").await;
    let received = hook.wait_for(1).await;
    let (headers, body, _) = &received[0];
    assert!(headers.get("x-cedar-signature").is_none());
    assert_eq!(headers["x-cedar-event"], "run.failed");
    assert_eq!(body["type"], "run.failed");
    assert_eq!(body["status"], "failed");
    assert_eq!(body["error"], "model is down");
}

#[tokio::test]
async fn failed_deliveries_are_retried_until_the_endpoint_takes_them() {
    let (hook, url) = Hook::serve(&[503, 429]).await;
    let app = TestApp::with_env(vec![Step::Reply(text("done"))], LOCAL).await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "callback_url": url }))
        .await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    let received = hook.wait_for(3).await;
    let ids: Vec<&Value> = received.iter().map(|(_, b, _)| &b["id"]).collect();
    assert_eq!(ids[0], ids[1], "every try carries the same delivery id");
    assert_eq!(ids[1], ids[2]);
    let (attempts, delivered, error) = wait_attempts(&app.state.store, &id, 3).await;
    assert_eq!((attempts, delivered, error), (3, true, None));
}

#[tokio::test]
async fn a_rejected_delivery_is_not_retried() {
    let (hook, url) = Hook::serve(&[410]).await;
    let app = TestApp::with_env(vec![Step::Reply(text("done"))], LOCAL).await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "callback_url": url }))
        .await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    hook.wait_for(1).await;
    let (attempts, delivered, error) = wait_attempts(&app.state.store, &id, 1).await;
    assert_eq!((attempts, delivered), (1, false));
    assert!(error.unwrap().contains("410"), "the endpoint's answer is recorded");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(hook.count(), 1);
    assert_eq!(app.state.store.next_delivery_at().await.unwrap(), None);
}

#[tokio::test]
async fn an_unreachable_endpoint_is_given_up_on_after_the_last_attempt() {
    // Nothing listens on this port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    drop(listener);
    let app = TestApp::with_env(vec![Step::Reply(text("done"))], LOCAL).await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "callback_url": url }))
        .await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    let (attempts, delivered, error) =
        wait_attempts(&app.state.store, &id, cedar::webhooks::MAX_ATTEMPTS).await;
    assert_eq!((attempts, delivered), (cedar::webhooks::MAX_ATTEMPTS, false));
    assert!(error.is_some());
    assert_eq!(app.state.store.next_delivery_at().await.unwrap(), None);
}

#[tokio::test]
async fn callback_requests_are_validated() {
    let app = TestApp::new(vec![]).await;
    let bad = |body: Value| {
        let app = app.clone();
        async move {
            let (status, resp) = app.json(Method::POST, "/v1/runs", Some(KEY_A), Some(body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
            resp["error"]["message"].as_str().unwrap().to_string()
        }
    };
    assert!(bad(json!({ "prompt": "hi", "callback_url": "ftp://hooks.example/x" }))
        .await
        .contains("http or https"));
    assert!(bad(json!({ "prompt": "hi", "callback_url": "hooks.example/x" }))
        .await
        .contains("not a valid URL"));
    // By default the callback may not point at this host or the internal network.
    assert!(bad(json!({ "prompt": "hi", "callback_url": "http://127.0.0.1:3000/hook" }))
        .await
        .contains("not a public address"));
    assert!(bad(json!({ "prompt": "hi", "callback_url": "http://[::1]:3000/hook" }))
        .await
        .contains("not a public address"));
    assert!(bad(json!({ "prompt": "hi", "callback_secret": "s" }))
        .await
        .contains("needs a callback_url"));
    assert!(bad(json!({ "prompt": "hi", "callback_url": "https://hooks.example/x", "callback_secret": "s".repeat(300) }))
        .await
        .contains("callback_secret is longer"));
    // A blank callback is no callback, and an empty secret is none.
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "callback_url": " ", "callback_secret": "" }))
        .await;
    assert_eq!(run["callback_url"], Value::Null);
    // The private-address rule can be lifted.
    let app = TestApp::with_env(vec![], LOCAL).await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "callback_url": "http://127.0.0.1:3000/hook" }))
        .await;
    assert_eq!(run["callback_url"], "http://127.0.0.1:3000/hook");
}

#[tokio::test]
async fn a_continuation_keeps_its_parents_callback() {
    let (hook, url) = Hook::serve(&[]).await;
    let app = TestApp::with_env(
        vec![Step::Reply(text("one")), Step::Reply(text("two")), Step::Reply(text("three"))],
        LOCAL,
    )
    .await;
    let parent = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "callback_url": url, "callback_secret": "s" }))
        .await;
    let parent_id = id_of(&parent);
    app.wait_status(KEY_A, &parent_id, "completed").await;
    let child = app
        .create_run_with(KEY_A, json!({ "prompt": "more", "parent_id": parent_id }))
        .await;
    assert_eq!(child["callback_url"], url);
    app.wait_status(KEY_A, &id_of(&child), "completed").await;
    let received = hook.wait_for(2).await;
    assert_eq!(received[1].1["run_id"], child["id"]);
    assert_eq!(received[1].1["parent_id"], parent_id);
    assert!(received[1].0.get("x-cedar-signature").is_some(), "the secret came along too");

    // Naming another URL replaces it; a blank one is refused as a URL, not taken as "none".
    let (other_hook, other_url) = Hook::serve(&[]).await;
    let grandchild = app
        .create_run_with(KEY_A, json!({ "prompt": "again", "parent_id": child["id"], "callback_url": other_url }))
        .await;
    assert_eq!(grandchild["callback_url"], other_url);
    app.wait_status(KEY_A, &id_of(&grandchild), "completed").await;
    other_hook.wait_for(1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(hook.count(), 2);
}

#[tokio::test]
async fn deliveries_left_over_from_a_previous_process_are_made_at_startup() {
    let (hook, url) = Hook::serve(&[]).await;
    let store = Store::connect_memory().await.unwrap();
    let new = |prompt: &str| NewRun {
        owner: "x".into(),
        prompt: prompt.into(),
        system: "s".into(),
        model_id: TEST_MODEL.into(),
        model: TEST_MODEL.into(),
        reasoning: None,
        max_turns: 1,
        parent_id: None,
        tools: None,
        attachments: vec![],
        callback_url: Some(url.clone()),
        callback_secret: None,
    };
    // One run finished without its webhook being sent, one was still running when the process died.
    let finished = store.insert_run_limited(&new("a"), 10).await.unwrap().unwrap();
    store
        .finish_run(
            finished.id,
            cedar::store::Finish {
                status: cedar::models::RunStatus::Completed,
                final_text: Some("ok"),
                stop_reason: Some("end_turn"),
                error: None,
                event_kind: "run.completed",
                event_data: json!({}),
            },
        )
        .await
        .unwrap();
    let orphan = store.insert_run_limited(&new("b"), 10).await.unwrap().unwrap();
    store.mark_running(orphan.id).await.unwrap();

    let app = TestApp::build(store, ScriptedLlm::new(vec![]), FakeFactory::new(), LOCAL).await;
    app.state.manager.recover().await.unwrap();
    // Recovery fails the orphan; the worker picks up both on its next pass.
    app.state.webhooks.deliver_due().await.unwrap();
    let received = hook.wait_for(2).await;
    let mut kinds: Vec<(String, String)> = received
        .iter()
        .map(|(_, b, _)| (b["run_id"].as_str().unwrap().into(), b["type"].as_str().unwrap().into()))
        .collect();
    kinds.sort();
    let mut want = vec![
        (finished.id.to_string(), "run.completed".to_string()),
        (orphan.id.to_string(), "run.failed".to_string()),
    ];
    want.sort();
    assert_eq!(kinds, want);
}

// -- the key's event stream --------------------------------------------------------------------

#[tokio::test]
async fn the_event_stream_replays_finished_runs_and_follows_new_ones() {
    let app = TestApp::new(vec![
        Step::Reply(text("one")),
        Step::Error("boom"),
        Step::Reply(text("three")),
    ])
    .await;
    let first = id_of(&app.create_run(KEY_A, "a").await);
    app.wait_status(KEY_A, &first, "completed").await;
    let other = id_of(&app.create_run(KEY_B, "b").await);
    app.wait_status(KEY_B, &other, "failed").await;

    // Replay: only this key's runs, oldest first, then the stream stays open.
    let mut stream = LiveStream::open_uri(&app, KEY_A, "/v1/events", None).await;
    let e = stream.next_event().await.unwrap();
    assert_eq!(e.event, "run.completed");
    assert_eq!(e.data["run_id"], first);
    assert_eq!(e.data["status"], "completed");
    assert_eq!(e.data["stop_reason"], "end_turn");
    assert_eq!(e.data["id"], e.id);
    assert!(e.data["finished_at"].is_string());
    let replayed = e.id;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), stream.next_event())
            .await
            .is_err(),
        "nothing else yet, and the stream stays open"
    );

    // A run that ends while the stream is open arrives live.
    let live = id_of(&app.create_run(KEY_A, "c").await);
    let e = tokio::time::timeout(Duration::from_secs(5), stream.next_event())
        .await
        .expect("a live notification")
        .unwrap();
    assert_eq!(e.event, "run.completed");
    assert_eq!(e.data["run_id"], live);
    assert!(e.id > replayed);
    let latest = e.id;

    // Resuming after the last id seen yields nothing old; `?after=` works the same way.
    let mut resumed = LiveStream::open_uri(&app, KEY_A, "/v1/events", Some(latest)).await;
    assert!(tokio::time::timeout(Duration::from_millis(100), resumed.next_event())
        .await
        .is_err());
    let mut from_first = LiveStream::open_uri(&app, KEY_A, &format!("/v1/events?after={replayed}"), None)
        .await;
    assert_eq!(from_first.next_event().await.unwrap().data["run_id"], live);

    // The other key sees only its own.
    let mut theirs = LiveStream::open_uri(&app, KEY_B, "/v1/events", None).await;
    let e = theirs.next_event().await.unwrap();
    assert_eq!((e.event.as_str(), e.data["run_id"].as_str().unwrap()), ("run.failed", other.as_str()));
    assert_eq!(e.data["error"], "boom");
    assert!(tokio::time::timeout(Duration::from_millis(100), theirs.next_event())
        .await
        .is_err());

    // A deleted run drops out of the replay.
    let (status, _) = app
        .json(Method::DELETE, &format!("/v1/runs/{first}"), Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let mut after_delete = LiveStream::open_uri(&app, KEY_A, "/v1/events", None).await;
    assert_eq!(after_delete.next_event().await.unwrap().data["run_id"], live);
}

#[tokio::test]
async fn the_event_stream_needs_a_key() {
    let app = TestApp::new(vec![]).await;
    let (status, _, _) = app.call(Method::GET, "/v1/events", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
