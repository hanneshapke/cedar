//! A small client for the cedar HTTP API.

use crate::error::AppError;
use reqwest::StatusCode;
use serde::Deserialize;
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
        self.send(req.timeout(REQUEST_TIMEOUT))
            .await?
            .json()
            .await
            .map_err(|e| AppError::BadGateway(format!("unreadable response from cedar: {e}")))
    }

    /// Start a run. With `parent`, it continues that run's conversation and sandbox workspace.
    pub async fn create_run(&self, prompt: &str, model: Option<&str>, parent: Option<Uuid>) -> Result<RemoteRun, AppError> {
        let mut body = json!({ "prompt": prompt });
        if let Some(m) = model {
            body["model"] = Value::String(m.to_string());
        }
        if let Some(p) = parent {
            body["parent_id"] = Value::String(p.to_string());
        }
        self.json(self.http.post(self.url("/runs")).json(&body)).await
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

    pub async fn delete_run(&self, id: Uuid) -> Result<(), AppError> {
        self.send(self.http.delete(self.url(&format!("/runs/{id}"))).timeout(REQUEST_TIMEOUT)).await?;
        Ok(())
    }

    /// Open the run's Server-Sent Events stream, starting after event `after`.
    pub async fn events(&self, id: Uuid, after: i64) -> Result<reqwest::Response, AppError> {
        self.send(self.http.get(self.url(&format!("/runs/{id}/events"))).query(&[("after", after)])).await
    }
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
