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
    /// The model's reasoning (`reasoning_content` or `reasoning`), if its server returns it. Shown
    /// to the user, not kept in the conversation.
    pub reasoning: Option<String>,
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
        let body = build_request(req, token_limit_field(&self.base_url));
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

/// The name a server at `base_url` takes the reply's token limit under. OpenAI's own API refuses
/// `max_tokens` for its reasoning models (GPT-5, o-series) and wants `max_completion_tokens`,
/// which every model there accepts; other servers keep `max_tokens`, which not all of them know
/// by the newer name.
fn token_limit_field(base_url: &str) -> &'static str {
    let host = reqwest::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    if host.as_deref() == Some("api.openai.com") {
        "max_completion_tokens"
    } else {
        "max_tokens"
    }
}

fn build_request(req: &MessageRequest<'_>, token_limit: &str) -> Value {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if !req.system.is_empty() {
        messages.push(json!({ "role": "system", "content": req.system }));
    }
    for m in req.messages {
        translate_message(m, &mut messages);
    }
    let mut body = json!({
        "model": req.model,
        "messages": messages,
    });
    body[token_limit] = json!(req.max_tokens);
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

    // Reasoning (`reasoning_content` / `reasoning`) is not part of the conversation we keep; it
    // is returned on its own for the event that shows it.
    let reasoning = ["reasoning_content", "reasoning"]
        .iter()
        .find_map(|k| message[k].as_str().filter(|t| !t.trim().is_empty()))
        .map(String::from);
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
        reasoning,
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
/// What a model server says about itself at `/metrics`, beside its API (so without the `/v1` of
/// `base_url`): llama.cpp's `llama-server` started with `--metrics`. `None` when the server has
/// no such page, can't be reached, or it is another server's.
pub async fn server_stats(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Option<Value> {
    let http = reqwest::Client::builder()
        .connect_timeout(timeout)
        .timeout(timeout)
        .build()
        .ok()?;
    let base = base_url.trim_end_matches('/');
    let url = format!("{}/metrics", base.strip_suffix("/v1").unwrap_or(base));
    let mut req = http.get(&url);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    llama_stats(&resp.text().await.ok()?)
}

/// llama-server's metrics (Prometheus text, every name prefixed `llamacpp:`) as the numbers
/// Settings shows. The speeds are averages since the server started, 0 before its first reply.
fn llama_stats(text: &str) -> Option<Value> {
    let metric = |name: &str| {
        text.lines().find_map(|line| {
            let (key, value) = line.trim().split_once(char::is_whitespace)?;
            let value: f64 = (key.strip_prefix("llamacpp:")? == name)
                .then(|| value.trim().parse().ok())??;
            value.is_finite().then_some(value)
        })
    };
    let count = |name: &str| metric(name).map(|v| v.max(0.0).round() as u64);
    // Without the tokens it has written, this is not llama-server's page.
    let generated = count("tokens_predicted_total")?;
    Some(json!({
        "server": "llama.cpp",
        "generated_tokens": generated,
        "generation_tokens_per_second": metric("predicted_tokens_seconds"),
        "prompt_tokens": count("prompt_tokens_total"),
        "prompt_tokens_per_second": metric("prompt_tokens_seconds"),
        "requests_processing": count("requests_processing"),
        "requests_deferred": count("requests_deferred"),
        "max_context_tokens": count("n_tokens_max"),
    }))
}

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
    model_ids(&body).ok_or_else(|| anyhow!("{url} did not answer like an OpenAI-compatible server"))
}

/// How large `model`'s context window is, as its server tells: vLLM, LM Studio and OpenRouter
/// put it in the model list (`max_model_len`, `context_length`), llama.cpp's server in `/props`
/// (`n_ctx`, per slot). `None` when it doesn't say or can't be asked. llama.cpp's `n_ctx_train`
/// is left alone: it is what the model was trained with, not what the server was started with.
pub async fn detect_context_window(
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
    timeout: Duration,
) -> Option<u32> {
    let http = reqwest::Client::builder()
        .connect_timeout(timeout)
        .timeout(timeout)
        .build()
        .ok()?;
    let get = |url: String| {
        let mut req = http.get(url);
        if let Some(key) = api_key {
            req = req.bearer_auth(key);
        }
        async move {
            let resp = req.send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            resp.json::<Value>().await.ok()
        }
    };
    let base = base_url.trim_end_matches('/');
    if let Some(body) = get(format!("{base}/models")).await {
        if let Some(window) = window_in_model_list(&body, model) {
            return Some(window);
        }
    }
    let root = base.strip_suffix("/v1").unwrap_or(base);
    let props = get(format!("{root}/props")).await?;
    window_in_props(&props)
}

