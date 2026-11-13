//! Asking the model server what it serves, so the user picks a name cedar can use.

use crate::i18n::{t, tf};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

/// Tidies what the user typed: a scheme is required, a trailing slash dropped, and `/v1` added
/// when no path was given (Ollama, LM Studio, and vLLM all serve the API under it).
pub fn normalize_base_url(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!(t("model.no_address"));
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let mut url = Url::parse(&with_scheme).map_err(|e| anyhow!(tf("model.bad_url", &[("raw", raw), ("error", &e.to_string())])))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!(t("model.bad_scheme"));
    }
    if url.host_str().is_none() {
        bail!(tf("model.no_host", &[("raw", raw)]));
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(if path.is_empty() { "/v1" } else { &path });
    Ok(url.to_string().trim_end_matches('/').to_string())
}

#[derive(Deserialize)]
struct Model {
    id: String,
}

/// A model list: OpenAI's `{"data": [...]}`, or the bare array some hosted providers answer with.
#[derive(Deserialize)]
#[serde(untagged)]
enum ModelList {
    Wrapped { data: Vec<Model> },
    Bare(Vec<Model>),
}

impl ModelList {
    fn ids(self) -> Vec<String> {
        let (Self::Wrapped { data } | Self::Bare(data)) = self;
        let mut ids: Vec<String> = data.into_iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

/// A hosted provider the setup page offers: its name, the address of its OpenAI-compatible API,
/// and where its documentation says how to get a key. Each address is the one the provider's own
/// documentation gives for the OpenAI SDK's `base_url`.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Provider {
    pub name: &'static str,
    pub base_url: &'static str,
    pub docs: &'static str,
}

const fn provider(name: &'static str, base_url: &'static str, docs: &'static str) -> Provider {
    Provider { name, base_url, docs }
}

/// The page lists them in this order: three first, then by name.
pub const PROVIDERS: &[Provider] = &[
    provider("Baseten", "https://inference.baseten.co/v1", "https://docs.baseten.co/"),
    provider("Nebius", "https://api.tokenfactory.nebius.com/v1", "https://docs.tokenfactory.nebius.com/"),
    provider("Doubleword", "https://api.doubleword.ai/v1", "https://docs.doubleword.ai/"),
    provider("Cerebras", "https://api.cerebras.ai/v1", "https://inference-docs.cerebras.ai/"),
    provider("DeepInfra", "https://api.deepinfra.com/v1/openai", "https://docs.deepinfra.com/"),
    provider("DeepSeek", "https://api.deepseek.com/v1", "https://api-docs.deepseek.com/"),
    provider("Fireworks AI", "https://api.fireworks.ai/inference/v1", "https://docs.fireworks.ai/"),
    provider("Groq", "https://api.groq.com/openai/v1", "https://console.groq.com/docs"),
    provider("Hugging Face", "https://router.huggingface.co/v1", "https://huggingface.co/docs/inference-providers"),
    provider("Hyperbolic", "https://api.hyperbolic.xyz/v1", "https://docs.hyperbolic.ai/"),
    provider("Mistral", "https://api.mistral.ai/v1", "https://docs.mistral.ai/"),
    provider("OpenRouter", "https://openrouter.ai/api/v1", "https://openrouter.ai/docs"),
    provider("SambaNova", "https://api.sambanova.ai/v1", "https://docs.sambanova.ai/"),
    provider("Scaleway", "https://api.scaleway.ai/v1", "https://www.scaleway.com/en/docs/generative-apis/"),
    provider("Together AI", "https://api.together.ai/v1", "https://docs.together.ai/"),
];

/// The three ways the setup page offers to get a model.
pub const SOURCE_LLAMA: &str = "llama";
pub const SOURCE_OWN: &str = "own";
pub const SOURCE_HOSTED: &str = "hosted";

fn host_of(base_url: &str) -> Option<String> {
    url::Url::parse(base_url.trim()).ok()?.host_str().map(str::to_ascii_lowercase)
}

/// Which of the three ways the saved model server is: what the page recorded when it was saved
/// (`saved`), or for a config from before it did, what the address says. Empty when no model
/// server is chosen.
pub fn source(saved: &str, llama: bool, base_url: &str) -> &'static str {
    if llama {
        return SOURCE_LLAMA;
    }
    if base_url.trim().is_empty() {
        return "";
    }
    match saved {
        SOURCE_OWN => SOURCE_OWN,
        SOURCE_HOSTED => SOURCE_HOSTED,
        _ => {
            let host = host_of(base_url);
            let hosted = host.is_some() && PROVIDERS.iter().any(|p| host_of(p.base_url) == host);
            if hosted {
                SOURCE_HOSTED
            } else {
                SOURCE_OWN
            }
        }
    }
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
        .with_context(|| tf("model.unreachable", &[("base", &base)]))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        bail!(tf("model.key_rejected", &[("base", &base), ("status", &status.to_string())]));
    }
    if !status.is_success() {
        bail!(tf("model.bad_status", &[("base", &base), ("status", &status.to_string())]));
    }
    let list: ModelList = resp
        .json()
        .await
        .with_context(|| tf("model.bad_list", &[("base", &base)]))?;
    let ids = list.ids();
    if ids.is_empty() {
        bail!(tf("model.no_models", &[("base", &base)]));
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

    #[test]
    fn model_lists_with_and_without_a_data_field() {
        let wrapped: ModelList = serde_json::from_str(r#"{"object":"list","data":[{"id":"b"},{"id":"a"},{"id":"b"}]}"#).unwrap();
        assert_eq!(wrapped.ids(), ["a", "b"]);
        let bare: ModelList = serde_json::from_str(r#"[{"id":"z","type":"chat"},{"id":"y"}]"#).unwrap();
        assert_eq!(bare.ids(), ["y", "z"]);
        assert!(serde_json::from_str::<ModelList>(r#"{"error":"no"}"#).is_err());
        assert!(serde_json::from_str::<ModelList>(r#""ok""#).is_err());
    }

    #[test]
    fn providers_are_well_formed() {
        let mut hosts = Vec::new();
        for p in PROVIDERS {
            assert!(p.base_url.starts_with("https://"), "{}", p.name);
            assert!(p.docs.starts_with("https://"), "{}", p.name);
            // Saved as it is listed: nothing is added to or cut from the address.
            assert_eq!(normalize_base_url(p.base_url).unwrap(), p.base_url, "{}", p.name);
            hosts.push(host_of(p.base_url).unwrap());
        }
        let count = hosts.len();
        hosts.sort();
        hosts.dedup();
        assert_eq!(hosts.len(), count, "two providers share a host");
        assert_eq!(PROVIDERS[..3].iter().map(|p| p.name).collect::<Vec<_>>(), ["Baseten", "Nebius", "Doubleword"]);
    }

    #[test]
    fn the_source_of_a_model_server() {
        // Nothing chosen.
        assert_eq!(source("", false, ""), "");
        assert_eq!(source("hosted", false, " "), "");
        // llama.cpp set up by the app, whatever else is recorded.
        assert_eq!(source("own", true, "http://127.0.0.1:18081/v1"), SOURCE_LLAMA);
        // What the page recorded.
        assert_eq!(source("hosted", false, "https://llm.example.com/v1"), SOURCE_HOSTED);
        assert_eq!(source("own", false, "https://api.groq.com/openai/v1"), SOURCE_OWN);
        // A config from before it was recorded: by the address.
        for p in PROVIDERS {
            assert_eq!(source("", false, p.base_url), SOURCE_HOSTED, "{}", p.name);
        }
        assert_eq!(source("", false, "https://API.Together.ai/v1/"), SOURCE_HOSTED);
        assert_eq!(source("", false, "http://127.0.0.1:11434/v1"), SOURCE_OWN);
        assert_eq!(source("", false, "https://llm.example.com/v1"), SOURCE_OWN);
        assert_eq!(source("llama", false, "http://10.0.0.5:8000/v1"), SOURCE_OWN);
    }
}

/// The registry entry the app manages. The chat's Settings page can add others beside it.
pub const REGISTRY_ID: &str = "default";

/// cedar's own default for the most tokens of one reply.
pub const DEFAULT_MAX_TOKENS: u32 = 16384;

/// What the app knows about a model beyond its name: the ones it installs for llama.cpp.
pub struct Extras<'a> {
    pub label: &'a str,
    pub max_tokens: u32,
    pub reasoning_levels: &'a [&'a str],
}

#[derive(Serialize)]
struct ModelSpec<'a> {
    base_url: &'a str,
    model: &'a str,
    label: &'a str,
    /// An empty string removes a saved key, which is what an emptied field means.
    api_key: &'a str,
    is_default: bool,
    /// Left out, cedar keeps what the entry has (the chat's Settings may have changed it).
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_levels: Option<&'a [&'a str]>,
    /// Empty: no default level, since one of another model's levels would be refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    default_reasoning: Option<&'a str>,
}

/// Registers (or updates) the model server in cedar's registry: `PUT /v1/admin/models/default`.
/// cedar checks the server by listing its models before it keeps the entry, and stores it in its
/// database, so this only needs to happen when the choice changes, but it is safe to repeat.
pub async fn register(
    cedar_url: &str,
    cedar_api_key: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
    extras: Option<Extras<'_>>,
) -> Result<()> {
    let base = normalize_base_url(base_url)?;
    let model = model.trim();
    if model.is_empty() {
        bail!(t("cmd.pick_model"));
    }
    let client = reqwest::Client::builder()
        // cedar itself gives the model server 20 s to answer.
        .timeout(Duration::from_secs(30))
        .build()?;
    let resp = client
        .put(format!("{cedar_url}/v1/admin/models/{REGISTRY_ID}"))
        .bearer_auth(cedar_api_key)
        .json(&ModelSpec {
            base_url: &base,
            model,
            label: extras.as_ref().map_or(model, |e| e.label),
            api_key: api_key.trim(),
            is_default: true,
            max_tokens: extras.as_ref().map(|e| e.max_tokens),
            reasoning_levels: extras.as_ref().map(|e| e.reasoning_levels),
            default_reasoning: extras.as_ref().map(|_| ""),
        })
        .send()
        .await
        .context("could not reach cedar to register the model")?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    let message = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or(body);
    bail!("cedar did not accept the model ({status}): {message}")
}
