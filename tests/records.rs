//! Records: the data runs keep for the API key's owner across conversations, the `records_*`
//! tools that save, read and drop it, the note that tells a run which sets it keeps, and
//! `/v1/records`, where people see, upload and delete it.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};

/// The tool results of a run, in order: `(output, is_error)`.
async fn tool_results(app: &TestApp, key: &str, id: &str) -> Vec<(String, bool)> {
    app.events(key, id, None)
        .await
        .iter()
        .filter(|e| e.data["type"] == "tool.result")
        .map(|e| {
            let d = &e.data["data"];
            (
                d["output"].as_str().unwrap_or_default().to_string(),
                d["is_error"] == true,
            )
        })
        .collect()
}

/// Run `prompt` to completion and return the run.
async fn run(app: &TestApp, key: &str, body: Value) -> Value {
    let run = app.create_run_with(key, body).await;
    app.wait_status(key, run["id"].as_str().unwrap(), "completed")
        .await
}

async fn sets(app: &TestApp, key: &str) -> Vec<Value> {
    let (status, body) = app.get("/v1/records", key).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["sets"].as_array().unwrap().clone()
}

async fn upload(app: &TestApp, key: &str, set: &str, body: Value) -> (StatusCode, Value) {
    app.json(
        Method::POST,
        &format!("/v1/records/{set}"),
        Some(key),
        Some(body),
    )
    .await
}

fn system(app: &TestApp, request: usize) -> String {
    app.llm.requests()[request]["system"]
        .as_str()
        .unwrap()
        .to_string()
}

