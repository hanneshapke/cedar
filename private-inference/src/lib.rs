//! The connect flow of Cedar Private Inference: how a Cedar app gets a provider key and the
//! models to register with it, without the user copying a key by hand.
//!
//! It follows cedar-plg's HTTP contract (`docs/contract.md` there, draft 1), which is modelled on
//! the OAuth device flow with a PKCE-style verifier:
//!
//! 1. [`Challenge::new`] makes a secret verifier and its public challenge.
//! 2. [`Client::start`] registers the challenge and gets a short code. The app shows the code and
//!    opens [`Started::verification_uri_complete`] in a browser, where the user signs in and
//!    approves after checking that the codes match.
//! 3. [`Client::poll`] (or [`Client::connect`], which loops) presents the verifier until the
//!    service hands over the [`Grant`], exactly once.
//!
//! The service is trusted with nothing about privacy: this crate takes a key, a base URL and a
//! model list from it, and that is all. Prompts never go through the service; Cedar sends them to
//! [`Grant::base_url`] directly.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

/// `ModelEntry::service` in cedar for the models this flow registers.
pub const SERVICE: &str = "cedar-private-inference";

/// The environment variable that points Cedar at another cedar-plg, for testing against a local
/// one (`http://localhost:3000`).
pub const URL_ENV: &str = "CEDAR_PRIVATE_INFERENCE_URL";

/// The production service's address. `None` until it is deployed; until then [`service_url`]
/// needs [`URL_ENV`].
pub const PRODUCTION_URL: Option<&str> = None;

/// The fixed registry ids of the two models a grant registers, so connecting again replaces them
/// instead of adding more.
pub const SMALL_ID: &str = "private-small";
pub const LARGE_ID: &str = "private-large";

/// The most characters of `client_name` the service keeps.
const MAX_CLIENT_NAME: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cedar Private Inference is not available in this build; set {URL_ENV} to a service address")]
    NotConfigured,
    #[error("{URL_ENV} is not an http(s) address: {0}")]
    BadServiceUrl(String),
    #[error("could not reach Cedar Private Inference: {0}")]
    Unreachable(#[source] reqwest::Error),
    /// The service refused the request (`invalid_request`, or a status the contract doesn't name).
    #[error("Cedar Private Inference refused the request ({status}): {message}")]
    Refused { status: u16, message: String },
    #[error("the connection was declined in the browser")]
    Denied,
    #[error("the code expired before it was approved; start again")]
    Expired,
    /// Unknown verifier, or the grant was already handed over (to this poll or another).
    #[error("this connection request is no longer valid; start again")]
    InvalidGrant,
    /// The service answered in a shape the contract doesn't allow.
    #[error("Cedar Private Inference sent an unexpected answer: {0}")]
    Malformed(String),
}

/// The address of the service: [`URL_ENV`] when set, else [`PRODUCTION_URL`].
pub fn service_url() -> Result<String, Error> {
    let url = std::env::var(URL_ENV)
        .ok()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .or_else(|| PRODUCTION_URL.map(String::from))
        .ok_or(Error::NotConfigured)?;
    check_http_url(&url).map_err(|_| Error::BadServiceUrl(url.clone()))?;
    Ok(url.trim_end_matches('/').to_string())
}

/// The secret verifier the app keeps and the challenge it shows the service:
/// `challenge = base64url(sha256(verifier))`, with `verifier` 32 random bytes in base64url.
#[derive(Clone)]
pub struct Challenge {
    verifier: String,
    challenge: String,
}

impl Challenge {
    pub fn new() -> Self {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self::from_verifier(URL_SAFE_NO_PAD.encode(bytes))
    }

    fn from_verifier(verifier: String) -> Self {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }

