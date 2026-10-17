//! Webhooks: a POST to a run's `callback_url` when the run ends, so a client needn't keep an
//! event stream open or poll.
//!
//! The notification is written by [`Store::finish_run`] in the same transaction as the run's
//! terminal event, so a run never ends without one and none is ever made up (an outbox). This
//! worker delivers the rows that have a callback: it posts the notification's JSON (see
//! [`crate::models::Notification`]), takes any 2xx as delivered, retries a connection failure, a timeout, a
//! 5xx, 408, 425 or 429 with a doubling wait ([`MAX_ATTEMPTS`] tries, from
//! `WEBHOOK_RETRY_SECS`), and gives up on any other 4xx. Undelivered rows survive a restart.
//! A delivery is signed when the run has a `callback_secret` (see [`signature`]).
//!
//! Like `fetch_url`, the calls come from the cedar host, so a `callback_url` may only point at
//! a public address unless `WEBHOOK_ALLOW_PRIVATE` is on; redirects are not followed.

use crate::events::EventHub;
use crate::store::{Delivery, Store};
use crate::tools::fetch::{check_public_url, PublicOnly};
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::header::{CONTENT_TYPE, USER_AGENT};
use reqwest::{redirect, Url};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How often a delivery is tried before the worker gives up on it.
pub const MAX_ATTEMPTS: i64 = 12;
/// The longest wait between two tries.
const MAX_BACKOFF: Duration = Duration::from_secs(3600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Deliveries made at once per pass.
const BATCH: u32 = 20;
/// With nothing due, how long the worker sleeps before looking again (a wake-up comes sooner).
const IDLE: Duration = Duration::from_secs(600);
pub const MAX_URL_LEN: usize = 2048;
pub const MAX_SECRET_LEN: usize = 256;
const AGENT: &str = concat!("cedar/", env!("CARGO_PKG_VERSION"));

/// The worker. Clone the `Arc` for the run manager; [`Webhooks::start`] runs the loop.
pub struct Webhooks {
    store: Store,
    hub: Arc<EventHub>,
    http: reqwest::Client,
    retry: Duration,
    stop: CancellationToken,
    /// One pass at a time, so a call from outside the loop (shutdown) can't double-deliver.
    pass: tokio::sync::Mutex<()>,
}

impl Webhooks {
    /// `allow_private` lets callbacks reach loopback and private addresses; `retry` is the wait
    /// before the first retry (each later one doubles it).
    pub fn new(
        store: Store,
        hub: Arc<EventHub>,
        allow_private: bool,
        retry: Duration,
    ) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(redirect::Policy::none())
            .no_proxy();
        if !allow_private {
            builder = builder.dns_resolver(Arc::new(PublicOnly));
        }
        Ok(Self {
            store,
            hub,
            http: builder.build().context("building the webhook client")?,
            retry,
            stop: CancellationToken::new(),
            pass: tokio::sync::Mutex::new(()),
        })
    }

    /// Deliver what is due, then whenever a run with a callback ends or a retry comes due,
    /// until [`Webhooks::stop`].
    pub fn start(self: &Arc<Self>) -> JoinHandle<()> {
        let worker = self.clone();
        tokio::spawn(async move {
            loop {
                match worker.deliver_due().await {
                    // More may be due (a batch was full, or a retry of 0s): go straight on.
                    Ok(n) if n > 0 => continue,
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %format!("{e:#}"), "webhook pass failed"),
                }
                let wait = match worker.store.next_delivery_at().await {
                    Ok(Some(at)) => (at - Utc::now()).to_std().unwrap_or(Duration::ZERO),
                    Ok(None) => IDLE,
                    Err(_) => Duration::from_secs(5),
                };
                tokio::select! {
                    _ = worker.stop.cancelled() => return,
                    _ = worker.hub.delivery_due() => {}
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        })
    }

    /// End the loop. A delivery in flight finishes first.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// One pass: every delivery whose time has come, at once. Returns how many were tried.
    pub async fn deliver_due(&self) -> Result<usize> {
        let _one_at_a_time = self.pass.lock().await;
        let due = self.store.due_deliveries(BATCH).await?;
        let calls = due.iter().map(|d| self.deliver(d));
        for (delivery, outcome) in due.iter().zip(futures_util::future::join_all(calls).await) {
            let id = delivery.notification.id;
            let run_id = delivery.notification.run_id;
            let record = match outcome {
                Ok(()) => {
                    tracing::info!(%run_id, url = %delivery.url, "webhook delivered");
                    Ok(())
                }
                Err((error, retryable)) => {
                    let attempt = delivery.attempts + 1;
                    let retry_at = (retryable && attempt < MAX_ATTEMPTS).then(|| {
                        let wait = self
                            .retry
                            .saturating_mul(1u32 << delivery.attempts.clamp(0, 20))
                            .min(MAX_BACKOFF);
                        Utc::now() + chrono::Duration::from_std(wait).unwrap_or_default()
                    });
                    if retry_at.is_some() {
                        tracing::warn!(%run_id, url = %delivery.url, attempt, error = %error, "webhook failed; will retry");
                    } else {
                        tracing::warn!(%run_id, url = %delivery.url, attempt, error = %error, "webhook failed; giving up");
                    }
                    Err((error, retry_at))
                }
            };
            self.store.record_delivery(id, record).await?;
        }
        Ok(due.len())
    }

    /// POST one notification. The error says what went wrong and whether to try again.
    async fn deliver(&self, d: &Delivery) -> Result<(), (String, bool)> {
        let body = serde_json::to_vec(&d.notification).map_err(|e| (e.to_string(), false))?;
        let mut req = self
            .http
            .post(&d.url)
            .timeout(REQUEST_TIMEOUT)
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, AGENT)
            .header("x-cedar-event", d.notification.kind)
            .header("x-cedar-delivery", d.notification.id.to_string())
            .header("x-cedar-run", d.notification.run_id.to_string());
        if let Some(secret) = &d.secret {
            req = req.header(
                "x-cedar-signature",
                signature(secret, Utc::now().timestamp(), &body),
            );
        }
        match req.body(body).send().await {
            Ok(resp) if resp.status().is_success() => Ok(()),
            Ok(resp) => {
                let status = resp.status();
                let again = status.is_server_error() || matches!(status.as_u16(), 408 | 425 | 429);
                Err((format!("the endpoint answered {status}"), again))
            }
            Err(e) => Err((e.without_url().to_string(), true)),
        }
    }
}

