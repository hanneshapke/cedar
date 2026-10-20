//! `bash`, `run_python`, `read_document`, and `generate_chart` against the fake sandbox: what gets staged,
//! what gets run, and which input errors the model hears about. Real execution is covered by
//! `tests/docker.rs`.

mod common;

use cedar::tools::{Bash, Tool, ToolContext};
use common::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// The tool result at the end of the messages sent in the model's `request`-th call.
fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

async fn run_calls(calls: Vec<(&str, Value)>) -> TestApp {
    let mut script: Vec<Step> = calls
        .into_iter()
        .enumerate()
        .map(|(i, (name, input))| Step::Reply(tool_use(&format!("t{i}"), name, input)))
        .collect();
    script.push(Step::Reply(text("done")));
    let app = TestApp::new(script).await;
    let created = app.create_run(KEY_A, "work").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    app
}

/// The single-quoted `<prefix>…<suffix>` path in `cmd`.
fn staged_path(cmd: &str, prefix: &str, suffix: &str) -> String {
    cmd.split('\'')
        .find(|s| s.starts_with(prefix) && s.ends_with(suffix))
        .unwrap_or_else(|| panic!("no {prefix} path in {cmd}"))
        .to_string()
}

#[tokio::test]
async fn bash_runs_the_command_as_given_and_lists_the_files_it_changed() {
    let sandbox = Arc::new(FakeSandbox::default());
    *sandbox.changed.lock().unwrap() = vec!["out/chart.png".into(), "notes.txt".into()];
    let ctx = ToolContext {
        sandbox: sandbox.clone(),
        browser: None,
        timeout: Duration::from_secs(5),
        max_output: 1000,
        model: None,
    };

    let msg = Bash
        .call(&ctx, &json!({ "command": "python3 fig.py && ls *.png" }))
        .await
        .unwrap();
    assert_eq!(
        msg,
        "ran: python3 fig.py && ls *.png\n[files created or modified]\n- out/chart.png\n- notes.txt"
    );

    // A marker is touched, the command is run untouched, then the files newer than the marker listed.
    let commands = sandbox.commands.lock().unwrap().clone();
    assert_eq!(commands.len(), 3, "{commands:?}");
    let marker = staged_path(&commands[0], "/tmp/cedar-bash-", ".mark");
    assert_eq!(commands[0], format!("touch '{marker}'"));
    assert_eq!(commands[1], "python3 fig.py && ls *.png");
    assert!(
        commands[2].contains(&format!("-newer '{marker}' -print")),
        "{}",
        commands[2]
    );

    // Nothing written: the result is just what the command printed.
    sandbox.changed.lock().unwrap().clear();
    let msg = Bash.call(&ctx, &json!({ "command": "ls" })).await.unwrap();
    assert_eq!(msg, "ran: ls");

    // A missing command is reported before anything runs.
    let before = sandbox.commands.lock().unwrap().len();
    assert!(Bash.call(&ctx, &json!({})).await.is_err());
    assert_eq!(sandbox.commands.lock().unwrap().len(), before);
}

#[tokio::test]
async fn run_python_stages_the_script_runs_it_and_lists_changes() {
    let code = "print('it\\'s \"fine\"')";
    let app = run_calls(vec![(
        "run_python",
        json!({ "code": code, "args": ["x y"] }),
    )])
    .await;

    let sandbox = app.factory.sandbox(0);
    let commands = sandbox.commands.lock().unwrap().clone();
    assert_eq!(commands.len(), 2, "{commands:?}");
    let script = staged_path(&commands[0], "/tmp/cedar-python-", ".py");
    let marker = script.replace(".py", ".mark");
    assert!(
        commands[0].starts_with(&format!("touch '{marker}' && PYTHONPATH=")),
        "{}",
        commands[0]
    );
    assert!(
        commands[0].contains(&format!(
            "python3 -- '{script}' 'x y'; s=$?; rm -f -- '{script}'"
        )),
        "{}",
        commands[0]
    );
    assert!(
        commands[1].contains(&format!("-newer '{marker}' -print")),
        "{}",
        commands[1]
    );
    assert_eq!(sandbox.files.lock().unwrap()[&script], code.as_bytes());

    let result = tool_result(&app, 1);
    assert!(result.get("is_error").is_none(), "{result}");
    assert!(result["content"]
        .as_str()
        .unwrap()
        .starts_with("ran: touch "));
}

#[tokio::test]
async fn read_document_runs_the_extractor_with_quoted_arguments() {
    let app = run_calls(vec![
        (
            "read_document",
            json!({ "path": "Q3 report.xlsx", "sheet": "Sales", "rows": "1-20" }),
        ),
        ("read_document", json!({ "path": "a.pdf", "pages": "zero" })),
    ])
    .await;
    let commands = app.factory.sandbox(0).commands.lock().unwrap().clone();
    assert_eq!(
        commands,
        ["cedar-read-document --sheet 'Sales' --rows '1-20' -- 'Q3 report.xlsx'"]
    );
    let bad = tool_result(&app, 2);
    assert_eq!(bad["is_error"], true);
    assert!(bad["content"]
        .as_str()
        .unwrap()
        .starts_with("invalid pages \"zero\""));
}

#[tokio::test]
async fn generate_chart_stages_a_checked_spec() {
    let spec = json!({ "path": "c.png", "kind": "line", "labels": ["a", "b"],
                       "series": [{ "name": "s", "values": [1, 2] }], "title": "T" });
    let app = run_calls(vec![
        ("generate_chart", spec.clone()),
        (
            "generate_chart",
            json!({ "path": "c.png", "kind": "line", "labels": ["a"],
                                   "series": [{ "values": [1, 2] }] }),
        ),
    ])
    .await;

    let sandbox = app.factory.sandbox(0);
    let commands = sandbox.commands.lock().unwrap().clone();
    assert_eq!(commands.len(), 1, "{commands:?}");
    let staged = staged_path(&commands[0], "/tmp/cedar-render-chart-", ".json");
    assert_eq!(
        commands[0],
        format!("cedar-render-chart -- '{staged}'; s=$?; rm -f -- '{staged}'; exit $s")
    );
    let written: Value = serde_json::from_slice(&sandbox.files.lock().unwrap()[&staged]).unwrap();
    assert_eq!(written, spec);
    assert_eq!(tool_result(&app, 1)["content"], "wrote c.png");

    let bad = tool_result(&app, 2);
    assert_eq!(bad["is_error"], true);
    assert_eq!(
        bad["content"],
        "series 1 has 2 values but there are 1 labels"
    );
}
