//! Integrations an admin can set up through the API instead of the environment: a Brave Search
//! key and a Google account ("Connect Google"). Their credentials are kept encrypted (see
//! [`crate::secrets`]), and saving or removing one rebuilds the tool set, so the next run sees the
//! change without a restart. Runs already going keep the tools they started with.
//!
//! The environment wins: a credential set there is reported as such and can't be changed here.

use crate::config::Config;
use crate::error::AppError;
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
use std::sync::{Arc, Mutex};
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
            cfg,
            tools: Arc::new(ArcSwap::from_pointee(tools)),
            http: http_client()?,
            pending: Mutex::default(),
            changing: tokio::sync::Mutex::default(),
        })
    }

    /// The current tool set, shared with the agent.
    pub fn tools(&self) -> Arc<ArcSwap<ToolSet>> {
        self.tools.clone()
    }

    /// Rebuild the tools from the environment plus the saved credentials. A credential that can't
    /// be decrypted is logged and left out rather than keeping cedar from starting.
    pub async fn reload(&self) -> Result<()> {
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
        Ok(json!({ "storage": self.secrets.is_some(), "brave": brave, "google": google }))
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
