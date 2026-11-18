//! Talking to Cedar from Slack, end to end: a fake Slack (its Web API, and a Socket Mode
//! WebSocket the bridge connects to) and a fake cedar whose runs can wait for an approval.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use cedar_chat::api::{self, AppState};
use cedar_chat::config::Config;
use cedar_chat::slack;
use cedar_chat::store::Store;
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

// -- the fake cedar ---------------------------------------------------------------------------------

#[derive(Default)]
struct Cedar {
    runs: Mutex<HashMap<Uuid, Value>>,
    /// Each run's events so far, as its stream sends them.
    events: Mutex<HashMap<Uuid, Vec<Value>>>,
    /// Every `POST /v1/runs` body, with who it was for.
    created: Mutex<Vec<(Value, Option<String>)>>,
    /// (run, path, body) of every answer to a plan or an approval.
    answers: Mutex<Vec<(Uuid, String, Value)>>,
}

type Shared = Arc<Cedar>;

fn event(seq: usize, kind: &str, data: Value) -> Value {
    json!({ "seq": seq, "type": kind, "data": data })
}

/// A prompt with WAIT waits for approval of a `bash` call; any other run answers at once.
async fn create_run(State(c): State<Shared>, headers: axum::http::HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
    let id = Uuid::new_v4();
    let prompt = body["prompt"].as_str().unwrap_or_default().to_string();
    let user = headers.get("x-cedar-user").and_then(|v| v.to_str().ok()).map(str::to_string);
    c.created.lock().unwrap().push((body.clone(), user));
    let waits = prompt.contains("WAIT");
    let final_text = format!("## Answer\nYou said **{prompt}**, see [the docs](https://docs.example/?a=1&b=2).");
    let run = if waits {
        json!({ "id": id, "status": "running", "final_text": null, "error": null, "stop_reason": null })
    } else {
        json!({ "id": id, "status": "completed", "final_text": final_text, "error": null, "stop_reason": "end_turn" })
    };
    let events = if waits {
        vec![
            event(1, "run.started", json!({})),
            event(2, "approval.required", json!({ "id": "a1", "call_id": "t1", "tool": "bash", "input": { "command": "curl https://x.example" }, "rule": "network", "reason": "It reaches the network.", "scope": null })),
        ]
    } else {
        vec![event(1, "run.started", json!({})), event(2, "run.completed", json!({ "final_text": final_text, "stop_reason": "end_turn" }))]
    };
    c.runs.lock().unwrap().insert(id, run.clone());
    c.events.lock().unwrap().insert(id, events);
    // As cedar does: a new run is queued, whatever happens next.
    (StatusCode::ACCEPTED, Json(json!({ "id": id, "status": "queued" }))).into_response()
}

