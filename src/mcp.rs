//! A small client for remote MCP servers over the streamable-HTTP transport: `initialize`,
//! `tools/list`, and `tools/call`, nothing else. Requests are authorized with an OAuth access token
//! that is refreshed from a long-lived refresh token (Google's servers take nothing else).

use crate::config::GoogleOAuthConfig;
use anyhow::{anyhow, bail, Context, Result};
use reqwest::header::{HeaderMap, ACCEPT, CONTENT_TYPE};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SESSION_HEADER: &str = "mcp-session-id";
/// Refresh the access token this long before Google says it expires.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);
/// Error bodies are cut to this many bytes before they reach the model or the log.
const ERROR_BODY_MAX: usize = 500;

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("building HTTP client")
}

/// Access tokens for one Google account, refreshed when they are about to expire.
pub struct GoogleOAuth {
    http: reqwest::Client,
    cfg: GoogleOAuthConfig,
    cached: Mutex<Option<(String, Instant)>>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

impl GoogleOAuth {
    pub fn new(cfg: GoogleOAuthConfig) -> Result<Self> {
        Ok(Self {
            http: http_client()?,
            cfg,
            cached: Mutex::new(None),
        })
    }

    /// A valid access token, refreshed first if the cached one is missing or about to expire.
    pub async fn token(&self, timeout: Duration) -> Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some((token, expires)) = cached.as_ref() {
            if Instant::now() + EXPIRY_MARGIN < *expires {
                return Ok(token.clone());
            }
        }
        let resp = self
            .http
            .post(&self.cfg.token_url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &self.cfg.client_id),
                ("client_secret", &self.cfg.client_secret),
                ("refresh_token", &self.cfg.refresh_token),
            ])
            .timeout(timeout)
            .send()
            .await
            .context("requesting a Google access token")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            // `invalid_grant` means the refresh token was revoked or expired: only a new one helps.
            bail!(
                "Google refused to issue an access token ({status}): {}",
                cut(&body)
            );
        }
        let body: TokenResponse = resp.json().await.context("reading the Google token")?;
        let expires = Instant::now() + Duration::from_secs(body.expires_in.unwrap_or(3600));
        *cached = Some((body.access_token.clone(), expires));
        Ok(body.access_token)
    }

    /// Forget the cached token, after a server rejected it.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}

/// A tool as `tools/list` describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// What a `tools/call` returned: its content flattened to text, and whether the tool failed.
#[derive(Debug, Clone, PartialEq)]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
}

/// One connection to one MCP server. The session the server hands out on `initialize` is kept and
/// re-established if the server forgets it.
pub struct McpClient {
    http: reqwest::Client,
    url: String,
    auth: std::sync::Arc<GoogleOAuth>,
    /// `Some` once initialized; the inner value is the server's session id, if it gave one.
    session: Mutex<Option<Option<String>>>,
    next_id: AtomicU64,
}

/// Why a single HTTP exchange failed, so [`McpClient::request`] knows whether a retry can help.
enum Failure {
    /// 401: the access token was rejected.
    Unauthorized,
    /// 404 on a request that carried a session id: the session has expired.
    SessionExpired,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for Failure {
    fn from(e: anyhow::Error) -> Self {
        Failure::Other(e)
    }
}

impl McpClient {
    pub fn new(url: String, auth: std::sync::Arc<GoogleOAuth>) -> Result<Self> {
        Ok(Self {
            http: http_client()?,
            url,
            auth,
            session: Mutex::new(None),
            next_id: AtomicU64::new(1),
        })
    }

