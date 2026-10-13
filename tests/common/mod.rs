//! Test doubles and a small HTTP harness shared by the integration tests.
#![allow(dead_code)]

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use cedar::api::{self, AppState};
use cedar::browser::{Browser, BrowserFactory};
use cedar::config::Config;
use cedar::llm::{LlmClient, LlmResponse, MessageRequest};
use cedar::models::Usage;
use cedar::sandbox::{
    check_workspace_path, ExecOutput, FileContent, Sandbox, SandboxFactory, SandboxFatal,
    WorkspaceFile, WorkspaceListing, WorkspaceMissing,
};
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
        Arc::new(Self {
            script: Mutex::new(steps.into()),
            requests: Mutex::new(vec![]),
        })
    }

    pub fn call_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// A snapshot of every request made so far (cloned, so no lock is held by the caller).
    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
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
                _ => script
                    .pop_front()
                    .ok_or_else(|| anyhow!("script exhausted"))?,
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
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
        },
    }
}

pub fn text(t: &str) -> LlmResponse {
    reply(vec![json!({ "type": "text", "text": t })], "end_turn")
}

pub fn tool_use(id: &str, name: &str, input: Value) -> LlmResponse {
    reply(
        vec![json!({ "type": "tool_use", "id": id, "name": name, "input": input })],
        "tool_use",
    )
}

// -- fake sandbox ----------------------------------------------------------------------------

/// A workspace's files, shared by every sandbox that mounts it.
pub type Files = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// Commands with special meaning: `fatal` (sandbox breaks), `fail` (exit 1 with stderr),
/// `big` (output larger than the cap), and the changed-files listing that follows `bash` and
/// `run_python` (it prints `changed`, one path per line). Anything else echoes `ran: <command>`.
#[derive(Default)]
pub struct FakeSandbox {
    pub files: Files,
    pub commands: Mutex<Vec<String>>,
    /// The workspace files the listing of changes reports.
    pub changed: Mutex<Vec<String>>,
    pub destroyed: AtomicBool,
}

#[async_trait]
impl Sandbox for FakeSandbox {
    async fn exec(
        &self,
        command: &str,
        _timeout: Duration,
        max_output: usize,
    ) -> Result<ExecOutput> {
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
            listing if listing.contains(" -newer ") => {
                let out = self.changed.lock().unwrap().join("\n");
                Ok(ExecOutput {
                    stdout_total: out.len(),
                    stdout: out.into_bytes(),
                    exit_code: Some(0),
                    ..Default::default()
                })
            }
            other => {
                let out = format!("ran: {other}");
                Ok(ExecOutput {
                    stdout_total: out.len(),
                    stdout: out.into_bytes(),
                    exit_code: Some(0),
                    ..Default::default()
                })
            }
        }
    }

    async fn read_file(&self, path: &str, max_bytes: usize) -> Result<FileContent> {
        let files = self.files.lock().unwrap();
        let bytes = files.get(path).ok_or_else(|| {
            anyhow!("head: cannot open '{path}' for reading: No such file or directory")
        })?;
        Ok(FileContent {
            bytes: bytes[..bytes.len().min(max_bytes)].to_vec(),
            truncated: bytes.len() > max_bytes,
        })
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), content.to_vec());
        Ok(())
    }

    async fn destroy(&self) {
        self.destroyed.store(true, Ordering::SeqCst);
    }
}

/// Workspaces are shared file maps, so a continuation sees the files its parent wrote.
pub struct FakeFactory {
    pub created: Mutex<Vec<(Uuid, Arc<FakeSandbox>)>>,
    pub workspaces: Mutex<HashMap<Uuid, Files>>,
    pub removed_workspaces: Mutex<Vec<Uuid>>,
    pub cleanups: AtomicUsize,
    pub healthy: AtomicBool,
    /// Make `create` fail with an error full of "internal" detail.
    pub fail_create: AtomicBool,
}

