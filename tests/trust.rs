//! What makes a run legible and steerable: its reasoning, cost and sandbox, the plan it commits
//! to (and that an interactive client can change), approvals for risky calls, and numbered
//! sources.

mod common;

use axum::http::{Method, StatusCode};
use cedar::models::Price;
use common::*;
use serde_json::{json, Value};

/// Follow a run's events until one of `kind` arrives.
async fn wait_for(app: &TestApp, id: &str, kind: &str) -> SseEvent {
    let mut stream = LiveStream::open(app, KEY_A, id, None).await;
    let found = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(e) = stream.next_event().await {
            if e.data["type"] == kind {
                return Some(e);
            }
        }
        None
    })
    .await;
    found
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("no {kind} event"))
}

fn of_kind<'a>(events: &'a [SseEvent], kind: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|e| e.data["type"] == kind)
        .map(|e| &e.data["data"])
        .collect()
}

fn id_of(run: &Value) -> String {
    run["id"].as_str().unwrap().to_string()
}

async fn post(app: &TestApp, uri: &str, body: Value) -> (StatusCode, Value) {
    app.json(Method::POST, uri, Some(KEY_A), Some(body)).await
}

/// The last tool result the model was sent before request `n`.
fn result_before(app: &TestApp, n: usize) -> Value {
    let requests = app.llm.requests();
    let msgs = requests[n]["messages"].as_array().unwrap();
    msgs.last().unwrap()["content"][0].clone()
}

#[tokio::test]
async fn reasoning_cost_and_the_sandbox_are_reported() {
    let mut answer = text("Done.");
    answer.reasoning = Some("The user wants a short answer.".into());
    let app = TestApp::with_env(vec![Step::Reply(answer)], &[("SANDBOX_MEMORY", "1g")]).await;
    let mut entry = app
        .state
        .store
        .get_model(TEST_MODEL)
        .await
        .unwrap()
        .unwrap();
    entry.price = Some(Price {
        input_usd_per_m: 2.0,
        output_usd_per_m: 10.0,
    });
    app.state.store.upsert_model(&entry).await.unwrap();
    app.state.integrations.reload().await.unwrap();

    let run = app.create_run(KEY_A, "hi").await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    let events = app.events(KEY_A, &id, None).await;

    let started = of_kind(&events, "run.started")[0];
    assert_eq!(
        started["sandbox"],
        json!({ "runtime": "openshell", "network": "none", "policy": "offline", "hosts": [], "memory": "1g", "cpus": 1.0 })
    );
    assert_eq!(started["interactive"], false);
    let reasoning = of_kind(&events, "assistant.reasoning");
    assert_eq!(reasoning.len(), 1);
    assert_eq!(reasoning[0]["text"], "The user wants a short answer.");
    assert_eq!(reasoning[0]["done"], true);
    assert!(reasoning[0]["duration_ms"].is_u64());
    // 10 input and 5 output tokens at $2 and $10 per million.
    let usage = of_kind(&events, "usage")[0];
    let cost = usage["cost_usd"].as_f64().unwrap();
    assert!((cost - 0.00007).abs() < 1e-12, "{cost}");
    // The reasoning is not kept in the conversation.
    let (_, messages) = app.get(&format!("/v1/runs/{id}/messages"), KEY_A).await;
    assert!(!messages.to_string().contains("short answer"));
}

