//! Client for OpenAI-compatible chat completions (e.g. a local vLLM server).
//!
//! cedar keeps conversations as content blocks (`text`, `tool_use`, `tool_result`, and `image` /
//! `document` for files sent with a prompt) and tool definitions as `{name, description,
//! input_schema}`. This module translates those to and from the chat-completions wire format, so
//! the agent loop and the stored history never see it.

use crate::models::Usage;
use anyhow::{anyhow, Context, Result};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub struct MessageRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    /// Sent as `reasoning_effort` when set (what the model accepts is up to its server).
    pub reasoning_effort: Option<&'a str>,
    pub system: &'a str,
    pub tools: &'a [Value],
    pub messages: &'a [Value],
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// The assistant's content blocks.
    pub content: Vec<Value>,
    pub stop_reason: Option<String>,
    pub usage: Usage,
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn create_message(&self, req: &MessageRequest<'_>) -> Result<LlmResponse>;
}

pub struct ChatCompletionsClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    max_attempts: u32,
    retry_base: Duration,
}

const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

impl ChatCompletionsClient {
    /// `base_url` includes the version prefix, e.g. `http://localhost:8000/v1`. The key is sent as
    /// a bearer token if present (vLLM only checks one when started with `--api-key`).
    pub fn new(base_url: String, api_key: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            base_url,
            api_key,
            max_attempts: 4,
            retry_base: Duration::from_secs(1),
        })
    }

    /// Total tries (first attempt included) and the first backoff delay, which doubles each time.
    pub fn with_retry(mut self, max_attempts: u32, base_delay: Duration) -> Self {
        self.max_attempts = max_attempts.max(1);
        self.retry_base = base_delay;
        self
    }

    fn backoff(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        let delay =
            retry_after.unwrap_or_else(|| self.retry_base * 2u32.saturating_pow(attempt - 1));
        let jitter_ms = u64::from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos()),
        ) % 250;
        delay.min(MAX_RETRY_DELAY) + Duration::from_millis(jitter_ms).min(self.retry_base)
    }
}

#[async_trait]
impl LlmClient for ChatCompletionsClient {
    async fn create_message(&self, req: &MessageRequest<'_>) -> Result<LlmResponse> {
        let body = build_request(req);
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut request = self.http.post(&url).json(&body);
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }
            let sent = request.send().await;

            // (error, whether it's worth retrying, server-suggested delay)
            let (err, retryable, retry_after) = match sent {
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(Duration::from_secs);
                    match resp.text().await {
                        Ok(text) if status.is_success() => return parse_response(&text),
                        Ok(text) => (
                            anyhow!("LLM API error {status}: {}", error_message(&text)),
                            status.as_u16() == 429 || status.is_server_error(),
                            retry_after,
                        ),
                        Err(e) => (anyhow!(e).context("reading API response"), true, None),
                    }
                }
                Err(e) => {
                    let retryable = e.is_timeout() || e.is_connect() || e.is_request();
                    (anyhow!(e).context("sending API request"), retryable, None)
                }
            };

            if !retryable || attempt >= self.max_attempts {
                return Err(err);
            }
            let delay = self.backoff(attempt, retry_after);
            tracing::warn!(attempt, ?delay, error = %err, "LLM request failed; retrying");
            tokio::time::sleep(delay).await;
        }
    }
}

// -- cedar blocks -> chat completions ----------------------------------------------------------

fn build_request(req: &MessageRequest<'_>) -> Value {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if !req.system.is_empty() {
        messages.push(json!({ "role": "system", "content": req.system }));
    }
    for m in req.messages {
        translate_message(m, &mut messages);
    }
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
    });
    if let Some(effort) = req.reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    if !req.tools.is_empty() {
        body["tools"] = req.tools.iter().map(translate_tool).collect();
    }
    body
}

fn translate_tool(def: &Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": def["name"],
            "description": def["description"],
            "parameters": def["input_schema"],
        },
    })
}