/// Check a `callback_url` as `POST /v1/runs` takes it: http(s), with a host that is public
/// unless `allow_private`. Returns it trimmed. The error is a message for the client.
pub async fn check_callback_url(raw: &str, allow_private: bool) -> Result<String, String> {
    let raw = raw.trim();
    if raw.len() > MAX_URL_LEN {
        return Err(format!("callback_url is longer than {MAX_URL_LEN} bytes"));
    }
    let url = Url::parse(raw).map_err(|e| format!("callback_url is not a valid URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("callback_url must be an http or https URL".into());
    }
    if url.host_str().is_none() {
        return Err("callback_url has no host".into());
    }
    check_public_url(raw, allow_private)
        .await
        .map_err(|e| format!("callback_url: {e}"))?;
    Ok(raw.to_string())
}

/// The `X-Cedar-Signature` header of a delivery: `t=<unix seconds>,v1=<hex>`, where the hex is
/// HMAC-SHA256 over `"<t>.<body>"` with the run's `callback_secret`. A receiver recomputes it
/// over the raw body and rejects a stale `t` to stop replays.
pub fn signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let mut message = timestamp.to_string().into_bytes();
    message.push(b'.');
    message.extend_from_slice(body);
    format!(
        "t={timestamp},v1={}",
        hex::encode(hmac_sha256(secret.as_bytes(), &message))
    )
}

/// HMAC-SHA256 (RFC 2104) over SHA-256's 64-byte block.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner: Vec<u8> = block.iter().map(|b| b ^ 0x36).collect();
    let outer: Vec<u8> = block.iter().map(|b| b ^ 0x5c).collect();
    let inner_hash = Sha256::new().chain_update(inner).chain_update(message).finalize();
    Sha256::new()
        .chain_update(outer)
        .chain_update(inner_hash)
        .finalize()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_the_rfc_4231_vectors() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex::encode(mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // A key longer than the block is hashed first (RFC 4231 case 6).
        let mac = hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(
            hex::encode(mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn signatures_carry_their_timestamp() {
        let sig = signature("s3cret", 1_700_000_000, br#"{"id":1}"#);
        assert!(sig.starts_with("t=1700000000,v1="), "{sig}");
        let expected = hex::encode(hmac_sha256(b"s3cret", br#"1700000000.{"id":1}"#));
        assert_eq!(sig, format!("t=1700000000,v1={expected}"));
        assert_ne!(signature("other", 1_700_000_000, br#"{"id":1}"#), sig);
    }

    #[tokio::test]
    async fn callback_urls_are_checked() {
        async fn err(raw: &str, private: bool) -> String {
            check_callback_url(raw, private).await.unwrap_err()
        }
        assert!(err("ftp://hooks.example/x", true).await.contains("http or https"));
        assert!(err("not a url", true).await.contains("not a valid URL"));
        assert!(err("http://", true).await.contains("valid URL"));
        assert!(err("http://127.0.0.1:3000/hook", false).await.contains("not a public address"));
        assert!(err("http://10.1.2.3/hook", false).await.contains("not a public address"));
        assert!(err(&format!("https://x.example/{}", "a".repeat(MAX_URL_LEN)), true).await.contains("longer than"));
        assert_eq!(
            check_callback_url(" http://127.0.0.1:3000/hook ", true).await.unwrap(),
            "http://127.0.0.1:3000/hook"
        );
        assert_eq!(
            check_callback_url("https://203.0.113.9/hook", true).await.unwrap(),
            "https://203.0.113.9/hook"
        );
    }
}
