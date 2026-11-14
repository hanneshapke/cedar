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

/// A hosted provider the pages offer: its name, the address of its OpenAI-compatible API (the one
/// its own documentation gives for the OpenAI SDK's `base_url`), and where that documentation is.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    pub docs: String,
}

/// A model server people run themselves: where it listens by default, and the key of the hint
/// (in each page's own texts) that says what it needs to work with Cedar.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub base_url: String,
    pub hint: String,
}

/// The providers and presets, in the order the pages list them.
#[derive(Deserialize, Debug)]
pub struct ModelServers {
    pub providers: Vec<Provider>,
    pub presets: Vec<Preset>,
}

/// One list for the setup page and the chat's Settings, which gets the same file from cedar-chat
/// (`/model-servers.json`).
pub fn model_servers() -> &'static ModelServers {
    static LIST: std::sync::LazyLock<ModelServers> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../chat/static/model-servers.json"))
            .expect("chat/static/model-servers.json is checked by a test")
    });
    &LIST
}

/// The two ways to name a model server on the setup page (the third, llama.cpp run by the app,
/// is `Config::llama`).
pub const SOURCE_OWN: &str = "own";
pub const SOURCE_HOSTED: &str = "hosted";

fn host_of(base_url: &str) -> Option<String> {
    url::Url::parse(base_url.trim()).ok()?.host_str().map(str::to_ascii_lowercase)
}

/// Which of the two the saved model server is: what the page recorded when it was saved
/// (`saved`), or for a config from before it did, what the address says. Empty when none is set.
pub fn source(saved: &str, base_url: &str) -> &'static str {
    if base_url.trim().is_empty() {
        return "";
    }
    match saved {
        SOURCE_OWN => SOURCE_OWN,
        SOURCE_HOSTED => SOURCE_HOSTED,
        _ => {
            let host = host_of(base_url);
            let hosted = host.is_some() && model_servers().providers.iter().any(|p| host_of(&p.base_url) == host);
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
    fn the_shared_list_is_well_formed() {
        let list = model_servers();
        let mut hosts = Vec::new();
        for p in &list.providers {
            assert!(p.base_url.starts_with("https://"), "{}", p.name);
            assert!(p.docs.starts_with("https://"), "{}", p.name);
            // Saved as it is listed: nothing is added to or cut from the address.
            assert_eq!(normalize_base_url(&p.base_url).unwrap(), p.base_url, "{}", p.name);
            hosts.push(host_of(&p.base_url).unwrap());
        }
        let count = hosts.len();
        hosts.sort();
        hosts.dedup();
        assert_eq!(hosts.len(), count, "two providers share a host");
        assert_eq!(list.providers[..3].iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Baseten", "Nebius", "Doubleword"]);
        assert_eq!(
            list.presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Ollama", "LM Studio", "vLLM", "llama.cpp", "SGLang"]
        );
        for p in &list.presets {
            assert_eq!(normalize_base_url(&p.base_url).unwrap(), p.base_url, "{}", p.name);
            assert!(p.hint.starts_with("preset."), "{}", p.name);
        }
    }

    #[test]
    fn the_source_of_a_model_server() {
        // Nothing chosen.
        assert_eq!(source("", ""), "");
        assert_eq!(source("hosted", " "), "");
        // What the page recorded.
        assert_eq!(source("hosted", "https://llm.example.com/v1"), SOURCE_HOSTED);
        assert_eq!(source("own", "https://api.groq.com/openai/v1"), SOURCE_OWN);
        // A config from before it was recorded: by the address.
        for p in &model_servers().providers {
            assert_eq!(source("", &p.base_url), SOURCE_HOSTED, "{}", p.name);
        }
        assert_eq!(source("", "https://API.Together.ai/v1/"), SOURCE_HOSTED);
        assert_eq!(source("", "http://127.0.0.1:11434/v1"), SOURCE_OWN);
        assert_eq!(source("", "https://llm.example.com/v1"), SOURCE_OWN);
        assert_eq!(source("llama", "http://10.0.0.5:8000/v1"), SOURCE_OWN);
    }

    #[test]
    fn what_cedar_is_sent_for_an_entry() {
        let own = Entry { id: REGISTRY_ID, base_url: "x", api_key: " key ", model: " qwen3:30b ", extras: None, make_default: false };
        let json = serde_json::to_value(spec(&own, "http://127.0.0.1:11434/v1")).unwrap();
        assert_eq!(json, serde_json::json!({
            "base_url": "http://127.0.0.1:11434/v1", "model": "qwen3:30b", "label": "qwen3:30b", "api_key": "key",
        }));
        // The one just chosen becomes the default; one the app installed brings what it knows.
        let extras = Extras { label: "Gemma 4 E4B", max_tokens: 8192, reasoning_levels: &["low", "high"], multimodal: true };
        let llama = Entry { id: LLAMA_ID, base_url: "x", api_key: "k", model: "gemma-4-e4b", extras: Some(extras), make_default: true };
        let json = serde_json::to_value(spec(&llama, "http://127.0.0.1:18081/v1")).unwrap();
        assert_eq!(json, serde_json::json!({
            "base_url": "http://127.0.0.1:18081/v1", "model": "gemma-4-e4b", "label": "Gemma 4 E4B", "api_key": "k",
            "is_default": true, "max_tokens": 8192, "reasoning_levels": ["low", "high"], "default_reasoning": "",
            "multimodal": true,
        }));
    }
}

/// The registry entries the app manages: the model server the user named on the setup page, and
/// the model the app runs with llama.cpp. The chat's Settings page can add others beside them.
pub const REGISTRY_ID: &str = "default";
pub const LLAMA_ID: &str = "llama";

/// What the app knows about a model beyond its name: the ones it installs for llama.cpp.
pub struct Extras<'a> {
    pub label: &'a str,
    pub max_tokens: u32,
    pub reasoning_levels: &'a [&'a str],
    /// It reads images (the app loads its vision projector), so cedar may send it some.
    pub multimodal: bool,
}

