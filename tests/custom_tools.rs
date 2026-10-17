//! Personal Python tools from `TOOLS_DIR`: how they reach the model, how a call runs in the
//! sandbox, and how the `custom` group behaves in the catalog and a run's tool selection.
//! Real execution in Docker is covered by `tests/docker.rs`.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::PathBuf;
use uuid::Uuid;

/// A fresh `TOOLS_DIR` holding `word_count` (the sample tool) and a broken folder.
fn tools_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cedar-custom-{}", Uuid::new_v4()));
    let write = |rel: &str, text: &str| {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write(
        "word_count/tool.json",
        r#"{ "description": "Count the words of a file.",
             "input_schema": { "type": "object",
                               "properties": { "path": { "type": "string" } },
                               "required": ["path"] },
             "timeout_secs": 7 }"#,
    );
    write("word_count/tool.py", "print('counted')");
    write("broken/tool.json", "not json");
    write("broken/tool.py", "");
    dir
}

fn tool_names(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_personal_tool_is_offered_and_runs_its_script_on_the_staged_input() {
    let dir = tools_dir();
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "word_count", json!({ "path": "notes.txt" }))),
            Step::Reply(text("done")),
        ],
        &[("TOOLS_DIR", dir.to_str().unwrap())],
    )
    .await;
    let run = app.create_run(KEY_A, "count").await;
    let id = run["id"].as_str().unwrap();
    app.wait_status(KEY_A, id, "completed").await;

    let first = &app.llm.requests()[0];
    let names = tool_names(first);
    assert!(names.contains(&"word_count".to_string()), "{names:?}");
    assert!(!names.contains(&"broken".to_string()), "{names:?}");
    let def = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "word_count")
        .unwrap();
    assert_eq!(def["description"], "Count the words of a file.");
    assert_eq!(def["input_schema"]["required"], json!(["path"]));

    let sandbox = app.factory.sandbox(0);
    let commands = sandbox.commands.lock().unwrap().clone();
    let cmd = commands
        .iter()
        .find(|c| c.contains("/opt/cedar/tools/word_count/tool.py"))
        .unwrap_or_else(|| panic!("the script was not run: {commands:?}"));
    assert!(cmd.contains("python3 -- '/opt/cedar/tools/word_count/tool.py' < '/tmp/cedar-tool-"));
    let staged = cmd
        .split('\'')
        .find(|s| s.starts_with("/tmp/cedar-tool-") && s.ends_with(".json"))
        .unwrap()
        .to_string();
    let files = sandbox.files.lock().unwrap();
    let input: Value = serde_json::from_slice(&files[&staged]).unwrap();
    assert_eq!(input, json!({ "path": "notes.txt" }));

    let requests = app.llm.requests();
    let result = &requests[1]["messages"].as_array().unwrap().last().unwrap()["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["content"], format!("ran: {cmd}"));
    assert!(result["is_error"].is_null() || result["is_error"] == false);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn the_custom_group_is_in_the_catalog_and_can_be_left_out_of_a_run() {
    let dir = tools_dir();
    let app = TestApp::with_env(
        vec![Step::Reply(text("done")), Step::Reply(text("done"))],
        &[("TOOLS_DIR", dir.to_str().unwrap())],
    )
    .await;
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    let group = catalog["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "custom")
        .expect("a custom group");
    assert_eq!(group["label"], "Personal tools");
    assert_eq!(group["available"], true);
    assert_eq!(group["tools"][0]["name"], "word_count");
    assert_eq!(group["tools"][0]["writes"], false);

    // A run that only picks the weather group doesn't see the personal tools.
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "x", "tools": ["weather"] }))
        .await;
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    assert!(!tool_names(&app.llm.requests()[0]).contains(&"word_count".to_string()));

    // Naming the group, or the tool, brings them in.
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "x", "tools": ["custom"] }))
        .await;
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    assert!(tool_names(&app.llm.requests()[1]).contains(&"word_count".to_string()));
    let (status, _) = app
        .json(
            axum::http::Method::POST,
            "/v1/runs",
            Some(KEY_A),
            Some(json!({ "prompt": "x", "tools": ["word_count"] })),
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn without_the_directory_there_is_no_custom_group_to_offer() {
    let app = TestApp::new(vec![]).await;
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    let group = catalog["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "custom")
        .expect("the group is listed even when empty");
    assert_eq!(group["available"], false);
    assert_eq!(group["reason"], "No TOOLS_DIR is set");
}
