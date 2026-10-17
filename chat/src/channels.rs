//! Telling the user out of the app when a response finishes: a phone push through Pushover, a
//! browser push through Web Push (a service worker shows it, tab open or not), or an email over
//! SMTP. The user picks one method and gives its credentials in Settings → Notifications; the
//! settings live in this service's database (see [`NotificationSettings`]) and the secrets are
//! never sent back to the page.
//!
//! What gets sent is decided by [`dispatch`], which `notify` calls a few seconds after cedar
//! says a run ended: nothing if the response was looked at meanwhile, or if the method is "none",
//! or if the scope leaves it out. Every delivery, test or not, is recorded as `last_delivery` so
//! Settings can show what happened last.

use crate::api::AppState;
use crate::error::AppError;
use crate::store::{PushSubscription, Store};
use chrono::{DateTime, Utc};
use jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256KeyPair};
use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as Email, Tokio1Executor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;
use web_push::{ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder};

pub const PUSHOVER_URL: &str = "https://api.pushover.net/1/messages.json";
/// How long after a run ends the out-of-app message goes, so a page that watched the response
/// end has time to mark it seen first.
pub const DISPATCH_DELAY: Duration = Duration::from_secs(3);
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a push service keeps a message for a browser that is offline.
const PUSH_TTL: u32 = 24 * 3600;

/// Which channel carries the message. `None` keeps notifications in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    None,
    Browser,
    Pushover,
    Email,
}

impl Method {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Method::None,
            "browser" => Method::Browser,
            "pushover" => Method::Pushover,
            "email" => Method::Email,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::None => "none",
            Method::Browser => "browser",
            Method::Pushover => "pushover",
            Method::Email => "email",
        }
    }
}

/// Which finished responses go out: the ones that need the user (a failure, a question, a
/// scheduled run), or every one that wasn't being watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Attention,
    All,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "attention" => Scope::Attention,
            "all" => Scope::All,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PushoverSettings {
    /// The application's API token and the user's key, from pushover.net.
    pub token: String,
    pub user: String,
}

impl PushoverSettings {
    pub fn configured(&self) -> bool {
        !self.token.is_empty() && !self.user.is_empty()
    }
}

/// `tls` connects over TLS (port 465), `starttls` upgrades a plain connection (port 587),
/// `none` stays plain (a relay on this machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    #[default]
    Tls,
    Starttls,
    None,
}

