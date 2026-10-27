//! Telling the user out of the app when a response finishes: a phone push through Pushover, a
//! browser push through Web Push (a service worker shows it, tab open or not), a push to the
//! iOS app through APNs (Apple's push service), a push to the Android app (through Firebase
//! Cloud Messaging, or UnifiedPush for a phone without Google's services, ntfy say), an email
//! over SMTP, or a notification from the Cedar desktop app, which follows `GET /api/notifications/desktop/events` and shows what
//! comes on it in Notification Center (a toast on Windows). Each method is set up with its credentials in Settings → Notifications, as many as
//! wanted; a default method covers conversations, and a scheduled agent can pick its own. The
//! settings live in this service's database (see [`NotificationSettings`]) and the secrets are
//! never sent back to the page.
//!
//! What gets sent is decided by [`dispatch`], which `notify` calls a few seconds after cedar
//! says a run ended: nothing if the response was looked at meanwhile, if the method that applies
//! is "none", or (for the default method) if the scope leaves it out. Every delivery, test or
//! not, is recorded as `last_delivery` so Settings can show what happened last.

use crate::api::AppState;
use crate::error::AppError;
use crate::i18n::{self, Lang, Outcome};
use crate::store::{AndroidDevice, ApnsDevice, PushSubscription, Store};
use chrono::{DateTime, Utc};
use jwt_simple::algorithms::{ECDSAP256KeyPairLike, ECDSAP256PublicKeyLike, ES256KeyPair};
use jwt_simple::claims::Claims;
use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as Email, Tokio1Executor};
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::sign::Signer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;
use web_push::{ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder};

pub const PUSHOVER_URL: &str = "https://api.pushover.net/1/messages.json";
/// How long after a run ends the out-of-app message goes, so a page that watched the response
/// end has time to mark it seen first.
pub const DISPATCH_DELAY: Duration = Duration::from_secs(3);
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a push service keeps a message for a browser that is offline.
const PUSH_TTL: u32 = 24 * 3600;
/// How many messages the Cedar app may fall behind on its stream before it misses some.
pub const DESKTOP_BUFFER: usize = 16;
pub const APNS_URL: &str = "https://api.push.apple.com";
pub const APNS_SANDBOX_URL: &str = "https://api.sandbox.push.apple.com";
/// How long one signed APNs provider token is reused. Apple takes a token for an hour and
/// asks that it isn't refreshed more often than every 20 minutes.
const APNS_TOKEN_LIFETIME: Duration = Duration::from_secs(50 * 60);
pub const FCM_URL: &str = "https://fcm.googleapis.com";
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// What the Firebase service account's access token is asked for: sending FCM messages, nothing else.
const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// How long one Google access token is reused. Google gives them for an hour.
const FCM_TOKEN_LIFETIME: Duration = Duration::from_secs(50 * 60);

/// Which channel carries the message. `None` keeps notifications in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    None,
    Browser,
    Pushover,
    Email,
    /// The Cedar app for macOS (see [`DESKTOP_BUFFER`]).
    Desktop,
    /// The iOS app, through APNs.
    Ios,
    /// The Android app, through FCM or UnifiedPush.
    Android,
}

impl Method {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Method::None,
            "browser" => Method::Browser,
            "pushover" => Method::Pushover,
            "email" => Method::Email,
            "desktop" => Method::Desktop,
            "ios" => Method::Ios,
            "android" => Method::Android,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::None => "none",
            Method::Browser => "browser",
            Method::Pushover => "pushover",
            Method::Email => "email",
            Method::Desktop => "desktop",
            Method::Ios => "ios",
            Method::Android => "android",
        }
    }

    /// For error messages: every name but `none`.
    pub const NAMES: &'static str = "desktop, browser, pushover, email, ios or android";
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

/// Which APNs to send through: `production` for an App Store or TestFlight build of the iOS
/// app, `sandbox` for one Xcode installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApnsEnvironment {
    #[default]
    Production,
    Sandbox,
}

impl ApnsEnvironment {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "production" => ApnsEnvironment::Production,
            "sandbox" => ApnsEnvironment::Sandbox,
            _ => return None,
        })
    }

    pub fn url(self) -> &'static str {
        match self {
            ApnsEnvironment::Production => APNS_URL,
            ApnsEnvironment::Sandbox => APNS_SANDBOX_URL,
        }
    }
}

/// The APNs credentials: an "APNs Auth Key" from the Apple developer account (a `.p8` file
/// and its key id), the team id, and the app's bundle id as the topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApnsSettings {
    pub team_id: String,
    pub key_id: String,
    /// The `.p8` file's contents (a PKCS#8 PEM).
    pub private_key: String,
    pub topic: String,
    pub environment: ApnsEnvironment,
}

/// The iOS app's bundle id, as `iOS/Cedar.xcodeproj` sets it.
pub const DEFAULT_APNS_TOPIC: &str = "dev.cedar.Cedar";

impl Default for ApnsSettings {
    fn default() -> Self {
        ApnsSettings {
            team_id: String::new(),
            key_id: String::new(),
            private_key: String::new(),
            topic: DEFAULT_APNS_TOPIC.into(),
            environment: ApnsEnvironment::Production,
        }
    }
}

