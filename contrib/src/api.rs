//! The routes, and the request path's checks (`docs/design/contrib-ingest-cloud-run.md`, §3–4).

use crate::config::Config;
use crate::ratelimit::RateLimit;
use crate::store::{Ledger, Receipt, Store};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use cedar_contrib_schema::scan;
use cedar_contrib_schema::validate::{self, Code, Limits};
use cedar_contrib_schema::{Bundle, License, SCHEMA_VERSION};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::net::SocketAddr;
use std::sync::Arc;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub store: Arc<dyn Store>,
    pub limiter: Arc<RateLimit>,
}

impl AppState {
    pub fn new(cfg: Config, store: Arc<dyn Store>) -> Self {
        let limiter = Arc::new(RateLimit::new(cfg.ip_per_minute));
        AppState { cfg: Arc::new(cfg), store, limiter }
    }

    fn limits(&self) -> Limits {
        Limits { max_bundle_bytes: self.cfg.max_bundle_bytes, ..Limits::default() }
    }

    /// The token that withdraws a receipt: never stored, recomputed from the receipt and the
    /// bundle's hash with the server's key.
    pub fn withdraw_token(&self, receipt_id: &str, sha256: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.cfg.hmac_key).expect("any key length works");
        mac.update(receipt_id.as_bytes());
        mac.update(b"|");
        mac.update(sha256.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    }
}

pub fn router(state: AppState) -> Router {
    let max = state.cfg.max_bundle_bytes + 1024;
    Router::new()
        .route("/v1/contributions", axum::routing::post(submit).layer(DefaultBodyLimit::max(max)))
        .route("/v1/contributions/{id}", get(status).delete(withdraw))
        .route("/v1/policy", get(policy))
        .route("/healthz", get(|| async { "ok" }))
        .fallback(|| async { ApiError::NotFound.into_response() })
        .with_state(state)
}

// -- errors ----------------------------------------------------------------------------------

/// `{"error": {"code", "message"}}`, like cedar's. The codes a client can act on are coarse on
/// purpose (§8 of the design): `schema`, `too_large`, `not_consistent`, `secret_detected`,
/// `model_not_allowed`, `duplicate`, `quota`, `rate_limited`.
#[derive(Debug)]
pub enum ApiError {
    Refused { status: StatusCode, code: &'static str, message: String },
    NotFound,
    Unauthorized,
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::Refused { status, code, message } => (status, code, message),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", "no such route".to_string()),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", "missing or wrong token".to_string()),
            ApiError::Internal(e) => {
                tracing::error!(error = %e, "request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", "something went wrong on our side".to_string())
            }
        };
        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}

fn refused(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::Refused { status, code, message: message.into() }
}

// -- the routes ------------------------------------------------------------------------------

/// The client a request came from, for the in-memory rate limit only: the first address in
/// `X-Forwarded-For` (what the load balancer or Cloud Run's front end sets), else the peer. It
/// is hashed with a per-process salt before it is used as a key, and never logged or stored.
pub struct Client(String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let forwarded = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip().to_string());
        Ok(Client(forwarded.or(peer).unwrap_or_else(|| "unknown".into())))
    }
}

fn rate_limited(state: &AppState, client: &Client) -> Result<(), ApiError> {
    if state.limiter.allow(&client.0) {
        Ok(())
    } else {
        Err(refused(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "too many requests; try again in a minute"))
    }
}

/// What this ingest takes, for clients to refuse locally what it would refuse.
async fn policy(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "schema_versions": [SCHEMA_VERSION],
        "models": state.cfg.models,
        "licenses": License::ALL.iter().map(|l| l.id()).collect::<Vec<_>>(),
        "max_bundle_bytes": state.cfg.max_bundle_bytes,
        "canary": cedar_contrib_schema::CANARY,
        "quotas": { "per_day": state.cfg.quota_day, "per_month": state.cfg.quota_month, "first_day": state.cfg.quota_new, "unlinked_per_cluster_day": state.cfg.quota_cluster },
    }))
}

