//! `POST /api/hooks/{hook_id}`: starting an agent's run from outside.
//!
//! The one route that takes neither the access token nor anything from the page: whoever calls
//! it (a home automation, a CI job, a phone shortcut) has the hook's address and its secret, and
//! nothing else of this server. The address is a random id, not the agent's. The secret comes as
//! `Authorization: Bearer <secret>`, or as a signature of the body made with it: cedar's own
//! `X-Cedar-Signature: t=<unix seconds>,v1=<hex>` (HMAC-SHA256 over `<t>.<body>`, at most
//! [`TOLERANCE_SECS`] old) or GitHub's `X-Hub-Signature-256: sha256=<hex>` (over the body).
//! Anything that doesn't check out is answered like a hook that doesn't exist.
//!
//! The body is handed to the run as data (see `prompt::fence`), never as the prompt. A call that
//! comes while the agent may not start a run (one is going, or its limits say wait) is kept, the
//! latest in place of any earlier one, and handed over by the scheduler when it may.

use super::*;
use crate::schedule::{start_event, Started};
use crate::store::{never, Polled};
use crate::trigger::Event;
use axum::body::Bytes;
use chrono::DateTime;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use std::time::Instant;

/// The largest body a hook takes.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// How old a signed call's timestamp may be, either way.
pub const TOLERANCE_SECS: i64 = 300;
/// Calls taken per minute for one hook, and for all hooks together.
const PER_HOOK_A_MINUTE: u32 = 60;
const ALL_A_MINUTE: u32 = 600;
const WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// Counts of recent calls, in memory: enough to keep a runaway sender from filling the log and
/// the database, not a defence against a flood.
#[derive(Default)]
pub struct Limits(std::sync::Mutex<HashMap<String, (Instant, u32)>>);

impl Limits {
    /// Count one call under `key`; false once there were `max` in the current minute.
    fn take(&self, key: &str, max: u32) -> bool {
        let mut counts = self.0.lock().unwrap();
        let now = Instant::now();
        if counts.len() > 1000 {
            counts.retain(|_, (since, _)| now.duration_since(*since) < WINDOW);
        }
        let entry = counts.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= max
    }
}

fn too_many() -> AppError {
    AppError::Upstream { status: StatusCode::TOO_MANY_REQUESTS, code: "too_many_requests".into(), message: "too many calls; try again in a minute".into() }
}

fn hmac_sha256(secret: &str, parts: &[&[u8]]) -> Option<Vec<u8>> {
    let key = PKey::hmac(secret.as_bytes()).ok()?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key).ok()?;
    for part in parts {
        signer.update(part).ok()?;
    }
    signer.sign_to_vec().ok()
}

fn same(a: &[u8], b: &[u8]) -> bool {
    // Digests of both, so the comparison takes the same time whatever the lengths.
    bool::from(Sha256::digest(a).ct_eq(&Sha256::digest(b)))
}

/// Whether the call carries the hook's secret, or a signature made with it.
pub fn authorized(headers: &HeaderMap, body: &[u8], secret: &str, now: DateTime<Utc>) -> bool {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim);
    if let Some(token) = header("authorization").and_then(|v| v.strip_prefix("Bearer ")) {
        return same(token.trim().as_bytes(), secret.as_bytes());
    }
    if let Some(sig) = header("x-cedar-signature") {
        let field = |name: &str| sig.split(',').find_map(|part| part.trim().strip_prefix(name));
        let (Some(t), Some(v1)) = (field("t="), field("v1=")) else { return false };
        let Ok(at) = t.parse::<i64>() else { return false };
        if (now.timestamp() - at).abs() > TOLERANCE_SECS {
            return false;
        }
        let (Ok(given), Some(expected)) = (hex::decode(v1), hmac_sha256(secret, &[t.as_bytes(), b".", body])) else { return false };
        return same(&given, &expected);
    }
    if let Some(sig) = header("x-hub-signature-256").and_then(|v| v.strip_prefix("sha256=")) {
        let (Ok(given), Some(expected)) = (hex::decode(sig), hmac_sha256(secret, &[body])) else { return false };
        return same(&given, &expected);
    }
    false
}

/// What the run is given: the body as text, under the headers that say what it is.
fn payload(headers: &HeaderMap, body: &[u8]) -> String {
    let mut out = String::new();
    for (name, value) in headers {
        let name = name.as_str();
        let tells = name == "content-type" || (name.starts_with("x-") && name.contains("event") && !name.contains("signature"));
        if let (true, Ok(value)) = (tells, value.to_str()) {
            out.push_str(&format!("{name}: {}\n", value.chars().take(200).collect::<String>()));
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(String::from_utf8_lossy(body).trim());
    out
}

pub(super) async fn receive(State(state): State<AppState>, Path(hook_id): Path<String>, headers: HeaderMap, body: Bytes) -> Result<(StatusCode, Json<Value>), AppError> {
    const GONE: AppError = AppError::NotFound("no such route");
    if !state.hook_limits.take("", ALL_A_MINUTE) {
        return Err(too_many());
    }
    if hook_id.len() != 32 || !hook_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GONE);
    }
    let Some(schedule) = state.store.get_schedule_by_hook(&hook_id).await? else { return Err(GONE) };
    if !state.hook_limits.take(&hook_id, PER_HOOK_A_MINUTE) {
        return Err(too_many());
    }
    let now = Utc::now();
    let secret = schedule.hook_secret.as_deref().unwrap_or_default();
    // A paused agent, and a wrong or missing secret, look like no hook at all.
    if !schedule.enabled || schedule.trigger_kind != "webhook" || secret.is_empty() || !authorized(&headers, &body, secret, now) {
        return Err(GONE);
    }
    let data = payload(&headers, &body);
    let event = Event::Webhook { bytes: body.len(), dropped: 0 };
    let answer = match start_event(&state, schedule.id, event, Some(data.clone()), now).await? {
        Started::Run(run) => json!({ "started": true, "queued": false, "conversation_id": run.conversation_id }),
        Started::Held(_) => {
            agents::hold(&state, &schedule, &data, body.len(), now).await?;
            json!({ "started": false, "queued": true })
        }
        Started::Failed(e) => json!({ "started": false, "queued": false, "error": e.to_string() }),
    };
    Ok((StatusCode::ACCEPTED, Json(answer)))
}