fn tool_names(app: &TestApp, request: usize) -> Vec<String> {
    app.llm.requests()[request]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn week(date: &str, km: f64, minutes: i64) -> Value {
    json!({ "date": date, "km": km, "minutes": minutes })
}

#[tokio::test]
async fn records_saved_in_one_conversation_are_queried_in_the_next() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "r1",
            "records_save",
            json!({
                "set": "running_log",
                "description": "One row per week from the user's running export",
                "key": ["date"],
                "records": [week("2026-10-26", 31.2, 170), week("2026-11-02", 34.0, 181)],
            }),
        )),
        Step::Reply(text("Saved two weeks.")),
        // Next conversation: the same week again (updated, not doubled), then a comparison.
        Step::Reply(tool_use(
            "r2",
            "records_save",
            json!({ "set": "running_log", "records": [week("2026-11-02", 35.5, 185)] }),
        )),
        Step::Reply(tool_use("r3", "records_describe", json!({ "set": "running_log" }))),
        Step::Reply(tool_use(
            "r4",
            "records_lookup",
            json!({ "sql": "SELECT date, km, round(km - lag(km) OVER (ORDER BY date), 1) AS change FROM running_log ORDER BY date" }),
        )),
        Step::Reply(text("You ran 4.3 km more than last week.")),
    ])
    .await;

    let first = run(
        &app,
        KEY_A,
        json!({ "prompt": "here is this week's running export", "interactive": true }),
    )
    .await;
    let names = tool_names(&app, 0);
    for name in [
        "records_list",
        "records_describe",
        "records_save",
        "records_lookup",
        "records_drop",
    ] {
        assert!(names.contains(&name.to_string()), "{names:?}");
    }
    let sys = system(&app, 0);
    assert!(
        sys.contains("You keep no records for this user yet."),
        "{sys}"
    );
    assert!(sys.contains("Use records_save"), "{sys}");
    // The memory note stays last in the prompt.
    assert!(sys.rfind("You don't remember anything").unwrap() > sys.rfind("records_drop").unwrap());

    let results = tool_results(&app, KEY_A, first["id"].as_str().unwrap()).await;
    assert_eq!(results.len(), 1);
    assert!(!results[0].1, "{}", results[0].0);
    let saved: Value = serde_json::from_str(&results[0].0).unwrap();
    assert_eq!(
        saved,
        json!({ "set": "running_log", "added": 2, "updated": 0, "count": 2, "key": ["date"] })
    );

    let listed = sets(&app, KEY_A).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["name"], "running_log");
    assert_eq!(listed[0]["count"], 2);
    assert_eq!(listed[0]["columns"], json!(["date", "km", "minutes"]));
    assert_eq!(listed[0]["key_fields"], json!(["date"]));

    // A new conversation, not a continuation: the set is in its system prompt, by name and size.
    let second = run(
        &app,
        KEY_A,
        json!({ "prompt": "how did this week compare?", "interactive": true }),
    )
    .await;
    let sys = system(&app, 2);
    assert!(sys.contains("Records you keep for this user"), "{sys}");
    assert!(
        sys.contains("- running_log: 2 records (date, km, minutes), last saved "),
        "{sys}"
    );
    assert!(sys.contains("One row per week"), "{sys}");

    let results = tool_results(&app, KEY_A, second["id"].as_str().unwrap()).await;
    assert_eq!(results.len(), 3);
    let saved: Value = serde_json::from_str(&results[0].0).unwrap();
    assert_eq!(
        (&saved["added"], &saved["updated"], &saved["count"]),
        (&json!(0), &json!(1), &json!(2))
    );
    let described: Value = serde_json::from_str(&results[1].0).unwrap();
    assert_eq!(described["set"]["name"], "running_log");
    assert_eq!(described["columns"][1]["name"], "km");
    assert_eq!(described["columns"][1]["type"], "number");
    assert_eq!(described["first"].as_array().unwrap().len(), 2);
    assert!(!results[2].1, "{}", results[2].0);
    let found: Value = serde_json::from_str(&results[2].0).unwrap();
    assert_eq!(found["columns"], json!(["date", "km", "change"]));
    assert_eq!(
        found["rows"],
        json!([["2026-10-26", 31.2, null], ["2026-11-02", 35.5, 4.3]])
    );
    assert_eq!(found["truncated"], false);

    // The API shows the records with the run that saved them last; deleting that run keeps them.
    let (status, body) = app.get("/v1/records/running_log", KEY_A).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let records = body["records"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["run_id"], first["id"]);
    assert_eq!(records[1]["run_id"], second["id"]);
    assert_eq!(records[1]["data"]["km"], 35.5);
    let (status, _) = app
        .json(
            Method::DELETE,
            &format!("/v1/runs/{}", second["id"].as_str().unwrap()),
            Some(KEY_A),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = app.get("/v1/records/running_log", KEY_A).await;
    assert_eq!(body["records"][1]["run_id"], Value::Null);
    assert_eq!(body["records"][1]["data"]["km"], 35.5);

    // Another key sees nothing of it.
    assert!(sets(&app, KEY_B).await.is_empty());
    let (status, _) = app.get("/v1/records/running_log", KEY_B).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn uploads_by_hand_and_deletes_through_the_api() {
    let app = TestApp::new(vec![]).await;
    let (status, body) = upload(
        &app,
        KEY_A,
        "weight",
        json!({ "records": [{ "date": "2026-11-02", "kg": 72.4 }], "description": "Morning weight", "key": ["date"], "health_data": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["added"], 1);
    assert!(body["set"]["health_data"].as_bool().unwrap());
    let (status, body) = upload(
        &app,
        KEY_A,
        "weight",
        json!({ "records": [{ "date": "2026-11-02", "kg": 72.0 }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((&body["added"], &body["updated"]), (&json!(0), &json!(1)));

    // Refusals say why.
    let (status, body) = upload(
        &app,
        KEY_A,
        "BadName",
        json!({ "records": [{ "a": 1 }], "description": "d" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a valid name"),
        "{body}"
    );
    let (status, body) = upload(&app, KEY_A, "weight", json!({ "records": [{ "kg": 1 }] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("key field"),
        "{body}"
    );
    let (status, body) = upload(&app, KEY_A, "notes", json!({ "records": [{ "a": 1 }] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("description"),
        "{body}"
    );

    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(defaults["records"]["max_sets"], 50);
    assert_eq!(defaults["records"]["max_items"], 10_000);

    let (status, _) = app
        .json(Method::DELETE, "/v1/records/weight", Some(KEY_B), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .json(Method::DELETE, "/v1/records/weight", Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .json(Method::DELETE, "/v1/records/weight", Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    upload(
        &app,
        KEY_A,
        "a",
        json!({ "records": [{ "x": 1 }], "description": "d" }),
    )
    .await;
    upload(
        &app,
        KEY_A,
        "b",
        json!({ "records": [{ "x": 1 }], "description": "d" }),
    )
    .await;
    let (status, _) = app
        .json(Method::DELETE, "/v1/records", Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(sets(&app, KEY_A).await.is_empty());
}

#[tokio::test]
async fn unattended_runs_save_and_read_but_cannot_drop() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "r1",
            "records_save",
            json!({ "set": "glucose", "description": "Daily glucose", "key": ["date"], "health_data": true,
                    "records": [{ "date": "2026-11-05", "avg": 6.1 }] }),
        )),
        Step::Reply(tool_use("r2", "records_drop", json!({ "set": "glucose" }))),
        Step::Reply(text("ok")),
    ])
    .await;
    let done = run(&app, KEY_A, json!({ "prompt": "weekly glucose check" })).await;
    let sys = system(&app, 0);
    assert!(sys.contains("records_drop is refused in this run"), "{sys}");
    let results = tool_results(&app, KEY_A, done["id"].as_str().unwrap()).await;
    assert!(!results[0].1, "{}", results[0].0);
    assert!(results[1].1);
    assert!(
        results[1].0.contains("refused in unattended runs"),
        "{}",
        results[1].0
    );
    assert_eq!(sets(&app, KEY_A).await.len(), 1);

    // The next run, attended, gets the health rules for the set, and may drop it.
    let app = TestApp::build(
        app.state.store.clone(),
        ScriptedLlm::new(vec![
            Step::Reply(tool_use("r3", "records_drop", json!({ "set": "glucose" }))),
            Step::Reply(text("Gone.")),
        ]),
        FakeFactory::new(),
        &[],
    )
    .await;
    let done = run(
        &app,
        KEY_A,
        json!({ "prompt": "drop my glucose records", "interactive": true }),
    )
    .await;
    let sys = system(&app, 0);
    assert!(
        sys.contains("The glucose record set holds personal health data"),
        "{sys}"
    );
    let results = tool_results(&app, KEY_A, done["id"].as_str().unwrap()).await;
    assert!(!results[0].1, "{}", results[0].0);
    assert!(sets(&app, KEY_A).await.is_empty());
}

#[tokio::test]
async fn a_lookup_that_writes_or_strays_is_refused() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "r1",
            "records_lookup",
            json!({ "sql": "DELETE FROM running_log" }),
        )),
        Step::Reply(tool_use(
            "r2",
            "records_lookup",
            json!({ "sql": "SELECT * FROM nothing" }),
        )),
        Step::Reply(text("ok")),
    ])
    .await;
    upload(
        &app,
        KEY_A,
        "running_log",
        json!({ "records": [week("2026-11-02", 1.0, 1)], "description": "d", "key": ["date"] }),
    )
    .await;
    let done = run(&app, KEY_A, json!({ "prompt": "x", "interactive": true })).await;
    let results = tool_results(&app, KEY_A, done["id"].as_str().unwrap()).await;
    assert!(results[0].1);
    assert!(
        results[0].0.contains("only a single SELECT"),
        "{}",
        results[0].0
    );
    assert!(results[1].1);
    assert!(
        results[1].0.contains("the tables are running_log"),
        "{}",
        results[1].0
    );
    assert_eq!(sets(&app, KEY_A).await[0]["count"], 1);
}

#[tokio::test]
async fn without_the_records_group_nothing_is_offered_or_listed() {
    let app = TestApp::new(vec![Step::Reply(text("ok"))]).await;
    upload(
        &app,
        KEY_A,
        "running_log",
        json!({ "records": [week("2026-11-02", 1.0, 1)], "description": "d" }),
    )
    .await;
    run(
        &app,
        KEY_A,
        json!({ "prompt": "hi", "interactive": true, "tools": ["memory"] }),
    )
    .await;
    let names = tool_names(&app, 0);
    assert!(
        !names.iter().any(|n| n.starts_with("records_")),
        "{names:?}"
    );
    let sys = system(&app, 0);
    assert!(!sys.contains("running_log"), "{sys}");
    assert!(!sys.contains("records_save"), "{sys}");
}

#[tokio::test]
async fn records_off_removes_the_tools_the_note_and_uploads() {
    let app = TestApp::with_env(vec![Step::Reply(text("ok"))], &[("RECORDS", "off")]).await;
    let (status, body) = upload(
        &app,
        KEY_A,
        "x",
        json!({ "records": [{ "a": 1 }], "description": "d" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    run(&app, KEY_A, json!({ "prompt": "hi", "interactive": true })).await;
    assert!(!tool_names(&app, 0)
        .iter()
        .any(|n| n.starts_with("records_")));
    assert!(!system(&app, 0).contains("records_save"));
    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(defaults["records"], Value::Null);
    let (_, tools) = app.get("/v1/tools", KEY_A).await;
    let group = tools["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "records")
        .unwrap();
    assert_eq!(group["available"], false);
    assert_eq!(group["reason"], "Turned off on the server (RECORDS=off)");
    // What was saved before can still be listed and deleted.
    assert!(sets(&app, KEY_A).await.is_empty());
    let (status, _) = app
        .json(Method::DELETE, "/v1/records", Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// The events of kind `kind` in a run, in order.
async fn events_of(app: &TestApp, key: &str, id: &str, kind: &str) -> Vec<Value> {
    app.events(key, id, None)
        .await
        .iter()
        .filter(|e| e.data["type"] == kind)
        .map(|e| e.data["data"].clone())
        .collect()
}

fn csv(name: &str, text: &str) -> Value {
    use base64::Engine;
    json!({ "name": name, "media_type": "text/csv", "data": base64::engine::general_purpose::STANDARD.encode(text) })
}

#[tokio::test]
async fn csv_and_json_attachments_are_imported_into_sets_unless_the_run_says_otherwise() {
    use base64::Engine;
    let app = TestApp::new(vec![Step::Repeat(text("ok"))]).await;
    let runs = csv("Running Log.csv", "Date,km,minutes\n2026-11-01,5.2,31\n2026-11-02,10,58\n");
    let weights = json!({ "name": "weights.json", "media_type": "application/json",
        "data": base64::engine::general_purpose::STANDARD.encode("[{\"date\": \"2026-11-01\", \"kg\": 71.5}]") });
    let first = run(&app, KEY_A, json!({ "prompt": "Here are my runs and weights", "attachments": [runs.clone(), weights] })).await;
    assert_eq!(first["import_attachments"], true);
    let imported = events_of(&app, KEY_A, first["id"].as_str().unwrap(), "records.updated").await;
    assert_eq!(imported.len(), 1);
    let done = imported[0]["imported"].as_array().unwrap();
    assert_eq!(done.len(), 2);
    assert_eq!((done[0]["file"].as_str(), done[0]["set"].as_str(), done[0]["added"].as_i64()), (Some("Running Log.csv"), Some("running_log"), Some(2)));
    assert_eq!(done[0]["key"], json!(["date"]));
    assert_eq!((done[1]["set"].as_str(), done[1]["added"].as_i64()), (Some("weights"), Some(1)));
    // The model is told where the data went.
    let opening = app.llm.requests()[0]["messages"][0]["content"].to_string();
    assert!(opening.contains("attachments/Running Log.csv: CSV file; its 2 rows were imported into the record set running_log (2 added, 0 updated; key date)"), "{opening}");
    assert!(opening.contains("weights.json: JSON file; its 1 rows were imported"), "{opening}");
    // The sets are there, typed, with the file as their description.
    let all = sets(&app, KEY_A).await;
    assert_eq!(all.iter().map(|s| s["name"].as_str().unwrap()).collect::<Vec<_>>(), ["running_log", "weights"]);
    assert_eq!(all[0]["description"], "Imported from Running Log.csv");
    assert_eq!(all[0]["columns"], json!(["date", "km", "minutes"]));
    let (_, rows) = app.get("/v1/records/running_log", KEY_A).await;
    assert_eq!(rows["records"][1]["data"], json!({ "date": "2026-11-02", "km": 10, "minutes": 58 }));
    assert_eq!(rows["records"][0]["run_id"], first["id"]);

    // The same file again updates rather than doubles, and a corrected row wins.
    let again = csv("running log.csv", "Date,km,minutes\n2026-11-02,10.5,58\n2026-11-03,3,20\n");
    let second = run(&app, KEY_A, json!({ "prompt": "corrected", "attachments": [again] })).await;
    let imported = events_of(&app, KEY_A, second["id"].as_str().unwrap(), "records.updated").await;
    assert_eq!((imported[0]["imported"][0]["added"].as_i64(), imported[0]["imported"][0]["updated"].as_i64()), (Some(1), Some(1)));
    let (_, rows) = app.get("/v1/records/running_log", KEY_A).await;
    assert_eq!(rows["records"].as_array().unwrap().len(), 3);

    // Opted out: saved in the workspace only, and the note says so.
    let third = run(&app, KEY_A, json!({ "prompt": "just look", "attachments": [runs.clone()], "import_attachments": false })).await;
    assert_eq!(third["import_attachments"], false);
    assert!(events_of(&app, KEY_A, third["id"].as_str().unwrap(), "records.updated").await.is_empty());
    let opening = app.llm.requests()[2]["messages"][0]["content"].to_string();
    assert!(opening.contains("Running Log.csv: CSV file; read it with read_file or run_python"), "{opening}");

    // A file with nothing in it is listed with the reason.
    let empty = csv("empty.csv", "a,b\n");
    let fourth = run(&app, KEY_A, json!({ "prompt": "empty", "attachments": [empty] })).await;
    assert!(events_of(&app, KEY_A, fourth["id"].as_str().unwrap(), "records.updated").await.is_empty());
    let opening = app.llm.requests()[3]["messages"][0]["content"].to_string();
    assert!(opening.contains("not imported into records (the CSV has a header but no rows)"), "{opening}");
    assert_eq!(sets(&app, KEY_A).await.len(), 2);

    // Without the records group, nothing is imported; with RECORDS_IMPORT_ATTACHMENTS=off the
    // default flips, and the request can still ask.
    let fifth = run(&app, KEY_A, json!({ "prompt": "x", "attachments": [runs.clone()], "tools": ["memory"] })).await;
    assert!(events_of(&app, KEY_A, fifth["id"].as_str().unwrap(), "records.updated").await.is_empty());
    let app = TestApp::with_env(vec![Step::Repeat(text("ok"))], &[("RECORDS_IMPORT_ATTACHMENTS", "off")]).await;
    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(defaults["records"]["import_attachments"], false);
    assert!(defaults["attachments"]["media_types"].as_array().unwrap().iter().any(|t| t == "text/csv"));
    let off = run(&app, KEY_A, json!({ "prompt": "x", "attachments": [runs.clone()] })).await;
    assert_eq!(off["import_attachments"], false);
    let on = run(&app, KEY_A, json!({ "prompt": "x", "attachments": [runs], "import_attachments": true })).await;
    assert_eq!(on["import_attachments"], true);
    assert_eq!(sets(&app, KEY_A).await.len(), 1);
}

#[tokio::test]
async fn the_memory_call_saves_the_data_the_user_handed_over() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(text("Logged.")),
            // The memory call proposes rows: two the user stated, one it made up, and one into
            // a set that already exists.
            Step::Reply(text(
                "{\"remember\": [], \"records\": [\
                 {\"set\": \"weight_log\", \"description\": \"Morning weights\", \"key\": [\"date\"], \"rows\": [\
                 {\"date\": \"2026-11-05\", \"kg\": 71.5}, {\"date\": \"2026-11-06\", \"kg\": 71.2}, {\"date\": \"2026-11-07\", \"kg\": 60}]}, \
                 {\"set\": \"running_log\", \"rows\": [{\"date\": \"2026-11-06\", \"km\": 8}]}]}",
            )),
            Step::Reply(text("ok")),
        ],
        &[("MEMORY_EXTRACT", "on")],
    )
    .await;
    upload(&app, KEY_A, "running_log", json!({ "description": "Runs", "key": ["date"], "records": [week("2026-11-01", 5.2, 31)] })).await;
    let done = run(
        &app,
        KEY_A,
        json!({ "prompt": "Log my weights: 71.5 kg yesterday, 71.2 today. And I ran 8 km today.", "interactive": true }),
    )
    .await;
    let body = app.llm.requests()[1]["messages"][0]["content"].as_str().unwrap().to_string();
    assert!(body.contains("Record sets kept for this person, name (columns):\n- running_log (date, km, minutes)"), "{body}");
    assert!(body.contains("\"records\": [{"), "{body}");

    let updated = events_of(&app, KEY_A, done["id"].as_str().unwrap(), "memory.updated").await;
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0]["records"], json!([
        { "set": "weight_log", "added": 2, "updated": 0 },
        { "set": "running_log", "added": 1, "updated": 0 },
    ]));
    let skipped = updated[0]["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0]["reason"].as_str().unwrap().contains("kg = 60"), "{skipped:?}");
    let all = sets(&app, KEY_A).await;
    assert_eq!(all.iter().map(|s| (s["name"].as_str().unwrap(), s["count"].as_i64().unwrap())).collect::<Vec<_>>(), [("running_log", 2), ("weight_log", 2)]);
    assert_eq!(all[1]["description"], "Morning weights");
    assert_eq!(all[1]["key_fields"], json!(["date"]));

    // The next run sees the sets, as it does any records.
    run(&app, KEY_A, json!({ "prompt": "how is it going?" })).await;
    assert!(system(&app, 2).contains("weight_log: 2 records"), "{}", system(&app, 2));
}