async fn submit(
    State(state): State<AppState>,
    client: Client,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    rate_limited(&state, &client)?;
    // 1. Shape.
    if body.len() > state.cfg.max_bundle_bytes {
        return Err(refused(StatusCode::PAYLOAD_TOO_LARGE, "too_large", format!("at most {} bytes", state.cfg.max_bundle_bytes)));
    }
    let bundle: Bundle = serde_json::from_slice(&body).map_err(|e| refused(StatusCode::BAD_REQUEST, "schema", format!("not a bundle: {e}")))?;
    let problems = validate::validate(&bundle, &state.limits());
    if let Some(p) = problems.first() {
        let (status, code) = match p.code {
            Code::Schema => (StatusCode::BAD_REQUEST, "schema"),
            Code::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
            Code::NotConsistent => (StatusCode::BAD_REQUEST, "not_consistent"),
        };
        return Err(refused(status, code, p.detail.clone()));
    }
    // 2. The model.
    if !state.cfg.model_allowed(&bundle.model.name) {
        return Err(refused(StatusCode::BAD_REQUEST, "model_not_allowed", format!("this corpus does not publish outputs of {}", bundle.model.name)));
    }
    // 4. Secrets: the client should have caught them; a bundle with one is not stored, at all.
    if let Some(s) = scan::secrets_in(&bundle).first() {
        return Err(refused(StatusCode::BAD_REQUEST, "secret_detected", s.clone()));
    }
    // 6. Quotas, before anything is written.
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let fingerprint = bundle.fingerprint();
    // Coarse on purpose: 16 bits of the SimHash put threads about the same thing in one
    // bucket most of the time, which is what the unlinked quota needs; the release job's
    // cluster analysis is the precise one.
    let cluster = fingerprint[..4].to_string();
    let mut ledger: Option<Ledger> = None;
    match &bundle.contributor {
        Some(p) => {
            let mut l = state.store.ledger(p).await?.unwrap_or(Ledger { created_day: today.clone(), days: Default::default() });
            let today_n = l.days.get(&today).copied().unwrap_or(0);
            if today_n >= state.cfg.quota_day {
                return Err(refused(StatusCode::TOO_MANY_REQUESTS, "quota", format!("at most {} contributions a day", state.cfg.quota_day)));
            }
            if l.created_day == today && today_n >= state.cfg.quota_new {
                return Err(refused(StatusCode::TOO_MANY_REQUESTS, "quota", format!("a new pseudonym may send {} on its first day; more tomorrow", state.cfg.quota_new)));
            }
            let since = (Utc::now() - Duration::days(30)).format("%Y-%m-%d").to_string();
            let month: u32 = l.days.iter().filter(|(d, _)| **d >= since).map(|(_, n)| n).sum();
            if month >= state.cfg.quota_month {
                return Err(refused(StatusCode::TOO_MANY_REQUESTS, "quota", format!("at most {} contributions in 30 days", state.cfg.quota_month)));
            }
            *l.days.entry(today.clone()).or_default() += 1;
            l.days.retain(|d, _| *d >= since);
            ledger = Some(l);
        }
        None => {
            if state.store.count_cluster(&cluster, &today).await? >= state.cfg.quota_cluster {
                return Err(refused(StatusCode::TOO_MANY_REQUESTS, "quota", format!("at most {} unlinked contributions a day that look alike; choose a pseudonym in Settings to send more", state.cfg.quota_cluster)));
            }
        }
    }
    // 5. Exact duplicate: claiming the hash is the write that decides.
    let sha256 = bundle.content_sha256();
    if !state.store.claim_hash(&sha256).await? {
        return Err(refused(StatusCode::CONFLICT, "duplicate", "this conversation was already contributed"));
    }
    // 7. Store.
    let receipt_id = uuid::Uuid::new_v4().to_string();
    let receipt = Receipt {
        receipt_id: receipt_id.clone(),
        sha256: sha256.clone(),
        object: format!("bundles/{}/{receipt_id}.json", &today[..7]),
        pseudonym: bundle.contributor.clone(),
        day: today.clone(),
        status: "quarantined".into(),
        released_in: vec![],
        fingerprint,
    };
    // Stored as received: the checks passed on these exact bytes.
    state.store.put_bundle(&receipt, &body).await?;
    state.store.put_receipt(&receipt).await?;
    match (&bundle.contributor, ledger) {
        (Some(p), Some(l)) => state.store.put_ledger(p, &l).await?,
        _ => state.store.note_cluster(&cluster, &today, &receipt_id).await?,
    }
    tracing::info!(receipt = %receipt_id, "quarantined");
    Ok((
        StatusCode::CREATED,
        Json(json!({ "receipt_id": receipt_id, "withdraw_token": state.withdraw_token(&receipt_id, &sha256), "status": "quarantined" })),
    ))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get("authorization")?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim)
}

/// The receipt, if the token matches it; `None` for an unknown id or a wrong token, which look
/// the same from outside.
async fn authorized(state: &AppState, id: &str, headers: &HeaderMap) -> Result<Option<Receipt>, ApiError> {
    let Some(token) = bearer(headers) else { return Ok(None) };
    if id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Ok(None);
    }
    let Some(receipt) = state.store.get_receipt(id).await? else { return Ok(None) };
    let expected = state.withdraw_token(&receipt.receipt_id, &receipt.sha256);
    if expected.as_bytes().ct_eq(token.as_bytes()).into() {
        Ok(Some(receipt))
    } else {
        Ok(None)
    }
}

async fn status(
    State(state): State<AppState>,
    client: Client,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    rate_limited(&state, &client)?;
    let Some(r) = authorized(&state, &id, &headers).await? else { return Err(ApiError::Unauthorized) };
    Ok(Json(json!({ "status": r.status, "submitted_day": r.day, "released_in": r.released_in })))
}

/// Withdraw: the bundle is deleted, the receipt marked, its hash freed and noted for the
/// exclusion list. Always `204`: whether the receipt existed is not revealed.
async fn withdraw(
    State(state): State<AppState>,
    client: Client,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    rate_limited(&state, &client)?;
    if let Some(mut r) = authorized(&state, &id, &headers).await? {
        if r.status != "withdrawn" {
            state.store.delete_bundle(&r).await?;
            state.store.release_hash(&r.sha256).await?;
            state.store.note_withdrawn(&r.sha256).await?;
            r.status = "withdrawn".into();
            state.store.put_receipt(&r).await?;
            tracing::info!(receipt = %r.receipt_id, "withdrawn");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}
