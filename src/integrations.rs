//! Integrations an admin can set up through the API instead of the environment: a Brave Search
//! key and a Google account ("Connect Google"). Their credentials are kept encrypted (see
//! [`crate::secrets`]), and saving or removing one rebuilds the tool set, so the next run sees the
//! change without a restart. Runs already going keep the tools they started with.
//!
//! The environment wins: a credential set there is reported as such and can't be changed here.

use crate::api::MAX_MODEL_LEN;
use crate::config::Config;
use crate::error::AppError;
use crate::llm::{list_models, SwappableLlm};
use crate::mcp::http_client;
use crate::secrets::{self, SecretStore};
use crate::store::Store;
use crate::tools::{BraveSearch, Overrides, ToolSet};
use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use base64::Engine;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::OsRng;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a "Connect Google" link stays usable.
const OAUTH_STATE_TTL: Duration = Duration::from_secs(600);
/// At most this many "Connect Google" attempts are pending at once.
const MAX_PENDING_STATES: usize = 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SECRET_LEN: usize = 500;

pub struct Integrations {
    cfg: Arc<Config>,
    secrets: Option<SecretStore>,
    tools: Arc<ArcSwap<ToolSet>>,
    http: reqwest::Client,
    /// "Connect Google" attempts in flight: state → (redirect URI, issued at).
    pending: Mutex<HashMap<String, (String, Instant)>>,
    /// Serializes changes, so two admins can't interleave a save and a rebuild.
    changing: tokio::sync::Mutex<()>,
    /// The model server client, when it can be repointed (see [`Self::attach_llm`]).
    llm: OnceLock<Arc<SwappableLlm>>,
    /// The model a run gets when it doesn't name one: the environment's, else the saved one, else
    /// the built-in default. Kept here so every run creation reads it without a query.
    default_model: ArcSwap<String>,
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
            secrets: cfg.secret_key.as_ref().map(|k| SecretStore::new(store, k)),
            default_model: ArcSwap::from_pointee(cfg.default_model.clone()),
            cfg,
            tools: Arc::new(ArcSwap::from_pointee(tools)),
            http: http_client()?,
            pending: Mutex::default(),
            changing: tokio::sync::Mutex::default(),
            llm: OnceLock::new(),
        })
    }

    /// The model a run gets when it doesn't name one.
    pub fn default_model(&self) -> String {
        self.default_model.load().to_string()
    }

    /// The current tool set, shared with the agent.
    pub fn tools(&self) -> Arc<ArcSwap<ToolSet>> {
        self.tools.clone()
    }

    /// Rebuild the tools from the environment plus the saved credentials. A credential that can't
    /// be decrypted is logged and left out rather than keeping cedar from starting.
    pub async fn reload(&self) -> Result<()> {
        self.apply_llm().await?;
        let mut overrides = Overrides::default();
        if let Some(secrets) = &self.secrets {
            for (name, slot) in [
                (secrets::BRAVE_API_KEY, &mut overrides.brave_api_key),
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
        }
        let tools = ToolSet::build(&self.cfg, &overrides)?;
        self.tools.store(Arc::new(tools));
        Ok(())
    }

    /// Let admins repoint the model server: `llm` is the client the agent and compaction use.
    /// Call [`Self::reload`] afterwards to apply a saved address.
    pub fn attach_llm(&self, llm: Arc<SwappableLlm>) {
        let _ = self.llm.set(llm);
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

    /// The model server address and key in effect: the environment's, else the saved ones, else
    /// the built-in default address.
    async fn llm_endpoint(&self) -> (String, Option<String>) {
        let cfg = &self.cfg;
        let url = match cfg.llm_base_url_from_env {
            true => None,
            false => self.saved(secrets::LLM_BASE_URL).await,
        };
        let key = match cfg.llm_api_key_from_env {
            true => None,
            false => self.saved(secrets::LLM_API_KEY).await,
        };
        (
            url.unwrap_or_else(|| cfg.llm_base_url.clone()),
            key.or_else(|| cfg.llm_api_key.clone()),
        )
    }

    /// The saved model name, unless the environment sets one (which then wins).
    async fn saved_model(&self) -> Option<String> {
        match self.cfg.llm_model_from_env {
            true => None,
            false => self.saved(secrets::LLM_MODEL).await,
        }
    }

    async fn apply_llm(&self) -> Result<()> {
        let model = self
            .saved_model()
            .await
            .unwrap_or_else(|| self.cfg.default_model.clone());
        self.default_model.store(Arc::new(model));
        if let Some(llm) = self.llm.get() {
            let (url, key) = self.llm_endpoint().await;
            if llm.base_url() != url {
                tracing::info!(base_url = %redact_url(&url), "model server");
            }
            llm.set(url, key)?;
        }
        Ok(())
    }

    /// Point cedar at another model server: check that `base_url` answers like an
    /// OpenAI-compatible server (listing its models), then save it and use it for the next model
    /// call. `api_key` and `model`: `None` keeps the saved one, empty removes it.
    pub async fn set_llm(
        &self,
        base_url: &str,
        api_key: Option<&str>,
        model: Option<&str>,
    ) -> Result<Value, AppError> {
        if self.llm.get().is_none() {
            return Err(AppError::Conflict(
                "this server's model client can't be changed".into(),
            ));
        }
        let secrets = self.writable(self.cfg.llm_base_url_from_env)?;
        if api_key.is_some() && self.cfg.llm_api_key_from_env {
            return Err(AppError::Conflict(
                "LLM_API_KEY is set in the server's environment; change it there".into(),
            ));
        }
        if model.is_some() && self.cfg.llm_model_from_env {
            return Err(AppError::Conflict(
                "LLM_MODEL is set in the server's environment; change it there".into(),
            ));
        }
        let model = model.map(clean_model_name).transpose()?;
        let url = normalize_llm_url(base_url).map_err(AppError::BadRequest)?;
        let key = match api_key.map(str::trim) {
            Some("") => None,
            Some(k) if k.len() > MAX_SECRET_LEN || k.chars().any(char::is_control) => {
                return Err(AppError::BadRequest("api_key is malformed".into()))
            }
            Some(k) => Some(k.to_string()),
            None => self.llm_endpoint().await.1,
        };
        let models = list_models(&url, key.as_deref(), REQUEST_TIMEOUT)
            .await
            .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;

        let _guard = self.changing.lock().await;
        let shown = redact_url(&url);
        secrets
            .put(
                secrets::LLM_BASE_URL,
                &url,
                &json!({ "base_url": shown, "models": models }),
            )
            .await?;
        match (api_key, &key) {
            (Some(_), Some(k)) => {
                secrets
                    .put(
                        secrets::LLM_API_KEY,
                        k,
                        &json!({ "last4": secrets::last4(k) }),
                    )
                    .await?
            }
            (Some(_), None) => {
                secrets.delete(secrets::LLM_API_KEY).await?;
            }
            (None, _) => {}
        }
        match &model {
            Some(Some(m)) => {
                secrets
                    .put(secrets::LLM_MODEL, m, &json!({ "model": m }))
                    .await?
            }
            Some(None) => {
                secrets.delete(secrets::LLM_MODEL).await?;
            }
            None => {}
        }
        self.apply_llm().await?;
        tracing::info!(base_url = %shown, models = models.len(), "model server changed");
        Ok(self.llm_status().await?)
    }

    /// Go back to the environment's (or the default) model server, key and model name.
    pub async fn remove_llm(&self) -> Result<(), AppError> {
        let secrets = self.writable(self.cfg.llm_base_url_from_env)?;
        let _guard = self.changing.lock().await;
        let removed = secrets.delete(secrets::LLM_BASE_URL).await?;
        let removed_key = secrets.delete(secrets::LLM_API_KEY).await?;
        let removed_model = secrets.delete(secrets::LLM_MODEL).await?;
        if !removed && !removed_key && !removed_model {
            return Err(AppError::NotFound("no model server address is saved"));
        }
        self.apply_llm().await?;
        Ok(())
    }

    async fn llm_status(&self) -> Result<Value> {
        let cfg = &self.cfg;
        let meta = match &self.secrets {
            Some(s) if !cfg.llm_base_url_from_env => s.meta(secrets::LLM_BASE_URL).await?,
            _ => None,
        };
        let key_saved = match &self.secrets {
            Some(s) if !cfg.llm_api_key_from_env => s.meta(secrets::LLM_API_KEY).await?.is_some(),
            _ => false,
        };
        let model_saved = match &self.secrets {
            Some(s) if !cfg.llm_model_from_env => s.meta(secrets::LLM_MODEL).await?.is_some(),
            _ => false,
        };
        let model_source = if cfg.llm_model_from_env {
            "env"
        } else if model_saved {
            "ui"
        } else {
            "default"
        };
        let (url, key) = self.llm_endpoint().await;
        let source = if cfg.llm_base_url_from_env {
            "env"
        } else if meta.is_some() {
            "ui"
        } else {
            "default"
        };
        let key_source = if cfg.llm_api_key_from_env {
            json!("env")
        } else if key_saved {
            json!("ui")
        } else {
            Value::Null
        };
        Ok(json!({
            "base_url": redact_url(&url),
            "source": source,
            "api_key_set": key.is_some(),
            "api_key_source": key_source,
            "default_model": self.default_model(),
            "model_source": model_source,
            "models": meta.as_ref().map(|m| m["models"].clone()),
            "changeable": self.llm.get().is_some(),
        }))
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
            ("browser", off("BROWSER")),
            ("gmail", google("gmail", "GMAIL")),
            ("calendar", google("calendar", "CALENDAR")),
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
        let llm = self.llm_status().await?;
        Ok(
            json!({ "storage": self.secrets.is_some(), "llm": llm, "brave": brave, "google": google }),
        )
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
/// without a trailing slash, query, or fragment.
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
    url.set_query(None);
    url.set_fragment(None);
    if url.path() == "/" {
        url.set_path("/v1");
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// A model name as stored: trimmed, `None` when empty (meaning "forget the saved one").
fn clean_model_name(raw: &str) -> Result<Option<String>, AppError> {
    let name = raw.trim();
    if name.len() > MAX_MODEL_LEN || name.chars().any(char::is_control) {
        return Err(AppError::BadRequest("model name is malformed".into()));
    }
    Ok((!name.is_empty()).then(|| name.to_string()))
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
        ] {
            assert!(normalize_llm_url(bad).is_err(), "{bad}");
        }
        assert_eq!(
            redact_url("http://user:pw@box:8000/v1"),
            "http://box:8000/v1"
        );
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