impl ApnsSettings {
    pub fn configured(&self) -> bool {
        !self.team_id.is_empty() && !self.key_id.is_empty() && !self.private_key.is_empty() && !self.topic.is_empty()
    }

    /// Tells one set of credentials from another, so a cached provider token is dropped when
    /// they change.
    fn fingerprint(&self) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(format!("{}\n{}\n{}", self.team_id, self.key_id, self.private_key).as_bytes());
        format!("{digest:x}")
    }
}

/// Android push through Firebase: the service account's key, the JSON file the Firebase console
/// gives (Project settings → Service accounts → Generate new private key). UnifiedPush needs
/// nothing here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AndroidSettings {
    /// The JSON file's contents, as pasted.
    pub service_account: String,
}

/// What a Firebase service account's JSON file holds that sending needs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServiceAccount {
    pub project_id: String,
    pub client_email: String,
    /// A PKCS#8 PEM with an RSA key.
    pub private_key: String,
    #[serde(default)]
    pub private_key_id: Option<String>,
}

impl ServiceAccount {
    /// Reads the JSON file; the error is for the user.
    pub fn parse(json: &str) -> Result<Self, String> {
        let account: ServiceAccount = serde_json::from_str(json).map_err(|_| {
            "the service-account key must be the JSON file Firebase gave you, with project_id, client_email and private_key".to_string()
        })?;
        if account.project_id.trim().is_empty() || account.client_email.trim().is_empty() {
            return Err("the service-account key has no project_id or client_email".into());
        }
        // It goes into the FCM address; Google's project ids are letters, digits and dashes
        // (with a domain and a colon in front for some old ones).
        if !account.project_id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b)) {
            return Err("the service-account key's project_id is not a Firebase project id".into());
        }
        if !account.private_key.contains("-----BEGIN PRIVATE KEY-----") {
            return Err("the service-account key's private_key must be a PEM, starting with -----BEGIN PRIVATE KEY-----".into());
        }
        Ok(account)
    }

    /// The RSA key the access token is asked for with.
    pub fn signing_key(&self) -> Result<PKey<Private>, String> {
        PKey::private_key_from_pem(self.private_key.as_bytes())
            .ok()
            .filter(|k| k.rsa().is_ok())
            .ok_or_else(|| "the service-account key's private_key is not a usable RSA key".to_string())
    }

    /// The signed assertion (a JWT, RS256) that Google trades for an access token to send with.
    fn assertion(&self) -> Result<String, String> {
        let mut header = json!({ "alg": "RS256", "typ": "JWT" });
        if let Some(kid) = self.private_key_id.as_deref().filter(|k| !k.is_empty()) {
            header["kid"] = json!(kid);
        }
        let now = Utc::now().timestamp();
        let claims = json!({ "iss": self.client_email, "scope": FCM_SCOPE, "aud": GOOGLE_TOKEN_URL, "iat": now, "exp": now + 3600 });
        let input = format!("{}.{}", b64url(header.to_string().as_bytes()), b64url(claims.to_string().as_bytes()));
        let key = self.signing_key()?;
        let signed = Signer::new(MessageDigest::sha256(), &key)
            .and_then(|mut signer| signer.update(input.as_bytes()).and_then(|_| signer.sign_to_vec()))
            .map_err(|e| format!("could not sign the Google token request: {e}"))?;
        Ok(format!("{input}.{}", b64url(&signed)))
    }
}

impl AndroidSettings {
    /// The service account, when one is set and readable.
    pub fn account(&self) -> Option<ServiceAccount> {
        ServiceAccount::parse(&self.service_account).ok()
    }

    /// Whether FCM can send: a usable service account is set.
    pub fn fcm_configured(&self) -> bool {
        self.account().is_some()
    }

    /// Tells one service account from another, so a cached access token is dropped when it changes.
    fn fingerprint(&self) -> String {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(self.service_account.as_bytes()))
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
    /// For responses in conversations, and for scheduled agents that pick none of their own.
    #[serde(alias = "method")]
    pub default_method: Method,
    pub scope: Scope,
    pub pushover: PushoverSettings,
    pub email: EmailSettings,
    pub apns: ApnsSettings,
    pub android: AndroidSettings,
    pub vapid: Option<Vapid>,
    /// When the Cedar app for macOS last opened its stream; set once it has, which is what makes
    /// `desktop` a method that can be picked (whether the app is listening right now is only
    /// known when sending).
    pub desktop_connected_at: Option<DateTime<Utc>>,
    pub last_delivery: Option<LastDelivery>,
}

/// How many browsers, iPhones and Android phones can be reached: what the methods that push
/// need. `unifiedpush` counts the Android phones (of `androids`) that need no Firebase key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reach {
    pub browsers: usize,
    pub iphones: usize,
    pub androids: usize,
    pub unifiedpush: usize,
}

impl Reach {
    pub fn new(subscriptions: &[PushSubscription], devices: &[ApnsDevice], androids: &[AndroidDevice]) -> Self {
        Reach {
            browsers: subscriptions.len(),
            iphones: devices.len(),
            androids: androids.len(),
            unifiedpush: androids.iter().filter(|d| d.transport == UNIFIEDPUSH).count(),
        }
    }
}