/// Append the chat-completions form of one stored message. A user message with `image` or
/// `document` blocks is sent as content parts (`image_url` and `file` data URLs); any other
/// message is sent as plain text. Block types the format has no place for are dropped.
fn translate_message(msg: &Value, out: &mut Vec<Value>) {
    let role = msg["role"].as_str().unwrap_or_default();
    let blocks = match &msg["content"] {
        Value::String(text) => {
            out.push(json!({ "role": role, "content": text }));
            return;
        }
        Value::Array(blocks) => blocks,
        _ => return,
    };
    let text = join_text(blocks);

    if role == "assistant" {
        let tool_calls: Vec<Value> = blocks
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| {
                let input = if b["input"].is_null() {
                    json!({})
                } else {
                    b["input"].clone()
                };
                json!({
                    "id": b["id"],
                    "type": "function",
                    "function": { "name": b["name"], "arguments": input.to_string() },
                })
            })
            .collect();
        if text.is_empty() && tool_calls.is_empty() {
            return;
        }
        let mut message = json!({ "role": "assistant", "content": text });
        if !tool_calls.is_empty() {
            message["tool_calls"] = Value::Array(tool_calls);
        }
        out.push(message);
        return;
    }

    // Tool results go first: they have to directly follow the assistant message that asked.
    for block in blocks.iter().filter(|b| b["type"] == "tool_result") {
        let output = tool_result_text(&block["content"]);
        let content = if block["is_error"] == true {
            format!("Error: {output}")
        } else {
            output
        };
        out.push(
            json!({ "role": "tool", "tool_call_id": block["tool_use_id"], "content": content }),
        );
    }
    if blocks.iter().any(|b| media_part(b).is_some()) {
        let parts: Vec<Value> = blocks
            .iter()
            .filter_map(|b| match b["type"].as_str() {
                Some("text") => b["text"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(|t| json!({ "type": "text", "text": t })),
                _ => media_part(b),
            })
            .collect();
        out.push(json!({ "role": role, "content": parts }));
    } else if !text.is_empty() {
        out.push(json!({ "role": role, "content": text }));
    }
}

/// The content part for an `image` or `document` block with a base64 source, if it is one.
fn media_part(block: &Value) -> Option<Value> {
    let source = &block["source"];
    let media_type = source["media_type"].as_str()?;
    let data = source["data"].as_str()?;
    let url = format!("data:{media_type};base64,{data}");
    match block["type"].as_str()? {
        "image" => Some(json!({ "type": "image_url", "image_url": { "url": url } })),
        "document" => Some(json!({
            "type": "file",
            "file": {
                "filename": block["title"].as_str().unwrap_or("document.pdf"),
                "file_data": url,
            },
        })),
        _ => None,
    }
}

fn join_text(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect()
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => join_text(blocks),
        _ => String::new(),
    }
}

// -- chat completions -> cedar blocks ----------------------------------------------------------

fn error_message(body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    parsed
        .as_ref()
        .and_then(|v| {
            // OpenAI: {"error": {"message": ..}}; vLLM: {"object": "error", "message": ..}.
            v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .or_else(|| v["message"].as_str())
        })
        .map(String::from)
        .unwrap_or_else(|| body.chars().take(300).collect())
}

fn parse_response(body: &str) -> Result<LlmResponse> {
    let v: Value = serde_json::from_str(body).context("API returned invalid JSON")?;
    let choice = v["choices"]
        .get(0)
        .ok_or_else(|| anyhow!("API response has no choices"))?;
    let message = &choice["message"];

    // Reasoning (`reasoning` / `reasoning_content`) is not part of the conversation we keep.
    let mut content = Vec::new();
    if let Some(text) = message["content"].as_str().filter(|t| !t.is_empty()) {
        content.push(json!({ "type": "text", "text": text }));
    }
    if let Some(calls) = message["tool_calls"].as_array() {
        content.extend(calls.iter().enumerate().map(|(i, c)| tool_use_block(c, i)));
    }

    let finish = choice["finish_reason"].as_str();
    let has_calls = content.iter().any(|b| b["type"] == "tool_use");
    let stop_reason = match finish {
        // Some servers report `stop` even when they parsed tool calls; they must still be run.
        Some("length") => Some("max_tokens"),
        _ if has_calls => Some("tool_use"),
        Some("stop") => Some("end_turn"),
        Some("tool_calls") => Some("tool_use"),
        other => other,
    };
    Ok(LlmResponse {
        content,
        stop_reason: stop_reason.map(String::from),
        usage: Usage {
            input_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        },
    })
}