impl Security {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "tls" => Security::Tls,
            "starttls" => Security::Starttls,
            "none" => Security::None,
            _ => return None,
        })
    }

    pub fn default_port(self) -> u16 {
        match self {
            Security::Tls => 465,
            Security::Starttls => 587,
            Security::None => 25,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EmailSettings {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: String,
    pub password: String,
    /// The sender and the recipient, as addresses (`Cedar <cedar@example.com>` works too).
    pub from: String,
    pub to: String,
}

impl EmailSettings {
    pub fn configured(&self) -> bool {
        !self.host.is_empty() && !self.from.is_empty() && !self.to.is_empty()
    }
}

/// The VAPID key pair that identifies this server to the push services, made on first use,
/// and the contact the services may write to about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Vapid {
    /// The raw P-256 private key, base64url.
    pub private_key: String,
    /// The uncompressed public key, base64url: what the browser subscribes with.
    pub public_key: String,
    pub contact: String,
}

/// What happened the last time a message went out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastDelivery {
    pub at: DateTime<Utc>,
    pub method: Method,
    pub ok: bool,
    pub detail: String,
    pub test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct NotificationSettings {
    pub method: Method,
    pub scope: Scope,
    pub pushover: PushoverSettings,
    pub email: EmailSettings,
    pub vapid: Option<Vapid>,
    pub last_delivery: Option<LastDelivery>,
}

impl NotificationSettings {
    /// What the page sees: everything but the secrets, which are only reported as set.
    pub fn status(&self, subscriptions: &[PushSubscription]) -> Value {
        json!({
            "method": self.method,
            "scope": self.scope,
            "pushover": {
                "configured": self.pushover.configured(),
                "token_set": !self.pushover.token.is_empty(),
                "user_set": !self.pushover.user.is_empty(),
                "user_last4": last4(&self.pushover.user),
            },
            "email": {
                "configured": self.email.configured(),
                "host": self.email.host,
                "port": if self.email.port == 0 { Value::Null } else { json!(self.email.port) },
                "security": self.email.security,
                "username": self.email.username,
                "password_set": !self.email.password.is_empty(),
                "from": self.email.from,
                "to": self.email.to,
            },
            "browser": {
                "public_key": self.vapid.as_ref().map(|v| v.public_key.clone()),
                "contact": self.vapid.as_ref().map(|v| v.contact.clone()).unwrap_or_default(),
                "subscriptions": subscriptions,
            },
            "last_delivery": self.last_delivery,
        })
    }

    /// Whether `method` has what it needs to send; the message says what is missing.
    pub fn check(&self, method: Method, subscriptions: usize) -> Result<(), String> {
        match method {
            Method::None => Ok(()),
            Method::Pushover if !self.pushover.configured() => {
                Err("Pushover needs the application token and the user key".into())
            }
            Method::Email if !self.email.configured() => {
                Err("email needs the SMTP server, a sender and a recipient".into())
            }
            Method::Browser if subscriptions == 0 => {
                Err("no browser has turned on push notifications yet".into())
            }
            _ => Ok(()),
        }
    }
}

fn last4(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.chars().rev().take(4).collect::<String>().chars().rev().collect())
}

/// What is sent, however it goes: a title, a line or two, and where to look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub body: String,
    /// The conversation's address in the chat, absolute (from `CHAT_PUBLIC_URL`).
    pub url: String,
    /// Collapses repeats of the same thing (the turn's id).
    pub tag: String,
}

/// A base64url (no padding) string, as push services and the browser use.
fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The server's VAPID keys, made and saved on first use.
pub async fn ensure_vapid(store: &Store) -> Result<NotificationSettings, AppError> {
    let mut settings = store.notification_settings().await?;
    if settings.vapid.is_none() {
        let pair = ES256KeyPair::generate();
        settings.vapid = Some(Vapid {
            private_key: b64url(&pair.to_bytes()),
            public_key: b64url(&pair.public_key().public_key().to_bytes_uncompressed()),
            contact: String::new(),
        });
        store.set_notification_settings(&settings).await?;
    }
    Ok(settings)
}

/// Send `message` by `method`, with the credentials in `settings`. The error is for the user.
pub async fn send(state: &AppState, settings: &NotificationSettings, method: Method, message: &Message) -> Result<String, String> {
    match method {
        Method::None => Err("no notification method is chosen".into()),
        Method::Pushover => send_pushover(state, &settings.pushover, message).await,
        Method::Email => send_email(&settings.email, message).await,
        Method::Browser => send_web_push(state, settings, message).await,
    }
}

async fn send_pushover(state: &AppState, pushover: &PushoverSettings, message: &Message) -> Result<String, String> {
    if !pushover.configured() {
        return Err("Pushover needs the application token and the user key".into());
    }
    let form = [
        ("token", pushover.token.as_str()),
        ("user", pushover.user.as_str()),
        ("title", message.title.as_str()),
        ("message", message.body.as_str()),
        ("url", message.url.as_str()),
        ("url_title", "Open in Cedar"),
    ];
    let resp = state
        .http
        .post(&state.cfg.pushover_url)
        .timeout(SEND_TIMEOUT)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("could not reach Pushover: {}", e.without_url()))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if status.is_success() && body["status"] == 1 {
        return Ok("Pushover accepted the message".into());
    }
    let errors = body["errors"]
        .as_array()
        .map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; "))
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| format!("HTTP {status}"));
    Err(format!("Pushover refused the message: {errors}"))
}