fn as_window(v: &Value) -> Option<u32> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n >= crate::models::MIN_CONTEXT_WINDOW)
}

/// The window a model list gives for `model`.
fn window_in_model_list(body: &Value, model: &str) -> Option<u32> {
    let list = body.get("data").and_then(Value::as_array).or_else(|| body.as_array())?;
    let entry = list.iter().find(|m| m["id"] == model)?;
    ["max_model_len", "context_length", "context_window", "max_context_length"]
        .iter()
        .find_map(|k| as_window(&entry[*k]))
}

/// The window llama.cpp's `/props` gives.
fn window_in_props(props: &Value) -> Option<u32> {
    as_window(&props["default_generation_settings"]["n_ctx"]).or_else(|| as_window(&props["n_ctx"]))
}

/// What a model server made of a prompt that carries an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProbe {
    /// It answered: the model takes images.
    Accepts,
    /// It refused the request as it stands: the model takes text only.
    Rejects,
    /// It could not be asked (unreachable, an error of its own, a refused key).
    Unknown,
}

/// A small white square, as a data URL: the image of [`accepts_images`]. 32 pixels a side,
/// because some servers refuse an image that is only a few pixels wide.
const PROBE_IMAGE: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAIAAAD8GO2jAAAAJklEQVR42u3NMQ0AAAwDoPo33arYsQQMkB6LQCAQCAQCgUAg+BIMi1X0ptsIcT0AAAAASUVORK5CYII=";

/// Whether `model` at an OpenAI-compatible server takes images: one chat completion with a small
/// image and room for a single token. A server that answers accepts them; one that refuses the
/// request (400, 404, 415, 422) does not. A server that silently drops the image looks like one
/// that accepts it, which no request can tell apart.
pub async fn accepts_images(
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
    timeout: Duration,
) -> ImageProbe {
    let Ok(http) = reqwest::Client::builder().connect_timeout(timeout).timeout(timeout).build() else {
        return ImageProbe::Unknown;
    };
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": [
            { "type": "text", "text": "." },
            { "type": "image_url", "image_url": { "url": PROBE_IMAGE } },
        ] }],
    });
    body[token_limit_field(base_url)] = json!(1);
    let mut req = http.post(&url).json(&body);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    match req.send().await.map(|resp| resp.status().as_u16()) {
        Ok(200..=299) => ImageProbe::Accepts,
        Ok(400 | 404 | 415 | 422) => ImageProbe::Rejects,
        _ => ImageProbe::Unknown,
    }
}