/// The Android phones' two ways to get a push.
pub const FCM: &str = "fcm";
pub const UNIFIEDPUSH: &str = "unifiedpush";

impl NotificationSettings {
    /// The methods that can send right now, in the order the pickers show them.
    pub fn configured(&self, reach: Reach) -> Vec<Method> {
        [Method::Desktop, Method::Browser, Method::Ios, Method::Android, Method::Pushover, Method::Email]
            .into_iter()
            .filter(|m| self.check(*m, reach).is_ok())
            .collect()
    }

    /// What the page sees: everything but the secrets, which are only reported as set.
    pub fn status(&self, subscriptions: &[PushSubscription], devices: &[ApnsDevice], androids: &[AndroidDevice]) -> Value {
        let reach = Reach::new(subscriptions, devices, androids);
        let account = self.android.account();
        json!({
            "default_method": self.default_method,
            "configured": self.configured(reach),
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
            "desktop": {
                "configured": self.desktop_connected_at.is_some(),
                "connected_at": self.desktop_connected_at,
            },
            "ios": {
                "configured": self.apns.configured(),
                "team_id": self.apns.team_id,
                "key_id": self.apns.key_id,
                "key_set": !self.apns.private_key.is_empty(),
                "topic": self.apns.topic,
                "environment": self.apns.environment,
                "devices": devices,
            },
            // `configured`: the server can push to some kind of Android phone. UnifiedPush needs
            // no credentials here, so a registered UnifiedPush phone is enough; FCM needs the key.
            "android": {
                "configured": account.is_some() || reach.unifiedpush > 0,
                "fcm_configured": account.is_some(),
                "project_id": account.as_ref().map(|a| a.project_id.clone()).unwrap_or_default(),
                "client_email": account.as_ref().map(|a| a.client_email.clone()).unwrap_or_default(),
                "key_set": !self.android.service_account.is_empty(),
                "devices": androids,
            },
            "last_delivery": self.last_delivery,
        })
    }

    /// Whether `method` has what it needs to send; the message says what is missing.
    pub fn check(&self, method: Method, reach: Reach) -> Result<(), String> {
        match method {
            Method::None => Ok(()),
            Method::Pushover if !self.pushover.configured() => {
                Err("Pushover needs the application token and the user key".into())
            }
            Method::Email if !self.email.configured() => {
                Err("email needs the SMTP server, a sender and a recipient".into())
            }
            Method::Browser if reach.browsers == 0 => {
                Err("no browser has turned on push notifications yet".into())
            }
            Method::Desktop if self.desktop_connected_at.is_none() => {
                Err("the Cedar app for macOS has not connected yet".into())
            }
            Method::Ios if !self.apns.configured() => {
                Err("iPhone push needs the team id, the key id, the APNs key and the app's bundle id".into())
            }
            Method::Ios if reach.iphones == 0 => {
                Err("no iPhone has registered for push yet; open the Cedar app and allow notifications".into())
            }
            Method::Android if reach.androids == 0 => {
                Err("no Android phone has registered for push yet; open the Cedar app and allow notifications".into())
            }
            Method::Android if reach.unifiedpush == 0 && !self.android.fcm_configured() => {
                Err("Android push through FCM needs the Firebase service-account key".into())
            }
            _ => Ok(()),
        }
    }
}

fn last4(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.chars().rev().take(4).collect::<String>().chars().rev().collect())
}

/// What is sent, however it goes: a title, a line or two, and where to look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub title: String,
    pub body: String,
    /// The conversation's address in the chat, absolute (from `CHAT_PUBLIC_URL`).
    pub url: String,
    /// Collapses repeats of the same thing (the turn's id).
    pub tag: String,
    /// The conversation to open, for a client that navigates by id rather than URL. A test
    /// notification has none.
    pub conversation_id: Option<Uuid>,
    /// The link's label where the channel shows one (Pushover), in the user's language.
    pub open_label: String,
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
        Method::Desktop => send_desktop(state, message),
        Method::Ios => send_apns(state, &settings.apns, message).await,
        Method::Android => send_android(state, &settings.android, message).await,
    }
}

/// Hand the message to the Cedar app, if it is following its stream.
fn send_desktop(state: &AppState, message: &Message) -> Result<String, String> {
    let mut payload = serde_json::to_value(message).map_err(|e| e.to_string())?;
    payload["type"] = json!("notification");
    match state.desktop.send(payload) {
        Ok(_) => Ok("handed to the Cedar app".into()),
        Err(_) => Err("the Cedar app for macOS is not listening; is it running?".into()),
    }
}

/// The connection to APNs: an HTTP/2 client, and the provider token (a signed JWT) that is
/// reused for [`APNS_TOKEN_LIFETIME`]. One per service, in `AppState`.
pub struct Apns {
    client: reqwest::Client,
    /// `CHAT_APNS_URL`, when set: where to send instead of Apple.
    base_url: Option<String>,
    token: Mutex<Option<ProviderToken>>,
}

struct ProviderToken {
    fingerprint: String,
    made_at: Instant,
    jwt: String,
}