#[tokio::test]
async fn a_negative_price_is_refused() {
    let app = TestApp::with_env(vec![], &[("CEDAR_ADMIN_KEYS", KEY_B)]).await;
    // Checked before the model's server is, so this server needn't exist.
    let (status, body) = app
        .json(
            Method::PUT,
            &format!("/v1/admin/models/{TEST_MODEL}"),
            Some(KEY_B),
            Some(
                json!({ "base_url": "http://127.0.0.1:9/v1", "model": TEST_MODEL,
                         "price": { "input_usd_per_m": -1, "output_usd_per_m": 1 } }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("price"), "{body}");
}

#[tokio::test]
async fn a_plan_is_made_and_followed_without_waiting_when_the_client_does_not_answer() {
    let app = TestApp::new(vec![
        Step::Reply(text(r#"{"steps": ["Find the figures", "Draw a chart"]}"#)),
        Step::Reply(text("Here it is.")),
    ])
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "chart the market", "plan": "on" }))
        .await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    let events = app.events(KEY_A, &id, None).await;

    let plan = of_kind(&events, "plan");
    assert_eq!(
        *plan[0],
        json!({ "steps": [{ "id": "1", "title": "Find the figures" }, { "id": "2", "title": "Draw a chart" }],
                "editable": false, "awaiting": false })
    );
    assert!(of_kind(&events, "plan.updated").is_empty());
    let usage = of_kind(&events, "usage");
    assert_eq!(usage[0]["purpose"], "plan");
    assert_eq!(usage.len(), 2);

    let requests = app.llm.requests();
    // The planning call has no tools and asks for JSON; the run then follows the plan.
    assert_eq!(requests[0]["tools"], json!([]));
    assert!(requests[0]["messages"]
        .to_string()
        .contains("chart the market"));
    let system = requests[1]["system"].as_str().unwrap();
    assert!(
        system.ends_with("\n1. Find the figures\n2. Draw a chart"),
        "{system}"
    );
    assert_eq!(requests[1]["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn auto_skips_short_prompts_and_a_useless_plan_is_dropped() {
    let app = TestApp::new(vec![
        Step::Reply(text("4")),
        Step::Reply(text("I can't break that down.")),
        Step::Reply(text("ok")),
    ])
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "what's 2 + 2?", "plan": "auto" }))
        .await;
    app.wait_status(KEY_A, &id_of(&run), "completed").await;
    assert_eq!(app.llm.call_count(), 1);

    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "do it", "plan": "on" }))
        .await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    let events = app.events(KEY_A, &id, None).await;
    assert!(of_kind(&events, "plan").is_empty());
    assert_eq!(app.llm.call_count(), 3);
}

#[tokio::test]
async fn an_interactive_run_waits_for_its_plan_and_follows_the_edited_one() {
    let app = TestApp::new(vec![
        Step::Reply(text(r#"["Search", "Read", "Write"]"#)),
        Step::Reply(text("Done.")),
    ])
    .await;
    let run = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "research this", "plan": "on", "interactive": true }),
        )
        .await;
    let id = id_of(&run);
    let plan = wait_for(&app, &id, "plan").await;
    assert_eq!(plan.data["data"]["awaiting"], true);
    assert_eq!(
        app.llm.call_count(),
        1,
        "the run waits before its first turn"
    );

    let (status, body) = post(&app, &format!("/v1/runs/{id}/plan"), json!({ "steps": [] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let edited = json!({ "steps": [{ "id": "2", "title": "Read the report" }, { "id": "9", "title": "Summarize" }] });
    let (status, _) = post(&app, &format!("/v1/runs/{id}/plan"), edited.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    app.wait_status(KEY_A, &id, "completed").await;
    // Nothing is waiting any more.
    let (status, _) = post(&app, &format!("/v1/runs/{id}/plan"), edited).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let events = app.events(KEY_A, &id, None).await;
    let updated = of_kind(&events, "plan.updated");
    assert_eq!(updated[0]["by"], "user");
    assert_eq!(
        updated[0]["steps"][1],
        json!({ "id": "9", "title": "Summarize" })
    );
    let system = app.llm.requests()[1]["system"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        system.ends_with("\n1. Read the report\n2. Summarize"),
        "{system}"
    );
}

#[tokio::test]
async fn an_unanswered_plan_times_out_into_the_proposed_one() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(text(r#"["A", "B"]"#)),
            Step::Reply(text("Done.")),
        ],
        &[("APPROVAL_TIMEOUT_SECS", "1")],
    )
    .await;
    let run = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "go", "plan": "on", "interactive": true }),
        )
        .await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "running").await;
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    app.wait_status(KEY_A, &id, "completed").await;
    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(of_kind(&events, "plan.updated")[0]["by"], "timeout");
}