    /// Every tool the server lists, following `nextCursor` across pages.
    pub async fn list_tools(&self, timeout: Duration) -> Result<Vec<RemoteTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        // A server that keeps returning cursors would otherwise keep us here forever.
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let result = self.request("tools/list", params, timeout).await?;
            for tool in result["tools"].as_array().into_iter().flatten() {
                let Some(name) = tool["name"].as_str() else {
                    continue;
                };
                tools.push(RemoteTool {
                    name: name.to_string(),
                    description: tool["description"].as_str().unwrap_or_default().to_string(),
                    input_schema: tool["inputSchema"].clone(),
                });
            }
            match result["nextCursor"].as_str() {
                Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
                _ => return Ok(tools),
            }
        }
        bail!("the server's tool list did not end after 50 pages")
    }

    pub async fn call_tool(
        &self,
        name: &str,
        args: &Value,
        timeout: Duration,
    ) -> Result<CallResult> {
        let args = if args.is_object() {
            args.clone()
        } else {
            json!({})
        };
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": args }),
                timeout,
            )
            .await?;
        Ok(CallResult {
            text: flatten_content(&result),
            is_error: result["isError"].as_bool().unwrap_or(false),
        })
    }

    /// Send one JSON-RPC request and return its `result`. A rejected token is refreshed and an
    /// expired session re-established, each once.
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let mut retried_auth = false;
        let mut retried_session = false;
        loop {
            let session = self.ensure_session(timeout).await?;
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
            match self.post(&body, session.as_deref(), timeout).await {
                Ok((_, reply)) => return rpc_result(reply, id, method),
                Err(Failure::Unauthorized) if !retried_auth => {
                    retried_auth = true;
                    self.auth.invalidate().await;
                }
                Err(Failure::SessionExpired) if !retried_session => {
                    retried_session = true;
                    *self.session.lock().await = None;
                }
                Err(Failure::Unauthorized) => {
                    bail!("the MCP server rejected the Google access token; check the refresh token's scopes")
                }
                Err(Failure::SessionExpired) => bail!("the MCP server keeps dropping the session"),
                Err(Failure::Other(e)) => return Err(e.context(format!("MCP {method}"))),
            }
        }
    }

    /// The session id to send, running the `initialize` handshake first if there is none yet.
    async fn ensure_session(&self, timeout: Duration) -> Result<Option<String>> {
        let mut session = self.session.lock().await;
        if let Some(id) = session.as_ref() {
            return Ok(id.clone());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let init = json!({ "jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "cedar", "version": env!("CARGO_PKG_VERSION") },
        } });
        let (headers, reply) = match self.post(&init, None, timeout).await {
            Ok(ok) => ok,
            Err(Failure::Unauthorized) => {
                // The cached token may have been revoked early; one fresh token is worth a try.
                self.auth.invalidate().await;
                self.post(&init, None, timeout).await.map_err(|e| match e {
                    Failure::Other(e) => e,
                    _ => anyhow!("the MCP server rejected the Google access token; check the refresh token's scopes"),
                })?
            }
            Err(Failure::SessionExpired) => bail!("MCP initialize: not found (404); check the URL"),
            Err(Failure::Other(e)) => return Err(e.context("MCP initialize")),
        };
        rpc_result(reply, id, "initialize")?;
        let sid = headers
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        match self.post(&note, sid.as_deref(), timeout).await {
            Ok(_) => {}
            Err(Failure::Other(e)) => return Err(e.context("MCP notifications/initialized")),
            Err(_) => bail!("MCP notifications/initialized was rejected"),
        }
        *session = Some(sid.clone());
        Ok(sid)
    }

    /// POST one message. Returns the response headers and the JSON-RPC reply (or `Null` for a
    /// notification's `202 Accepted`). The reply may come as plain JSON or as an SSE stream.
    async fn post(
        &self,
        body: &Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<(HeaderMap, Value), Failure> {
        let token = self.auth.token(timeout).await?;
        let mut req = self
            .http
            .post(&self.url)
            .bearer_auth(token)
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .json(body)
            .timeout(timeout);
        if let Some(sid) = session {
            req = req.header(SESSION_HEADER, sid);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| anyhow!("{}", without_url(e)))?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let is_sse = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let text = resp
            .text()
            .await
            .map_err(|e| anyhow!("reading the response: {}", without_url(e)))?;
        match status {
            StatusCode::UNAUTHORIZED => return Err(Failure::Unauthorized),
            StatusCode::NOT_FOUND if session.is_some() => return Err(Failure::SessionExpired),
            s if !s.is_success() => {
                return Err(anyhow!("HTTP {s}: {}", cut(&text)).into());
            }
            _ => {}
        }
        if body.get("id").is_none() || text.trim().is_empty() {
            return Ok((headers, Value::Null));
        }
        let reply = if is_sse {
            sse_reply(&text, &body["id"])
                .ok_or_else(|| anyhow!("the event stream held no reply"))?
        } else {
            serde_json::from_str(&text)
                .map_err(|e| anyhow!("the reply is not JSON ({e}): {}", cut(&text)))?
        };
        Ok((headers, reply))
    }
}