impl Apns {
    pub fn new(base_url: Option<String>) -> reqwest::Result<Self> {
        // Apple only speaks HTTP/2, over TLS. A plain-http override (a mock in tests) is spoken
        // to as the server likes.
        let mut builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(10));
        if base_url.as_deref().is_none_or(|u| u.starts_with("https://")) {
            builder = builder.http2_prior_knowledge();
        }
        Ok(Apns { client: builder.build()?, base_url, token: Mutex::new(None) })
    }

    fn url_for(&self, settings: &ApnsSettings, device_token: &str) -> String {
        let base = self.base_url.as_deref().unwrap_or_else(|| settings.environment.url());
        format!("{base}/3/device/{device_token}")
    }

    /// The provider token for `settings`: the cached one while it is fresh and for the same
    /// key, else a newly signed one. `fresh` forces a new one (after Apple said it expired).
    fn provider_token(&self, settings: &ApnsSettings, fresh: bool) -> Result<String, String> {
        let fingerprint = settings.fingerprint();
        let mut cached = self.token.lock().unwrap_or_else(|e| e.into_inner());
        if !fresh {
            if let Some(t) = cached.as_ref() {
                if t.fingerprint == fingerprint && t.made_at.elapsed() < APNS_TOKEN_LIFETIME {
                    return Ok(t.jwt.clone());
                }
            }
        }
        let key = ES256KeyPair::from_pem(&settings.private_key)
            .map_err(|_| "the APNs key is not a usable .p8 file (a PKCS#8 PEM with an EC P-256 key)".to_string())?
            .with_key_id(&settings.key_id);
        // Apple wants exactly `iss` (the team) and `iat`; the rest of the standard claims stay out.
        let mut claims = Claims::create(jwt_simple::prelude::Duration::from_hours(1)).with_issuer(&settings.team_id);
        claims.expires_at = None;
        claims.invalid_before = None;
        let jwt = key.sign(claims).map_err(|e| format!("could not sign the APNs token: {e}"))?;
        *cached = Some(ProviderToken { fingerprint, made_at: Instant::now(), jwt: jwt.clone() });
        Ok(jwt)
    }
}

/// One push per registered iPhone; a device Apple no longer knows is dropped. Succeeds if any
/// iPhone took it.
async fn send_apns(state: &AppState, settings: &ApnsSettings, message: &Message) -> Result<String, String> {
    if !settings.configured() {
        return Err("iPhone push needs the team id, the key id, the APNs key and the app's bundle id".into());
    }
    let devices = state.store.apns_devices().await.map_err(|e| e.to_string())?;
    if devices.is_empty() {
        return Err("no iPhone has registered for push yet; open the Cedar app and allow notifications".into());
    }
    let payload = json!({
        "aps": {
            "alert": { "title": message.title, "body": message.body },
            "sound": "default",
            "thread-id": message.conversation_id.map(|id| id.to_string()).unwrap_or_else(|| message.tag.clone()),
        },
        "url": message.url,
        "conversation_id": message.conversation_id,
        "tag": message.tag,
    })
    .to_string();
    let mut delivered = 0;
    let mut dropped = 0;
    let mut errors = Vec::new();
    for device in &devices {
        let mut outcome = apns_one(state, settings, device, &payload, false).await;
        if matches!(&outcome, Err((false, e)) if e.contains("ExpiredProviderToken")) {
            outcome = apns_one(state, settings, device, &payload, true).await;
        }
        match outcome {
            Ok(()) => {
                delivered += 1;
                let _ = state.store.touch_apns_device(device.id).await;
            }
            Err((gone, error)) => {
                if gone {
                    dropped += 1;
                    let _ = state.store.delete_apns_device(device.id).await;
                } else {
                    errors.push(error);
                }
            }
        }
    }
    if delivered > 0 {
        let mut detail = format!("sent to {delivered} iPhone{}", if delivered == 1 { "" } else { "s" });
        if dropped > 0 {
            detail.push_str(&format!(", {dropped} no longer registered"));
        }
        if !errors.is_empty() {
            detail.push_str(&format!("; failed: {}", errors.join("; ")));
        }
        Ok(detail)
    } else if !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        Err("every registered iPhone has gone; open the Cedar app again to register it".into())
    }
}

/// `Err((gone, message))`: `gone` when Apple says the device token is dead.
async fn apns_one(state: &AppState, settings: &ApnsSettings, device: &ApnsDevice, payload: &str, fresh_token: bool) -> Result<(), (bool, String)> {
    let token = state.apns.provider_token(settings, fresh_token).map_err(|e| (false, e))?;
    let resp = state
        .apns
        .client
        .post(state.apns.url_for(settings, &device.token))
        .timeout(SEND_TIMEOUT)
        .header("authorization", format!("bearer {token}"))
        .header("apns-topic", &settings.topic)
        .header("apns-push-type", "alert")
        .header("apns-priority", "10")
        .header("apns-collapse-id", message_tag(payload))
        .header("content-type", "application/json")
        .body(payload.to_string())
        .send()
        .await
        .map_err(|e| (false, format!("could not reach APNs: {}", e.without_url())))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let reason = body["reason"].as_str().unwrap_or("").to_string();
    let gone = status.as_u16() == 410 || matches!(reason.as_str(), "BadDeviceToken" | "DeviceTokenNotForTopic" | "Unregistered");
    let detail = if reason.is_empty() { format!("APNs answered {status}") } else { format!("APNs answered {status}: {reason}") };
    Err((gone, detail))
}