async fn send_email(email: &EmailSettings, message: &Message) -> Result<String, String> {
    if !email.configured() {
        return Err("email needs the SMTP server, a sender and a recipient".into());
    }
    let from: Mailbox = email.from.parse().map_err(|e| format!("the sender address is invalid: {e}"))?;
    let to: Mailbox = email.to.parse().map_err(|e| format!("the recipient address is invalid: {e}"))?;
    let mail = Email::builder()
        .from(from)
        .to(to)
        .subject(&message.title)
        .header(ContentType::TEXT_PLAIN)
        .body(format!("{}\n\n{}\n", message.body, message.url))
        .map_err(|e| format!("could not build the email: {e}"))?;
    let port = if email.port == 0 { email.security.default_port() } else { email.port };
    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&email.host).port(port);
    builder = match email.security {
        Security::None => builder.tls(Tls::None),
        Security::Tls | Security::Starttls => {
            let params = TlsParameters::new(email.host.clone()).map_err(|e| format!("TLS setup failed: {e}"))?;
            builder.tls(match email.security {
                Security::Tls => Tls::Wrapper(params),
                _ => Tls::Required(params),
            })
        }
    };
    if !email.username.is_empty() {
        builder = builder.credentials(Credentials::new(email.username.clone(), email.password.clone()));
    }
    let transport = builder.timeout(Some(SEND_TIMEOUT)).build();
    match transport.send(mail).await {
        Ok(response) => Ok(format!("the mail server accepted it: {}", response.message().collect::<Vec<_>>().join(" ").trim())),
        Err(e) => Err(format!("the mail server refused it: {e}")),
    }
}

/// One push per subscribed browser; a subscription the push service no longer knows is dropped.
/// Succeeds if any browser took it.
async fn send_web_push(state: &AppState, settings: &NotificationSettings, message: &Message) -> Result<String, String> {
    let subscriptions = state.store.push_subscriptions().await.map_err(|e| e.to_string())?;
    if subscriptions.is_empty() {
        return Err("no browser has turned on push notifications yet".into());
    }
    let Some(vapid) = &settings.vapid else {
        return Err("the server has no push keys yet; turn push on in a browser first".into());
    };
    let payload = json!({ "title": message.title, "body": message.body, "url": message.url, "tag": message.tag }).to_string();
    let mut delivered = 0;
    let mut dropped = 0;
    let mut errors = Vec::new();
    for sub in &subscriptions {
        match push_one(state, vapid, sub, payload.as_bytes()).await {
            Ok(()) => {
                delivered += 1;
                let _ = state.store.touch_push_subscription(sub.id).await;
            }
            Err((gone, error)) => {
                if gone {
                    dropped += 1;
                    let _ = state.store.delete_push_subscription(sub.id).await;
                } else {
                    errors.push(error);
                }
            }
        }
    }
    if delivered > 0 {
        let mut detail = format!("sent to {delivered} browser{}", if delivered == 1 { "" } else { "s" });
        if dropped > 0 {
            detail.push_str(&format!(", {dropped} no longer subscribed"));
        }
        if !errors.is_empty() {
            detail.push_str(&format!("; failed: {}", errors.join("; ")));
        }
        Ok(detail)
    } else if !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        Err("every subscribed browser has gone; turn push on again".into())
    }
}

