//! A small client for the cedar HTTP API.

use crate::error::AppError;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// The parts of a cedar run this service uses.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteRun {
    pub id: Uuid,
    pub status: String,
    pub final_text: Option<String>,
    pub error: Option<String>,
    /// Why a completed run stopped: `end_turn`, `max_tokens`, or `ask_user` when it is waiting
    /// for the user's answer.
    #[serde(default)]
    pub stop_reason: Option<String>,
}

pub fn is_terminal(status: &str) -> bool {
    !matches!(status, "queued" | "running")
}

#[derive(Clone)]
pub struct CedarClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Starting a run with attachments sends up to tens of MB that cedar decodes before answering.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Installing a community tool includes `pip install` of its packages.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(900);

/// A file sent with a message, as cedar's `POST /v1/runs` takes it: standard base64, no prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub name: String,
    pub media_type: String,
    pub data: String,
}

impl CedarClient {
    pub fn new(base_url: &str, api_key: &str) -> anyhow::Result<Self> {
        // No overall timeout on the client: event streams stay open for the whole run.
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).build()?;
        Ok(Self { http, base_url: base_url.trim_end_matches('/').to_string(), api_key: api_key.to_string() })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1{path}", self.base_url)
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response, AppError> {
        let resp = req
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| AppError::BadGateway(format!("could not reach cedar: {e}")))?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        Err(upstream_error(resp).await)
    }

    async fn json<T: for<'de> Deserialize<'de>>(&self, req: reqwest::RequestBuilder) -> Result<T, AppError> {
        self.json_within(req, REQUEST_TIMEOUT).await
    }

    async fn json_within<T: for<'de> Deserialize<'de>>(&self, req: reqwest::RequestBuilder, timeout: Duration) -> Result<T, AppError> {
        self.send(req.timeout(timeout))
            .await?
            .json()
            .await
            .map_err(|e| AppError::BadGateway(format!("unreadable response from cedar: {e}")))
    }

    /// Start a run. With `opts.parent`, it continues that run's conversation and sandbox workspace.
    pub async fn create_run(&self, prompt: &str, opts: &RunOptions<'_>) -> Result<RemoteRun, AppError> {
        let mut body = json!({ "prompt": prompt });
        if let Some(m) = opts.model {
            body["model"] = json!(m);
        }
        if let Some(r) = opts.reasoning {
            body["reasoning"] = json!(r);
        }
        if let Some(s) = opts.system {
            body["system"] = json!(s);
        }
        if let Some(n) = opts.max_turns {
            body["max_turns"] = json!(n);
        }
        if let Some(t) = opts.tools {
            body["tools"] = json!(t);
        }
        if let Some(p) = opts.parent {
            body["parent_id"] = Value::String(p.to_string());
        }
        if let Some(p) = opts.plan {
            body["plan"] = json!(p);
        }
        if opts.interactive {
            body["interactive"] = json!(true);
        }
        if let Some(l) = opts.locale {
            body["locale"] = json!(l);
        }
        let mut timeout = REQUEST_TIMEOUT;
        if let Some(a) = opts.attachments.filter(|a| !a.is_empty()) {
            body["attachments"] = json!(a);
            timeout = UPLOAD_TIMEOUT;
        }
        self.json_within(self.http.post(self.url("/runs")).json(&body), timeout).await
    }

    /// The tool groups cedar offers: `{groups: [{id, label, available, reason?, tools}]}`.
    pub async fn tools(&self) -> Result<Value, AppError> {
        self.json(self.http.get(self.url("/tools"))).await
    }

    /// cedar's defaults for runs: `{model, model_id, system, max_turns_cap, attachments}`.
    pub async fn defaults(&self) -> Result<Value, AppError> {
        self.json(self.http.get(self.url("/defaults"))).await
    }

    /// The models cedar has registered, in picker order: each
    /// `{id, label, model, reasoning_levels, default_reasoning, is_default}`.
    pub async fn models(&self) -> Result<Vec<Value>, AppError> {
        let body: Value = self.json(self.http.get(self.url("/models"))).await?;
        Ok(body["models"].as_array().cloned().unwrap_or_default())
    }

    /// What cedar's runs remember about the user (`GET /v1/memory`): `{items, limits}`. `None` from
    /// a cedar from before memory, which doesn't know the route.
    pub async fn memory(&self) -> Result<Option<Value>, AppError> {
        match self.json(self.http.get(self.url("/memory"))).await {
            Err(AppError::Upstream { status: StatusCode::NOT_FOUND, .. }) => Ok(None),
            other => other.map(Some),
        }
    }

    /// Remember `text` by hand; cedar answers with the item.
    pub async fn add_memory(&self, text: &str) -> Result<Value, AppError> {
        self.json(self.http.post(self.url("/memory")).json(&json!({ "text": text }))).await
    }

    /// Forget one memory; a 404 means it isn't there (any more).
    pub async fn delete_memory(&self, id: Uuid) -> Result<(), AppError> {
        self.send(self.http.delete(self.url(&format!("/memory/{id}"))).timeout(REQUEST_TIMEOUT)).await?;
        Ok(())
    }

    /// Forget everything cedar's runs remember about the user.
    pub async fn delete_all_memory(&self) -> Result<(), AppError> {
        self.send(self.http.delete(self.url("/memory")).timeout(REQUEST_TIMEOUT)).await?;
        Ok(())
    }

    // -- admin: integrations (needs CEDAR_API_KEY to be one of cedar's CEDAR_ADMIN_KEYS) --------

    pub async fn integrations(&self) -> Result<Value, AppError> {
        admin(self.json(self.http.get(self.url("/admin/integrations"))).await)
    }

    pub async fn set_brave_key(&self, api_key: &str) -> Result<Value, AppError> {
        let req = self.http.put(self.url("/admin/integrations/brave")).json(&json!({ "api_key": api_key }));
        admin(self.json(req).await)
    }

    /// The registered models as admins see them (server address, whether a key is saved, what
    /// the server listed).
    pub async fn models_admin(&self) -> Result<Value, AppError> {
        admin(self.json(self.http.get(self.url("/admin/models"))).await)
    }

    /// The models a server lists, before registering one. `query` is cedar's `{base_url,
    /// api_key?, id?}`, passed through as it is.
    pub async fn server_models(&self, query: &Value) -> Result<Value, AppError> {
        let req = self.http.post(self.url("/admin/server-models")).json(query);
        admin(self.json(req).await)
    }

    /// Register a model or change one. `spec` is cedar's `{base_url, model, label?, api_key?,
    /// reasoning_levels?, default_reasoning?, is_default?}`, passed through as it is.
    pub async fn set_model(&self, id: &str, spec: &Value) -> Result<Value, AppError> {
        let req = self.http.put(self.url(&format!("/admin/models/{id}"))).json(spec);
        admin(self.json(req).await)
    }

    /// cedar's server-wide settings: `{max_image_side, sandbox_idle_minutes, defaults}`.
    pub async fn server_settings(&self) -> Result<Value, AppError> {
        admin(self.json(self.http.get(self.url("/admin/settings"))).await)
    }

    /// Change cedar's server-wide settings. `body` is cedar's `{max_image_side?,
    /// sandbox_idle_minutes?}`, passed through.
    pub async fn set_server_settings(&self, body: &Value) -> Result<Value, AppError> {
        let req = self.http.put(self.url("/admin/settings")).json(body);
        admin(self.json(req).await)
    }

    pub async fn remove_model(&self, id: &str) -> Result<(), AppError> {
        let req = self.http.delete(self.url(&format!("/admin/models/{id}"))).timeout(REQUEST_TIMEOUT);
        admin(self.send(req).await.map(drop))
    }

    pub async fn remove_brave_key(&self) -> Result<(), AppError> {
        let req = self.http.delete(self.url("/admin/integrations/brave")).timeout(REQUEST_TIMEOUT);
        admin(self.send(req).await.map(drop))
    }

    pub async fn set_bestbuy_key(&self, api_key: &str) -> Result<Value, AppError> {
        let req = self.http.put(self.url("/admin/integrations/bestbuy")).json(&json!({ "api_key": api_key }));
        admin(self.json(req).await)
    }

    pub async fn remove_bestbuy_key(&self) -> Result<(), AppError> {
        let req = self.http.delete(self.url("/admin/integrations/bestbuy")).timeout(REQUEST_TIMEOUT);
        admin(self.send(req).await.map(drop))
    }

    /// Save a SerpApi key for `search_flights`; cedar checks it first and answers with the
    /// integrations status.
    pub async fn set_serpapi_key(&self, api_key: &str) -> Result<Value, AppError> {
        let req = self.http.put(self.url("/admin/integrations/serpapi")).json(&json!({ "api_key": api_key }));
        admin(self.json(req).await)
    }

    pub async fn remove_serpapi_key(&self) -> Result<(), AppError> {
        let req = self.http.delete(self.url("/admin/integrations/serpapi")).timeout(REQUEST_TIMEOUT);
        admin(self.send(req).await.map(drop))
    }

    /// The Google sign-in URL for "Connect Google"; Google sends the browser back to `redirect_uri`.
    pub async fn google_authorize(&self, redirect_uri: &str) -> Result<String, AppError> {
        let req = self.http.post(self.url("/admin/integrations/google/authorize")).json(&json!({ "redirect_uri": redirect_uri }));
        let resp: Value = admin(self.json(req).await)?;
        resp["url"].as_str().map(String::from).ok_or_else(|| AppError::BadGateway("cedar returned no sign-in URL".into()))
    }

    pub async fn google_callback(&self, state: &str, code: &str) -> Result<Value, AppError> {
        let req = self.http.post(self.url("/admin/integrations/google/callback")).json(&json!({ "state": state, "code": code }));
        admin(self.json(req).await)
    }

    pub async fn remove_google(&self) -> Result<(), AppError> {
        let req = self.http.delete(self.url("/admin/integrations/google")).timeout(REQUEST_TIMEOUT);
        admin(self.send(req).await.map(drop))
    }

    // -- admin: personal and community tools ---------------------------------------------------

    /// Every personal tool in cedar's `TOOLS_DIR`, with its permissions and saved secrets.
    pub async fn personal_tools(&self) -> Result<Value, AppError> {
        admin(self.json(self.http.get(self.url("/admin/tools"))).await)
    }

    /// Save a secret a personal tool declares. `tool` and `secret` were checked by the caller.
    pub async fn set_tool_secret(
        &self,
        tool: &str,
        secret: &str,
        value: &str,
    ) -> Result<Value, AppError> {
        let req = self
            .http
            .put(self.url(&format!("/admin/tools/{tool}/secrets/{secret}")))
            .json(&json!({ "value": value }));
        admin(self.json(req).await)
    }

    pub async fn remove_tool_secret(&self, tool: &str, secret: &str) -> Result<(), AppError> {
        let req = self
            .http
            .delete(self.url(&format!("/admin/tools/{tool}/secrets/{secret}")))
            .timeout(REQUEST_TIMEOUT);
        admin(self.send(req).await.map(drop))
    }

    /// Install a tool's packages again.
    pub async fn setup_tool(&self, tool: &str) -> Result<Value, AppError> {
        let req = self
            .http
            .post(self.url(&format!("/admin/tools/{tool}/setup")));
        admin(self.json_within(req, INSTALL_TIMEOUT).await)
    }

    /// The community registry's tools, with what is installed.
    pub async fn marketplace(&self, refresh: bool) -> Result<Value, AppError> {
        let req = self
            .http
            .get(self.url("/admin/marketplace"))
            .query(&[("refresh", refresh)]);
        admin(self.json(req).await)
    }

    /// Install or update a community tool; `body` is cedar's `{accept: {...}}`, passed through.
    pub async fn install_tool(&self, tool: &str, body: &Value) -> Result<Value, AppError> {
        let req = self
            .http
            .post(self.url(&format!("/admin/marketplace/{tool}/install")))
            .json(body);
        admin(self.json_within(req, INSTALL_TIMEOUT).await)
    }

    pub async fn uninstall_tool(&self, tool: &str) -> Result<(), AppError> {
        let req = self
            .http
            .delete(self.url(&format!("/admin/marketplace/{tool}")))
            .timeout(REQUEST_TIMEOUT);
        admin(self.send(req).await.map(drop))
    }

    /// `None` if cedar no longer knows the run.
    pub async fn get_run(&self, id: Uuid) -> Result<Option<RemoteRun>, AppError> {
        match self.json(self.http.get(self.url(&format!("/runs/{id}")))).await {
            Err(AppError::Upstream { status: StatusCode::NOT_FOUND, .. }) => Ok(None),
            other => other.map(Some),
        }
    }

    pub async fn cancel_run(&self, id: Uuid) -> Result<(), AppError> {
        self.send(self.http.post(self.url(&format!("/runs/{id}/cancel"))).timeout(REQUEST_TIMEOUT)).await?;
        Ok(())
    }

    /// Agree to the plan a run is waiting on: `{"steps": [{id, title}, ...]}`.
    pub async fn answer_plan(&self, id: Uuid, body: &Value) -> Result<Value, AppError> {
        self.json(self.http.post(self.url(&format!("/runs/{id}/plan"))).json(body)).await
    }

    /// Answer an approval a run is waiting on: `{"decision": "allow" | "allow_always" | "deny"}`.
    pub async fn decide_approval(&self, id: Uuid, approval_id: &str, body: &Value) -> Result<Value, AppError> {
        let path = format!("/runs/{id}/approvals/{}", encode_path(approval_id));
        self.json(self.http.post(self.url(&path)).json(body)).await
    }

    pub async fn delete_run(&self, id: Uuid) -> Result<(), AppError> {
        self.send(self.http.delete(self.url(&format!("/runs/{id}"))).timeout(REQUEST_TIMEOUT)).await?;
        Ok(())
    }

    /// Open the run's Server-Sent Events stream, starting after event `after`.
    pub async fn events(&self, id: Uuid, after: i64) -> Result<reqwest::Response, AppError> {
        self.send(self.http.get(self.url(&format!("/runs/{id}/events"))).query(&[("after", after)])).await
    }

    /// Open the key's stream of finished runs (`GET /v1/events`), from after notification `after`.
    pub async fn notifications(&self, after: i64) -> Result<reqwest::Response, AppError> {
        self.send(self.http.get(self.url("/events")).query(&[("after", after)])).await
    }

    /// Ask cedar to compact a finished run's conversation into one self-contained prompt.
    pub async fn compact_run(&self, id: Uuid, model: Option<&str>) -> Result<String, AppError> {
        self.compact(json!({ "run_id": id }), model).await
    }

    /// The same, over a plain-text transcript (for a run cedar no longer has).
    pub async fn compact_transcript(&self, transcript: &str, model: Option<&str>) -> Result<String, AppError> {
        self.compact(json!({ "transcript": transcript }), model).await
    }

    async fn compact(&self, mut body: Value, model: Option<&str>) -> Result<String, AppError> {
        if let Some(m) = model {
            body["model"] = Value::String(m.to_string());
        }
        // One model call, which can take a while on a busy server.
        let resp: Value = self
            .send(self.http.post(self.url("/compact")).json(&body).timeout(Duration::from_secs(300)))
            .await?
            .json()
            .await
            .map_err(|e| AppError::BadGateway(format!("unreadable response from cedar: {e}")))?;
        match resp["prompt"].as_str().map(str::trim).filter(|p| !p.is_empty()) {
            Some(prompt) => Ok(prompt.to_string()),
            None => Err(AppError::BadGateway("cedar returned no prompt".into())),
        }
    }

    /// The files in the run's workspace: `{workspace_id, files: [{path, size, modified_at}], truncated}`.
    pub async fn list_files(&self, id: Uuid) -> Result<Value, AppError> {
        self.json(self.http.get(self.url(&format!("/runs/{id}/files")))).await
    }

    /// Start downloading a workspace file. The response carries the file's type, length and name.
    pub async fn download_file(&self, id: Uuid, path: &str) -> Result<reqwest::Response, AppError> {
        let url = self.url(&format!("/runs/{id}/files/{}", encode_path(path)));
        self.send(self.http.get(url).timeout(Duration::from_secs(300))).await
    }
}