#[derive(Serialize)]
struct ModelSpec<'a> {
    base_url: &'a str,
    model: &'a str,
    label: &'a str,
    /// An empty string removes a saved key, which is what an emptied field means.
    api_key: &'a str,
    /// Sent only for the entry the user just chose: left out, cedar keeps the default it has,
    /// which may be one picked in the chat's Settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    is_default: Option<bool>,
    /// Left out, cedar keeps what the entry has (the chat's Settings may have changed it).
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_levels: Option<&'a [&'a str]>,
    /// Empty: no default level, since one of another model's levels would be refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    default_reasoning: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    multimodal: Option<bool>,
}

/// One entry for cedar's registry.
pub struct Entry<'a> {
    /// [`REGISTRY_ID`] or [`LLAMA_ID`].
    pub id: &'a str,
    pub base_url: &'a str,
    pub api_key: &'a str,
    pub model: &'a str,
    pub extras: Option<Extras<'a>>,
    /// The user just chose this one: it becomes the model new chats use.
    pub make_default: bool,
}

fn spec<'a>(entry: &'a Entry<'a>, base: &'a str) -> ModelSpec<'a> {
    let model = entry.model.trim();
    let extras = entry.extras.as_ref();
    ModelSpec {
        base_url: base,
        model,
        label: extras.map_or(model, |e| e.label),
        api_key: entry.api_key.trim(),
        is_default: entry.make_default.then_some(true),
        max_tokens: extras.map(|e| e.max_tokens),
        reasoning_levels: extras.map(|e| e.reasoning_levels),
        default_reasoning: extras.map(|_| ""),
        multimodal: extras.map(|e| e.multimodal),
    }
}

fn admin_client() -> Result<reqwest::Client> {
    // cedar itself gives the model server 20 s to answer.
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?)
}

/// Removes an entry from cedar's registry; one that is not there is fine.
pub async fn unregister(cedar_url: &str, cedar_api_key: &str, id: &str) -> Result<()> {
    let resp = admin_client()?
        .delete(format!("{cedar_url}/v1/admin/models/{id}"))
        .bearer_auth(cedar_api_key)
        .send()
        .await
        .context("could not reach cedar to remove the model")?;
    let status = resp.status();
    if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    bail!("cedar did not remove the model ({status})")
}

/// The address of an entry in cedar's registry, if it has that entry.
pub async fn registered_address(cedar_url: &str, cedar_api_key: &str, id: &str) -> Option<String> {
    let resp = admin_client().ok()?.get(format!("{cedar_url}/v1/admin/models")).bearer_auth(cedar_api_key).send().await.ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    let entry = body["models"].as_array()?.iter().find(|m| m["id"] == id)?;
    entry["base_url"].as_str().map(str::to_string)
}

/// Registers (or updates) an entry in cedar's registry: `PUT /v1/admin/models/<id>`. cedar
/// checks the server by listing its models before it keeps the entry, and stores it in its
/// database, so this only needs to happen when the choice changes, but it is safe to repeat.
pub async fn register(cedar_url: &str, cedar_api_key: &str, entry: Entry<'_>) -> Result<()> {
    let Entry { id, base_url, model, .. } = entry;
    let base = normalize_base_url(base_url)?;
    let model = model.trim();
    if model.is_empty() {
        bail!(t("cmd.pick_model"));
    }
    let resp = admin_client()?
        .put(format!("{cedar_url}/v1/admin/models/{id}"))
        .bearer_auth(cedar_api_key)
        .json(&spec(&entry, &base))
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