    /// Only ever sent to the poll endpoint; whoever holds it can collect the key.
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

impl Default for Challenge {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Challenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Challenge")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

/// What the app shows while it waits: the code, and where to approve it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Started {
    /// E.g. `NHKK-KZGJ`. The approval page shows the same code.
    pub user_code: String,
    pub verification_uri: String,
    /// `verification_uri` with the challenge in it: the page to open in the browser.
    pub verification_uri_complete: String,
    /// Seconds until the code expires.
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

/// What one poll found.
#[derive(Debug, Clone, PartialEq)]
pub enum PollOutcome {
    /// Not decided yet; poll again after the interval.
    Pending,
    /// Polled too fast; wait longer before the next one.
    SlowDown,
    /// The user declined. Stop.
    Denied,
    /// Not approved in time. Start again.
    Expired,
    /// Approved, but the service couldn't create the key yet. Poll again.
    Unavailable,
    /// Approved: the key and the models. The service hands this over once.
    Granted(Grant),
}

/// The key, where to use it and with which models.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Grant {
    /// The provider's OpenAI-compatible address, e.g. `https://api.doubleword.ai/v1`.
    pub base_url: String,
    pub api_key: String,
    /// The provider's name, e.g. `doubleword`.
    pub provider: String,
    pub models: Vec<GrantModel>,
    /// Where the user sees their balance, tops up, and rotates or revokes the key.
    pub account_url: String,
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grant")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("provider", &self.provider)
            .field("models", &self.models)
            .field("account_url", &self.account_url)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GrantModel {
    /// The provider's name for the model, sent as `model` in each request.
    pub id: String,
    /// A display name that carries the size ("DeepSeek V4.1 Flash 120B"), so cedar's routing can
    /// rank it: register it as the entry's label.
    pub name: String,
    pub role: Role,
    /// The total parameter count in billions.
    pub params_b: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Small,
    Large,
    /// A role a later version of the contract adds.
    #[serde(other)]
    Other,
}

impl Grant {
    /// The models to register, each with its fixed registry id ([`SMALL_ID`], [`LARGE_ID`]), the
    /// small one first. A grant with one model registers it as the small one. Models with a role
    /// this version doesn't know, and a second model of the same role, are left out.
    pub fn entries(&self) -> Vec<(&'static str, &GrantModel)> {
        let small = self.models.iter().find(|m| m.role == Role::Small);
        let large = self.models.iter().find(|m| m.role == Role::Large);
        match (small, large) {
            (Some(s), Some(l)) => vec![(SMALL_ID, s), (LARGE_ID, l)],
            (Some(only), None) | (None, Some(only)) => vec![(SMALL_ID, only)],
            (None, None) => vec![],
        }
    }

    /// What to register in cedar: for each model, its registry id and the body of cedar's
    /// `PUT /v1/admin/models/{id}`. The small one is made the default; routing moves a run to the
    /// large one when the task needs it. Whether a model reads images is left to cedar to find
    /// out (`detect_images`).
    pub fn model_specs(&self) -> Vec<(&'static str, serde_json::Value)> {
        self.entries()
            .into_iter()
            .map(|(id, m)| {
                let mut spec = serde_json::json!({
                    "base_url": self.base_url,
                    "model": m.id,
                    "label": m.name,
                    "api_key": self.api_key,
                    "service": SERVICE,
                    "account_url": self.account_url,
                    "detect_images": true,
                });
                if id == SMALL_ID {
                    spec["is_default"] = true.into();
                }
                (id, spec)
            })
            .collect()
    }

    fn check(&self) -> Result<(), Error> {
        let bad = |what: &str| Err(Error::Malformed(what.to_string()));
        if check_http_url(&self.base_url).is_err() {
            return bad("base_url is not an http(s) address");
        }
        if check_http_url(&self.account_url).is_err() {
            return bad("account_url is not an http(s) address");
        }
        if self.api_key.trim().is_empty() || self.api_key.chars().any(char::is_control) {
            return bad("api_key is empty or malformed");
        }
        if self.entries().is_empty() {
            return bad("no small or large model");
        }
        for m in &self.models {
            if m.id.trim().is_empty() || m.name.trim().is_empty() {
                return bad("a model without an id or name");
            }
        }
        Ok(())
    }
}

/// How long registering a new key's models is retried: a provider refuses a key for a few seconds
/// after it is created, topped up or rotated, and cedar checks the key by listing the models.
pub const ACTIVATION_WAIT: Duration = Duration::from_secs(60);

/// Runs `attempt` until it succeeds or [`ACTIVATION_WAIT`] has passed, waiting 2 s, then 4, 8
/// and 10 s between tries. The last error is returned.
pub async fn retry_while_activating<T, E, F, Fut>(attempt: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    retry_for(ACTIVATION_WAIT, Duration::from_secs(2), attempt).await
}

async fn retry_for<T, E, F, Fut>(limit: Duration, first: Duration, mut attempt: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let deadline = Instant::now() + limit;
    let mut wait = first;
    loop {
        match attempt().await {
            Ok(v) => return Ok(v),
            Err(e) if Instant::now() + wait < deadline => {
                tracing::info!(error = %e, ?wait, "the new key isn't accepted yet; trying again");
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(first * 5);
            }
            Err(e) => return Err(e),
        }
    }
}

fn check_http_url(s: &str) -> Result<(), ()> {
    match url::Url::parse(s) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.host().is_some() => Ok(()),
        _ => Err(()),
    }
}

/// The contract's error body: `{"error": "<code>", "error_description"?: "..."}`.
#[derive(Deserialize)]
struct ErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    /// The shortest wait between polls, whatever `interval` the service gives.
    poll_floor: Duration,
    /// How much longer to wait after each `slow_down`.
    slow_down_step: Duration,
}