impl FakeFactory {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            created: Mutex::new(vec![]),
            workspaces: Mutex::default(),
            removed_workspaces: Mutex::default(),
            cleanups: AtomicUsize::new(0),
            healthy: AtomicBool::new(true),
            fail_create: AtomicBool::new(false),
        })
    }

    pub fn sandbox(&self, index: usize) -> Arc<FakeSandbox> {
        self.created.lock().unwrap()[index].1.clone()
    }

    fn workspace_files(&self, workspace_id: Uuid) -> Result<Files> {
        self.workspaces
            .lock()
            .unwrap()
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| WorkspaceMissing.into())
    }

    pub fn all_destroyed(&self) -> bool {
        self.created
            .lock()
            .unwrap()
            .iter()
            .all(|(_, s)| s.destroyed.load(Ordering::SeqCst))
    }
}

#[async_trait]
impl SandboxFactory for FakeFactory {
    async fn create(&self, run_id: Uuid, workspace_id: Uuid) -> Result<Arc<dyn Sandbox>> {
        if self.fail_create.load(Ordering::SeqCst) {
            return Err(anyhow!(
                "dial unix /Users/someone/.docker/run/docker.sock: connect: no such file"
            ));
        }
        let files = self
            .workspaces
            .lock()
            .unwrap()
            .entry(workspace_id)
            .or_default()
            .clone();
        let sandbox = Arc::new(FakeSandbox {
            files,
            ..Default::default()
        });
        self.created.lock().unwrap().push((run_id, sandbox.clone()));
        Ok(sandbox)
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }

    async fn remove_workspace(&self, workspace_id: Uuid) -> Result<()> {
        self.workspaces.lock().unwrap().remove(&workspace_id);
        self.removed_workspaces.lock().unwrap().push(workspace_id);
        Ok(())
    }

    async fn list_workspaces(&self) -> Result<Vec<Uuid>> {
        Ok(self.workspaces.lock().unwrap().keys().copied().collect())
    }

    async fn list_workspace_files(&self, workspace_id: Uuid) -> Result<WorkspaceListing> {
        let files = self.workspace_files(workspace_id)?;
        let files = files.lock().unwrap();
        let mut files: Vec<WorkspaceFile> = files
            .iter()
            .map(|(path, bytes)| WorkspaceFile {
                path: path.clone(),
                size: bytes.len() as u64,
                modified_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            })
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(WorkspaceListing {
            files,
            truncated: false,
        })
    }

    async fn read_workspace_file(
        &self,
        workspace_id: Uuid,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<FileContent>> {
        check_workspace_path(path)?;
        let files = self.workspace_files(workspace_id)?;
        let files = files.lock().unwrap();
        Ok(files.get(path).map(|bytes| FileContent {
            bytes: bytes[..bytes.len().min(max_bytes)].to_vec(),
            truncated: bytes.len() > max_bytes,
        }))
    }

    async fn health(&self) -> Result<()> {
        if self.healthy.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(anyhow!("docker is down"))
        }
    }
}

// -- fake browser ----------------------------------------------------------------------------

/// Answers every request with a page whose text echoes the request, so tests can see what
/// reached the browser. A click on ref 99 fails like the driver does for a stale ref.
#[derive(Default)]
pub struct FakeBrowser {
    pub requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl Browser for FakeBrowser {
    async fn call(&self, request: &Value, _timeout: Duration) -> Result<Value> {
        self.requests.lock().unwrap().push(request.clone());
        if request["ref"] == 99 {
            return Err(anyhow!(
                "[99] is not on the current page; take a snapshot to see what is"
            ));
        }
        Ok(json!({
            "url": "https://example.com/listing",
            "title": "A listing",
            "status": 200,
            "text": format!("# A listing\n[1] button \"Show all reviews\"\nlast request: {request}"),
            "visible_chars": 40,
        }))
    }
}

#[derive(Default)]
pub struct FakeBrowserFactory {
    pub created: Mutex<Vec<(Uuid, Arc<FakeBrowser>)>>,
    pub destroyed: Mutex<Vec<Uuid>>,
    pub cleanups: AtomicUsize,
    pub unhealthy: AtomicBool,
}

impl FakeBrowserFactory {
    pub fn browser(&self, index: usize) -> Arc<FakeBrowser> {
        self.created.lock().unwrap()[index].1.clone()
    }
}

#[async_trait]
impl BrowserFactory for FakeBrowserFactory {
    async fn create(&self, run_id: Uuid) -> Result<Arc<dyn Browser>> {
        let browser = Arc::new(FakeBrowser::default());
        self.created.lock().unwrap().push((run_id, browser.clone()));
        Ok(browser)
    }