/// Hand over the delivery a webhook agent is holding, if it may run now. Called by the
/// scheduler, for which such an agent is "due" as long as it holds one.
pub(crate) async fn deliver_held(state: &AppState, schedule: &Schedule, now: DateTime<Utc>) -> Result<bool, AppError> {
    let idle = Polled { rev: schedule.trigger_rev, state: None, next_run_at: never(), checked_at: now, error: None, failures: 0 };
    let Some(pending) = schedule.state.as_ref().map(|s| &s["pending"]).filter(|p| p.is_object()) else {
        state.store.mark_polled(schedule.id, &idle).await?;
        return Ok(false);
    };
    let event = Event::Webhook { bytes: pending["bytes"].as_u64().unwrap_or(0) as usize, dropped: pending["dropped"].as_u64().unwrap_or(0) as u32 };
    let data = pending["data"].as_str().map(String::from);
    match start_event(state, schedule.id, event, data, now).await? {
        Started::Run(_) => {
            state.store.mark_polled(schedule.id, &idle).await?;
            Ok(true)
        }
        Started::Failed(_) => {
            state.store.mark_polled(schedule.id, &idle).await?;
            Ok(false)
        }
        Started::Held(_) => {
            let wait = Polled { state: schedule.state.as_ref(), next_run_at: now + agents::retry_after(), ..idle };
            state.store.mark_polled(schedule.id, &wait).await?;
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, String)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn hmac_matches_the_rfc_4231_test_vector() {
        // RFC 4231, test case 2.
        let mac = hmac_sha256("Jefe", &[b"what do ya want ", b"for nothing?"]).unwrap();
        assert_eq!(hex::encode(mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn a_call_is_let_in_by_the_secret_or_a_signature_made_with_it() {
        let now = Utc::now();
        let (secret, body) = ("s3cret", br#"{"a":1}"#);
        assert!(authorized(&headers(&[("authorization", "Bearer s3cret".into())]), body, secret, now));
        assert!(!authorized(&headers(&[("authorization", "Bearer s3cre".into())]), body, secret, now));
        assert!(!authorized(&headers(&[("authorization", "Basic s3cret".into())]), body, secret, now));
        assert!(!authorized(&HeaderMap::new(), body, secret, now));

        let t = now.timestamp().to_string();
        let sig = |t: &str, key: &str, body: &[u8]| hex::encode(hmac_sha256(key, &[t.as_bytes(), b".", body]).unwrap());
        let cedar = |t: &str, v1: String| headers(&[("x-cedar-signature", format!("t={t},v1={v1}"))]);
        assert!(authorized(&cedar(&t, sig(&t, secret, body)), body, secret, now));
        assert!(!authorized(&cedar(&t, sig(&t, "other", body)), body, secret, now), "another key");
        assert!(!authorized(&cedar(&t, sig(&t, secret, body)), b"{\"a\":2}", secret, now), "another body");
        let old = (now.timestamp() - TOLERANCE_SECS - 1).to_string();
        assert!(!authorized(&cedar(&old, sig(&old, secret, body)), body, secret, now), "a replay from earlier");
        assert!(!authorized(&headers(&[("x-cedar-signature", "v1=00".into())]), body, secret, now));

        let hub = hex::encode(hmac_sha256(secret, &[body]).unwrap());
        assert!(authorized(&headers(&[("x-hub-signature-256", format!("sha256={hub}"))]), body, secret, now));
        assert!(!authorized(&headers(&[("x-hub-signature-256", format!("sha256={hub}"))]), b"x", secret, now));
        assert!(!authorized(&headers(&[("x-hub-signature-256", "sha256=zz".into())]), body, secret, now));
    }

    #[test]
    fn the_payload_keeps_what_says_what_it_is() {
        let h = headers(&[("content-type", "application/json".into()), ("x-github-event", "push".into()), ("x-hub-signature-256", "sha256=ab".into()), ("authorization", "Bearer k".into())]);
        let text = payload(&h, b" {\"ref\":\"main\"}\n");
        assert!(text.contains("content-type: application/json\n") && text.contains("x-github-event: push\n"), "{text}");
        assert!(text.ends_with("\n\n{\"ref\":\"main\"}"), "{text}");
        assert!(!text.contains("Bearer") && !text.contains("sha256="), "{text}");
    }

    #[test]
    fn calls_are_counted_per_minute() {
        let limits = Limits::default();
        assert!((0..3).all(|_| limits.take("a", 3)));
        assert!(!limits.take("a", 3));
        assert!(limits.take("b", 3), "another hook has its own count");
    }
}
