//! Client for the Anthropic Messages API.
//!
//! Message content is kept as raw JSON: server-tool result blocks (web search) carry opaque
//! `encrypted_content` that must be sent back to the API exactly as received.

use crate::models::Usage;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

pub struct MessageRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub system: &'a str,
    pub tools: &'a [Value],
    pub messages: &'a [Value],
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// The assistant's content blocks, verbatim.
    pub content: Vec<Value>,
    pub stop_reason: Option<String>,
    pub usage: Usage,
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn create_message(&self, req: &MessageRequest<'_>) -> Result<LlmResponse>;
}

pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    max_attempts: u32,
    retry_base: Duration,
}

const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

impl AnthropicClient {
    pub fn new(api_key: String, base_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            api_key,
            base_url,
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
impl LlmClient for AnthropicClient {
    async fn create_message(&self, req: &MessageRequest<'_>) -> Result<LlmResponse> {
        let body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "system": req.system,
            "tools": req.tools,
            "messages": req.messages,
        });
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));

        let mut attempt = 0;
        loop {
            attempt += 1;
            let sent = self
                .http
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await;

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
                            anyhow!("Anthropic API error {status}: {}", error_message(&text)),
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
            tracing::warn!(attempt, ?delay, error = %err, "Anthropic request failed; retrying");
            tokio::time::sleep(delay).await;
        }
    }
}

fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| body.chars().take(300).collect())
}

fn parse_response(body: &str) -> Result<LlmResponse> {
    let v: Value = serde_json::from_str(body).context("API returned invalid JSON")?;
    let content = v["content"]
        .as_array()
        .cloned()
        .ok_or_else(|| anyhow!("API response has no content array"))?;
    Ok(LlmResponse {
        content,
        stop_reason: v["stop_reason"].as_str().map(String::from),
        usage: Usage {
            input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_stop_reason_and_usage() {
        let body = r#"{"content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn",
                       "usage":{"input_tokens":12,"output_tokens":3}}"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.content.len(), 1);
        assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(
            r.usage,
            Usage {
                input_tokens: 12,
                output_tokens: 3
            }
        );
        assert!(parse_response("{}").is_err());
        assert!(parse_response("nope").is_err());
    }

    #[test]
    fn extracts_api_error_messages() {
        assert_eq!(
            error_message(r#"{"error":{"type":"x","message":"bad model"}}"#),
            "bad model"
        );
        assert_eq!(error_message("gateway timeout"), "gateway timeout");
    }

    #[test]
    fn backoff_grows_and_honors_retry_after() {
        let c = AnthropicClient::new("k".into(), "http://x".into())
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
