//! `ask_user`: a run that asks the user something stops with the questions, and the answer starts
//! the next run of the conversation.

mod common;

use common::*;
use serde_json::{json, Value};

fn questions() -> Value {
    json!({ "questions": [{
        "question": "Which format should the report be in?",
        "header": "Format",
        "options": [{ "label": "PDF", "description": "A printable file" }, "Markdown", "Other"],
    }] })
}

#[tokio::test]
async fn asking_ends_the_run_and_the_answer_continues_it() {
    let app = TestApp::new(vec![
        Step::Reply(reply(
            vec![
                json!({ "type": "text", "text": "A quick question first." }),
                json!({ "type": "tool_use", "id": "q1", "name": "ask_user", "input": questions() }),
            ],
            "tool_use",
        )),
        Step::Reply(text("Here is the PDF.")),
    ])
    .await;
    let first = app.create_run(KEY_A, "write me a report").await;
    let first_id = first["id"].as_str().unwrap().to_string();
    let run = app.wait_status(KEY_A, &first_id, "completed").await;

    // One model call: the run stopped as soon as the question was asked.
    assert_eq!(app.llm.call_count(), 1);
    assert_eq!(run["stop_reason"], "ask_user");
    assert_eq!(
        run["final_text"],
        "A quick question first.\n\nWhich format should the report be in?\n   - PDF: A printable file\n   - Markdown"
    );

    // The tool result is what the chat draws the form from: tidied questions, "Other" left out.
    let events = app.events(KEY_A, &first_id, None).await;
    let result = events
        .iter()
        .find(|e| e.data["type"] == "tool.result")
        .expect("a tool.result event");
    assert_eq!(result.data["data"]["is_error"], false);
    let output: Value =
        serde_json::from_str(result.data["data"]["output"].as_str().unwrap()).unwrap();
    assert_eq!(
        output["questions"],
        json!([{ "question": "Which format should the report be in?", "header": "Format",
                 "multi_select": false,
                 "options": [{ "label": "PDF", "description": "A printable file" }, { "label": "Markdown" }] }])
    );
    let done = events.last().unwrap();
    assert_eq!(done.data["type"], "run.completed");
    assert_eq!(done.data["data"]["stop_reason"], "ask_user");

    // The answer is the next run's prompt; the model sees the question's result, then the answer.
    let second = app
        .create_run_with(KEY_A, json!({ "prompt": "PDF", "parent_id": first_id }))
        .await;
    let run = app
        .wait_status(KEY_A, second["id"].as_str().unwrap(), "completed")
        .await;
    assert_eq!(run["final_text"], "Here is the PDF.");
    let requests = app.llm.requests();
    let msgs = requests[1]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
    assert_eq!(msgs[2]["content"][0]["tool_use_id"], "q1");
    assert!(msgs[2]["content"][0].get("is_error").is_none());
    assert_eq!(msgs[3], json!({ "role": "user", "content": "PDF" }));
}

#[tokio::test]
async fn a_bad_question_goes_back_to_the_model() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "q1",
            "ask_user",
            json!({ "questions": [{ "question": "Which?", "options": ["only one"] }] }),
        )),
        Step::Reply(text("I'll assume the default.")),
    ])
    .await;
    let run = app.create_run(KEY_A, "go").await;
    let run = app
        .wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    // The call failed, so the run went on instead of waiting for an answer.
    assert_eq!(app.llm.call_count(), 2);
    assert_eq!(run["stop_reason"], "end_turn");
    let requests = app.llm.requests();
    let result = &requests[1]["messages"].as_array().unwrap().last().unwrap()["content"][0];
    assert_eq!(result["is_error"], true);
    assert!(result["content"]
        .as_str()
        .unwrap()
        .contains("question 1: give at least 2 options"));
}

#[tokio::test]
async fn the_tool_can_be_turned_off() {
    let app = TestApp::with_env(vec![Step::Reply(text("ok"))], &[("ASK_USER", "off")]).await;
    let run = app.create_run(KEY_A, "go").await;
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    let request = &app.llm.requests()[0];
    let tools: Vec<&str> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(!tools.contains(&"ask_user"), "{tools:?}");
    assert!(!request["system"].as_str().unwrap().contains("ask_user"));
}
