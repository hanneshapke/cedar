//! Test doubles and a small HTTP harness shared by the integration tests.
#![allow(dead_code)]

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use cedar::api::{self, AppState};
use cedar::config::Config;
use cedar::llm::{LlmClient, LlmResponse, MessageRequest};
use cedar::models::Usage;
use cedar::sandbox::{ExecOutput, FileContent, Sandbox, SandboxFactory, SandboxFatal};
use cedar::store::Store;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tower::ServiceExt;
use uuid::Uuid;

// -- scripted LLM ----------------------------------------------------------------------------

#[derive(Clone)]
pub enum Step {
    Reply(LlmResponse),
    /// Like `Reply`, but never consumed: every later call gets the same response.
    Repeat(LlmResponse),
    Error(&'static str),
    /// Never returns; the run has to be cancelled (or time out).
    Block,
    /// Returns once the semaphore has a permit.
    Gate(Arc<Semaphore>, LlmResponse),
    Panic,
}

#[derive(Default)]
pub struct ScriptedLlm {
    script: Mutex<VecDeque<Step>>,
    pub requests: Mutex<Vec<Value>>,
}

impl ScriptedLlm {
    pub fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self { script: Mutex::new(steps.into()), requests: Mutex::new(vec![]) })
    }

    pub fn call_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn create_message(&self, req: &MessageRequest<'_>) -> Result<LlmResponse> {
        self.requests.lock().unwrap().push(json!({
            "model": req.model, "system": req.system, "max_tokens": req.max_tokens,
            "tools": req.tools, "messages": req.messages,
        }));
        let step = {
            let mut script = self.script.lock().unwrap();
            match script.front() {
                Some(Step::Repeat(r)) => Step::Reply(r.clone()),
                _ => script.pop_front().ok_or_else(|| anyhow!("script exhausted"))?,
            }
        };
        match step {
            Step::Reply(r) | Step::Repeat(r) => Ok(r),
            Step::Error(m) => Err(anyhow!(m)),
            Step::Block => std::future::pending().await,
            Step::Gate(gate, r) => {
                gate.acquire().await.unwrap().forget();
                Ok(r)
            }
            Step::Panic => panic!("scripted panic"),
        }
    }
}

pub fn reply(content: Vec<Value>, stop_reason: &str) -> LlmResponse {
    LlmResponse {
        content,
        stop_reason: Some(stop_reason.into()),
        usage: Usage { input_tokens: 10, output_tokens: 5 },
    }
}

pub fn text(t: &str) -> LlmResponse {
    reply(vec![json!({ "type": "text", "text": t })], "end_turn")
}

pub fn tool_use(id: &str, name: &str, input: Value) -> LlmResponse {
    reply(vec![json!({ "type": "tool_use", "id": id, "name": name, "input": input })], "tool_use")
}

// -- fake sandbox ----------------------------------------------------------------------------

/// Commands with special meaning: `fatal` (sandbox breaks), `fail` (exit 1 with stderr),
/// `big` (output larger than the cap). Anything else echoes `ran: <command>`.
#[derive(Default)]
pub struct FakeSandbox {
    pub files: Mutex<HashMap<String, Vec<u8>>>,
    pub commands: Mutex<Vec<String>>,
    pub destroyed: AtomicBool,
}

#[async_trait]
impl Sandbox for FakeSandbox {
    async fn exec(&self, command: &str, _timeout: Duration, max_output: usize) -> Result<ExecOutput> {
        self.commands.lock().unwrap().push(command.to_string());
        match command {
            "fatal" => Err(SandboxFatal("wedged".into()).into()),
            "fail" => Ok(ExecOutput {
                stdout: b"partial".to_vec(),
                stderr: b"boom".to_vec(),
                stdout_total: 7,
                stderr_total: 4,
                exit_code: Some(1),
            }),
            "big" => Ok(ExecOutput {
                stdout: vec![b'x'; max_output],
                stdout_total: 100_000,
                exit_code: Some(0),
                ..Default::default()
            }),
            other => {
                let out = format!("ran: {other}");
                Ok(ExecOutput { stdout_total: out.len(), stdout: out.into_bytes(), exit_code: Some(0), ..Default::default() })
            }
        }
    }

    async fn read_file(&self, path: &str, max_bytes: usize) -> Result<FileContent> {
        let files = self.files.lock().unwrap();
        let bytes = files.get(path).ok_or_else(|| anyhow!("head: cannot open '{path}' for reading: No such file or directory"))?;
        Ok(FileContent { bytes: bytes[..bytes.len().min(max_bytes)].to_vec(), truncated: bytes.len() > max_bytes })
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        self.files.lock().unwrap().insert(path.to_string(), content.to_vec());
        Ok(())
    }

    async fn destroy(&self) {
        self.destroyed.store(true, Ordering::SeqCst);
    }
}

pub struct FakeFactory {
    pub created: Mutex<Vec<(Uuid, Arc<FakeSandbox>)>>,
    pub cleanups: AtomicUsize,
    pub healthy: AtomicBool,
}

