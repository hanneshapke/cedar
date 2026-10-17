//! `fetch_url` and `download_file` against a local web server. The server is on 127.0.0.1, so
//! these runs set `WEB_FETCH_ALLOW_PRIVATE`; one test leaves it unset to check the guard.

mod common;

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::Router;
use common::*;
use serde_json::{json, Value};

const PAGE: &str = "<!doctype html><html><head><title>Cedar &amp; tools</title>\
    <script>tracking()</script></head><body><h1>Release notes</h1>\
    <p>Version <b>2</b> adds <a href=\"https://example.com/docs\">docs</a>.</p></body></html>";

async fn start() -> String {
    let app = Router::new()
        .route(
            "/page",
            get(|| async { ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], PAGE) }),
        )
        .route(
            "/data.json",
            get(|| async { ([(header::CONTENT_TYPE, "application/json")], r#"{"a": 1}"#) }),
        )
        .route(
            "/report.pdf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/pdf")],
                    b"%PDF-1.7 fake".to_vec(),
                )
            }),
        )
        .route("/old", get(|| async { Redirect::permanent("/page") }))
        .route(
            "/missing",
            get(|| async { StatusCode::NOT_FOUND.into_response() }),
        )
        .route(
            "/long",
            get(|| async { ([(header::CONTENT_TYPE, "text/plain")], "x".repeat(100_000)) }),
        )
        .route(
            "/huge.bin",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/octet-stream")],
                    vec![0u8; 26 * 1024 * 1024],
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// The tool result at the end of the messages sent in the model's `request`-th call.
fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

/// Run one tool call per entry with private addresses allowed, then return the app.
async fn run_calls(calls: Vec<(&str, Value)>, env: &[(&str, &str)]) -> TestApp {
    let mut script: Vec<Step> = calls
        .into_iter()
        .enumerate()
        .map(|(i, (name, input))| Step::Reply(tool_use(&format!("t{i}"), name, input)))
        .collect();
    script.push(Step::Reply(text("done")));
    let app = TestApp::with_env(script, env).await;
    let created = app.create_run(KEY_A, "use the web").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    app
}

const PRIVATE_OK: &[(&str, &str)] = &[("WEB_FETCH_ALLOW_PRIVATE", "on")];

#[tokio::test]
async fn a_page_is_fetched_as_text_with_its_title() {
    let base = start().await;
    let app = run_calls(
        vec![
            ("fetch_url", json!({ "url": format!("{base}/page") })),
            ("fetch_url", json!({ "url": format!("{base}/old") })),
            ("fetch_url", json!({ "url": format!("{base}/data.json") })),
        ],
        PRIVATE_OK,
    )
    .await;

    let page = tool_result(&app, 1);
    assert!(page.get("is_error").is_none(), "{page}");
    let content = page["content"].as_str().unwrap();
    assert!(
        // Prefixed with its source number, which the model cites it by.
        content.starts_with(&format!("[1] {base}/page\nURL: {base}/page\nTitle: Cedar & tools\n\n")),
        "{content}"
    );
    assert!(content.contains("Release notes"), "{content}");
    assert!(content.contains("**2**"), "{content}");
    assert!(content.contains("https://example.com/docs"), "{content}");
    assert!(!content.contains("tracking"), "{content}");

    // Redirects are followed and the final URL reported.
    let redirected = tool_result(&app, 2)["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        redirected.starts_with(&format!("[1] {base}/page\nURL: {base}/page\n")),
        "{redirected}"
    );

    let data = tool_result(&app, 3)["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(data, format!("[2] {base}/data.json\nURL: {base}/data.json\n\n{{\"a\": 1}}"));
}

#[tokio::test]
async fn long_pages_are_cut_and_can_be_continued() {
    let base = start().await;
    let url = format!("{base}/long");
    let app = run_calls(
        vec![
            ("fetch_url", json!({ "url": url })),
            ("fetch_url", json!({ "url": url, "start": 99_990 })),
        ],
        PRIVATE_OK,
    )
    .await;
    let first = tool_result(&app, 1)["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(first.len() < 33_000, "{}", first.len());
    assert!(
        first.contains("more characters; call fetch_url with start="),
        "{}",
        &first[first.len() - 200..]
    );
    let second = tool_result(&app, 2)["content"]
        .as_str()
        .unwrap()
        .to_string();
    // The same page keeps its source number.
    assert_eq!(second, format!("[1] {url}\nURL: {url}\n\n{}", "x".repeat(10)));
}

#[tokio::test]
async fn binary_files_errors_and_internal_addresses_are_reported() {
    let base = start().await;
    let app = run_calls(
        vec![
            ("fetch_url", json!({ "url": format!("{base}/report.pdf") })),
            ("fetch_url", json!({ "url": format!("{base}/missing") })),
            ("fetch_url", json!({ "url": "file:///etc/passwd" })),
        ],
        PRIVATE_OK,
    )
    .await;
    let pdf = tool_result(&app, 1);
    assert_eq!(pdf["is_error"], true);
    let msg = pdf["content"].as_str().unwrap();
    assert!(
        msg.contains("is application/pdf, not text; use download_file"),
        "{msg}"
    );

    let missing = tool_result(&app, 2);
    assert_eq!(missing["is_error"], true);
    assert!(missing["content"]
        .as_str()
        .unwrap()
        .ends_with("returned HTTP 404 Not Found"));

    let file = tool_result(&app, 3);
    assert_eq!(file["content"], "only http and https URLs can be fetched");

    // Without WEB_FETCH_ALLOW_PRIVATE, the same local server is off limits, by IP or by name.
    let port = base.rsplit(':').next().unwrap();
    let app = run_calls(
        vec![
            ("fetch_url", json!({ "url": format!("{base}/page") })),
            (
                "download_file",
                json!({ "url": format!("http://localhost:{port}/page"), "path": "p.html" }),
            ),
        ],
        &[],
    )
    .await;
    let by_ip = tool_result(&app, 1);
    assert_eq!(by_ip["is_error"], true);
    assert_eq!(by_ip["content"], "127.0.0.1 is not a public address");
    let by_name = tool_result(&app, 2);
    assert_eq!(by_name["is_error"], true);
    let msg = by_name["content"].as_str().unwrap();
    assert!(
        msg.contains("does not resolve to a public address"),
        "{msg}"
    );
    assert!(app.factory.sandbox(0).files.lock().unwrap().is_empty());
}

#[tokio::test]
async fn downloads_land_in_the_workspace() {
    let base = start().await;
    let app = run_calls(
        vec![
            (
                "download_file",
                json!({ "url": format!("{base}/report.pdf"), "path": "in/report.pdf" }),
            ),
            (
                "download_file",
                json!({ "url": format!("{base}/huge.bin"), "path": "huge.bin" }),
            ),
            (
                "download_file",
                json!({ "url": format!("{base}/missing"), "path": "m.txt" }),
            ),
            ("download_file", json!({ "url": format!("{base}/page") })),
        ],
        PRIVATE_OK,
    )
    .await;

    let ok = tool_result(&app, 1);
    assert!(ok.get("is_error").is_none(), "{ok}");
    assert_eq!(
        ok["content"],
        format!("[1] {base}/report.pdf\nsaved {base}/report.pdf to in/report.pdf: 13 bytes, application/pdf")
    );
    let huge = tool_result(&app, 2);
    assert_eq!(huge["is_error"], true);
    assert!(huge["content"]
        .as_str()
        .unwrap()
        .contains("larger than the 25 MB download limit"));
    assert_eq!(tool_result(&app, 3)["is_error"], true);
    assert_eq!(tool_result(&app, 4)["content"], "missing path");

    let files = app.factory.sandbox(0).files.lock().unwrap().clone();
    assert_eq!(files.len(), 1, "{:?}", files.keys());
    assert_eq!(files["in/report.pdf"], b"%PDF-1.7 fake");
}