/// How a run is started; `None` fields leave cedar's defaults (or, for a continuation, the
/// parent's).
#[derive(Debug, Default, Clone, Copy)]
pub struct RunOptions<'a> {
    pub model: Option<&'a str>,
    /// One of the model's reasoning levels.
    pub reasoning: Option<&'a str>,
    pub system: Option<&'a str>,
    pub max_turns: Option<u32>,
    pub tools: Option<&'a [String]>,
    pub parent: Option<Uuid>,
    /// Images (for a multimodal model) and PDFs (for any) to send with the prompt.
    pub attachments: Option<&'a [Attachment]>,
    /// Whether the run plans: `off`, `on`, `auto` or `tool`. `None` leaves it to cedar.
    pub plan: Option<&'a str>,
    /// Someone is there to answer: the run waits for its plan to be agreed and for risky calls
    /// to be approved. Off for scheduled runs.
    pub interactive: bool,
    /// The user's language and region; `None` leaves cedar's default (or the parent's).
    pub locale: Option<&'a str>,
}

/// cedar refuses the admin routes to a key that isn't an admin key: say which setting to fix.
fn admin<T>(result: Result<T, AppError>) -> Result<T, AppError> {
    match result {
        Err(AppError::Upstream { status: StatusCode::FORBIDDEN, .. }) => Err(AppError::BadGateway(
            "cedar refused: add this server's CEDAR_API_KEY to cedar's CEDAR_ADMIN_KEYS".into(),
        )),
        other => other,
    }
}

/// Percent-encode a workspace path for a URL, keeping `/` as the separator.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn upstream_error(resp: reqwest::Response) -> AppError {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let message = body["error"]["message"].as_str().unwrap_or("cedar request failed").to_string();
    if status == StatusCode::UNAUTHORIZED {
        // That's our key, not the browser's; a 401 here would look like a bad access token.
        return AppError::BadGateway("cedar rejected CEDAR_API_KEY".into());
    }
    if status.is_server_error() {
        return AppError::BadGateway(format!("cedar error {status}: {message}"));
    }
    let code = body["error"]["code"].as_str().unwrap_or("upstream").to_string();
    AppError::Upstream { status, code, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_encoded_per_segment() {
        assert_eq!(encode_path("out/rust-intro.pdf"), "out/rust-intro.pdf");
        assert_eq!(encode_path("a b/it's #1?.md"), "a%20b/it%27s%20%231%3F.md");
        assert_eq!(encode_path("✓.txt"), "%E2%9C%93.txt");
    }
}