/// The collapse id, read back from the payload so the two stay the same.
fn message_tag(payload: &str) -> String {
    serde_json::from_str::<Value>(payload).ok().and_then(|v| v["tag"].as_str().map(str::to_string)).unwrap_or_default()
}

/// The Google access token for the Firebase service account, reused for [`FCM_TOKEN_LIFETIME`].
/// One per service, in `AppState`; where it is asked for and where the pushes go are
/// `CHAT_GOOGLE_TOKEN_URL` and `CHAT_FCM_URL`.
#[derive(Default)]
pub struct Fcm {
    token: Mutex<Option<AccessToken>>,
}

struct AccessToken {
    fingerprint: String,
    made_at: Instant,
    lifetime: Duration,
    token: String,
}

impl Fcm {
    /// The access token for `settings`: the cached one while it is fresh and for the same
    /// service account, else a new one from Google. `fresh` forces a new one (after FCM said 401).
    async fn access_token(&self, state: &AppState, settings: &AndroidSettings, account: &ServiceAccount, fresh: bool) -> Result<String, String> {
        let fingerprint = settings.fingerprint();
        if !fresh {
            let cached = self.token.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(t) = cached.as_ref() {
                if t.fingerprint == fingerprint && t.made_at.elapsed() < t.lifetime {
                    return Ok(t.token.clone());
                }
            }
        }
        let assertion = account.assertion()?;
        let resp = state
            .http
            .post(&state.cfg.google_token_url)
            .timeout(SEND_TIMEOUT)
            .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", assertion.as_str())])
            .send()
            .await
            .map_err(|e| format!("could not reach Google for an FCM token: {}", e.without_url()))?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        match body["access_token"].as_str() {
            Some(token) if status.is_success() => {
                // A little short of what Google says, so a token isn't used as it runs out.
                let lifetime = body["expires_in"]
                    .as_u64()
                    .map(|s| Duration::from_secs(s.saturating_sub(5 * 60)))
                    .map_or(FCM_TOKEN_LIFETIME, |l| l.min(FCM_TOKEN_LIFETIME));
                let token = token.to_string();
                *self.token.lock().unwrap_or_else(|e| e.into_inner()) = Some(AccessToken { fingerprint, made_at: Instant::now(), lifetime, token: token.clone() });
                Ok(token)
            }
            _ => {
                let why = body["error_description"].as_str().or(body["error"].as_str()).map(str::to_string).unwrap_or_else(|| format!("HTTP {status}"));
                Err(format!("Google refused the Firebase service-account key: {why}"))
            }
        }
    }
}

/// What an Android push carries: only which turn ended and where, never what was said. The app
/// asks the API for the rest when it gets one, so nothing of a conversation passes through
/// Google or a UnifiedPush server (which is why the UnifiedPush body goes unencrypted). Every
/// value is a string, as FCM's `data` wants. `kind` is `test` for a test notification (the one
/// message without a conversation), else `turn`.
fn android_payload(message: &Message) -> Value {
    json!({
        "kind": if message.conversation_id.is_some() { "turn" } else { "test" },
        "conversation_id": message.conversation_id.map(|id| id.to_string()).unwrap_or_default(),
        "tag": message.tag,
    })
}

/// One push per registered Android phone, through FCM or its UnifiedPush endpoint; a phone the
/// service no longer knows is dropped. Succeeds if any phone took it.
async fn send_android(state: &AppState, settings: &AndroidSettings, message: &Message) -> Result<String, String> {
    let devices = state.store.android_devices().await.map_err(|e| e.to_string())?;
    if devices.is_empty() {
        return Err("no Android phone has registered for push yet; open the Cedar app and allow notifications".into());
    }
    let data = android_payload(message);
    let account = settings.account();
    let mut delivered = 0;
    let mut dropped = 0;
    let mut errors: Vec<String> = Vec::new();
    for device in &devices {
        let outcome = if device.transport == UNIFIEDPUSH {
            unifiedpush_one(state, device, &data).await
        } else if let Some(account) = &account {
            let mut outcome = fcm_one(state, settings, account, device, &data, false).await;
            if matches!(&outcome, Err((false, e)) if e.contains("401")) {
                outcome = fcm_one(state, settings, account, device, &data, true).await;
            }
            outcome
        } else {
            Err((false, "Android push through FCM needs the Firebase service-account key".into()))
        };
        match outcome {
            Ok(()) => {
                delivered += 1;
                let _ = state.store.touch_android_device(device.id).await;
            }
            Err((true, _)) => {
                dropped += 1;
                let _ = state.store.delete_android_device(device.id).await;
            }
            // The same failure for every phone (a missing key, Google down) is said once.
            Err((false, error)) if !errors.contains(&error) => errors.push(error),
            Err(_) => {}
        }
    }
    if delivered > 0 {
        let mut detail = format!("sent to {delivered} Android phone{}", if delivered == 1 { "" } else { "s" });
        if dropped > 0 {
            detail.push_str(&format!(", {dropped} no longer registered"));
        }
        if !errors.is_empty() {
            detail.push_str(&format!("; failed: {}", errors.join("; ")));
        }
        Ok(detail)
    } else if !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        Err("every registered Android phone has gone; open the Cedar app again to register it".into())
    }
}