async fn get_run(State(c): State<Shared>, Path(id): Path<Uuid>) -> axum::response::Response {
    match c.runs.lock().unwrap().get(&id) {
        Some(run) => Json(run.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": { "code": "not_found", "message": "run not found" } }))).into_response(),
    }
}

/// The run's events from `after` on, kept open (polling) until its last one.
async fn run_events(State(c): State<Shared>, Path(id): Path<Uuid>, axum::extract::RawQuery(q): axum::extract::RawQuery) -> axum::response::Response {
    let after: usize = q.unwrap_or_default().split('&').find_map(|p| p.strip_prefix("after=")).and_then(|n| n.parse().ok()).unwrap_or(0);
    let stream = futures_util::stream::unfold((c, after, false), move |(c, mut sent, done)| async move {
        if done {
            return None;
        }
        loop {
            let next = c.events.lock().unwrap().get(&id).and_then(|events| events.get(sent).cloned());
            if let Some(e) = next {
                sent += 1;
                let last = matches!(e["type"].as_str(), Some("run.completed" | "run.failed" | "run.cancelled"));
                let block = format!("id: {}\nevent: {}\ndata: {e}\n\n", e["seq"], e["type"].as_str().unwrap());
                return Some((Ok::<_, std::io::Error>(block), (c, sent, last)));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    ([("content-type", "text/event-stream")], Body::from_stream(stream)).into_response()
}

/// Deciding the approval ends the run.
async fn decide(State(c): State<Shared>, Path((id, aid)): Path<(Uuid, String)>, Json(body): Json<Value>) -> axum::response::Response {
    c.answers.lock().unwrap().push((id, format!("approvals/{aid}"), body.clone()));
    let mut runs = c.runs.lock().unwrap();
    let run = runs.get_mut(&id).unwrap();
    if run["status"] != "running" {
        return (StatusCode::CONFLICT, Json(json!({ "error": { "code": "conflict", "message": "not waiting" } }))).into_response();
    }
    run["status"] = json!("completed");
    run["final_text"] = json!("Ran it.");
    run["stop_reason"] = json!("end_turn");
    let mut events = c.events.lock().unwrap();
    let list = events.get_mut(&id).unwrap();
    let n = list.len();
    list.push(event(n + 1, "approval.resolved", json!({ "id": aid, "decision": body["decision"], "by": "user" })));
    list.push(event(n + 2, "run.completed", json!({ "final_text": "Ran it.", "stop_reason": "end_turn" })));
    (StatusCode::ACCEPTED, Json(json!({}))).into_response()
}

fn fake_cedar(c: Shared) -> Router {
    Router::new()
        .route("/v1/runs", post(create_run))
        .route("/v1/runs/{id}", get(get_run))
        .route("/v1/runs/{id}/events", get(run_events))
        .route("/v1/runs/{id}/approvals/{aid}", post(decide))
        // Nothing finishes through the key's stream here: the bridge's own watch answers.
        .route("/v1/events", get(|| async { ([("content-type", "text/event-stream")], ": keep-alive\n\n") }))
        .route("/v1/models", get(|| async { Json(json!({ "models": [{ "id": "model-a", "is_default": true }] })) }))
        .with_state(c)
}

// -- the fake Slack ---------------------------------------------------------------------------------

#[derive(Default)]
struct FakeSlack {
    /// (method, body) of every Web API call.
    calls: Mutex<Vec<(String, Value)>>,
    ws_url: Mutex<String>,
}

async fn slack_method(State(s): State<Arc<FakeSlack>>, Path(method): Path<String>, body: axum::body::Bytes) -> Json<Value> {
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let n = {
        let mut calls = s.calls.lock().unwrap();
        calls.push((method.clone(), parsed));
        calls.len()
    };
    Json(match method.as_str() {
        "auth.test" => json!({ "ok": true, "user_id": "UBOT", "team_id": "T1", "team": "Northwind", "user": "cedar" }),
        "apps.connections.open" => json!({ "ok": true, "url": *s.ws_url.lock().unwrap() }),
        "users.info" => json!({ "ok": true, "user": { "name": "ann", "real_name": "Ann Example", "profile": { "display_name": "Ann" }, "locale": "en-US" } }),
        _ => json!({ "ok": true, "ts": format!("{n}.000") }),
    })
}

struct Harness {
    app: Router,
    cedar: Shared,
    slack: Arc<FakeSlack>,
    ws: WebSocketStream<TcpStream>,
    stop: CancellationToken,
    envelopes: u32,
}

impl Harness {
    async fn new() -> Self {
        let cedar = Shared::default();
        let cedar_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cedar_url = format!("http://{}", cedar_listener.local_addr().unwrap());
        tokio::spawn(axum::serve(cedar_listener, fake_cedar(cedar.clone())).into_future());

        let slack = Arc::new(FakeSlack::default());
        let slack_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let slack_url = format!("http://{}/api", slack_listener.local_addr().unwrap());
        let api = Router::new().route("/api/{method}", get(slack_method).post(slack_method)).with_state(slack.clone());
        tokio::spawn(axum::serve(slack_listener, api).into_future());
        let ws_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        *slack.ws_url.lock().unwrap() = format!("ws://{}/socket", ws_listener.local_addr().unwrap());

        let env: HashMap<&str, String> = HashMap::from([
            ("CEDAR_URL", cedar_url),
            ("CEDAR_API_KEY", "cedar-key".into()),
            ("CHAT_PUBLIC_URL", "https://chat.example".into()),
            ("CHAT_SLACK_APP_TOKEN", "xapp-test".into()),
            ("CHAT_SLACK_BOT_TOKEN", "xoxb-test".into()),
            ("CHAT_SLACK_API_URL", slack_url),
            ("CHAT_UPDATE_CHECK", "off".into()),
            ("CHAT_CONTRIBUTIONS_URL", "off".into()),
        ]);
        let cfg = Config::from_lookup(|k| env.get(k).cloned()).unwrap();
        let store = Store::connect_memory().await.unwrap();
        let state = AppState::new(cfg, store).unwrap();
        let stop = CancellationToken::new();
        slack::start(state.clone(), stop.clone());

        let (stream, _) = tokio::time::timeout(Duration::from_secs(5), ws_listener.accept()).await.expect("the bridge connects").unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(Message::Text(json!({ "type": "hello" }).to_string().into())).await.unwrap();
        Self { app: api::router(state), cedar, slack, ws, stop, envelopes: 0 }
    }

    /// Send an envelope down the socket and wait for its acknowledgement.
    async fn send(&mut self, kind: &str, payload: Value) {
        self.envelopes += 1;
        let id = format!("env-{}", self.envelopes);
        self.ws.send(Message::Text(json!({ "envelope_id": id, "type": kind, "payload": payload }).to_string().into())).await.unwrap();
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), self.ws.next()).await.expect("an ack").unwrap().unwrap();
            if let Message::Text(t) = msg {
                let ack: Value = serde_json::from_str(t.as_str()).unwrap();
                assert_eq!(ack["envelope_id"], id);
                return;
            }
        }
    }

    async fn message(&mut self, event: Value) {
        self.envelopes += 1;
        let payload = json!({ "team_id": "T1", "event_id": format!("Ev{}", self.envelopes), "event": event });
        self.send("events_api", payload).await;
    }

    async fn press(&mut self, user: &str, action: &str, value: &str) {
        let payload = json!({
            "type": "block_actions",
            "user": { "id": user, "team_id": "T1" },
            "team": { "id": "T1" },
            "channel": { "id": "D1" },
            "message": { "ts": "99.000", "thread_ts": "10.000" },
            "actions": [{ "action_id": action, "value": value }],
        });
        self.send("interactive", payload).await;
    }

    fn calls(&self, method: &str) -> Vec<Value> {
        self.slack.calls.lock().unwrap().iter().filter(|(m, _)| m == method).map(|(_, b)| b.clone()).collect()
    }

    /// Wait until `method` has been called `n` times, and return those calls.
    async fn wait_calls(&self, method: &str, n: usize) -> Vec<Value> {
        for _ in 0..250 {
            let calls = self.calls(method);
            if calls.len() >= n {
                return calls;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{method} was called {} times, not {n}: {:?}", self.calls(method).len(), self.slack.calls.lock().unwrap());
    }

    async fn request(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

fn code_in(text: &str) -> String {
    let start = text.rfind("*").and_then(|end| text[..end].rfind('*').map(|s| s + 1)).unwrap();
    text[start..text.rfind('*').unwrap()].to_string()
}

fn dm(user: &str, text: &str, ts: &str, thread_ts: Option<&str>) -> Value {
    json!({ "type": "message", "channel_type": "im", "channel": "D1", "user": user, "text": text, "ts": ts, "thread_ts": thread_ts })
}

#[tokio::test]
async fn a_linked_account_talks_to_cedar_in_threads_and_approves_from_slack() {
    let mut h = Harness::new().await;

    // An account the app doesn't know is sent a code, and nothing reaches cedar.
    h.message(dm("U1", "hello", "1.000", None)).await;
    let posted = h.wait_calls("chat.postMessage", 1).await;
    assert_eq!(posted[0]["channel"], "D1");
    assert_eq!(posted[0]["thread_ts"], "1.000");
    let code = code_in(posted[0]["text"].as_str().unwrap());
    assert_eq!(code.len(), 9, "{code}");
    assert!(h.cedar.created.lock().unwrap().is_empty());

    // Settings -> Slack takes the code once, and the account is told.
    let (status, _) = h.request("POST", "/api/slack/link", Some(json!({ "code": "WXYZ-2345" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = h.request("POST", "/api/slack/link", Some(json!({ "code": code.to_lowercase() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["link"]["slack_user"], "U1");
    assert_eq!(body["link"]["name"], "Ann");
    let posted = h.wait_calls("chat.postMessage", 2).await;
    assert_eq!(posted[1]["channel"], "U1");
    assert!(posted[1]["text"].as_str().unwrap().contains("linked"), "{}", posted[1]);
    let (status, _) = h.request("POST", "/api/slack/link", Some(json!({ "code": code }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a code works once");
    let (_, settings) = h.request("GET", "/api/slack", None).await;
    assert_eq!(settings["available"], true);
    assert_eq!(settings["workspace"]["team_name"], "Northwind");
    assert_eq!(settings["links"].as_array().unwrap().len(), 1);

    // A message is a run with someone reading along; the answer comes back in its thread, in
    // Slack's markup, with a link to the conversation.
    h.message(dm("U1", "what is cedar?", "5.000", None)).await;
    let posted = h.wait_calls("chat.postMessage", 3).await;
    let answer = &posted[2];
    assert_eq!(answer["thread_ts"], "5.000");
    let text = answer["text"].as_str().unwrap();
    assert!(text.starts_with("*Answer*\nYou said *what is cedar?*, see <https://docs.example/?a=1&amp;b=2|the docs>."), "{text}");
    assert!(text.contains("<https://chat.example/#/c/"), "{text}");
    {
        let created = h.cedar.created.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].0["prompt"], "what is cedar?");
        assert_eq!(created[0].0["interactive"], true);
    }
    let reactions: Vec<Value> = h.calls("reactions.add");
    assert!(reactions.iter().any(|r| r["name"] == "eyes" && r["timestamp"] == "5.000"));
    h.wait_calls("reactions.add", 2).await;
    assert!(h.calls("reactions.add").iter().any(|r| r["name"] == "white_check_mark"));

    // The conversation is on the page too.
    let (_, conversations) = h.request("GET", "/api/conversations", None).await;
    let list = conversations.as_array().or_else(|| conversations["conversations"].as_array()).cloned().unwrap_or_default();
    assert!(list.iter().any(|c| c["title"] == "what is cedar?"), "{conversations}");

    // A reply in the thread continues the run; a new message starts afresh.
    h.message(dm("U1", "and more", "6.000", Some("5.000"))).await;
    h.wait_calls("chat.postMessage", 4).await;
    {
        let created = h.cedar.created.lock().unwrap();
        let first = h.cedar.runs.lock().unwrap().keys().copied().collect::<Vec<_>>();
        assert_eq!(created.len(), 2);
        let parent = created[1].0["parent_id"].as_str().expect("the reply continues the run");
        assert!(first.iter().any(|id| id.to_string() == parent));
    }

    // A run that needs approval asks in the thread; only its owner's press counts.
    h.message(dm("U1", "please WAIT", "10.000", None)).await;
    let posted = h.wait_calls("chat.postMessage", 5).await;
    let ask = &posted[4];
    assert_eq!(ask["thread_ts"], "10.000");
    let buttons = ask["blocks"][1]["elements"].as_array().unwrap();
    assert_eq!(buttons.len(), 2, "no 'always' without a scope: {buttons:?}");
    let value = buttons[0]["value"].as_str().unwrap().to_string();
    assert!(value.ends_with("|a1"));

    h.press("U2", "cedar_allow", &value).await;
    let ephemeral = h.wait_calls("chat.postEphemeral", 1).await;
    assert_eq!(ephemeral[0]["user"], "U2");
    assert!(h.cedar.answers.lock().unwrap().is_empty(), "someone else can't approve");

    h.press("U1", "cedar_allow", &value).await;
    let updated = h.wait_calls("chat.update", 1).await;
    assert!(updated[0]["blocks"][1]["elements"][0]["text"].as_str().unwrap().contains("Allowed"), "{}", updated[0]);
    {
        let answers = h.cedar.answers.lock().unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].1, "approvals/a1");
        assert_eq!(answers[0].2["decision"], "allow");
    }
    let posted = h.wait_calls("chat.postMessage", 6).await;
    assert!(posted[5]["text"].as_str().unwrap().starts_with("Ran it."), "{}", posted[5]);
    assert_eq!(posted[5]["thread_ts"], "10.000");

    // Someone else in a channel gets a code of their own, only to them, and starts nothing.
    let runs = h.cedar.created.lock().unwrap().len();
    h.message(json!({ "type": "app_mention", "channel": "C1", "user": "U2", "text": "<@UBOT> hi", "ts": "20.000" })).await;
    let ephemeral = h.wait_calls("chat.postEphemeral", 2).await;
    assert_eq!(ephemeral[1]["user"], "U2");
    assert_eq!(ephemeral[1]["channel"], "C1");
    assert_eq!(h.cedar.created.lock().unwrap().len(), runs);

    // Unlinking stops the account from reaching Cedar.
    let (status, _) = h.request("DELETE", "/api/slack/links/T1/U1", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    h.message(dm("U1", "still there?", "30.000", None)).await;
    let posted = h.wait_calls("chat.postMessage", 7).await;
    assert!(posted[6]["text"].as_str().unwrap().contains("code"), "{}", posted[6]);
    assert_eq!(h.cedar.created.lock().unwrap().len(), runs);
}

#[tokio::test]
async fn the_same_event_twice_is_handled_once() {
    let mut h = Harness::new().await;
    let payload = json!({ "team_id": "T1", "event_id": "EvSame", "event": dm("U9", "hi", "1.000", None) });
    h.send("events_api", payload.clone()).await;
    h.send("events_api", payload).await;
    h.wait_calls("chat.postMessage", 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.calls("chat.postMessage").len(), 1);
}