fn tool_use_block(call: &Value, index: usize) -> Value {
    let id = call["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map_or_else(|| format!("call_{index}"), String::from);
    let name = call["function"]["name"].as_str().unwrap_or_default();
    // Arguments are a JSON string, but be lenient: some servers send an object. Anything that
    // isn't an object becomes `{}` so the tool reports the missing fields back to the model.
    let input = match &call["function"]["arguments"] {
        Value::String(raw) => serde_json::from_str::<Value>(raw).ok(),
        other => Some(other.clone()),
    }
    .filter(Value::is_object)
    .unwrap_or_else(|| {
        tracing::warn!(tool = name, "tool call arguments are not a JSON object");
        json!({})
    });
    json!({ "type": "tool_use", "id": id, "name": name, "input": input })
}

/// One client per registered model, keyed by the entry's id and swapped while cedar runs: an
/// admin's change to the registry (see `crate::integrations`) applies to the next model call,
/// in a new run or one already going.
#[derive(Default)]
pub struct LlmRouter {
    clients: ArcSwap<HashMap<String, Arc<dyn LlmClient>>>,
}

impl LlmRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The client for a registered model, or `None` if the entry is gone.
    pub fn client_for(&self, model_id: &str) -> Option<Arc<dyn LlmClient>> {
        self.clients.load().get(model_id).cloned()
    }

    /// Use `client` for the model `id` from now on.
    pub fn set(&self, id: &str, client: Arc<dyn LlmClient>) {
        self.clients.rcu(|clients| {
            let mut next = HashMap::clone(clients);
            next.insert(id.to_string(), client.clone());
            next
        });
    }

    pub fn remove(&self, id: &str) {
        self.clients.rcu(|clients| {
            let mut next = HashMap::clone(clients);
            next.remove(id);
            next
        });
    }

    /// Replace every client at once.
    pub fn replace_all(&self, clients: HashMap<String, Arc<dyn LlmClient>>) {
        self.clients.store(Arc::new(clients));
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.clients.load().keys().cloned().collect();
        ids.sort();
        ids
    }
}