/// The `result` of a JSON-RPC reply, or its `error` as an error.
fn rpc_result(reply: Value, id: u64, method: &str) -> Result<Value> {
    if reply["id"] != json!(id) {
        bail!("{method}: the reply does not answer the request");
    }
    if let Some(err) = reply.get("error") {
        let message = err["message"].as_str().unwrap_or("unknown error");
        bail!("{method} failed: {message} (code {})", err["code"]);
    }
    Ok(reply["result"].clone())
}

/// The JSON-RPC response with `id` among the `data:` payloads of an SSE body. Server requests and
/// notifications sent before it are skipped.
fn sse_reply(body: &str, id: &Value) -> Option<Value> {
    let mut data = String::new();
    let mut found = None;
    let mut flush = |data: &mut String| {
        if let Ok(msg) = serde_json::from_str::<Value>(data) {
            if found.is_none() && &msg["id"] == id && msg.get("method").is_none() {
                found = Some(msg);
            }
        }
        data.clear();
    };
    for line in body.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            flush(&mut data);
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    flush(&mut data);
    found
}

/// A `tools/call` result as text: text blocks as they are, embedded text resources by their text,
/// other blocks as a placeholder. A result with no content falls back to `structuredContent`.
pub fn flatten_content(result: &Value) -> String {
    let mut parts = Vec::new();
    for block in result["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => parts.push(block["text"].as_str().unwrap_or_default().to_string()),
            Some("resource") if block["resource"]["text"].is_string() => {
                parts.push(block["resource"]["text"].as_str().unwrap().to_string())
            }
            Some("resource_link") => parts.push(format!(
                "[link: {}]",
                block["uri"].as_str().unwrap_or_default()
            )),
            Some(other) => parts.push(format!("[{other} content omitted]")),
            None => {}
        }
    }
    if parts.is_empty() {
        if let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null()) {
            return structured.to_string();
        }
    }
    parts.join("\n")
}

fn cut(s: &str) -> String {
    crate::tools::truncate_output(s.trim(), ERROR_BODY_MAX)
}

/// A reqwest error without the URL it carries.
fn without_url(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "the request timed out".into()
    } else if e.is_connect() {
        "could not connect".into()
    } else {
        e.without_url().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_is_picked_out_of_an_event_stream() {
        let body = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\r\n\r\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"roots/list\"}\n\n\
                    data: {\"jsonrpc\":\"2.0\",\n\
                    data:  \"id\":7,\"result\":{\"ok\":true}}\n\n";
        let reply = sse_reply(body, &json!(7)).unwrap();
        assert_eq!(reply["result"]["ok"], true);
        assert!(sse_reply(body, &json!(8)).is_none());
        // A stream that ends without a blank line still counts.
        assert!(sse_reply("data: {\"id\":1,\"result\":{}}", &json!(1)).is_some());
    }

    #[test]
    fn rpc_errors_become_errors() {
        let err = rpc_result(
            json!({ "jsonrpc": "2.0", "id": 3, "error": { "code": -32602, "message": "bad params" } }),
            3,
            "tools/call",
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "tools/call failed: bad params (code -32602)"
        );
        assert!(rpc_result(json!({ "id": 4, "result": {} }), 3, "x").is_err());
        assert_eq!(
            rpc_result(json!({ "id": 3, "result": { "a": 1 } }), 3, "x").unwrap(),
            json!({ "a": 1 })
        );
    }

    #[test]
    fn content_is_flattened_to_text() {
        let result = json!({ "content": [
            { "type": "text", "text": "first" },
            { "type": "image", "data": "...", "mimeType": "image/png" },
            { "type": "resource", "resource": { "uri": "x", "text": "embedded" } },
            { "type": "resource_link", "uri": "https://example.com/a" },
        ] });
        assert_eq!(
            flatten_content(&result),
            "first\n[image content omitted]\nembedded\n[link: https://example.com/a]"
        );
        assert_eq!(
            flatten_content(&json!({ "content": [], "structuredContent": { "n": 1 } })),
            r#"{"n":1}"#
        );
        assert_eq!(flatten_content(&json!({})), "");
    }
}