impl Client {
    /// A client for the service at `service_url` (see [`service_url`]).
    pub fn new(service_url: &str) -> Result<Self, Error> {
        check_http_url(service_url).map_err(|_| Error::BadServiceUrl(service_url.into()))?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(Error::Unreachable)?;
        Ok(Self {
            http,
            base: service_url.trim_end_matches('/').to_string(),
            poll_floor: Duration::from_secs(1),
            // RFC 8628 § 3.5.
            slow_down_step: Duration::from_secs(5),
        })
    }

    /// Change the poll timing (tests poll a stub much faster than a person approves).
    pub fn with_poll_timing(mut self, floor: Duration, slow_down_step: Duration) -> Self {
        self.poll_floor = floor;
        self.slow_down_step = slow_down_step;
        self
    }

    /// Step 1: register `challenge` and get the code to show. `client_name` (e.g. "Cedar Desktop
    /// on my-laptop") is shown on the approval page.
    pub async fn start(
        &self,
        challenge: &Challenge,
        client_name: Option<&str>,
    ) -> Result<Started, Error> {
        let mut body = serde_json::json!({ "challenge": challenge.challenge() });
        if let Some(name) = client_name.map(str::trim).filter(|n| !n.is_empty()) {
            let name: String = name.chars().take(MAX_CLIENT_NAME).collect();
            body["client_name"] = name.into();
        }
        let (status, text) = self.post("/api/connect/start", &body).await?;
        if status != 200 {
            return Err(refused(status, &text));
        }
        let started: Started =
            serde_json::from_str(&text).map_err(|e| Error::Malformed(format!("start: {e}")))?;
        if check_http_url(&started.verification_uri_complete).is_err() {
            return Err(Error::Malformed(
                "verification_uri_complete is not an http(s) address".into(),
            ));
        }
        Ok(started)
    }

    /// Step 3, once: ask whether the user approved.
    pub async fn poll(&self, verifier: &str) -> Result<PollOutcome, Error> {
        let body = serde_json::json!({ "verifier": verifier });
        let (status, text) = self.post("/api/connect/poll", &body).await?;
        if status == 200 {
            let grant: Grant =
                serde_json::from_str(&text).map_err(|e| Error::Malformed(format!("grant: {e}")))?;
            grant.check()?;
            return Ok(PollOutcome::Granted(grant));
        }
        let code = serde_json::from_str::<ErrorBody>(&text)
            .ok()
            .map(|b| b.error);
        match (status, code.as_deref()) {
            (400, Some("authorization_pending")) => Ok(PollOutcome::Pending),
            (400, Some("slow_down")) => Ok(PollOutcome::SlowDown),
            (400, Some("access_denied")) => Ok(PollOutcome::Denied),
            (400, Some("expired_token")) => Ok(PollOutcome::Expired),
            (400, Some("invalid_grant")) => Err(Error::InvalidGrant),
            (503, _) => Ok(PollOutcome::Unavailable),
            _ => Err(refused(status, &text)),
        }
    }