#[tokio::test]
async fn in_tool_mode_the_model_proposes_a_plan_when_it_wants_one() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "p1",
            "propose_plan",
            json!({ "steps": ["Find the figures", "Draw a chart"] }),
        )),
        Step::Reply(tool_use(
            "p2",
            "propose_plan",
            json!({ "steps": ["Again", "And again"] }),
        )),
        Step::Reply(text("Here it is.")),
    ])
    .await;
    let run = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "chart the market", "plan": "tool" }),
        )
        .await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    let events = app.events(KEY_A, &id, None).await;

    // No planning call before the run: the model's first request offers the tool instead.
    let requests = app.llm.requests();
    assert_eq!(requests.len(), 3);
    let tools: Vec<&str> = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(tools.contains(&"propose_plan"), "{tools:?}");
    assert!(requests[0]["system"]
        .as_str()
        .unwrap()
        .contains("call propose_plan"));

    let plan = of_kind(&events, "plan");
    assert_eq!(plan.len(), 1, "a second plan is refused");
    assert_eq!(
        *plan[0],
        json!({ "steps": [{ "id": "1", "title": "Find the figures" }, { "id": "2", "title": "Draw a chart" }],
                "editable": false, "awaiting": false, "origin": "tool" })
    );
    // The plan is its own event, not a tool card.
    assert!(of_kind(&events, "tool.call").is_empty());
    let first = result_before(&app, 1);
    assert_eq!(first["tool_use_id"], "p1");
    assert!(
        first["content"]
            .as_str()
            .unwrap()
            .starts_with("The plan is shown"),
        "{first}"
    );
    let again = result_before(&app, 2);
    assert_eq!(again["is_error"], true);
    assert!(
        again["content"]
            .as_str()
            .unwrap()
            .contains("already a plan"),
        "{again}"
    );
}

#[tokio::test]
async fn the_tool_is_offered_only_in_tool_mode_and_bad_steps_go_back_to_the_model() {
    let app = TestApp::new(vec![
        Step::Reply(text("ok")),
        Step::Reply(tool_use(
            "p1",
            "propose_plan",
            json!({ "steps": ["just one"] }),
        )),
        Step::Reply(text("Fine, no plan.")),
    ])
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hi", "plan": "off" }))
        .await;
    app.wait_status(KEY_A, &id_of(&run), "completed").await;
    let request = &app.llm.requests()[0];
    assert!(!request["tools"].to_string().contains("propose_plan"));
    assert!(!request["system"].as_str().unwrap().contains("propose_plan"));

    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "go", "plan": "tool" }))
        .await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    let result = result_before(&app, 2);
    assert_eq!(result["is_error"], true);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("2 to 7 steps, got 1"),
        "{result}"
    );
    assert!(of_kind(&app.events(KEY_A, &id, None).await, "plan").is_empty());
}

#[tokio::test]
async fn an_interactive_run_waits_mid_run_for_the_plan_the_model_proposed() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "p1",
            "propose_plan",
            json!({ "steps": ["Search", "Write"] }),
        )),
        Step::Reply(text("Done.")),
    ])
    .await;
    let run = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "research this", "plan": "tool", "interactive": true }),
        )
        .await;
    let id = id_of(&run);
    let plan = wait_for(&app, &id, "plan").await;
    assert_eq!(plan.data["data"]["awaiting"], true);
    assert_eq!(plan.data["data"]["editable"], true);
    assert_eq!(plan.data["data"]["origin"], "tool");
    assert_eq!(app.llm.call_count(), 1, "the run waits on the plan");

    let edited = json!({ "steps": [{ "id": "1", "title": "Search the archive" }, { "id": "3", "title": "Summarize" }] });
    let (status, _) = post(&app, &format!("/v1/runs/{id}/plan"), edited).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    app.wait_status(KEY_A, &id, "completed").await;

    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(of_kind(&events, "plan.updated")[0]["by"], "user");
    let result = result_before(&app, 1);
    let content = result["content"].as_str().unwrap();
    assert!(content.starts_with("The user agreed"), "{content}");
    assert!(
        content.ends_with("\n1. Search the archive\n2. Summarize"),
        "{content}"
    );
}