/// `Err((gone, message))`: `gone` when FCM says the registration token is dead: a 404 or
/// `UNREGISTERED`, or a 400 that blames the token (the app was reinstalled, or the token is
/// garbage). Other 400s are this server's fault and keep the phone.
async fn fcm_one(state: &AppState, settings: &AndroidSettings, account: &ServiceAccount, device: &AndroidDevice, data: &Value, fresh_token: bool) -> Result<(), (bool, String)> {
    let token = state.fcm.access_token(state, settings, account, fresh_token).await.map_err(|e| (false, e))?;
    let body = json!({
        "message": {
            "token": device.token,
            "data": data,
            "android": { "priority": "high", "collapse_key": data["tag"] },
        }
    });
    let resp = state
        .http
        .post(format!("{}/v1/projects/{}/messages:send", state.cfg.fcm_url, account.project_id))
        .timeout(SEND_TIMEOUT)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| (false, format!("could not reach FCM: {}", e.without_url())))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let error = &body["error"];
    let code = error["details"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|d| d["errorCode"].as_str())
        .or(error["status"].as_str())
        .unwrap_or("")
        .to_string();
    let text = error["message"].as_str().unwrap_or("");
    let gone = status.as_u16() == 404 || code == "UNREGISTERED" || (status.as_u16() == 400 && text.contains("registration token"));
    let detail = if code.is_empty() { format!("FCM answered {status}") } else { format!("FCM answered {status}: {code}") };
    Err((gone, detail))
}