/// `Err((gone, message))`: `gone` when the push service says the subscription is dead.
async fn push_one(state: &AppState, vapid: &Vapid, sub: &PushSubscription, payload: &[u8]) -> Result<(), (bool, String)> {
    let info = SubscriptionInfo::new(sub.endpoint.clone(), sub.p256dh.clone(), sub.auth.clone());
    let mut signature = VapidSignatureBuilder::from_base64(&vapid.private_key, web_push::URL_SAFE_NO_PAD, &info)
        .map_err(|e| (false, format!("the push keys are unusable: {e}")))?;
    if !vapid.contact.is_empty() {
        let contact = if vapid.contact.contains(':') { vapid.contact.clone() } else { format!("mailto:{}", vapid.contact) };
        signature.add_claim("sub", contact);
    }
    let signature = signature.build().map_err(|e| (false, format!("could not sign the push: {e}")))?;
    let mut builder = WebPushMessageBuilder::new(&info);
    builder.set_ttl(PUSH_TTL);
    builder.set_urgency(web_push::Urgency::High);
    builder.set_payload(ContentEncoding::Aes128Gcm, payload);
    builder.set_vapid_signature(signature);
    let built = builder.build().map_err(|e| (false, format!("could not encrypt the push: {e}")))?;
    let mut req = state
        .http
        .post(built.endpoint.to_string())
        .timeout(SEND_TIMEOUT)
        .header("TTL", built.ttl.to_string());
    if let Some(urgency) = built.urgency {
        req = req.header("Urgency", urgency.to_string());
    }
    let Some(payload) = built.payload else {
        return Err((false, "nothing to send".into()));
    };
    req = req
        .header("Content-Encoding", payload.content_encoding.to_str())
        .header("Content-Type", "application/octet-stream");
    for (name, value) in payload.crypto_headers {
        req = req.header(name, value);
    }
    let resp = req
        .body(payload.content)
        .send()
        .await
        .map_err(|e| (false, format!("could not reach the push service: {}", e.without_url())))?;
    let status = resp.status();
    match status.as_u16() {
        200..=299 => Ok(()),
        404 | 410 => Err((true, format!("the push service answered {status}"))),
        _ => {
            let body = resp.text().await.unwrap_or_default();
            let body = body.trim();
            Err((false, if body.is_empty() { format!("the push service answered {status}") } else { format!("the push service answered {status}: {}", body.chars().take(200).collect::<String>()) }))
        }
    }
}

/// Record how a send went, for Settings.
pub async fn record(store: &Store, method: Method, test: bool, outcome: &Result<String, String>) -> Result<(), AppError> {
    let mut settings = store.notification_settings().await?;
    settings.last_delivery = Some(LastDelivery {
        at: Utc::now(),
        method,
        ok: outcome.is_ok(),
        detail: match outcome {
            Ok(d) | Err(d) => d.clone(),
        },
        test,
    });
    store.set_notification_settings(&settings).await?;
    Ok(())
}

/// The message for a finished turn: what happened, in which conversation, and where to look.
pub fn message_for(state: &AppState, turn: &crate::store::Turn, title: Option<&str>, scheduled: bool) -> Message {
    let name = title.unwrap_or("New chat");
    let needs_answer = turn.status == "completed" && turn.stop_reason.as_deref() == Some("ask_user");
    let what = if needs_answer {
        "needs your answer"
    } else if turn.status == "failed" {
        "failed"
    } else {
        "finished"
    };
    let kind = if scheduled { "A scheduled run" } else { "A response" };
    let body = match (&turn.error, needs_answer, &turn.final_text) {
        (Some(error), _, _) => format!("{kind} in \"{name}\" failed: {error}"),
        (None, true, Some(text)) => format!("Cedar asks in \"{name}\":\n{}", excerpt(text)),
        (None, _, Some(text)) => format!("{kind} in \"{name}\" finished:\n{}", excerpt(text)),
        (None, _, None) => format!("{kind} in \"{name}\" {what}."),
    };
    Message {
        title: format!("{name}: {what}"),
        body,
        url: format!("{}/#/c/{}", state.cfg.public_url, turn.conversation_id),
        tag: turn.id.to_string(),
    }
}