/// The models an OpenAI-compatible server lists at `{base_url}/models`, to check an address
/// before using it. Errors say whether the server couldn't be reached or refused.
pub async fn list_models(
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<String>> {
    let http = reqwest::Client::builder()
        .connect_timeout(timeout)
        .timeout(timeout)
        .build()
        .context("building HTTP client")?;
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut req = http.get(&url);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("could not reach {url}: {}", root_cause(&e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("{url} answered {status}"));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|_| anyhow!("{url} did not answer like an OpenAI-compatible server"))?;
    let models = body["data"]
        .as_array()
        .ok_or_else(|| anyhow!("{url} did not answer like an OpenAI-compatible server"))?
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect();
    Ok(models)
}

/// The innermost error message ("Connection refused"), which says more than reqwest's own.
fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(messages: &[Value]) -> Value {
        build_request(&MessageRequest {
            model: "m",
            max_tokens: 50,
            reasoning_effort: None,
            system: "be nice",
            tools: &[],
            messages,
        })
    }

    #[test]
    fn builds_the_request_with_a_system_message_and_function_tools() {
        let tools = [json!({ "name": "bash", "description": "run it",
                             "input_schema": { "type": "object" } })];
        let messages = [json!({ "role": "user", "content": "hello" })];
        let body = build_request(&MessageRequest {
            model: "m",
            max_tokens: 50,
            reasoning_effort: None,
            system: "be nice",
            tools: &tools,
            messages: &messages,
        });
        assert_eq!(
            body,
            json!({
                "model": "m",
                "max_tokens": 50,
                "messages": [
                    { "role": "system", "content": "be nice" },
                    { "role": "user", "content": "hello" },
                ],
                "tools": [{ "type": "function", "function": {
                    "name": "bash", "description": "run it", "parameters": { "type": "object" } } }],
            })
        );
        assert!(request(&[]).get("tools").is_none());
    }

    #[test]
    fn a_reasoning_level_is_sent_as_reasoning_effort() {
        let messages = [json!({ "role": "user", "content": "hello" })];
        let body = build_request(&MessageRequest {
            model: "m",
            max_tokens: 50,
            reasoning_effort: Some("high"),
            system: "",
            tools: &[],
            messages: &messages,
        });
        assert_eq!(
            body,
            json!({
                "model": "m",
                "max_tokens": 50,
                "reasoning_effort": "high",
                "messages": [{ "role": "user", "content": "hello" }],
            })
        );
        assert!(request(&[]).get("reasoning_effort").is_none());
    }

    #[test]
    fn the_router_hands_out_the_client_of_a_registered_model() {
        let router = LlmRouter::new();
        assert!(router.client_for("a").is_none());
        let client: Arc<dyn LlmClient> =
            Arc::new(ChatCompletionsClient::new("http://a/v1".into(), None).unwrap());
        router.set("a", client);
        assert!(router.client_for("a").is_some());
        assert_eq!(router.ids(), ["a"]);
        router.remove("a");
        assert!(router.client_for("a").is_none());
    }

    #[test]
    fn assistant_tool_use_becomes_tool_calls_with_string_arguments() {
        let body = request(&[json!({ "role": "assistant", "content": [
            { "type": "text", "text": "running" },
            { "type": "tool_use", "id": "t1", "name": "bash", "input": { "command": "ls" } },
            { "type": "tool_use", "id": "t2", "name": "bash" },
        ] })]);
        assert_eq!(
            body["messages"][1],
            json!({ "role": "assistant", "content": "running", "tool_calls": [
                { "id": "t1", "type": "function",
                  "function": { "name": "bash", "arguments": "{\"command\":\"ls\"}" } },
                { "id": "t2", "type": "function",
                  "function": { "name": "bash", "arguments": "{}" } },
            ] })
        );
    }

    #[test]
    fn tool_results_become_tool_messages_before_any_user_text() {
        let body = request(&[json!({ "role": "user", "content": [
            { "type": "tool_result", "tool_use_id": "t1", "content": "ok" },
            { "type": "tool_result", "tool_use_id": "t2", "content": "boom", "is_error": true },
            { "type": "text", "text": "next" },
        ] })]);
        assert_eq!(
            body["messages"].as_array().unwrap()[1..],
            [
                json!({ "role": "tool", "tool_call_id": "t1", "content": "ok" }),
                json!({ "role": "tool", "tool_call_id": "t2", "content": "Error: boom" }),
                json!({ "role": "user", "content": "next" }),
            ]
        );
    }

    #[test]
    fn user_images_and_documents_become_content_parts() {
        let body = request(&[json!({ "role": "user", "content": [
            { "type": "tool_result", "tool_use_id": "t1", "content": "ok" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "aGk=" }, "title": "a.png" },
            { "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": "cGRm" }, "title": "b.pdf" },
            { "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": "cGRm" } },
            { "type": "text", "text": "what are these?" },
        ] })]);
        assert_eq!(
            body["messages"].as_array().unwrap()[1..],
            [
                json!({ "role": "tool", "tool_call_id": "t1", "content": "ok" }),
                json!({ "role": "user", "content": [
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,aGk=" } },
                    { "type": "file", "file": { "filename": "b.pdf", "file_data": "data:application/pdf;base64,cGRm" } },
                    { "type": "file", "file": { "filename": "document.pdf", "file_data": "data:application/pdf;base64,cGRm" } },
                    { "type": "text", "text": "what are these?" },
                ] }),
            ]
        );
    }

    #[test]
    fn user_messages_without_attachments_stay_strings() {
        let body = request(&[json!({ "role": "user", "content": [
            { "type": "text", "text": "a" },
            { "type": "text", "text": "b" },
        ] })]);
        assert_eq!(
            body["messages"][1],
            json!({ "role": "user", "content": "ab" })
        );
    }

    #[test]
    fn blocks_without_a_chat_equivalent_are_dropped() {
        let body = request(&[
            json!({ "role": "assistant", "content": [
                { "type": "server_tool_use", "id": "s1", "name": "web_search", "input": {} },
                { "type": "web_search_tool_result", "tool_use_id": "s1", "content": [] },
            ] }),
            json!({ "role": "assistant", "content": [
                { "type": "thinking", "thinking": "hm" },
                { "type": "text", "text": "answer" },
            ] }),
        ]);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages.len(),
            2,
            "system + the one non-empty assistant turn"
        );
        assert_eq!(
            messages[1],
            json!({ "role": "assistant", "content": "answer" })
        );
    }

    #[test]
    fn parses_text_and_usage() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hi"},
                       "finish_reason":"stop"}],
                       "usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}}"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.content, [json!({ "type": "text", "text": "hi" })]);
        assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(
            r.usage,
            Usage {
                input_tokens: 12,
                output_tokens: 3
            }
        );
        assert!(parse_response("{}").is_err());
        assert!(parse_response(r#"{"choices":[]}"#).is_err());
        assert!(parse_response("nope").is_err());
    }

    #[test]
    fn parses_tool_calls_and_ignores_reasoning() {
        let body = json!({ "choices": [{ "finish_reason": "tool_calls", "message": {
            "content": null,
            "reasoning": "let me think",
            "tool_calls": [
                { "id": "c1", "type": "function",
                  "function": { "name": "bash", "arguments": "{\"command\":\"ls\"}" } },
                { "type": "function", "function": { "name": "read_file", "arguments": {"path": "a"} } },
            ],
        } }] })
        .to_string();
        let r = parse_response(&body).unwrap();
        assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(
            r.content,
            [
                json!({ "type": "tool_use", "id": "c1", "name": "bash", "input": { "command": "ls" } }),
                json!({ "type": "tool_use", "id": "call_1", "name": "read_file", "input": { "path": "a" } }),
            ]
        );
    }

    #[test]
    fn tool_calls_win_over_a_stop_finish_reason_but_not_over_length() {
        let call = json!([{ "id": "c1", "function": { "name": "bash", "arguments": "{}" } }]);
        let reply = |finish: &str| {
            json!({ "choices": [{ "finish_reason": finish, "message": { "tool_calls": call } }] })
                .to_string()
        };
        assert_eq!(
            parse_response(&reply("stop"))
                .unwrap()
                .stop_reason
                .as_deref(),
            Some("tool_use")
        );
        assert_eq!(
            parse_response(&reply("length"))
                .unwrap()
                .stop_reason
                .as_deref(),
            Some("max_tokens")
        );
    }

    #[test]
    fn malformed_tool_arguments_become_an_empty_object() {
        let call =
            |args: Value| json!({ "id": "c1", "function": { "name": "bash", "arguments": args } });
        assert_eq!(tool_use_block(&call(json!("{oops")), 0)["input"], json!({}));
        assert_eq!(tool_use_block(&call(json!("[1]")), 0)["input"], json!({}));
        assert_eq!(tool_use_block(&call(json!(null)), 0)["input"], json!({}));
    }

    #[test]
    fn maps_other_finish_reasons() {
        let reply = |finish: &str| {
            json!({ "choices": [{ "finish_reason": finish, "message": { "content": "x" } }] })
                .to_string()
        };
        let stop = |finish| parse_response(&reply(finish)).unwrap().stop_reason;
        assert_eq!(stop("length").as_deref(), Some("max_tokens"));
        assert_eq!(stop("content_filter").as_deref(), Some("content_filter"));
    }

    #[test]
    fn extracts_api_error_messages() {
        assert_eq!(
            error_message(r#"{"error":{"type":"x","message":"bad model"}}"#),
            "bad model"
        );
        assert_eq!(
            error_message(r#"{"object":"error","message":"model not found","code":404}"#),
            "model not found"
        );
        assert_eq!(error_message(r#"{"error":"nope"}"#), "nope");
        assert_eq!(error_message("gateway timeout"), "gateway timeout");
    }

    #[test]
    fn backoff_grows_and_honors_retry_after() {
        let c = ChatCompletionsClient::new("http://x".into(), None)
            .unwrap()
            .with_retry(4, Duration::from_millis(100));
        assert!(c.backoff(1, None) >= Duration::from_millis(100));
        assert!(c.backoff(3, None) >= Duration::from_millis(400));
        assert!(c.backoff(1, Some(Duration::from_secs(7))) >= Duration::from_secs(7));
        assert!(
            c.backoff(1, Some(Duration::from_secs(9999)))
                < MAX_RETRY_DELAY + Duration::from_secs(1)
        );
    }
}