#[tokio::test]
async fn a_risky_call_waits_and_a_declined_one_does_not_run() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "bash", json!({ "command": "rm -rf out" }))),
        Step::Reply(text("Left it alone.")),
    ])
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "clean up", "interactive": true }))
        .await;
    let id = id_of(&run);
    let asked = wait_for(&app, &id, "approval.required").await;
    let approval = &asked.data["data"];
    assert_eq!(approval["tool"], "bash");
    assert_eq!(approval["rule"], "deletes");
    assert_eq!(approval["scope"], Value::Null);
    assert_eq!(approval["call_id"], "t1");
    let aid = approval["id"].as_str().unwrap();

    let uri = format!("/v1/runs/{id}/approvals/{aid}");
    let (status, _) = post(&app, &uri, json!({ "decision": "maybe" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(
        &app,
        &format!("/v1/runs/{id}/approvals/nope"),
        json!({ "decision": "deny" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = post(&app, &uri, json!({ "decision": "deny" })).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    app.wait_status(KEY_A, &id, "completed").await;

    assert!(app
        .factory
        .sandbox(0)
        .commands
        .lock()
        .unwrap()
        .iter()
        .all(|c| !c.contains("rm -rf")));
    let result = result_before(&app, 1);
    assert_eq!(result["is_error"], true);
    assert!(
        result["content"].as_str().unwrap().contains("declined"),
        "{result}"
    );
    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(
        kinds(&events),
        [
            "run.started",
            "usage",
            "approval.required",
            "approval.resolved",
            "tool.call",
            "tool.result",
            "usage",
            "assistant.text",
            "run.completed"
        ]
    );
    assert_eq!(of_kind(&events, "approval.resolved")[0]["decision"], "deny");
}

#[tokio::test]
async fn always_allowing_remembers_the_scope_for_the_key() {
    let install = |id: &str| tool_use(id, "bash", json!({ "command": "pip install rich" }));
    let app = TestApp::with_env(
        vec![
            Step::Reply(install("t1")),
            Step::Reply(install("t2")),
            Step::Reply(text("Installed.")),
        ],
        &[("SANDBOX_POLICY", "packages")],
    )
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "set up", "interactive": true }))
        .await;
    let id = id_of(&run);
    let asked = wait_for(&app, &id, "approval.required").await;
    let approval = &asked.data["data"];
    assert_eq!(approval["rule"], "installs");
    assert_eq!(approval["scope"], "rich");
    let uri = format!(
        "/v1/runs/{id}/approvals/{}",
        approval["id"].as_str().unwrap()
    );
    post(&app, &uri, json!({ "decision": "allow_always" })).await;
    app.wait_status(KEY_A, &id, "completed").await;

    // Asked once; the second install of the same package went ahead.
    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(of_kind(&events, "approval.required").len(), 1);
    let commands = app.factory.sandbox(0).commands.lock().unwrap().clone();
    assert_eq!(
        commands
            .iter()
            .filter(|c| c.contains("pip install rich"))
            .count(),
        2,
        "{commands:?}"
    );
    let owner = cedar::auth::AuthKeys::new(&[KEY_A.into()])
        .authenticate(KEY_A)
        .unwrap()
        .0;
    assert!(app
        .state
        .store
        .granted(&owner, "installs", &["rich".into()])
        .await
        .unwrap());
}

#[tokio::test]
async fn runs_whose_client_does_not_answer_are_not_gated() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use("t1", "bash", json!({ "command": "rm notes.txt" }))),
        Step::Reply(text("Removed.")),
    ])
    .await;
    let run = app.create_run(KEY_A, "remove notes.txt").await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    let events = app.events(KEY_A, &id, None).await;
    assert!(of_kind(&events, "approval.required").is_empty());
    assert!(app
        .factory
        .sandbox(0)
        .commands
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.contains("rm notes.txt")));
}

#[tokio::test]
async fn approvals_can_be_turned_off_and_time_out() {
    let rm = || tool_use("t1", "bash", json!({ "command": "rm x" }));
    let app = TestApp::with_env(
        vec![Step::Reply(rm()), Step::Reply(text("ok"))],
        &[("TOOL_APPROVALS", "off")],
    )
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "go", "interactive": true }))
        .await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;
    assert!(of_kind(&app.events(KEY_A, &id, None).await, "approval.required").is_empty());

    // A wait longer than the run's time limit doesn't fail the run: waiting doesn't count.
    let app = TestApp::with_env(
        vec![Step::Reply(rm()), Step::Reply(text("ok"))],
        &[("APPROVAL_TIMEOUT_SECS", "2"), ("RUN_TIMEOUT_SECS", "1")],
    )
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "go", "interactive": true }))
        .await;
    let id = id_of(&run);
    wait_for(&app, &id, "approval.required").await;
    tokio::time::sleep(std::time::Duration::from_millis(2300)).await;
    app.wait_status(KEY_A, &id, "completed").await;
    let events = app.events(KEY_A, &id, None).await;
    assert_eq!(
        of_kind(&events, "approval.resolved")[0]["decision"],
        "timeout"
    );
    let result = result_before(&app, 1);
    assert!(
        result["content"].as_str().unwrap().contains("in time"),
        "{result}"
    );
}