/// The ids in a model list: OpenAI's `{"data": [{"id": ...}]}`, or the bare array of the same
/// objects that some hosted providers answer with.
fn model_ids(body: &Value) -> Option<Vec<String>> {
    let list = body.get("data").and_then(Value::as_array).or_else(|| body.as_array())?;
    Some(list.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
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

    #[test]
    fn context_windows_are_read_where_servers_put_them() {
        let vllm = json!({ "data": [{ "id": "other", "max_model_len": 4096 }, { "id": "m", "max_model_len": 32768 }] });
        assert_eq!(window_in_model_list(&vllm, "m"), Some(32768));
        let llama = json!({ "data": [{ "id": "m", "meta": { "n_ctx_train": 262144 } }] });
        assert_eq!(window_in_model_list(&llama, "m"), None);
        let props = json!({ "default_generation_settings": { "n_ctx": 32768 } });
        assert_eq!(window_in_props(&props), Some(32768));
        assert_eq!(window_in_props(&json!({ "n_ctx": 8192 })), Some(8192));
        assert_eq!(window_in_props(&json!({ "n_ctx": 0 })), None);
    }

    #[test]
    fn llama_servers_metrics_become_stats() {
        let page = "# HELP llamacpp:prompt_tokens_total Number of prompt tokens processed.\n\
                    # TYPE llamacpp:prompt_tokens_total counter\n\
                    llamacpp:prompt_tokens_total 5120\n\
                    llamacpp:prompt_tokens_seconds 812.5\n\
                    llamacpp:tokens_predicted_total 2048\n\
                    llamacpp:predicted_tokens_seconds 41.25\n\
                    llamacpp:n_tokens_max 9000\n\
                    llamacpp:requests_processing 1\n\
                    llamacpp:requests_deferred 0\n";
        assert_eq!(
            llama_stats(page),
            Some(json!({
                "server": "llama.cpp",
                "generated_tokens": 2048, "generation_tokens_per_second": 41.25,
                "prompt_tokens": 5120, "prompt_tokens_per_second": 812.5,
                "requests_processing": 1, "requests_deferred": 0, "max_context_tokens": 9000,
            }))
        );
        // A number it leaves out, or that is no number, is null; the rest stands.
        let some = llama_stats("llamacpp:tokens_predicted_total 0\nllamacpp:predicted_tokens_seconds nan\n").unwrap();
        assert_eq!((&some["generated_tokens"], &some["generation_tokens_per_second"]), (&json!(0), &Value::Null));
        // Another server's metrics, or a page that is none.
        assert_eq!(llama_stats("vllm:generation_tokens_total 12\n"), None);
        assert_eq!(llama_stats("<html>not found</html>"), None);
    }

    #[test]
    fn model_lists_with_and_without_a_data_field() {
        let ids = vec!["a".to_string(), "b".to_string()];
        assert_eq!(model_ids(&json!({"object": "list", "data": [{"id": "a"}, {"id": "b"}]})), Some(ids.clone()));
        assert_eq!(model_ids(&json!([{"id": "a", "type": "chat"}, {"id": "b"}])), Some(ids));
        assert_eq!(model_ids(&json!({"data": []})), Some(vec![]));
        // Not a list at all: an error page, or another API's answer.
        assert_eq!(model_ids(&json!({"error": "no"})), None);
        assert_eq!(model_ids(&json!({"data": "x"})), None);
        assert_eq!(model_ids(&json!("ok")), None);
    }

    #[test]
    fn openai_gets_max_completion_tokens_and_other_servers_max_tokens() {
        assert_eq!(token_limit_field("https://api.openai.com/v1"), "max_completion_tokens");
        assert_eq!(token_limit_field("https://API.OpenAI.com/v1/"), "max_completion_tokens");
        assert_eq!(token_limit_field("http://localhost:8000/v1"), "max_tokens");
        assert_eq!(token_limit_field("https://openrouter.ai/api/v1"), "max_tokens");
        assert_eq!(token_limit_field("https://api.openai.com.example.net/v1"), "max_tokens");
        assert_eq!(token_limit_field("not a url"), "max_tokens");

        let messages = [json!({ "role": "user", "content": "hello" })];
        let req = MessageRequest {
            model: "gpt-5",
            max_tokens: 50,
            reasoning_effort: Some("low"),
            system: "",
            tools: &[],
            messages: &messages,
        };
        let body = build_request(&req, token_limit_field("https://api.openai.com/v1"));
        assert_eq!(body["max_completion_tokens"], 50);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["reasoning_effort"], "low");
    }

    fn request(messages: &[Value]) -> Value {
        build_request(&MessageRequest {
            model: "m",
            max_tokens: 50,
            reasoning_effort: None,
            system: "be nice",
            tools: &[],
            messages,
        }, "max_tokens")
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
        }, "max_tokens");
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
        }, "max_tokens");
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
    fn parses_tool_calls_and_keeps_reasoning_out_of_the_content() {
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
        assert_eq!(r.reasoning.as_deref(), Some("let me think"));
    }

    #[test]
    fn reasoning_content_is_preferred_and_blank_reasoning_is_none() {
        let body = |message: Value| {
            json!({ "choices": [{ "finish_reason": "stop", "message": message }] }).to_string()
        };
        let r = parse_response(&body(json!({ "content": "hi", "reasoning_content": "vllm", "reasoning": "other" }))).unwrap();
        assert_eq!(r.reasoning.as_deref(), Some("vllm"));
        let r = parse_response(&body(json!({ "content": "hi", "reasoning_content": "  " }))).unwrap();
        assert_eq!(r.reasoning, None);
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
