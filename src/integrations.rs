//! Integrations an admin can set up through the API instead of the environment: the model
//! registry (the models runs can use, each with its server and API key), a Brave Search key, a
//! Best Buy key and a Google account ("Connect Google"). Credentials are kept encrypted (see [`crate::secrets`]).
//! Saving or removing a key rebuilds the tool set, and a change to the registry rebuilds the
//! model clients, so the next run (or the next model call of one already going) sees the change
//! without a restart. Runs already going keep the tools they started with.
//!
//! Personal tools get their secrets here too, one per variable their manifest declares.
//!
//! The environment wins for the Brave and Best Buy keys and the Google account: a credential set there is
//! reported as such and can't be changed here. Models are only ever registered here.

use crate::api::MAX_MODEL_LEN;
use crate::config::{Config, LEGACY_DEFAULT_LLM_BASE_URL, LEGACY_DEFAULT_MODEL};
use crate::error::AppError;
use crate::llm::{list_models, ChatCompletionsClient, LlmClient, LlmRouter};
use crate::mcp::http_client;
use crate::models::{ModelEntry, Price};
use crate::secrets::{self, SecretStore};
use crate::store::Store;
use crate::tools::custom::SavedSecrets;
use crate::tools::{BestBuy, BraveSearch, Overrides, Tool, ToolSet};
use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use base64::Engine;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::OsRng;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a "Connect Google" link stays usable.
const OAUTH_STATE_TTL: Duration = Duration::from_secs(600);
/// At most this many "Connect Google" attempts are pending at once.
const MAX_PENDING_STATES: usize = 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SECRET_LEN: usize = 500;
const MAX_MODEL_ID_LEN: usize = 64;
const MAX_LABEL_LEN: usize = 100;
const MAX_REASONING_LEVELS: usize = 16;
const MAX_REASONING_LEVEL_LEN: usize = 32;
/// The id the single model server saved by earlier versions becomes.
const LEGACY_MODEL_ID: &str = "default";

pub struct Integrations {
    cfg: Arc<Config>,
    store: Store,
    secrets: Option<SecretStore>,
    tools: Arc<ArcSwap<ToolSet>>,
    http: reqwest::Client,
    /// "Connect Google" attempts in flight: state → (redirect URI, issued at).
    pending: Mutex<HashMap<String, (String, Instant)>>,
    /// Serializes changes, so two admins can't interleave a save and a rebuild.
    changing: tokio::sync::Mutex<()>,
    /// The model clients, when this server drives them (see [`Self::attach_router`]).
    router: OnceLock<Arc<LlmRouter>>,
    /// The registry as last loaded, in picker order. Kept here so every run creation reads it
    /// without a query.
    models: ArcSwap<Vec<ModelEntry>>,
}

/// What an admin sends to register or change a model (`PUT /v1/admin/models/{id}`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    /// What the pickers show. Absent: the existing label, else the model name.
    pub label: Option<String>,
    pub base_url: String,
    /// The model's name on the server.
    pub model: String,
    /// Absent keeps the saved key; empty removes it.
    pub api_key: Option<String>,
    /// The `reasoning_effort` values the model accepts. Absent keeps the saved ones (none for a
    /// new entry).
    pub reasoning_levels: Option<Vec<String>>,
    /// The level a run gets when it names none; empty for none. Absent keeps the saved one.
    pub default_reasoning: Option<String>,
    /// The model accepts images with a prompt. Absent keeps the saved value (`false`
    /// for a new entry).
    pub multimodal: Option<bool>,
    /// The most tokens one reply may take, a reasoning model's thinking included (1 to
    /// 1000000). Absent keeps the saved value (16384 for a new entry).
    pub max_tokens: Option<u32>,
    /// Make this the model runs get when they name none. Absent keeps it as it is.
    pub is_default: Option<bool>,
    /// What its tokens cost, for the cost shown with a run's usage. Absent keeps the saved price;
    /// `null` removes it.
    #[serde(default, deserialize_with = "present")]
    pub price: Option<Option<Price>>,
}

/// What an admin sends to see which models a server lists before registering one
/// (`POST /v1/admin/server-models`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerModelsQuery {
    pub base_url: String,
    /// The key to list with. Absent or empty: the saved key of the registered model `id`, if any.
    pub api_key: Option<String>,
    /// A registered model whose saved key is used when no `api_key` is given.
    pub id: Option<String>,
}

/// Tells an explicit `null` (`Some(None)`) apart from an absent field (`None`, via `default`).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
struct TokenResponse {
    refresh_token: Option<String>,
    id_token: Option<String>,
    scope: Option<String>,
}

impl Integrations {
    /// Starts with the tools the environment configures; [`Self::reload`] adds saved credentials.
    pub fn new(cfg: Arc<Config>, store: Store) -> Result<Self> {
        let tools = ToolSet::from_config(&cfg)?;
        Ok(Self {
            secrets: cfg
                .secret_key
                .as_ref()
                .map(|k| SecretStore::new(store.clone(), k)),
            store,
            cfg,
            tools: Arc::new(ArcSwap::from_pointee(tools)),
            http: http_client()?,
            pending: Mutex::default(),
            changing: tokio::sync::Mutex::default(),
            router: OnceLock::new(),
            models: ArcSwap::from_pointee(vec![]),
        })
    }