/// `Err((gone, message))`: `gone` when the UnifiedPush server says the endpoint is dead (404 or
/// 410), as for Web Push.
async fn unifiedpush_one(state: &AppState, device: &AndroidDevice, data: &Value) -> Result<(), (bool, String)> {
    let resp = state
        .http
        .post(&device.token)
        .timeout(SEND_TIMEOUT)
        .header("Content-Type", "application/json")
        .header("TTL", PUSH_TTL.to_string())
        .header("Urgency", "high")
        .body(data.to_string())
        .send()
        .await
        .map_err(|e| (false, format!("could not reach the UnifiedPush server: {}", e.without_url())))?;
    let status = resp.status();
    match status.as_u16() {
        200..=299 => Ok(()),
        404 | 410 => Err((true, format!("the UnifiedPush server answered {status}"))),
        _ => {
            let body = resp.text().await.unwrap_or_default();
            let body = body.trim();
            Err((false, if body.is_empty() { format!("the UnifiedPush server answered {status}") } else { format!("the UnifiedPush server answered {status}: {}", body.chars().take(200).collect::<String>()) }))
        }
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
        ("url_title", message.open_label.as_str()),
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
/// It is written in the language of `locale` (the one saved in Settings).
pub fn message_for(
    state: &AppState,
    turn: &crate::store::Turn,
    title: Option<&str>,
    scheduled: bool,
    locale: Option<&str>,
) -> Message {
    let needs_answer = turn.status == "completed" && turn.stop_reason.as_deref() == Some("ask_user");
    let outcome = if needs_answer {
        Outcome::NeedsAnswer
    } else if turn.status == "failed" {
        Outcome::Failed
    } else {
        Outcome::Finished
    };
    let excerpt = turn.final_text.as_deref().map(excerpt);
    let (title, body) =
        i18n::turn_message(locale, title, outcome, scheduled, turn.error.as_deref(), excerpt.as_deref());
    Message {
        title,
        body,
        open_label: Lang::of(locale).open_in_cedar().into(),
        url: format!("{}/#/c/{}", state.cfg.public_url, turn.conversation_id),
        tag: turn.id.to_string(),
        conversation_id: Some(turn.conversation_id),
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
/// has looked at it since: by the schedule's own method for a scheduled run that has one (then
/// every finished or failed run goes), else by the default method within the scope. Returns
/// what was done, for logs and tests.
pub async fn dispatch(state: &AppState, turn_id: Uuid) -> Result<Option<Result<String, String>>, AppError> {
    let Some(turn) = state.store.get_turn(turn_id).await? else { return Ok(None) };
    if turn.seen {
        return Ok(None);
    }
    let settings = state.store.notification_settings().await?;
    let conversation = state.store.get_conversation(turn.conversation_id).await?;
    let schedule = match conversation.as_ref().and_then(|c| c.schedule_id) {
        Some(id) => state.store.get_schedule(id).await?,
        None => None,
    };
    let own = schedule.as_ref().and_then(|s| s.notify.as_deref()).and_then(Method::parse);
    let method = own.unwrap_or(settings.default_method);
    if method == Method::None {
        return Ok(None);
    }
    let scheduled = schedule.is_some();
    let scope = if own.is_some() { Scope::All } else { settings.scope };
    if !wanted(scope, &turn, scheduled) {
        return Ok(None);
    }
    let locale = state.store.preferences().await?.locale;
    let message = message_for(state, &turn, conversation.as_ref().and_then(|c| c.title.as_deref()), scheduled, locale.as_deref());
    let outcome = send(state, &settings, method, &message).await;
    match &outcome {
        Ok(detail) => tracing::info!(turn_id = %turn.id, method = method.as_str(), detail, "notification sent"),
        Err(error) => tracing::warn!(turn_id = %turn.id, method = method.as_str(), error, "notification failed"),
    }
    record(&state.store, method, false, &outcome).await?;
    Ok(Some(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_scopes_and_security_round_trip() {
        for m in [Method::None, Method::Browser, Method::Pushover, Method::Email, Method::Desktop, Method::Ios, Method::Android] {
            assert_eq!(Method::parse(m.as_str()), Some(m));
        }
        assert_eq!(Method::parse("carrier pigeon"), None);
        assert_eq!(Scope::parse("all"), Some(Scope::All));
        assert_eq!(Security::parse("starttls").map(Security::default_port), Some(587));
        let json = serde_json::to_string(&NotificationSettings::default()).unwrap();
        assert!(json.contains("\"default_method\":\"none\""));
        let back: NotificationSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(back, NotificationSettings::default());
        // Settings saved when the field was still `method` still read.
        let old: NotificationSettings = serde_json::from_str("{\"method\":\"email\"}").unwrap();
        assert_eq!(old.default_method, Method::Email);
        // Settings saved before iPhone push existed read with its defaults.
        assert_eq!(old.apns.topic, DEFAULT_APNS_TOPIC);
        assert_eq!(ApnsEnvironment::parse("sandbox").map(ApnsEnvironment::url), Some(APNS_SANDBOX_URL));
    }

    #[test]
    fn the_status_hides_the_secrets() {
        let settings = NotificationSettings {
            default_method: Method::Pushover,
            pushover: PushoverSettings { token: "app-token-secret".into(), user: "user-key-abcd".into() },
            email: EmailSettings { host: "smtp.example".into(), password: "hunter2".into(), from: "a@x".into(), to: "b@x".into(), ..Default::default() },
            vapid: Some(Vapid { private_key: "PRIVATE".into(), public_key: "PUBLIC".into(), contact: "me@x".into() }),
            apns: ApnsSettings { team_id: "TEAM42".into(), key_id: "KEY42".into(), private_key: "-----BEGIN PRIVATE KEY-----\nP8SECRET\n".into(), ..Default::default() },
            ..Default::default()
        };
        let status = settings.status(&[], &[], &[]).to_string();
        for secret in ["app-token-secret", "user-key-abcd", "hunter2", "PRIVATE", "P8SECRET"] {
            assert!(!status.contains(secret), "{secret} leaked: {status}");
        }
        assert!(status.contains("\"user_last4\":\"abcd\""));
        assert!(status.contains("\"password_set\":true"));
        assert!(status.contains("\"public_key\":\"PUBLIC\""));
        assert!(status.contains("\"key_set\":true") && status.contains("\"team_id\":\"TEAM42\""), "{status}");
        assert!(status.contains("\"configured\":[\"pushover\",\"email\"]"), "{status}");
        let both = Reach { browsers: 2, iphones: 1, ..Default::default() };
        assert_eq!(settings.configured(both), [Method::Browser, Method::Ios, Method::Pushover, Method::Email]);
        let none = Reach::default();
        assert!(settings.check(Method::Desktop, none).unwrap_err().contains("Cedar app"));
        let with_app = NotificationSettings { desktop_connected_at: Some(Utc::now()), ..settings.clone() };
        assert_eq!(with_app.configured(none), [Method::Desktop, Method::Pushover, Method::Email]);
        assert!(settings.check(Method::Pushover, none).is_ok());
        assert!(settings.check(Method::Browser, none).unwrap_err().contains("no browser"));
        assert!(settings.check(Method::Browser, Reach { browsers: 1, ..Default::default() }).is_ok());
        assert!(settings.check(Method::Ios, none).unwrap_err().contains("no iPhone"));
        assert!(settings.check(Method::Ios, both).is_ok());
        assert!(NotificationSettings::default().check(Method::Ios, both).unwrap_err().contains("team id"));
        assert!(NotificationSettings::default().check(Method::Email, none).unwrap_err().contains("SMTP"));
    }

    /// A service account's JSON file as Firebase gives it, with a new RSA key.
    fn service_account() -> (String, PKey<Private>) {
        let key = PKey::from_rsa(openssl::rsa::Rsa::generate(2048).unwrap()).unwrap();
        let pem = String::from_utf8(key.private_key_to_pem_pkcs8().unwrap()).unwrap();
        let json = json!({
            "type": "service_account",
            "project_id": "cedar-test",
            "private_key_id": "kid-1",
            "private_key": pem,
            "client_email": "fcm@cedar-test.iam.gserviceaccount.com",
            "token_uri": "https://oauth2.googleapis.com/token",
        });
        (json.to_string(), key)
    }

    #[test]
    fn android_needs_a_phone_and_for_fcm_the_key() {
        let (json, key) = service_account();
        let none = NotificationSettings::default();
        let fcm_phone = Reach { androids: 1, ..Default::default() };
        let ntfy_phone = Reach { androids: 1, unifiedpush: 1, ..Default::default() };
        assert!(none.check(Method::Android, Reach::default()).unwrap_err().contains("no Android phone"));
        assert!(none.check(Method::Android, fcm_phone).unwrap_err().contains("service-account key"));
        assert!(none.check(Method::Android, ntfy_phone).is_ok(), "UnifiedPush needs no key");
        let with_key = NotificationSettings { android: AndroidSettings { service_account: json.clone() }, ..Default::default() };
        assert!(with_key.check(Method::Android, fcm_phone).is_ok());
        assert_eq!(with_key.configured(fcm_phone), [Method::Android]);
        let status = with_key.status(&[], &[], &[]);
        assert_eq!(status["android"]["project_id"], "cedar-test");
        assert_eq!((status["android"]["fcm_configured"].as_bool(), status["android"]["key_set"].as_bool()), (Some(true), Some(true)));
        assert!(!status.to_string().contains("PRIVATE KEY"), "the key stays on the server");
        let status = none.status(&[], &[], &[]);
        assert_eq!((status["android"]["configured"].as_bool(), status["android"]["project_id"].as_str()), (Some(false), Some("")));

        // The assertion Google trades for an access token: RS256, signed with the account's key.
        let account = ServiceAccount::parse(&json).unwrap();
        let jwt = account.assertion().unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        use base64::Engine;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header: Value = serde_json::from_slice(&b64.decode(parts[0]).unwrap()).unwrap();
        assert_eq!((header["alg"].as_str(), header["kid"].as_str()), (Some("RS256"), Some("kid-1")));
        let claims: Value = serde_json::from_slice(&b64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "fcm@cedar-test.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], FCM_SCOPE);
        assert_eq!(claims["aud"], GOOGLE_TOKEN_URL);
        assert_eq!(claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(), 3600);
        let mut verifier = openssl::sign::Verifier::new(MessageDigest::sha256(), &key).unwrap();
        verifier.update(format!("{}.{}", parts[0], parts[1]).as_bytes()).unwrap();
        assert!(verifier.verify(&b64.decode(parts[2]).unwrap()).unwrap(), "the signature checks out");

        assert!(ServiceAccount::parse("{}").unwrap_err().contains("JSON file"));
        let no_pem = json!({ "project_id": "p", "client_email": "e", "private_key": "nope" }).to_string();
        assert!(ServiceAccount::parse(&no_pem).unwrap_err().contains("PEM"));
        let odd = json!({ "project_id": "p/../x", "client_email": "e", "private_key": "-----BEGIN PRIVATE KEY-----" }).to_string();
        assert!(ServiceAccount::parse(&odd).unwrap_err().contains("project id"));
        let bad_key = ServiceAccount::parse(&json!({ "project_id": "p", "client_email": "e", "private_key": "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n" }).to_string()).unwrap();
        assert!(bad_key.signing_key().unwrap_err().contains("RSA key"));
    }

    #[test]
    fn android_pushes_carry_no_text() {
        let id = Uuid::now_v7();
        let message = Message { title: "Secret plans: finished".into(), body: "the plan is…".into(), url: "http://x/#/c/1".into(), tag: "t-1".into(), conversation_id: Some(id), open_label: "Open in Cedar".into() };
        assert_eq!(android_payload(&message), json!({ "kind": "turn", "conversation_id": id.to_string(), "tag": "t-1" }));
        let test = Message { conversation_id: None, ..message };
        assert_eq!(android_payload(&test), json!({ "kind": "test", "conversation_id": "", "tag": "t-1" }));
    }

    #[test]
    fn provider_tokens_are_signed_and_reused() {
        let apns = Apns::new(Some("http://127.0.0.1:1".into())).unwrap();
        let key = ES256KeyPair::generate();
        let settings = ApnsSettings { team_id: "TEAM42".into(), key_id: "KEY42".into(), private_key: key.to_pem().unwrap(), ..Default::default() };
        let token = apns.provider_token(&settings, false).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        use base64::Engine;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header: Value = serde_json::from_slice(&b64.decode(parts[0]).unwrap()).unwrap();
        assert_eq!((header["alg"].as_str(), header["kid"].as_str()), (Some("ES256"), Some("KEY42")));
        let claims: Value = serde_json::from_slice(&b64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "TEAM42");
        assert!(claims["iat"].is_number() && claims.get("exp").is_none(), "{claims}");
        assert_eq!(apns.provider_token(&settings, false).unwrap(), token, "reused while fresh");
        assert_ne!(apns.provider_token(&settings, true).unwrap(), token, "a fresh one on demand");
        let other = ApnsSettings { key_id: "KEY43".into(), ..settings.clone() };
        assert_ne!(apns.provider_token(&other, false).unwrap(), token, "a new one for a new key");
        assert_eq!(apns.url_for(&settings, "abc"), "http://127.0.0.1:1/3/device/abc");
        let apple = Apns::new(None).unwrap();
        assert_eq!(apple.url_for(&other, "abc"), "https://api.push.apple.com/3/device/abc");
        let bad = ApnsSettings { private_key: "nope".into(), ..settings };
        assert!(apple.provider_token(&bad, false).unwrap_err().contains(".p8"));
    }

    #[test]
    fn excerpts_are_cut() {
        assert_eq!(excerpt("  short  "), "short");
        let long = "word ".repeat(100);
        let cut = excerpt(&long);
        assert!(cut.ends_with('…') && cut.chars().count() <= 301);
    }
}
