//! The `browser` tool through the API, with a fake browser factory: when browsers start and stop,
//! what reaches them, and what the model gets back.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

const BROWSER_ON: &[(&str, &str)] = &[("BROWSER", "on")];

/// The tool result at the end of the messages sent in the model's `request`-th call.
fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

/// Run one tool call per entry, then return the app once the run has completed.
async fn run_calls(calls: Vec<(&str, Value)>, env: &[(&str, &str)]) -> TestApp {
    let mut script: Vec<Step> = calls
        .into_iter()
        .enumerate()
        .map(|(i, (name, input))| Step::Reply(tool_use(&format!("t{i}"), name, input)))
        .collect();
    script.push(Step::Reply(text("done")));
    let app = TestApp::with_env(script, env).await;
    let created = app.create_run(KEY_A, "look at the listing").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    app
}

#[tokio::test]
async fn one_browser_serves_the_whole_run_and_is_removed_with_it() {
    let app = run_calls(
        vec![
            ("browser", json!({ "action": "snapshot" })),
            ("browser", json!({ "action": "click", "ref": 1 })),
            ("browser", json!({ "action": "scroll" })),
        ],
        BROWSER_ON,
    )
    .await;

    let created = app.browsers.created.lock().unwrap().clone();
    assert_eq!(created.len(), 1, "started once, on first use");
    let run_id = created[0].0;
    assert_eq!(
        *app.browsers.browser(0).requests.lock().unwrap(),
        [
            json!({ "action": "snapshot" }),
            json!({ "action": "click", "ref": 1 }),
            json!({ "action": "scroll", "direction": "down" }),
        ]
    );
    assert_eq!(*app.browsers.destroyed.lock().unwrap(), [run_id]);
    assert!(app.factory.all_destroyed());

    let page = tool_result(&app, 2);
    assert!(page.get("is_error").is_none(), "{page}");
    let content = page["content"].as_str().unwrap();
    assert!(
        content.starts_with("URL: https://example.com/listing\nTitle: A listing\n\n# A listing\n[1] button \"Show all reviews\""),
        "{content}"
    );
}

#[tokio::test]
async fn runs_that_never_browse_start_no_browser() {
    let app = run_calls(vec![("bash", json!({ "command": "ls" }))], BROWSER_ON).await;
    assert!(app.browsers.created.lock().unwrap().is_empty());
    assert!(app.browsers.destroyed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn mistakes_go_back_to_the_model_as_errors() {
    let app = run_calls(
        vec![
            ("browser", json!({ "action": "click" })),
            ("browser", json!({ "action": "click", "ref": 99 })),
            (
                "browser",
                json!({ "action": "navigate", "url": "http://127.0.0.1:8080/admin" }),
            ),
            (
                "browser",
                json!({ "action": "navigate", "url": "file:///etc/passwd" }),
            ),
        ],
        BROWSER_ON,
    )
    .await;

    for (call, expected) in [
        (1, "click needs `ref`"),
        (2, "[99] is not on the current page"),
        (3, "127.0.0.1 is not a public address"),
        (4, "only http and https URLs"),
    ] {
        let result = tool_result(&app, call);
        assert_eq!(result["is_error"], true, "{result}");
        let content = result["content"].as_str().unwrap();
        assert!(content.contains(expected), "call {call}: {content}");
    }
    // Only the request that passed cedar's checks reached the browser.
    assert_eq!(
        *app.browsers.browser(0).requests.lock().unwrap(),
        [json!({ "action": "click", "ref": 99 })]
    );
}

#[tokio::test]
async fn private_addresses_can_be_allowed_for_intranet_deployments() {
    let app = run_calls(
        vec![(
            "browser",
            json!({ "action": "navigate", "url": "http://127.0.0.1:8080/admin" }),
        )],
        &[("BROWSER", "on"), ("WEB_FETCH_ALLOW_PRIVATE", "on")],
    )
    .await;
    assert!(tool_result(&app, 1).get("is_error").is_none());
    assert_eq!(
        *app.browsers.browser(0).requests.lock().unwrap(),
        [json!({ "action": "navigate", "url": "http://127.0.0.1:8080/admin" })]
    );
}

#[tokio::test]
async fn without_the_setting_there_is_no_browser_tool() {
    let app = run_calls(vec![("browser", json!({ "action": "snapshot" }))], &[]).await;
    let tools = app.llm.requests()[0]["tools"].clone();
    assert!(
        !tools.to_string().contains("\"browser\""),
        "browser tool offered: {tools}"
    );
    let result = tool_result(&app, 1);
    assert_eq!(result["content"], "unknown tool: browser");
    assert!(app.browsers.created.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_browser_is_offered_and_explained_when_on() {
    let app = run_calls(vec![], BROWSER_ON).await;
    let request = &app.llm.requests()[0];
    let names: Vec<&str> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"browser"), "{names:?}");
    assert!(
        request["system"]
            .as_str()
            .unwrap()
            .contains("use the browser tool"),
        "{}",
        request["system"]
    );
}

#[tokio::test]
async fn a_cancelled_run_still_removes_its_browser() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "browser", json!({ "action": "snapshot" }))),
            Step::Block,
        ],
        BROWSER_ON,
    )
    .await;
    let created = app.create_run(KEY_A, "browse").await;
    let id = created["id"].as_str().unwrap().to_string();
    // Wait until the browser has been used and the model is thinking again.
    for _ in 0..200 {
        if app.llm.call_count() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(app.browsers.created.lock().unwrap().len(), 1);
    let (status, _) = app
        .json(
            Method::POST,
            &format!("/v1/runs/{id}/cancel"),
            Some(KEY_A),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    app.wait_status(KEY_A, &id, "cancelled").await;
    assert_eq!(app.browsers.destroyed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn readyz_checks_the_browser_only_when_it_is_on() {
    let off = TestApp::new(vec![]).await;
    let (_, body) = off.json(Method::GET, "/readyz", None, None).await;
    assert_eq!(body["checks"], json!({ "database": "ok", "sandbox": "ok" }));

    let on = TestApp::with_env(vec![], BROWSER_ON).await;
    let (status, body) = on.json(Method::GET, "/readyz", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["checks"],
        json!({ "database": "ok", "sandbox": "ok", "browser": "ok" })
    );

    on.browsers.unhealthy.store(true, Ordering::SeqCst);
    let (status, body) = on.json(Method::GET, "/readyz", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["checks"]["browser"], "failed");
    assert!(!body.to_string().contains("image is missing"));
}

#[tokio::test]
async fn startup_sweeps_leftover_browsers_only_when_on() {
    let off = TestApp::new(vec![]).await;
    off.state.manager.recover().await.unwrap();
    assert_eq!(off.browsers.cleanups.load(Ordering::SeqCst), 0);

    let on = TestApp::with_env(vec![], BROWSER_ON).await;
    on.state.manager.recover().await.unwrap();
    assert_eq!(on.browsers.cleanups.load(Ordering::SeqCst), 1);
}