    /// The registered models, in picker order, as last loaded.
    pub fn models(&self) -> Arc<Vec<ModelEntry>> {
        self.models.load_full()
    }

    /// The model a run gets when it doesn't name one, if any is registered.
    pub fn default_entry(&self) -> Option<ModelEntry> {
        self.models.load().iter().find(|m| m.is_default).cloned()
    }

    /// The current tool set, shared with the agent.
    pub fn tools(&self) -> Arc<ArcSwap<ToolSet>> {
        self.tools.clone()
    }

    /// Rebuild the tools from the environment plus the saved credentials. A credential that can't
    /// be decrypted is logged and left out rather than keeping cedar from starting.
    pub async fn reload(&self) -> Result<()> {
        self.apply_models().await?;
        let mut overrides = Overrides::default();
        if let Some(secrets) = &self.secrets {
            for (name, slot) in [
                (secrets::BRAVE_API_KEY, &mut overrides.brave_api_key),
                (secrets::BESTBUY_API_KEY, &mut overrides.bestbuy_api_key),
                (
                    secrets::GOOGLE_REFRESH_TOKEN,
                    &mut overrides.google_refresh_token,
                ),
            ] {
                match secrets.get(name).await {
                    Ok(v) => *slot = v,
                    Err(e) => {
                        tracing::warn!(error = %format!("{e:#}"), "ignoring a saved integration")
                    }
                }
            }
            overrides.tool_secrets = self.saved_tool_secrets(secrets).await?;
        }
        let tools = ToolSet::build(&self.cfg, &overrides)?;
        self.tools.store(Arc::new(tools));
        Ok(())
    }

    /// Every secret saved for a personal tool, decrypted. One that can't be read is logged and
    /// left out.
    async fn saved_tool_secrets(&self, secrets: &SecretStore) -> Result<SavedSecrets> {
        let mut out = SavedSecrets::new();
        for key in secrets
            .names_with_prefix(secrets::TOOL_SECRET_PREFIX)
            .await?
        {
            let Some((tool, name)) = key[secrets::TOOL_SECRET_PREFIX.len()..].split_once(':')
            else {
                continue;
            };
            match secrets.get(&key).await {
                Ok(Some(value)) => {
                    out.entry(tool.to_string())
                        .or_default()
                        .insert(name.to_string(), value);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(tool, secret = name, error = %format!("{e:#}"), "ignoring a saved tool secret")
                }
            }
        }
        Ok(out)
    }

    /// The names of the secrets saved for `tool`.
    async fn saved_tool_secret_names(&self, tool: &str) -> Result<HashSet<String>> {
        let Some(secrets) = &self.secrets else {
            return Ok(HashSet::new());
        };
        let prefix = secrets::tool_secret(tool, "");
        Ok(secrets
            .names_with_prefix(&prefix)
            .await?
            .into_iter()
            .map(|k| k[prefix.len()..].to_string())
            .collect())
    }

    /// Every personal tool, for admins: where it comes from, what it asks for, which of its
    /// secrets are saved (never their values).
    pub async fn personal_tools(&self) -> Result<Value> {
        let set = self.tools.load_full();
        let mut tools = Vec::new();
        for tool in set.personal() {
            let saved = self.saved_tool_secret_names(tool.name()).await?;
            tools.push(tool.info(&saved));
        }
        Ok(json!({
            "tools_dir": self.cfg.sandbox.tools_dir.is_some(),
            "storage": self.secrets.is_some(),
            "network": self.cfg.sandbox.tools_network,
            "tools": tools,
        }))
    }

    /// Save a secret a personal tool declares, then rebuild the tools so its calls get it.
    pub async fn set_tool_secret(
        &self,
        tool: &str,
        name: &str,
        value: &str,
    ) -> Result<(), AppError> {
        let secrets = self.writable(false)?;
        let set = self.tools.load_full();
        let declared = set
            .personal()
            .into_iter()
            .find(|t| t.name() == tool)
            .ok_or(AppError::NotFound("no such personal tool"))?
            .secrets()
            .iter()
            .any(|s| s.name == name);
        if !declared {
            return Err(AppError::BadRequest(format!(
                "{tool} does not declare a secret named {name:?}"
            )));
        }
        if value.is_empty() || value.len() > MAX_SECRET_LEN || value.chars().any(char::is_control) {
            return Err(AppError::BadRequest("value is missing or malformed".into()));
        }
        let _guard = self.changing.lock().await;
        secrets
            .put(
                &secrets::tool_secret(tool, name),
                value,
                &json!({ "last4": secrets::last4(value) }),
            )
            .await?;
        self.reload().await?;
        tracing::info!(tool, secret = name, "tool secret saved");
        Ok(())
    }

