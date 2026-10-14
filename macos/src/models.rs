//! Asking the model server what it serves, so the user picks a name cedar can use.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::time::Duration;
use url::Url;

/// Tidies what the user typed: a scheme is required, a trailing slash dropped, and `/v1` added
/// when no path was given (Ollama, LM Studio, and vLLM all serve the API under it).
pub fn normalize_base_url(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("enter the model server's address, for example http://127.0.0.1:11434/v1");
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let mut url = Url::parse(&with_scheme).map_err(|e| anyhow!("{raw} is not a valid URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("the address must start with http:// or https://");
    }
    if url.host_str().is_none() {
        bail!("{raw} has no host");
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(if path.is_empty() { "/v1" } else { &path });
    Ok(url.to_string().trim_end_matches('/').to_string())
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
}

/// `GET <base>/models`, the OpenAI-compatible model list.
pub async fn list_models(base_url: &str, api_key: &str) -> Result<Vec<String>> {
    let base = normalize_base_url(base_url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let mut req = client.get(format!("{base}/models"));
    if !api_key.trim().is_empty() {
        req = req.bearer_auth(api_key.trim());
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("could not reach {base}; is the model server running?"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        bail!("{base} rejected the API key ({status})");
    }
    if !status.is_success() {
        bail!("{base}/models answered {status}; check the address (it usually ends in /v1)");
    }
    let list: ModelList = resp
        .json()
        .await
        .with_context(|| format!("{base}/models did not return an OpenAI-style model list"))?;
    let mut ids: Vec<String> = list.data.into_iter().map(|m| m.id).collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        bail!("{base} lists no models; load one in the model server first");
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls_get_a_scheme_and_a_v1_path() {
        assert_eq!(normalize_base_url("127.0.0.1:11434").unwrap(), "http://127.0.0.1:11434/v1");
        assert_eq!(normalize_base_url("http://localhost:1234/v1/").unwrap(), "http://localhost:1234/v1");
        assert_eq!(normalize_base_url("https://spark.tail.net:8000/v1?x=1").unwrap(), "https://spark.tail.net:8000/v1");
        assert_eq!(normalize_base_url("http://host/api").unwrap(), "http://host/api");
        assert!(normalize_base_url("").is_err());
        assert!(normalize_base_url("ftp://x").is_err());
    }
}
