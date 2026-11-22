//! A service key acting for its users (`CEDAR_SERVICE_KEYS`, `X-Cedar-User`): what a user's runs
//! make and remember is that user's, and the service follows every user's finished runs.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::time::Duration;
use tower::ServiceExt;

const ALICE: &str = "0190a7c0-0000-7000-8000-00000000000a";
const BOB: &str = "0190a7c0-0000-7000-8000-00000000000b";

/// One request with `key`, for `user` when given.
async fn as_user(app: &TestApp, method: Method, uri: &str, key: &str, user: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {key}"));
    if let Some(user) = user {
        req = req.header("x-cedar-user", user);
    }
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn wait_done(app: &TestApp, user: Option<&str>, id: &str) {
    for _ in 0..500 {
        let (_, run) = as_user(app, Method::GET, &format!("/v1/runs/{id}"), KEY_A, user, None).await;
        if run["status"] == "completed" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {id} did not complete");
}

#[tokio::test]
async fn a_service_key_keeps_its_users_apart() {
    let app = TestApp::with_env(
        vec![Step::Reply(text("for alice")), Step::Reply(text("for bob")), Step::Reply(text("for the key"))],
        &[("CEDAR_SERVICE_KEYS", KEY_A)],
    )
    .await;

    // Runs: each user sees their own, and neither the other's nor the key's.
    let (status, run) = as_user(&app, Method::POST, "/v1/runs", KEY_A, Some(ALICE), Some(json!({ "prompt": "a" }))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    let alice_run = run["id"].as_str().unwrap().to_string();
    wait_done(&app, Some(ALICE), &alice_run).await;
    let (_, run) = as_user(&app, Method::POST, "/v1/runs", KEY_A, Some(BOB), Some(json!({ "prompt": "b" }))).await;
    let bob_run = run["id"].as_str().unwrap().to_string();
    wait_done(&app, Some(BOB), &bob_run).await;
    let (_, run) = as_user(&app, Method::POST, "/v1/runs", KEY_A, None, Some(json!({ "prompt": "k" }))).await;
    let key_run = run["id"].as_str().unwrap().to_string();
    wait_done(&app, None, &key_run).await;

    let uri = format!("/v1/runs/{alice_run}");
    assert_eq!(as_user(&app, Method::GET, &uri, KEY_A, Some(BOB), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(as_user(&app, Method::GET, &uri, KEY_A, None, None).await.0, StatusCode::NOT_FOUND);
    let (_, list) = as_user(&app, Method::GET, "/v1/runs", KEY_A, Some(BOB), None).await;
    let ids: Vec<&str> = list["runs"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [bob_run.as_str()]);

    // Memory and records follow the user too.
    let (status, _) = as_user(&app, Method::POST, "/v1/memory", KEY_A, Some(ALICE), Some(json!({ "text": "Alice likes tea" }))).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, mine) = as_user(&app, Method::GET, "/v1/memory", KEY_A, Some(ALICE), None).await;
    assert_eq!(mine["items"].as_array().unwrap().len(), 1);
    for other in [Some(BOB), None] {
        let (_, theirs) = as_user(&app, Method::GET, "/v1/memory", KEY_A, other, None).await;
        assert!(theirs["items"].as_array().unwrap().is_empty(), "{theirs}");
    }
    let (status, _) = as_user(&app, Method::POST, "/v1/records/books", KEY_A, Some(BOB), Some(json!({ "records": [{ "title": "Dune" }], "description": "Books read" }))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(as_user(&app, Method::GET, "/v1/records/books", KEY_A, Some(ALICE), None).await.0, StatusCode::NOT_FOUND);

    // The service's stream of everyone: all three runs, in the order they ended. Its plain
    // stream stays its own.
    let mut all = LiveStream::open_uri(&app, KEY_A, "/v1/events?users=all", None).await;
    for want in [&alice_run, &bob_run, &key_run] {
        let e = tokio::time::timeout(Duration::from_secs(5), all.next_event()).await.unwrap().unwrap();
        assert_eq!(e.data["run_id"], want.as_str());
    }
    let mut own = LiveStream::open_uri(&app, KEY_A, "/v1/events", None).await;
    let e = tokio::time::timeout(Duration::from_secs(5), own.next_event()).await.unwrap().unwrap();
    assert_eq!(e.data["run_id"], key_run.as_str());
}

#[tokio::test]
async fn only_a_service_key_may_name_a_user() {
    let app = TestApp::with_env(vec![], &[("CEDAR_SERVICE_KEYS", KEY_A)]).await;
    // Another key naming a user is refused, not answered as itself.
    let (status, err) = as_user(&app, Method::GET, "/v1/runs", KEY_B, Some(ALICE), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(err["error"]["message"].as_str().unwrap().contains("CEDAR_SERVICE_KEYS"));
    assert_eq!(as_user(&app, Method::GET, "/v1/events?users=all", KEY_B, None, None).await.0, StatusCode::FORBIDDEN);
    // The user is a UUID.
    assert_eq!(as_user(&app, Method::GET, "/v1/runs", KEY_A, Some("alice"), None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(as_user(&app, Method::GET, "/v1/runs", KEY_A, Some(ALICE), None).await.0, StatusCode::OK);
}
