//! What the Gmail and Calendar tools share when they use Google's REST APIs (`GOOGLE_BACKEND=api`):
//! an authorized HTTP client and the choice of which tools to offer.
//!
//! The tools are named like those of Google's Gmail and Calendar MCP servers, so `GMAIL_TOOLS` and
//! `CALENDAR_TOOLS` mean the same with either backend. Unlike the MCP servers, the REST APIs serve
//! any Google account, personal Gmail included.

use crate::mcp::{cut, http_client, without_url, GoogleOAuth};
use anyhow::{anyhow, bail, Result};
use reqwest::{Method, StatusCode, Url};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// One Google REST API (Gmail or Calendar) acting for the configured account.
pub struct GoogleApi {
    http: reqwest::Client,
    base: Url,
    auth: Arc<GoogleOAuth>,
    /// "Gmail" or "Calendar", for error messages.
    service: &'static str,
}

impl GoogleApi {
    pub fn new(base: &str, auth: Arc<GoogleOAuth>, service: &'static str) -> Result<Self> {
        let base =
            Url::parse(base).map_err(|e| anyhow!("invalid {service} API URL {base:?}: {e}"))?;
        if base.cannot_be_a_base() {
            bail!("invalid {service} API URL {base}");
        }
        Ok(Self {
            http: http_client()?,
            base,
            auth,
            service,
        })
    }

    /// The base URL with `segments` appended, each percent-encoded, so an ID the model passes
    /// can't reach a different endpoint.
    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("checked in new")
            .pop_if_empty()
            .extend(segments);
        url
    }

    pub async fn get(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
        timeout: Duration,
    ) -> Result<Value> {
        self.send(Method::GET, segments, query, None, timeout).await
    }

    pub async fn post(&self, segments: &[&str], body: &Value, timeout: Duration) -> Result<Value> {
        self.send(Method::POST, segments, &[], Some(body), timeout)
            .await
    }

    /// Send one request and return its JSON body. A rejected access token is refreshed once.
    async fn send(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let url = self.url(segments);
        let mut retried = false;
        loop {
            let token = self.auth.token(timeout).await?;
            let mut req = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(token)
                .query(query)
                .timeout(timeout);
            if let Some(body) = body {
                req = req.json(body);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| anyhow!("{} API: {}", self.service, without_url(e)))?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| {
                anyhow!(
                    "{} API: reading the response: {}",
                    self.service,
                    without_url(e)
                )
            })?;
            if status == StatusCode::UNAUTHORIZED && !retried {
                retried = true;
                self.auth.invalidate().await;
                continue;
            }
            if !status.is_success() {
                bail!(
                    "{} API error ({status}): {}",
                    self.service,
                    error_message(&text)
                );
            }
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_str(&text).map_err(|e| {
                anyhow!(
                    "{} API: the reply is not JSON ({e}): {}",
                    self.service,
                    cut(&text)
                )
            });
        }
    }
}

/// Google's `{"error": {"message": ...}}`, or the body itself if it isn't one.
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| cut(body))
}

/// The tools of `implemented` that `allowed` names (`*` names all), in `implemented`'s order.
/// Allowed names cedar doesn't implement are logged and skipped.
pub fn select<'a>(prefix: &str, allowed: &[String], implemented: &[&'a str]) -> Vec<&'a str> {
    let all = allowed.iter().any(|t| t == "*");
    for name in allowed {
        if name != "*" && !implemented.contains(&name.as_str()) {
            tracing::warn!(service = %prefix, tool = %name, "allowed tool is not implemented for Google's REST API; it is left out");
        }
    }
    implemented
        .iter()
        .copied()
        .filter(|t| all || allowed.iter().any(|a| a == t))
        .collect()
}

/// An optional string argument, trimmed; empty counts as missing.
pub fn str_arg<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input[key].as_str().map(str::trim).filter(|s| !s.is_empty())
}

pub fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    str_arg(input, key).ok_or_else(|| anyhow!("{key} is required"))
}

/// An optional page size, clamped to `1..=max`.
pub fn page_size(input: &Value, default: u64, max: u64) -> u64 {
    input["pageSize"].as_u64().unwrap_or(default).clamp(1, max)
}

/// A list of strings given as an array (or, leniently, one comma-separated string).
pub fn str_list(input: &Value, key: &str) -> Vec<String> {
    match &input[key] {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(s) => s
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// `v` without nulls, empty strings, and empty arrays, which only cost tokens.
pub fn without_empty(v: Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, without_empty(v)))
                .filter(|(_, v)| match v {
                    Value::Null => false,
                    Value::String(s) => !s.is_empty(),
                    Value::Array(a) => !a.is_empty(),
                    _ => true,
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(without_empty).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GoogleOAuthConfig;
    use serde_json::json;

    fn api(base: &str) -> GoogleApi {
        let auth = GoogleOAuth::new(GoogleOAuthConfig {
            client_id: "c".into(),
            client_secret: "s".into(),
            refresh_token: "r".into(),
            token_url: "http://localhost/token".into(),
        })
        .unwrap();
        GoogleApi::new(base, Arc::new(auth), "Calendar").unwrap()
    }

    #[test]
    fn ids_are_encoded_as_single_path_segments() {
        let api = api("https://www.googleapis.com/calendar/v3");
        assert_eq!(
            api.url(&["calendars", "en.usa#holiday@group.v.calendar.google.com", "events"])
                .as_str(),
            "https://www.googleapis.com/calendar/v3/calendars/en.usa%23holiday@group.v.calendar.google.com/events"
        );
        assert_eq!(
            api.url(&["calendars", "../../oauth2", "events"]).path(),
            "/calendar/v3/calendars/..%2F..%2Foauth2/events"
        );
    }

    #[test]
    fn google_error_messages_are_extracted() {
        assert_eq!(
            error_message(r#"{"error":{"code":403,"message":"Gmail API has not been used"}}"#),
            "Gmail API has not been used"
        );
        assert_eq!(error_message("plain failure"), "plain failure");
    }

    #[test]
    fn tools_are_selected_in_implementation_order() {
        let allowed = vec!["b".to_string(), "a".to_string(), "send".to_string()];
        assert_eq!(select("gmail", &allowed, &["a", "b", "c"]), ["a", "b"]);
        assert_eq!(select("gmail", &["*".into()], &["a", "b"]), ["a", "b"]);
    }

    #[test]
    fn list_arguments_accept_arrays_and_strings() {
        assert_eq!(
            str_list(&json!({ "to": ["a@x.com", " ", "b@x.com"] }), "to"),
            ["a@x.com", "b@x.com"]
        );
        assert_eq!(
            str_list(&json!({ "to": "a@x.com, b@x.com" }), "to"),
            ["a@x.com", "b@x.com"]
        );
        assert!(str_list(&json!({}), "to").is_empty());
    }

    #[test]
    fn empty_values_are_dropped() {
        assert_eq!(
            without_empty(
                json!({ "a": null, "b": "", "c": [], "d": [{ "e": null, "f": 1 }], "g": false })
            ),
            json!({ "d": [{ "f": 1 }], "g": false })
        );
    }
}