#[tokio::test]
async fn the_model_is_asked_to_cite_when_it_can_read_the_web() {
    let app = TestApp::new(vec![Step::Reply(text("ok"))]).await;
    let run = app.create_run(KEY_A, "hi").await;
    app.wait_status(KEY_A, &id_of(&run), "completed").await;
    let system = app.llm.requests()[0]["system"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(system.contains("Cite sources as [n]"), "{system}");
}

#[tokio::test]
async fn a_blocked_connection_is_reported_and_the_model_told() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "t1",
            "bash",
            json!({ "command": "curl https://example.com/data" }),
        )),
        Step::Reply(text("Couldn't reach it.")),
    ])
    .await;
    let run = app.create_run(KEY_A, "fetch it").await;
    let id = id_of(&run);
    app.wait_status(KEY_A, &id, "completed").await;

    let events = app.events(KEY_A, &id, None).await;
    let denied = of_kind(&events, "sandbox.denied");
    assert_eq!(denied.len(), 1);
    assert_eq!(
        *denied[0],
        json!({ "call_id": "t1", "program": "/usr/bin/curl", "host": "example.com", "port": 443,
                "what": "connect", "reason": "transparent_tcp_policy_denied" })
    );
    // Nobody can be asked in a run that isn't interactive.
    assert!(of_kind(&events, "approval.required").is_empty());
    let result = result_before(&app, 1);
    let content = result["content"].as_str().unwrap();
    assert!(
        content.contains("[the sandbox's network policy blocked]\n- curl → example.com:443"),
        "{content}"
    );
    assert!(content.contains("nobody can be asked"), "{content}");
    assert!(app.factory.sandbox(0).allowed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_blocked_connection_can_be_allowed_and_always_is_remembered_for_the_key() {
    let curl = |id: &str| {
        tool_use(
            id,
            "bash",
            json!({ "command": "curl https://api.example.com/v1" }),
        )
    };
    let app = TestApp::new(vec![
        Step::Reply(curl("t1")),
        Step::Reply(text("Blocked; allowed now.")),
        Step::Reply(curl("t2")),
        Step::Reply(text("Fetched.")),
    ])
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "fetch it", "interactive": true }))
        .await;
    let id = id_of(&run);
    let asked = wait_for(&app, &id, "approval.required").await;
    let approval = &asked.data["data"];
    assert_eq!(approval["rule"], "network");
    assert_eq!(approval["call_id"], "t1");
    assert_eq!(approval["scope"], "curl → api.example.com:443");
    assert_eq!(
        approval["input"],
        json!({ "program": "/usr/bin/curl", "host": "api.example.com", "port": 443 })
    );
    let uri = format!(
        "/v1/runs/{id}/approvals/{}",
        approval["id"].as_str().unwrap()
    );
    post(&app, &uri, json!({ "decision": "allow_always" })).await;
    app.wait_status(KEY_A, &id, "completed").await;

    let endpoint = cedar::policy::Endpoint {
        binary: "/usr/bin/curl".into(),
        host: "api.example.com".into(),
        port: 443,
    };
    assert_eq!(
        *app.factory.sandbox(0).allowed.lock().unwrap(),
        std::slice::from_ref(&endpoint)
    );
    let content = result_before(&app, 1)["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        content.contains(
            "The user allowed /usr/bin/curl → api.example.com:443; run the command again."
        ),
        "{content}"
    );

    // The next run of the same key starts with it allowed, and isn't asked again.
    let next = app
        .create_run_with(KEY_A, json!({ "prompt": "again", "interactive": true }))
        .await;
    let next = id_of(&next);
    app.wait_status(KEY_A, &next, "completed").await;
    assert_eq!(*app.factory.sandbox(1).allowed.lock().unwrap(), [endpoint]);
    let events = app.events(KEY_A, &next, None).await;
    assert!(of_kind(&events, "approval.required").is_empty());
    assert!(of_kind(&events, "sandbox.denied").is_empty());
    let content = result_before(&app, 3)["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(content, "fetched api.example.com");
}

#[tokio::test]
async fn a_declined_connection_stays_blocked() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "t1",
            "bash",
            json!({ "command": "curl https://evil.example/x" }),
        )),
        Step::Reply(text("Left it.")),
    ])
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "fetch it", "interactive": true }))
        .await;
    let id = id_of(&run);
    let asked = wait_for(&app, &id, "approval.required").await;
    let uri = format!(
        "/v1/runs/{id}/approvals/{}",
        asked.data["data"]["id"].as_str().unwrap()
    );
    post(&app, &uri, json!({ "decision": "deny" })).await;
    app.wait_status(KEY_A, &id, "completed").await;
    assert!(app.factory.sandbox(0).allowed.lock().unwrap().is_empty());
    let content = result_before(&app, 1)["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        content.contains("The user did not allow /usr/bin/curl → evil.example:443"),
        "{content}"
    );
}