    /// Step 3 until it's decided: poll every `started.interval` seconds (longer after each
    /// `slow_down`), until the grant arrives, the user declines, or `started.expires_in` passes.
    /// A poll that can't reach the service is tried again at the next interval.
    pub async fn connect(&self, challenge: &Challenge, started: &Started) -> Result<Grant, Error> {
        let deadline = Instant::now() + Duration::from_secs(started.expires_in);
        let mut interval = Duration::from_secs(started.interval).max(self.poll_floor);
        loop {
            tokio::time::sleep(interval).await;
            if Instant::now() >= deadline {
                return Err(Error::Expired);
            }
            match self.poll(challenge.verifier()).await {
                Ok(PollOutcome::Granted(grant)) => return Ok(grant),
                Ok(PollOutcome::Pending | PollOutcome::Unavailable) => {}
                Ok(PollOutcome::SlowDown) => interval += self.slow_down_step,
                Ok(PollOutcome::Denied) => return Err(Error::Denied),
                Ok(PollOutcome::Expired) => return Err(Error::Expired),
                Err(Error::Unreachable(e)) => {
                    tracing::warn!(error = %e, "could not reach Cedar Private Inference; polling again");
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn post(&self, path: &str, body: &serde_json::Value) -> Result<(u16, String), Error> {
        let resp = self
            .http
            .post(format!("{}{path}", self.base))
            .json(body)
            .send()
            .await
            .map_err(Error::Unreachable)?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(Error::Unreachable)?;
        Ok((status, text))
    }
}

fn refused(status: u16, body: &str) -> Error {
    let message = match serde_json::from_str::<ErrorBody>(body) {
        Ok(b) => b.error_description.unwrap_or(b.error),
        Err(_) => body.chars().take(300).collect(),
    };
    Error::Refused { status, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_the_hash_of_the_verifier() {
        let c = Challenge::new();
        assert_eq!(c.verifier().len(), 43);
        assert_eq!(c.challenge().len(), 43);
        assert!(c
            .challenge()
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
        assert_ne!(c.verifier(), Challenge::new().verifier());
        // RFC 7636 appendix B: the same construction, with its published values.
        let known = Challenge::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(
            known.challenge(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert!(!format!("{known:?}").contains(known.verifier()));
    }

    fn grant(models: serde_json::Value) -> Grant {
        serde_json::from_value(serde_json::json!({
            "base_url": "https://api.example/v1", "api_key": "sk-1", "provider": "p",
            "models": models, "account_url": "https://plg.example/account",
            "a_field_from_a_later_version": true,
        }))
        .unwrap()
    }

    #[test]
    fn a_grant_registers_its_small_model_first_and_ignores_unknown_roles() {
        let g = grant(serde_json::json!([
            { "id": "big", "name": "Big 2.8T", "role": "large", "params_b": 2800 },
            { "id": "odd", "name": "Odd 1B", "role": "embedding", "params_b": 1 },
            { "id": "small", "name": "Small 120B", "role": "small", "params_b": 120 },
        ]));
        let ids: Vec<_> = g
            .entries()
            .iter()
            .map(|(id, m)| (*id, m.id.as_str()))
            .collect();
        assert_eq!(ids, [(SMALL_ID, "small"), (LARGE_ID, "big")]);
        assert!(g.check().is_ok());
        assert!(!format!("{g:?}").contains("sk-1"));

        let one = grant(
            serde_json::json!([{ "id": "l", "name": "L 70B", "role": "large", "params_b": 70 }]),
        );
        assert_eq!(one.entries()[0].0, SMALL_ID);

        let none =
            grant(serde_json::json!([{ "id": "x", "name": "X", "role": "other", "params_b": 1 }]));
        assert!(matches!(none.check(), Err(Error::Malformed(_))));
        let mut bad = one.clone();
        bad.base_url = "file:///etc".into();
        assert!(matches!(bad.check(), Err(Error::Malformed(_))));
        let mut bad = one;
        bad.api_key = " ".into();
        assert!(matches!(bad.check(), Err(Error::Malformed(_))));
    }

    #[test]
    fn the_specs_register_both_models_with_the_service_and_the_small_one_as_default() {
        let g = grant(serde_json::json!([
            { "id": "big", "name": "Big 2.8T", "role": "large", "params_b": 2800 },
            { "id": "small", "name": "Small 120B", "role": "small", "params_b": 120 },
        ]));
        let specs = g.model_specs();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].0, SMALL_ID);
        assert_eq!(
            specs[0].1,
            serde_json::json!({
                "base_url": "https://api.example/v1", "model": "small", "label": "Small 120B",
                "api_key": "sk-1", "service": "cedar-private-inference",
                "account_url": "https://plg.example/account", "detect_images": true, "is_default": true,
            })
        );
        assert_eq!((specs[1].0, specs[1].1.get("is_default")), (LARGE_ID, None));
    }

    #[tokio::test]
    async fn activation_retries_until_it_works_or_time_is_up() {
        let tries = std::cell::Cell::new(0);
        let ok = retry_for(Duration::from_secs(1), Duration::from_millis(1), || {
            tries.set(tries.get() + 1);
            let n = tries.get();
            async move { if n < 3 { Err("403") } else { Ok(n) } }
        })
        .await;
        assert_eq!(ok, Ok(3));
        let tries = std::cell::Cell::new(0);
        let failed: Result<(), _> = retry_for(Duration::from_millis(30), Duration::from_millis(5), || {
            tries.set(tries.get() + 1);
            async { Err("403") }
        })
        .await;
        assert_eq!(failed, Err("403"));
        assert!(tries.get() >= 2, "tried {} times", tries.get());
    }

    #[test]
    fn the_service_url_comes_from_the_environment() {
        // One test touches the variable, so tests running in parallel don't race on it.
        std::env::remove_var(URL_ENV);
        if PRODUCTION_URL.is_none() {
            assert!(matches!(service_url(), Err(Error::NotConfigured)));
        }
        std::env::set_var(URL_ENV, " http://localhost:3000/ ");
        assert_eq!(service_url().unwrap(), "http://localhost:3000");
        std::env::set_var(URL_ENV, "localhost:3000");
        assert!(matches!(service_url(), Err(Error::BadServiceUrl(_))));
        std::env::remove_var(URL_ENV);
    }
}
