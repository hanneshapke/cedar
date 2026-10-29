//! The `generate_pdf` tool against the fake sandbox: what gets staged, what gets run, and what the
//! model hears back. Real rendering is covered by `tests/docker.rs`.

mod common;

use common::*;
use serde_json::{json, Value};

/// The tool result at the end of the messages sent in the model's `request`-th call.
fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

async fn run(steps: Vec<Step>) -> TestApp {
    let app = TestApp::new(steps).await;
    let created = app.create_run(KEY_A, "make a pdf").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    app
}

#[tokio::test]
async fn the_document_is_staged_rendered_and_reported() {
    let app = run(vec![
        Step::Reply(tool_use(
            "t1",
            "generate_pdf",
            json!({ "path": "out/report.pdf", "content": "# Hello\n\nworld", "title": "Q3 'report'" }),
        )),
        Step::Reply(text("done")),
    ])
    .await;

    let names: Vec<_> = app.llm.requests()[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"generate_pdf".to_string()), "{names:?}");

    let sandbox = app.factory.sandbox(0);
    let commands = sandbox.commands.lock().unwrap().clone();
    assert_eq!(commands.len(), 1);
    let cmd = &commands[0];
    assert!(cmd.starts_with("cedar-render-pdf --format markdown --title 'Q3 '\\''report'\\''' -- '/tmp/cedar-render-pdf-"), "{cmd}");
    assert!(
        cmd.contains(".md' 'out/report.pdf'; s=$?; rm -f -- '/tmp/cedar-render-pdf-"),
        "{cmd}"
    );
    assert!(cmd.ends_with("; exit $s"), "{cmd}");

    // The Markdown was written to the staging path named in the command.
    let staged = cmd
        .split('\'')
        .find(|s| s.starts_with("/tmp/cedar-render-pdf-"))
        .unwrap()
        .to_string();
    let files = sandbox.files.lock().unwrap();
    assert_eq!(files[&staged], b"# Hello\n\nworld");

    // The fake sandbox doesn't print the renderer's statistics, so the summary is just the path.
    let result = tool_result(&app, 1);
    assert_eq!(result["tool_use_id"], "t1");
    assert_eq!(result["content"], "wrote out/report.pdf");
    assert!(result.get("is_error").is_none());
}

#[tokio::test]
async fn html_input_is_staged_with_an_html_extension() {
    let app = run(vec![
        Step::Reply(tool_use(
            "t1",
            "generate_pdf",
            json!({ "path": "page.pdf", "content": "<h1>Hi</h1>", "format": "html" }),
        )),
        Step::Reply(text("done")),
    ])
    .await;
    let cmd = app.factory.sandbox(0).commands.lock().unwrap()[0].clone();
    assert!(
        cmd.starts_with("cedar-render-pdf --format html -- '/tmp/cedar-render-pdf-"),
        "{cmd}"
    );
    assert!(cmd.contains(".html' 'page.pdf';"), "{cmd}");
}

#[tokio::test]
async fn bad_input_is_an_error_the_model_can_fix() {
    let app = run(vec![
        Step::Reply(tool_use(
            "t1",
            "generate_pdf",
            json!({ "path": "report.txt", "content": "# Hi" }),
        )),
        Step::Reply(tool_use(
            "t2",
            "generate_pdf",
            json!({ "path": "report.pdf", "content": "# Hi", "format": "latex" }),
        )),
        Step::Reply(tool_use(
            "t3",
            "generate_pdf",
            json!({ "path": "report.pdf" }),
        )),
        Step::Reply(text("done")),
    ])
    .await;

    let first = tool_result(&app, 1);
    assert_eq!(first["is_error"], true);
    assert_eq!(first["content"], "path must end in .pdf");
    let second = tool_result(&app, 2);
    assert_eq!(second["is_error"], true);
    assert_eq!(
        second["content"],
        "invalid format \"latex\"; use markdown or html"
    );
    let third = tool_result(&app, 3);
    assert_eq!(third["is_error"], true);
    assert_eq!(third["content"], "missing content");

    // Nothing was staged or run for the rejected calls.
    let sandbox = app.factory.sandbox(0);
    assert!(sandbox.commands.lock().unwrap().is_empty());
    assert!(sandbox.files.lock().unwrap().is_empty());
}