/// The first few lines of a reply, short enough for a push.
fn excerpt(text: &str) -> String {
    const MAX_CHARS: usize = 300;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(MAX_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// Whether a finished turn is worth sending under `scope`. Stopped runs never are.
pub fn wanted(scope: Scope, turn: &crate::store::Turn, scheduled: bool) -> bool {
    if !matches!(turn.status.as_str(), "completed" | "failed") {
        return false;
    }
    match scope {
        Scope::All => true,
        Scope::Attention => {
            turn.status == "failed" || turn.stop_reason.as_deref() == Some("ask_user") || scheduled
        }
    }
}

/// Send the out-of-app message for a turn that ended, if the settings call for one and nobody
/// has looked at it since. Returns what was done, for logs and tests.
pub async fn dispatch(state: &AppState, turn_id: Uuid) -> Result<Option<Result<String, String>>, AppError> {
    let Some(turn) = state.store.get_turn(turn_id).await? else { return Ok(None) };
    if turn.seen {
        return Ok(None);
    }
    let settings = state.store.notification_settings().await?;
    if settings.method == Method::None {
        return Ok(None);
    }
    let conversation = state.store.get_conversation(turn.conversation_id).await?;
    let scheduled = conversation.as_ref().is_some_and(|c| c.schedule_id.is_some());
    if !wanted(settings.scope, &turn, scheduled) {
        return Ok(None);
    }
    let message = message_for(state, &turn, conversation.as_ref().and_then(|c| c.title.as_deref()), scheduled);
    let outcome = send(state, &settings, settings.method, &message).await;
    match &outcome {
        Ok(detail) => tracing::info!(turn_id = %turn.id, method = settings.method.as_str(), detail, "notification sent"),
        Err(error) => tracing::warn!(turn_id = %turn.id, method = settings.method.as_str(), error, "notification failed"),
    }
    record(&state.store, settings.method, false, &outcome).await?;
    Ok(Some(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_scopes_and_security_round_trip() {
        for m in [Method::None, Method::Browser, Method::Pushover, Method::Email] {
            assert_eq!(Method::parse(m.as_str()), Some(m));
        }
        assert_eq!(Method::parse("carrier pigeon"), None);
        assert_eq!(Scope::parse("all"), Some(Scope::All));
        assert_eq!(Security::parse("starttls").map(Security::default_port), Some(587));
        let json = serde_json::to_string(&NotificationSettings::default()).unwrap();
        assert!(json.contains("\"method\":\"none\""));
        let back: NotificationSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(back, NotificationSettings::default());
    }

    #[test]
    fn the_status_hides_the_secrets() {
        let settings = NotificationSettings {
            method: Method::Pushover,
            pushover: PushoverSettings { token: "app-token-secret".into(), user: "user-key-abcd".into() },
            email: EmailSettings { host: "smtp.example".into(), password: "hunter2".into(), from: "a@x".into(), to: "b@x".into(), ..Default::default() },
            vapid: Some(Vapid { private_key: "PRIVATE".into(), public_key: "PUBLIC".into(), contact: "me@x".into() }),
            ..Default::default()
        };
        let status = settings.status(&[]).to_string();
        for secret in ["app-token-secret", "user-key-abcd", "hunter2", "PRIVATE"] {
            assert!(!status.contains(secret), "{secret} leaked: {status}");
        }
        assert!(status.contains("\"user_last4\":\"abcd\""));
        assert!(status.contains("\"password_set\":true"));
        assert!(status.contains("\"public_key\":\"PUBLIC\""));
        assert!(settings.check(Method::Pushover, 0).is_ok());
        assert!(settings.check(Method::Browser, 0).unwrap_err().contains("no browser"));
        assert!(settings.check(Method::Browser, 1).is_ok());
        assert!(NotificationSettings::default().check(Method::Email, 0).unwrap_err().contains("SMTP"));
    }

    #[test]
    fn excerpts_are_cut() {
        assert_eq!(excerpt("  short  "), "short");
        let long = "word ".repeat(100);
        let cut = excerpt(&long);
        assert!(cut.ends_with('…') && cut.chars().count() <= 301);
    }
}