    pub async fn remove_tool_secret(&self, tool: &str, name: &str) -> Result<(), AppError> {
        let secrets = self.writable(false)?;
        let _guard = self.changing.lock().await;
        if !secrets.delete(&secrets::tool_secret(tool, name)).await? {
            return Err(AppError::NotFound("no such secret is saved"));
        }
        self.reload().await?;
        Ok(())
    }

    /// Forget every secret saved for `tool` (when it is uninstalled). Doesn't rebuild the tools.
    pub async fn forget_tool_secrets(&self, tool: &str) -> Result<()> {
        if let Some(secrets) = &self.secrets {
            for key in secrets
                .names_with_prefix(&secrets::tool_secret(tool, ""))
                .await?
            {
                secrets.delete(&key).await?;
            }
        }
        Ok(())
    }

    /// Hold this while changing `TOOLS_DIR`, so a rebuild doesn't read a half-made change.
    pub async fn lock_changes(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.changing.lock().await
    }

    /// Let this server drive the model clients: `router` is what the agent and compaction call.
    /// Call [`Self::reload`] afterwards to load the registry into it.
    pub fn attach_router(&self, router: Arc<LlmRouter>) {
        let _ = self.router.set(router);
    }

    async fn saved(&self, name: &str) -> Option<String> {
        let secrets = self.secrets.as_ref()?;
        match secrets.get(name).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "ignoring a saved integration");
                None
            }
        }
    }

    /// A registered model's API key, if one is saved and can be read.
    async fn model_api_key(&self, id: &str) -> Option<String> {
        let name = secrets::model_api_key(id);
        if self.secrets.is_none() {
            if self.store.has_secret(&name).await.unwrap_or(false) {
                tracing::warn!(
                    model = id,
                    "the model's saved API key can't be read without CEDAR_SECRET_KEY; calling without it"
                );
            }
            return None;
        }
        self.saved(&name).await
    }

    /// Load the registry (migrating a model server saved by an earlier version first) and
    /// rebuild the model clients from it.
    async fn apply_models(&self) -> Result<()> {
        self.migrate_legacy_server().await?;
        let entries = self.store.list_models().await?;
        if let Some(router) = self.router.get() {
            let mut clients: HashMap<String, Arc<dyn LlmClient>> = HashMap::new();
            for entry in &entries {
                let key = self.model_api_key(&entry.id).await;
                let client = ChatCompletionsClient::new(entry.base_url.clone(), key)
                    .with_context(|| format!("model {}", entry.id))?;
                clients.insert(entry.id.clone(), Arc::new(client));
            }
            router.replace_all(clients);
        }
        self.models.store(Arc::new(entries));
        Ok(())
    }

    /// Versions before the registry kept one model server (address, key, model name) in the
    /// secrets. Turn it into the entry `default` once, then forget the old rows.
    async fn migrate_legacy_server(&self) -> Result<()> {
        let legacy = [
            secrets::LEGACY_LLM_BASE_URL,
            secrets::LEGACY_LLM_API_KEY,
            secrets::LEGACY_LLM_MODEL,
        ];
        let mut any = false;
        for name in legacy {
            any |= self.store.has_secret(name).await?;
        }
        if !any {
            return Ok(());
        }
        let Some(secrets) = &self.secrets else {
            tracing::warn!(
                "a model server saved by an earlier version can't be migrated into the model registry without CEDAR_SECRET_KEY"
            );
            return Ok(());
        };
        let read = |name: &'static str| async move {
            match secrets.get(name).await {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "leaving the saved model server as it is");
                    None
                }
            }
        };
        let (Some(base_url), Some(api_key), Some(model)) = (
            read(secrets::LEGACY_LLM_BASE_URL).await,
            read(secrets::LEGACY_LLM_API_KEY).await,
            read(secrets::LEGACY_LLM_MODEL).await,
        ) else {
            return Ok(());
        };
        if self.store.get_model(LEGACY_MODEL_ID).await?.is_none() {
            let listed = secrets
                .meta(secrets::LEGACY_LLM_BASE_URL)
                .await?
                .and_then(|m| serde_json::from_value(m["models"].clone()).ok());
            let model = model.unwrap_or_else(|| LEGACY_DEFAULT_MODEL.to_string());
            let base_url = base_url.unwrap_or_else(|| LEGACY_DEFAULT_LLM_BASE_URL.to_string());
            let now = Utc::now();
            let entry = ModelEntry {
                id: LEGACY_MODEL_ID.into(),
                label: "Default".into(),
                reasoning_levels: suggested_reasoning_levels(&model),
                model,
                base_url,
                default_reasoning: None,
                multimodal: false,
                max_tokens: crate::models::DEFAULT_MAX_TOKENS,
                is_default: true,
                price: None,
                position: 0,
                listed_models: listed,
                created_at: now,
                updated_at: now,
            };
            self.store.upsert_model(&entry).await?;
            if let Some(key) = &api_key {
                secrets
                    .put(
                        &secrets::model_api_key(LEGACY_MODEL_ID),
                        key,
                        &json!({ "last4": secrets::last4(key) }),
                    )
                    .await?;
            }
            tracing::info!(
                base_url = %redact_url(&entry.base_url),
                model = %entry.model,
                "the saved model server is now the registered model \"default\""
            );
        }
        for name in legacy {
            self.store.delete_secret(name).await?;
        }
        Ok(())
    }

    /// Register a model, or change a registered one: check that `base_url` answers like an
    /// OpenAI-compatible server (listing its models), then save the entry and use it for the next
    /// model call. See [`ModelSpec`] for what absent fields mean.
    pub async fn set_model(&self, id: &str, spec: ModelSpec) -> Result<Value, AppError> {
        let id = clean_model_id(id)?;
        let existing = self.store.get_model(&id).await?;
        let model = clean_model_name(&spec.model)?
            .ok_or_else(|| AppError::BadRequest("model must not be empty".into()))?;
        let label = match spec.label.as_deref().map(str::trim) {
            Some("") | None => existing
                .as_ref()
                .map_or_else(|| model.clone(), |e| e.label.clone()),
            Some(l) => l.to_string(),
        };
        if label.chars().count() > MAX_LABEL_LEN || label.chars().any(char::is_control) {
            return Err(AppError::BadRequest("label is malformed".into()));
        }
        let url = normalize_llm_url(&spec.base_url).map_err(AppError::BadRequest)?;
        let reasoning_levels = match spec.reasoning_levels {
            Some(levels) => clean_reasoning_levels(&levels)?,
            None => existing
                .as_ref()
                .map(|e| e.reasoning_levels.clone())
                .unwrap_or_default(),
        };
        let default_reasoning = match spec.default_reasoning.as_deref().map(str::trim) {
            Some("") => None,
            Some(level) => Some(level.to_string()),
            None => existing.as_ref().and_then(|e| e.default_reasoning.clone()),
        };
        if let Some(level) = &default_reasoning {
            if !reasoning_levels.iter().any(|l| l == level) {
                return Err(AppError::BadRequest(format!(
                    "default_reasoning {level:?} is not one of the reasoning levels"
                )));
            }
        }
        let is_default = spec
            .is_default
            .unwrap_or_else(|| existing.as_ref().is_some_and(|e| e.is_default));
        let multimodal = spec
            .multimodal
            .unwrap_or_else(|| existing.as_ref().is_some_and(|e| e.multimodal));
        let max_tokens = spec.max_tokens.unwrap_or_else(|| {
            existing
                .as_ref()
                .map_or(crate::models::DEFAULT_MAX_TOKENS, |e| e.max_tokens)
        });
        if !(1..=crate::models::MAX_MAX_TOKENS).contains(&max_tokens) {
            return Err(AppError::BadRequest(format!(
                "max_tokens must be between 1 and {}",
                crate::models::MAX_MAX_TOKENS
            )));
        }
        let price = match spec.price {
            Some(Some(p)) => {
                let ok = |v: f64| v.is_finite() && (0.0..=100_000.0).contains(&v);
                if !ok(p.input_usd_per_m) || !ok(p.output_usd_per_m) {
                    return Err(AppError::BadRequest(
                        "price must be between 0 and 100000 US dollars per million tokens".into(),
                    ));
                }
                Some(p)
            }
            Some(None) => None,
            None => existing.as_ref().and_then(|e| e.price),
        };

        let key_name = secrets::model_api_key(&id);
        let key = match spec.api_key.as_deref().map(str::trim) {
            Some("") => None,
            Some(k) if k.len() > MAX_SECRET_LEN || k.chars().any(char::is_control) => {
                return Err(AppError::BadRequest("api_key is malformed".into()))
            }
            Some(k) => {
                self.writable(false)?;
                Some(k.to_string())
            }
            None => self.model_api_key(&id).await,
        };
        let listed = list_models(&url, key.as_deref(), REQUEST_TIMEOUT)
            .await
            .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;

        let _guard = self.changing.lock().await;
        let now = Utc::now();
        let entry = ModelEntry {
            id: id.clone(),
            label,
            model,
            base_url: url,
            reasoning_levels,
            default_reasoning,
            multimodal,
            max_tokens,
            is_default,
            price,
            position: existing.as_ref().map_or(0, |e| e.position),
            listed_models: Some(listed),
            created_at: existing.as_ref().map_or(now, |e| e.created_at),
            updated_at: now,
        };
        let saved = self.store.upsert_model(&entry).await?;
        match (spec.api_key.as_deref(), &key) {
            (Some(_), Some(k)) => {
                self.writable(false)?
                    .put(&key_name, k, &json!({ "last4": secrets::last4(k) }))
                    .await?
            }
            (Some(_), None) => {
                self.store.delete_secret(&key_name).await?;
            }
            (None, _) => {}
        }
        self.apply_models().await?;
        tracing::info!(
            model = %saved.id,
            base_url = %saved.base_url,
            name = %saved.model,
            listed = saved.listed_models.as_ref().map_or(0, Vec::len),
            "model registered"
        );
        Ok(self.model_status(&saved).await?)
    }

    /// The models the server at `base_url` lists, so an admin picks a name it serves. Nothing is
    /// saved. Returns `{base_url, models}` with the address as it would be stored and the names
    /// sorted.
    pub async fn server_models(&self, query: ServerModelsQuery) -> Result<Value, AppError> {
        let url = normalize_llm_url(&query.base_url).map_err(AppError::BadRequest)?;
        let key = match query.api_key.as_deref().map(str::trim) {
            Some(k) if k.len() > MAX_SECRET_LEN || k.chars().any(char::is_control) => {
                return Err(AppError::BadRequest("api_key is malformed".into()))
            }
            Some(k) if !k.is_empty() => Some(k.to_string()),
            _ => match query.id.as_deref().map(str::trim) {
                Some(id) if !id.is_empty() => self.model_api_key(id).await,
                _ => None,
            },
        };
        let mut models = list_models(&url, key.as_deref(), REQUEST_TIMEOUT)
            .await
            .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
        models.sort();
        models.dedup();
        Ok(json!({ "base_url": url, "models": models }))
    }

    /// Forget a registered model and its key. Runs using it fail at their next model call.
    pub async fn remove_model(&self, id: &str) -> Result<(), AppError> {
        let _guard = self.changing.lock().await;
        if !self.store.delete_model(id).await? {
            return Err(AppError::NotFound("no such model is registered"));
        }
        self.store
            .delete_secret(&secrets::model_api_key(id))
            .await?;
        self.apply_models().await?;
        tracing::info!(model = id, "model removed");
        Ok(())
    }

    /// Every registered model, as shown to admins. Never includes a key.
    pub async fn models_status(&self) -> Result<Value> {
        let mut out = Vec::new();
        for entry in self.models.load().iter() {
            out.push(self.model_status(entry).await?);
        }
        Ok(json!({ "models": out }))
    }

    async fn model_status(&self, entry: &ModelEntry) -> Result<Value> {
        let mut status = entry.public();
        status["base_url"] = json!(entry.base_url);
        status["api_key_set"] = json!(
            self.store
                .has_secret(&secrets::model_api_key(&entry.id))
                .await?
        );
        status["listed_models"] = json!(entry.listed_models);
        status["created_at"] = json!(entry.created_at);
        status["updated_at"] = json!(entry.updated_at);
        Ok(status)
    }

    /// Why a tool group has no tools on this server, for the clients' settings pages.
    pub fn unavailable_reasons(&self) -> Vec<(&'static str, String)> {
        let cfg = &self.cfg;
        let off = |flag: &str| format!("Turned off on the server ({flag}=off)");
        let google = |prefix: &str, flag: &str| {
            let on = cfg.google_apis.iter().any(|a| a.prefix == prefix)
                || cfg.mcp_servers.iter().any(|s| s.prefix == prefix);
            if on {
                "No Google account is connected".to_string()
            } else {
                off(flag)
            }
        };
        vec![
            ("web_search", "Needs a Brave Search API key".into()),
            ("web_fetch", off("WEB_FETCH")),
            ("weather", off("WEATHER")),
            ("maps", off("MAPS")),
            ("recipes", off("RECIPES")),
            ("transit", off("TRANSIT")),
            ("stocks", off("STOCKS")),
            (
                "products",
                if cfg.products {
                    "Needs a Brave Search or Best Buy API key".to_string()
                } else {
                    off("PRODUCTS")
                },
            ),
            ("ask_user", off("ASK_USER")),
            ("browser", off("BROWSER")),
            ("gmail", google("gmail", "GMAIL")),
            ("calendar", google("calendar", "CALENDAR")),
            (
                "custom",
                match &cfg.sandbox.tools_dir {
                    None => "No TOOLS_DIR is set".to_string(),
                    Some(dir) => format!("No usable tool folder in {}", dir.display()),
                },
            ),
        ]
    }

    /// What is set up, and where it comes from. Never includes a secret.
    pub async fn status(&self) -> Result<Value> {
        let meta = |name: &'static str| async move {
            match &self.secrets {
                Some(s) => s.meta(name).await,
                None => Ok(None),
            }
        };
        let brave = match (&self.cfg.brave_api_key, meta(secrets::BRAVE_API_KEY).await?) {
            (Some(key), _) => {
                json!({ "configured": true, "source": "env", "last4": secrets::last4(key) })
            }
            (None, Some(m)) => json!({ "configured": true, "source": "ui", "last4": m["last4"] }),
            (None, None) => json!({ "configured": false, "source": null }),
        };
        let mut bestbuy = match (
            &self.cfg.bestbuy_api_key,
            meta(secrets::BESTBUY_API_KEY).await?,
        ) {
            (Some(key), _) => {
                json!({ "configured": true, "source": "env", "last4": secrets::last4(key) })
            }
            (None, Some(m)) => json!({ "configured": true, "source": "ui", "last4": m["last4"] }),
            (None, None) => json!({ "configured": false, "source": null }),
        };
        bestbuy["enabled"] = json!(self.cfg.products);
        let google = match &self.cfg.google_oauth {
            None => json!({ "enabled": false, "configured": false, "source": null }),
            Some(oauth) => {
                let mut g = match (
                    &oauth.refresh_token,
                    meta(secrets::GOOGLE_REFRESH_TOKEN).await?,
                ) {
                    (Some(_), _) => json!({ "configured": true, "source": "env" }),
                    (None, Some(m)) => {
                        json!({ "configured": true, "source": "ui", "account": m["account"], "scopes": m["scopes"] })
                    }
                    (None, None) => json!({ "configured": false, "source": null }),
                };
                g["enabled"] = json!(true);
                g["services"] = json!(self.google_services());
                g
            }
        };
        Ok(json!({
            "storage": self.secrets.is_some(),
            "models": self.models.load().len(),
            "brave": brave,
            "bestbuy": bestbuy,
            "google": google,
        }))
    }

    fn google_services(&self) -> Vec<&str> {
        let cfg = &self.cfg;
        cfg.google_apis
            .iter()
            .map(|a| a.prefix.as_str())
            .chain(cfg.mcp_servers.iter().map(|s| s.prefix.as_str()))
            .collect()
    }

    fn writable(&self, in_env: bool) -> Result<&SecretStore, AppError> {
        if in_env {
            return Err(AppError::Conflict(
                "this is set in the server's environment; change it there".into(),
            ));
        }
        self.secrets.as_ref().ok_or_else(|| {
            AppError::Conflict("saving integrations needs CEDAR_SECRET_KEY on the server".into())
        })
    }

    /// Check a Brave Search key with one search, then save it and rebuild the tools.
    pub async fn set_brave_key(&self, key: &str) -> Result<(), AppError> {
        let secrets = self.writable(self.cfg.brave_api_key.is_some())?;
        let key = key.trim();
        if key.is_empty() || key.len() > MAX_SECRET_LEN || key.chars().any(char::is_control) {
            return Err(AppError::BadRequest(
                "api_key is missing or malformed".into(),
            ));
        }
        BraveSearch::new(key.to_string(), self.cfg.brave_base_url.clone())?
            .check_key(REQUEST_TIMEOUT)
            .await
            .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
        let _guard = self.changing.lock().await;
        secrets
            .put(
                secrets::BRAVE_API_KEY,
                key,
                &json!({ "last4": secrets::last4(key) }),
            )
            .await?;
        self.reload().await?;
        tracing::info!("Brave Search key saved; web_search is on");
        Ok(())
    }

    pub async fn remove_brave_key(&self) -> Result<(), AppError> {
        let secrets = self.writable(self.cfg.brave_api_key.is_some())?;
        let _guard = self.changing.lock().await;
        if !secrets.delete(secrets::BRAVE_API_KEY).await? {
            return Err(AppError::NotFound("no Brave Search key is saved"));
        }
        self.reload().await?;
        tracing::info!("Brave Search key removed");
        Ok(())
    }

    /// Check a Best Buy key with one small search, then save it and rebuild the tools, which
    /// adds Best Buy's catalog to `search_products` (and turns it on without a Brave key) unless
    /// `PRODUCTS=off`.
    pub async fn set_bestbuy_key(&self, key: &str) -> Result<(), AppError> {
        let secrets = self.writable(self.cfg.bestbuy_api_key.is_some())?;
        let key = key.trim();
        if key.is_empty() || key.len() > MAX_SECRET_LEN || key.chars().any(char::is_control) {
            return Err(AppError::BadRequest(
                "api_key is missing or malformed".into(),
            ));
        }
        BestBuy::new(key.to_string(), self.cfg.products_api_url.clone())?
            .check_key(REQUEST_TIMEOUT)
            .await
            .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
        let _guard = self.changing.lock().await;
        secrets
            .put(
                secrets::BESTBUY_API_KEY,
                key,
                &json!({ "last4": secrets::last4(key) }),
            )
            .await?;
        self.reload().await?;
        tracing::info!("Best Buy key saved; search_products is on");
        Ok(())
    }

    pub async fn remove_bestbuy_key(&self) -> Result<(), AppError> {
        let secrets = self.writable(self.cfg.bestbuy_api_key.is_some())?;
        let _guard = self.changing.lock().await;
        if !secrets.delete(secrets::BESTBUY_API_KEY).await? {
            return Err(AppError::NotFound("no Best Buy key is saved"));
        }
        self.reload().await?;
        tracing::info!("Best Buy key removed");
        Ok(())
    }

    /// Start "Connect Google": the URL to send the admin's browser to. Google sends it back to
    /// `redirect_uri` with a code and the `state` returned here, which works once.
    pub fn google_authorize(&self, redirect_uri: &str) -> Result<String, AppError> {
        let oauth = self.cfg.google_oauth.as_ref().ok_or_else(|| {
            AppError::Conflict("Gmail and Calendar are off on the server (GMAIL, CALENDAR)".into())
        })?;
        self.writable(oauth.refresh_token.is_some())?;
        let redirect = reqwest::Url::parse(redirect_uri)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https") && redirect_uri.len() <= 500)
            .ok_or_else(|| AppError::BadRequest("redirect_uri must be an http(s) URL".into()))?;

        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let state = hex::encode(bytes);
        {
            let mut pending = self.pending.lock().unwrap();
            pending.retain(|_, (_, at)| at.elapsed() < OAUTH_STATE_TTL);
            if pending.len() >= MAX_PENDING_STATES {
                return Err(AppError::TooManyRequests(
                    "too many Google connections in progress; try again in a few minutes".into(),
                ));
            }
            pending.insert(state.clone(), (redirect.to_string(), Instant::now()));
        }
        let url = reqwest::Url::parse_with_params(
            &oauth.auth_url,
            &[
                ("client_id", oauth.client_id.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("response_type", "code"),
                ("scope", &oauth.scopes.join(" ")),
                ("access_type", "offline"),
                // Without it Google only returns a refresh token the first time an account consents.
                ("prompt", "consent"),
                ("include_granted_scopes", "true"),
                ("state", &state),
            ],
        )
        .context("GOOGLE_AUTH_URL is not a valid URL")?;
        Ok(url.into())
    }

    /// Finish "Connect Google": trade the code for a refresh token, save it, rebuild the tools.
    pub async fn google_callback(&self, state: &str, code: &str) -> Result<Value, AppError> {
        let oauth = self.cfg.google_oauth.as_ref().ok_or(AppError::NotFound(
            "Gmail and Calendar are off on the server",
        ))?;
        let secrets = self.writable(oauth.refresh_token.is_some())?;
        let redirect_uri = {
            let mut pending = self.pending.lock().unwrap();
            match pending.remove(state) {
                Some((uri, at)) if at.elapsed() < OAUTH_STATE_TTL => uri,
                _ => {
                    return Err(AppError::BadRequest(
                        "this Google sign-in link is unknown, used, or expired; start again".into(),
                    ))
                }
            }
        };
        if code.is_empty() || code.len() > 2000 {
            return Err(AppError::BadRequest("code is missing or malformed".into()));
        }

        let resp = self
            .http
            .post(&oauth.token_url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", &oauth.client_id),
                ("client_secret", &oauth.client_secret),
                ("redirect_uri", &redirect_uri),
            ])
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| AppError::BadGateway(format!("could not reach Google: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            let why = body["error_description"]
                .as_str()
                .or(body["error"].as_str())
                .unwrap_or("no details");
            return Err(AppError::BadRequest(format!(
                "Google refused the sign-in ({status}): {why}"
            )));
        }
        let tokens: TokenResponse = resp
            .json()
            .await
            .map_err(|e| AppError::BadGateway(format!("unreadable answer from Google: {e}")))?;
        let refresh_token = tokens.refresh_token.ok_or_else(|| {
            AppError::BadRequest("Google returned no refresh token; remove cedar's access in your Google account and connect again".into())
        })?;
        let account = tokens.id_token.as_deref().and_then(email_from_id_token);
        let scopes: Vec<&str> = tokens
            .scope
            .as_deref()
            .map(|s| s.split(' ').filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();

        let _guard = self.changing.lock().await;
        secrets
            .put(
                secrets::GOOGLE_REFRESH_TOKEN,
                &refresh_token,
                &json!({ "account": account, "scopes": scopes }),
            )
            .await?;
        self.reload().await?;
        tracing::info!(account = ?account, "Google account connected");
        Ok(json!({ "account": account }))
    }

    pub async fn remove_google(&self) -> Result<(), AppError> {
        let in_env = self
            .cfg
            .google_oauth
            .as_ref()
            .is_some_and(|o| o.refresh_token.is_some());
        let secrets = self.writable(in_env)?;
        let _guard = self.changing.lock().await;
        if !secrets.delete(secrets::GOOGLE_REFRESH_TOKEN).await? {
            return Err(AppError::NotFound("no Google account is connected"));
        }
        self.reload().await?;
        tracing::info!("Google account disconnected");
        Ok(())
    }
}

/// A model server address as stored: an http(s) URL with a host, its path defaulting to `/v1`,
/// without a trailing slash, query, fragment, user or password (the key goes in `api_key`).
fn normalize_llm_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let bad =
        || "base_url must be an http(s) URL with a host, like http://10.0.0.5:8000/v1".to_string();
    if raw.len() > 500 {
        return Err(bad());
    }
    let mut url = reqwest::Url::parse(raw).map_err(|_| bad())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(bad());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("base_url must not carry a user or password; put the key in api_key".into());
    }
    url.set_query(None);
    url.set_fragment(None);
    if url.path() == "/" {
        url.set_path("/v1");
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// A model name as stored: trimmed, `None` when empty.
fn clean_model_name(raw: &str) -> Result<Option<String>, AppError> {
    let name = raw.trim();
    if name.len() > MAX_MODEL_LEN || name.chars().any(char::is_control) {
        return Err(AppError::BadRequest("model name is malformed".into()));
    }
    Ok((!name.is_empty()).then(|| name.to_string()))
}

/// A registry id: lowercase letters, digits, `.`, `_` and `-`, starting with a letter or digit.
fn clean_model_id(raw: &str) -> Result<String, AppError> {
    let id = raw.trim();
    let ok = !id.is_empty()
        && id.len() <= MAX_MODEL_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !ok {
        return Err(AppError::BadRequest(format!(
            "model id {id:?} is malformed: use lowercase letters, digits, '.', '_' and '-' (at most {MAX_MODEL_ID_LEN} characters)"
        )));
    }
    Ok(id.to_string())
}

/// Reasoning levels as stored: trimmed, in the order given, without repeats; each a short token
/// as a server would take it in `reasoning_effort` (`none`, `low`, `high`, ...).
fn clean_reasoning_levels(raw: &[String]) -> Result<Vec<String>, AppError> {
    let mut levels: Vec<String> = Vec::new();
    for level in raw {
        let level = level.trim();
        if level.is_empty() {
            continue;
        }
        let ok = level.len() <= MAX_REASONING_LEVEL_LEN
            && level
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        if !ok {
            return Err(AppError::BadRequest(format!(
                "reasoning level {level:?} is malformed: use letters, digits, '_', '-' and '.' (at most {MAX_REASONING_LEVEL_LEN} characters)"
            )));
        }
        if !levels.iter().any(|l| l == level) {
            levels.push(level.to_string());
        }
    }
    if levels.len() > MAX_REASONING_LEVELS {
        return Err(AppError::BadRequest(format!(
            "at most {MAX_REASONING_LEVELS} reasoning levels"
        )));
    }
    Ok(levels)
}

/// The reasoning levels a model server saved by an earlier version most likely takes: DeepSeek V4
/// on vLLM accepts these as `reasoning_effort`; for anything else the admin fills them in.
fn suggested_reasoning_levels(model: &str) -> Vec<String> {
    if model.to_ascii_lowercase().contains("deepseek-v4") {
        ["none", "low", "high", "max"]
            .into_iter()
            .map(String::from)
            .collect()
    } else {
        vec![]
    }
}

/// A URL without the user and password it may carry, for showing and logging.
fn redact_url(raw: &str) -> String {
    match reqwest::Url::parse(raw) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.as_str().trim_end_matches('/').to_string()
        }
        Err(_) => raw.to_string(),
    }
}