impl FakeFactory {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { created: Mutex::new(vec![]), cleanups: AtomicUsize::new(0), healthy: AtomicBool::new(true) })
    }

    pub fn sandbox(&self, index: usize) -> Arc<FakeSandbox> {
        self.created.lock().unwrap()[index].1.clone()
    }

    pub fn all_destroyed(&self) -> bool {
        self.created.lock().unwrap().iter().all(|(_, s)| s.destroyed.load(Ordering::SeqCst))
    }
}

#[async_trait]
impl SandboxFactory for FakeFactory {
    async fn create(&self, run_id: Uuid) -> Result<Arc<dyn Sandbox>> {
        let sandbox = Arc::new(FakeSandbox::default());
        self.created.lock().unwrap().push((run_id, sandbox.clone()));
        Ok(sandbox)
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }

    async fn health(&self) -> Result<()> {
        if self.healthy.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(anyhow!("docker is down"))
        }
    }
}

// -- HTTP harness ----------------------------------------------------------------------------

pub const KEY_A: &str = "key-a";
pub const KEY_B: &str = "key-b";

#[derive(Clone)]
pub struct TestApp {
    pub state: AppState,
    pub router: Router,
    pub llm: Arc<ScriptedLlm>,
    pub factory: Arc<FakeFactory>,
}

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub id: i64,
    pub event: String,
    pub data: Value,
}

pub fn test_config(extra: &[(&str, &str)]) -> Config {
    let mut env: HashMap<String, String> = [
        ("ANTHROPIC_API_KEY", "sk-test"),
        ("CEDAR_API_KEYS", "key-a,key-b"),
        ("MAX_ACTIVE_RUNS_PER_KEY", "10"),
        ("MAX_TURNS_CAP", "10"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    Config::from_lookup(|k| env.get(k).cloned()).unwrap()
}

impl TestApp {
    pub async fn new(script: Vec<Step>) -> Self {
        Self::with_env(script, &[]).await
    }

    pub async fn with_env(script: Vec<Step>, extra: &[(&str, &str)]) -> Self {
        let store = Store::connect_memory().await.unwrap();
        Self::build(store, ScriptedLlm::new(script), FakeFactory::new(), extra)
    }

    pub fn build(store: Store, llm: Arc<ScriptedLlm>, factory: Arc<FakeFactory>, extra: &[(&str, &str)]) -> Self {
        let state = AppState::new(test_config(extra), store, llm.clone(), factory.clone());
        let router = api::router(state.clone());
        Self { state, router, llm, factory }
    }

    pub async fn call(&self, method: Method, uri: &str, key: Option<&str>, body: Option<Value>) -> (StatusCode, axum::http::HeaderMap, String) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(key) = key {
            req = req.header("authorization", format!("Bearer {key}"));
        }
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        (parts.status, parts.headers, String::from_utf8(bytes.to_vec()).unwrap())
    }

    pub async fn json(&self, method: Method, uri: &str, key: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let (status, _, text) = self.call(method, uri, key, body).await;
        let value = if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}")) };
        (status, value)
    }

    pub async fn get(&self, uri: &str, key: &str) -> (StatusCode, Value) {
        self.json(Method::GET, uri, Some(key), None).await
    }

    pub async fn create_run(&self, key: &str, prompt: &str) -> Value {
        self.create_run_with(key, json!({ "prompt": prompt })).await
    }

    pub async fn create_run_with(&self, key: &str, body: Value) -> Value {
        let (status, run) = self.json(Method::POST, "/v1/runs", Some(key), Some(body)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        run
    }

    /// Poll until the run's status is `want`, panicking after 5 seconds.
    pub async fn wait_status(&self, key: &str, id: &str, want: &str) -> Value {
        let mut last = Value::Null;
        for _ in 0..500 {
            let (status, run) = self.get(&format!("/v1/runs/{id}"), key).await;
            assert_eq!(status, StatusCode::OK, "{run}");
            if run["status"] == want {
                return run;
            }
            last = run;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("run {id} never reached {want}; last seen: {last}");
    }

    /// Read a run's SSE stream to its end. `last_event_id` is sent as the `Last-Event-ID` header.
    pub async fn events(&self, key: &str, id: &str, last_event_id: Option<i64>) -> Vec<SseEvent> {
        let mut req = Request::builder()
            .uri(format!("/v1/runs/{id}/events"))
            .header("authorization", format!("Bearer {key}"));
        if let Some(n) = last_event_id {
            req = req.header("last-event-id", n.to_string());
        }
        let resp = self.router.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        parse_sse(std::str::from_utf8(&bytes).unwrap())
    }
}

pub fn parse_sse(raw: &str) -> Vec<SseEvent> {
    raw.split("\n\n")
        .filter_map(|block| {
            let (mut id, mut event, mut data) = (None, None, None);
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("id:") {
                    id = Some(v.trim().parse().unwrap());
                } else if let Some(v) = line.strip_prefix("event:") {
                    event = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("data:") {
                    data = Some(serde_json::from_str(v.trim()).unwrap());
                }
            }
            Some(SseEvent { id: id?, event: event?, data: data? })
        })
        .collect()
}

pub fn kinds(events: &[SseEvent]) -> Vec<&str> {
    events.iter().map(|e| e.event.as_str()).collect()
}