    async fn destroy(&self, run_id: Uuid) {
        self.destroyed.lock().unwrap().push(run_id);
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }

    async fn health(&self) -> Result<()> {
        if self.unhealthy.load(Ordering::SeqCst) {
            Err(anyhow!("browser image is missing"))
        } else {
            Ok(())
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
    pub browsers: Arc<FakeBrowserFactory>,
}

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub id: i64,
    pub event: String,
    pub data: Value,
}

pub fn test_config(extra: &[(&str, &str)]) -> Config {
    let mut env: HashMap<String, String> = [
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

    pub fn build(
        store: Store,
        llm: Arc<ScriptedLlm>,
        factory: Arc<FakeFactory>,
        extra: &[(&str, &str)],
    ) -> Self {
        let browsers = Arc::new(FakeBrowserFactory::default());
        let state = AppState::with_browsers(
            test_config(extra),
            store,
            llm.clone(),
            factory.clone(),
            browsers.clone(),
        )
        .unwrap();
        let router = api::router(state.clone());
        Self {
            state,
            router,
            llm,
            factory,
            browsers,
        }
    }

    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, axum::http::HeaderMap, String) {
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
        let resp = self
            .router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        (
            parts.status,
            parts.headers,
            String::from_utf8(bytes.to_vec()).unwrap(),
        )
    }

    pub async fn json(
        &self,
        method: Method,
        uri: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (status, _, text) = self.call(method, uri, key, body).await;
        let value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}"))
        };
        (status, value)
    }

    pub async fn get(&self, uri: &str, key: &str) -> (StatusCode, Value) {
        self.json(Method::GET, uri, Some(key), None).await
    }

    pub async fn create_run(&self, key: &str, prompt: &str) -> Value {
        self.create_run_with(key, json!({ "prompt": prompt })).await
    }

    pub async fn create_run_with(&self, key: &str, body: Value) -> Value {
        let (status, run) = self
            .json(Method::POST, "/v1/runs", Some(key), Some(body))
            .await;
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
        let resp = self
            .router
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        parse_sse(std::str::from_utf8(&bytes).unwrap())
    }
}

/// An SSE response read incrementally, so a test can observe events while a run is still going.
pub struct LiveStream {
    body: Body,
    buf: String,
}

impl LiveStream {
    pub async fn open(app: &TestApp, key: &str, id: &str, last_event_id: Option<i64>) -> Self {
        let mut req = Request::builder()
            .uri(format!("/v1/runs/{id}/events"))
            .header("authorization", format!("Bearer {key}"));
        if let Some(n) = last_event_id {
            req = req.header("last-event-id", n.to_string());
        }
        let resp = app
            .router
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        Self {
            body: resp.into_body(),
            buf: String::new(),
        }
    }

    /// The next event, or `None` once the server has closed the stream.
    pub async fn next_event(&mut self) -> Option<SseEvent> {
        loop {
            while let Some(end) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..end + 2).collect();
                // Keep-alive comments don't parse into events; skip them.
                if let Some(event) = parse_sse(&block).pop() {
                    return Some(event);
                }
            }
            let frame = self.body.frame().await?.expect("stream error");
            if let Ok(data) = frame.into_data() {
                self.buf.push_str(std::str::from_utf8(&data).unwrap());
            }
        }
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
            Some(SseEvent {
                id: id?,
                event: event?,
                data: data?,
            })
        })
        .collect()
}

pub fn kinds(events: &[SseEvent]) -> Vec<&str> {
    events.iter().map(|e| e.event.as_str()).collect()
}