/// The `email` claim of an ID token. It comes straight from Google's token endpoint over TLS, so
/// it is read without checking the signature; it is only shown to admins.
fn email_from_id_token(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims["email"].as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_server_addresses_are_normalized_and_redacted() {
        assert_eq!(
            normalize_llm_url(" http://10.0.0.5:8000 ").unwrap(),
            "http://10.0.0.5:8000/v1"
        );
        assert_eq!(
            normalize_llm_url("https://gpu.example/v1/?x=1#y").unwrap(),
            "https://gpu.example/v1"
        );
        assert_eq!(
            normalize_llm_url("http://box:9000/openai/v1").unwrap(),
            "http://box:9000/openai/v1"
        );
        for bad in [
            "",
            "box:8000",
            "ftp://box/v1",
            "http://",
            "file:///etc/passwd",
            "http://user:pw@box:8000/v1",
        ] {
            assert!(normalize_llm_url(bad).is_err(), "{bad}");
        }
        assert_eq!(
            redact_url("http://user:pw@box:8000/v1"),
            "http://box:8000/v1"
        );
    }

    #[test]
    fn model_ids_and_reasoning_levels_are_checked() {
        assert_eq!(
            clean_model_id(" deepseek-v4.flash_1 ").unwrap(),
            "deepseek-v4.flash_1"
        );
        for bad in ["", "Deep", "-a", "a b", "a/b", &"x".repeat(65)] {
            assert!(clean_model_id(bad).is_err(), "{bad:?}");
        }
        let levels =
            clean_reasoning_levels(&["none".into(), " high ".into(), "".into(), "none".into()])
                .unwrap();
        assert_eq!(levels, ["none", "high"]);
        assert!(clean_reasoning_levels(&["hi gh".into()]).is_err());
        assert_eq!(
            suggested_reasoning_levels("deepseek-ai/DeepSeek-V4-Flash"),
            ["none", "low", "high", "max"]
        );
        assert!(suggested_reasoning_levels("gpt-oss-120b").is_empty());
    }

    #[test]
    fn the_email_is_read_from_the_id_token() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"email":"ana@example.com","sub":"1"}"#);
        assert_eq!(
            email_from_id_token(&format!("h.{payload}.sig")).as_deref(),
            Some("ana@example.com")
        );
        assert_eq!(email_from_id_token("garbage"), None);
        assert_eq!(email_from_id_token("a.!!!.c"), None);
    }
}
