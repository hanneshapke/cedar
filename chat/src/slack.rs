//! Talking to Cedar from Slack.
//!
//! A Slack app is the way in: its tokens set in the environment (`CHAT_SLACK_APP_TOKEN` and
//! `CHAT_SLACK_BOT_TOKEN`), or saved by an admin in Settings -> Slack ([`SlackHub`]). Its events come
//! over Slack's Socket Mode: this service opens a WebSocket to Slack and Slack sends each event
//! down it, so Slack never needs to reach this server and no public address is involved.
//!
//! - **Who.** A Slack account is a person here once it is linked: the app sends an account it
//!   doesn't know a code, and the person enters it in Settings -> Slack while signed in here.
//!   Each side has then shown it is who it says. Nothing anyone else in the workspace writes
//!   reaches Cedar.
//! - **What.** A message to the app (a direct message) or one that mentions it in a channel
//!   starts a conversation, which is that Slack thread: its replies continue the conversation,
//!   the way a message on the page does (`api::post_message`), and the conversation is listed
//!   on the page like any other. Only the person who started a thread continues it.
//! - **While a run goes**, the turn's event stream is followed (`watch`): a plan to agree to and a
//!   tool call to approve are posted in the thread with buttons, and only the person who asked
//!   can press them. The answer is posted when the run ends, whichever of `watch` and `notify`
//!   learns of it first (`turn_finished`; the database makes sure it is posted once).
//!
//! The run is an interactive one, the same as a message on the page: what the person asks is
//! their own prompt, so it isn't fenced off as data the way a webhook's body is.

use crate::api::{self, AppState, ConversationRef, NewMessage};
use crate::cedar;
use crate::config::SlackConfig;
use crate::error::AppError;
use crate::i18n::Lang;
use crate::store::{SlackTurn, Turn};
use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How long a link code may be entered.
pub const CODE_TTL: Duration = Duration::from_secs(15 * 60);
/// The codes waiting to be entered at once; the oldest goes first beyond this.
const MAX_CODES: usize = 1000;
/// The event ids remembered, so an event Slack sends again isn't handled twice.
const MAX_SEEN: usize = 2000;
/// Slack shows up to 40,000 characters of a message but asks for under 4,000.
const MAX_CHUNK_CHARS: usize = 3800;
/// A section block's text is at most 3,000 characters.
const MAX_SECTION_CHARS: usize = 2900;
/// How much of a tool call's input an approval shows.
const MAX_INPUT_CHARS: usize = 600;
/// Without anything from Slack for this long the connection is pinged, and after twice this it is
/// given up and opened again.
const QUIET: Duration = Duration::from_secs(60);
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// The letters of a link code: no 0/O or 1/I, so it can be read off a screen.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

const ACTION_ALLOW: &str = "cedar_allow";
const ACTION_ALWAYS: &str = "cedar_allow_always";
const ACTION_DENY: &str = "cedar_deny";
const ACTION_PLAN_GO: &str = "cedar_plan_go";
const ACTION_PLAN_STOP: &str = "cedar_plan_stop";

/// The Slack app in use. Tokens set in the environment are the app for as long as the server runs;
/// without them, an admin saves (or removes) the app's tokens in Settings -> Slack, and the
/// connection follows (see `start`).
pub struct SlackHub {
    /// The app from the environment, which Settings can't change.
    env: bool,
    api_url: String,
    current: std::sync::RwLock<Option<Arc<Slack>>>,
    /// Bumped each time the app changes, so the connection is opened again with it.
    changed: tokio::sync::watch::Sender<u64>,
}

/// Where the Slack app's tokens come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    Settings,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Env => "env",
            Source::Settings => "settings",
        }
    }
}

impl SlackHub {
    pub fn new(env: Option<SlackConfig>, api_url: String) -> Result<Self> {
        let current = env.clone().map(Slack::new).transpose()?.map(Arc::new);
        Ok(Self { env: env.is_some(), api_url, current: std::sync::RwLock::new(current), changed: tokio::sync::watch::channel(0).0 })
    }

    /// The app, if there is one.
    pub fn get(&self) -> Option<Arc<Slack>> {
        self.current.read().unwrap().clone()
    }

    pub fn source(&self) -> Option<Source> {
        match (self.env, self.get().is_some()) {
            (true, _) => Some(Source::Env),
            (false, true) => Some(Source::Settings),
            (false, false) => None,
        }
    }

    fn set(&self, slack: Option<Arc<Slack>>) {
        *self.current.write().unwrap() = slack;
        self.changed.send_modify(|n| *n += 1);
    }

    /// Take up the tokens saved in Settings, unless the environment has some.
    pub async fn load_saved(&self, store: &crate::store::Store) -> Result<()> {
        if self.env {
            return Ok(());
        }
        if let Some((app_token, bot_token)) = store.slack_app().await? {
            self.set(Some(Arc::new(Slack::new(SlackConfig { app_token, bot_token, api_url: self.api_url.clone() })?)));
        }
        Ok(())
    }

    /// Check a pair of tokens with Slack (the bot token with `auth.test`, the app-level token by
    /// opening a connection it then doesn't use), save them, and connect with them. Returns who
    /// the bot is. Refused while the environment sets the app.
    pub async fn save(&self, store: &crate::store::Store, app_token: &str, bot_token: &str) -> Result<Me, TokenError> {
        if self.env {
            return Err(TokenError::FromEnv);
        }
        let (app_token, bot_token) = (app_token.trim(), bot_token.trim());
        if !app_token.starts_with("xapp-") {
            return Err(TokenError::Invalid("the app-level token starts with xapp-".into()));
        }
        if !bot_token.starts_with("xoxb-") {
            return Err(TokenError::Invalid("the bot token starts with xoxb-".into()));
        }
        let slack = Slack::new(SlackConfig { app_token: app_token.into(), bot_token: bot_token.into(), api_url: self.api_url.clone() }).map_err(TokenError::Other)?;
        let me = slack.identify().await.map_err(|e| TokenError::Invalid(format!("Slack refused the bot token: {}", slack_reason(&e))))?;
        slack
            .open_connection()
            .await
            .map_err(|e| TokenError::Invalid(format!("Slack refused the app-level token (it needs the connections:write scope, and Socket Mode on): {}", slack_reason(&e))))?;
        store.set_slack_app(Some((app_token, bot_token))).await.map_err(TokenError::Other)?;
        self.set(Some(Arc::new(slack)));
        Ok(me)
    }

    /// Forget the tokens saved in Settings and disconnect. Refused while the environment sets the app.
    pub async fn remove(&self, store: &crate::store::Store) -> Result<(), TokenError> {
        if self.env {
            return Err(TokenError::FromEnv);
        }
        store.set_slack_app(None).await.map_err(TokenError::Other)?;
        self.set(None);
        Ok(())
    }
}

/// Why tokens couldn't be saved.
#[derive(Debug)]
pub enum TokenError {
    /// The environment sets the app.
    FromEnv,
    /// Not tokens Slack takes, and why.
    Invalid(String),
    Other(anyhow::Error),
}

/// Slack's own reason (`invalid_auth`, `not_allowed_token_type`, ...) or the error as it is.
fn slack_reason(e: &anyhow::Error) -> String {
    let text = e.to_string();
    text.rsplit("failed: ").next().unwrap_or(&text).to_string()
}

/// The Slack app as this service sees it.
pub struct Slack {
    cfg: SlackConfig,
    http: reqwest::Client,
    /// Who the bot is, from `auth.test` (filled once connected).
    me: RwLock<Option<Me>>,
    /// The link codes sent and not yet entered, by code.
    codes: Mutex<HashMap<String, PendingLink>>,
    /// The latest event ids handled, oldest first.
    seen: Mutex<(VecDeque<String>, HashSet<String>)>,
    /// The turns being watched, so a turn is followed once.
    watching: Mutex<HashSet<Uuid>>,
}

/// The bot's own identity in the workspace.
#[derive(Debug, Clone)]
pub struct Me {
    pub user_id: String,
    pub team: String,
    pub team_name: Option<String>,
    pub name: Option<String>,
}

/// A Slack account that asked to be linked and was sent a code.
#[derive(Debug, Clone)]
pub struct PendingLink {
    pub team: String,
    pub team_name: Option<String>,
    pub slack_user: String,
    pub name: Option<String>,
    expires: Instant,
}

/// A message someone wrote to the app.
#[derive(Debug, Clone, PartialEq)]
struct Incoming {
    team: String,
    channel: String,
    user: String,
    text: String,
    ts: String,
    thread_ts: Option<String>,
    /// It mentions the app.
    mentioned: bool,
    /// It is a direct message to the app.
    direct: bool,
}

impl Incoming {
    /// The thread it belongs to: its own ts when it starts one.
    fn root(&self) -> &str {
        self.thread_ts.as_deref().unwrap_or(&self.ts)
    }
}

impl Slack {
    pub fn new(cfg: SlackConfig) -> Result<Self> {
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).build().context("building HTTP client")?;
        Ok(Self {
            cfg,
            http,
            me: RwLock::new(None),
            codes: Mutex::default(),
            seen: Mutex::default(),
            watching: Mutex::default(),
        })
    }

    /// Who the bot is, once connected.
    pub async fn me(&self) -> Option<Me> {
        self.me.read().await.clone()
    }

    // -- Slack's Web API ------------------------------------------------------------------------

    /// Call a Web API method with the bot token and a JSON body. A rate limit is waited out once.
    async fn call(&self, method: &str, body: &Value) -> Result<Value> {
        self.send(method, |req| req.bearer_auth(&self.cfg.bot_token).header("content-type", "application/json; charset=utf-8").body(body.to_string()))
            .await
    }

    /// Call a method that reads, with its arguments in the query.
    async fn call_get(&self, method: &str, query: &[(&str, &str)]) -> Result<Value> {
        let url = format!("{}/{method}", self.cfg.api_url);
        for attempt in 0..2 {
            let resp = self.http.get(&url).bearer_auth(&self.cfg.bot_token).query(query).timeout(CALL_TIMEOUT).send().await?;
            match self.answer(method, resp).await? {
                Some(v) => return Ok(v),
                None if attempt == 0 => continue,
                None => break,
            }
        }
        bail!("Slack kept rate-limiting {method}")
    }

    async fn send(&self, method: &str, build: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder) -> Result<Value> {
        let url = format!("{}/{method}", self.cfg.api_url);
        for attempt in 0..2 {
            let resp = build(self.http.post(&url)).timeout(CALL_TIMEOUT).send().await?;
            match self.answer(method, resp).await? {
                Some(v) => return Ok(v),
                None if attempt == 0 => continue,
                None => break,
            }
        }
        bail!("Slack kept rate-limiting {method}")
    }

    /// Slack's answer, or `None` after waiting out a rate limit (the caller tries once more).
    async fn answer(&self, method: &str, resp: reqwest::Response) -> Result<Option<Value>> {
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(1)
                .min(30);
            tokio::time::sleep(Duration::from_secs(wait)).await;
            return Ok(None);
        }
        let status = resp.status();
        let body: Value = resp.json().await.with_context(|| format!("Slack's {method} answered {status} without JSON"))?;
        if body["ok"] != true {
            bail!("Slack's {method} failed: {}", body["error"].as_str().unwrap_or("no reason given"));
        }
        Ok(Some(body))
    }

    /// Learn who the bot is (`auth.test`).
    async fn identify(&self) -> Result<Me> {
        let who = self.send("auth.test", |req| req.bearer_auth(&self.cfg.bot_token)).await?;
        let me = Me {
            user_id: who["user_id"].as_str().ok_or_else(|| anyhow!("auth.test named no bot user"))?.to_string(),
            team: who["team_id"].as_str().unwrap_or_default().to_string(),
            team_name: who["team"].as_str().map(str::to_string),
            name: who["user"].as_str().map(str::to_string),
        };
        *self.me.write().await = Some(me.clone());
        Ok(me)
    }

    /// The WebSocket address of a new Socket Mode connection (`apps.connections.open`, with the
    /// app-level token).
    async fn open_connection(&self) -> Result<String> {
        let opened = self.send("apps.connections.open", |req| req.bearer_auth(&self.cfg.app_token)).await?;
        opened["url"].as_str().map(str::to_string).ok_or_else(|| anyhow!("apps.connections.open gave no url"))
    }

    /// Post `text` (mrkdwn) in a channel or thread, with `blocks` if given. Returns its ts.
    pub async fn post(&self, channel: &str, thread_ts: Option<&str>, text: &str, blocks: Option<&Value>) -> Result<String> {
        let mut body = json!({ "channel": channel, "text": text, "unfurl_links": false, "unfurl_media": false });
        if let Some(ts) = thread_ts {
            body["thread_ts"] = json!(ts);
        }
        if let Some(blocks) = blocks {
            body["blocks"] = blocks.clone();
        }
        let posted = self.call("chat.postMessage", &body).await?;
        Ok(posted["ts"].as_str().unwrap_or_default().to_string())
    }

    /// A message only `user` sees.
    async fn post_ephemeral(&self, channel: &str, user: &str, thread_ts: Option<&str>, text: &str) -> Result<()> {
        let mut body = json!({ "channel": channel, "user": user, "text": text });
        if let Some(ts) = thread_ts {
            body["thread_ts"] = json!(ts);
        }
        self.call("chat.postEphemeral", &body).await.map(|_| ())
    }

    async fn update(&self, channel: &str, ts: &str, text: &str, blocks: &Value) -> Result<()> {
        self.call("chat.update", &json!({ "channel": channel, "ts": ts, "text": text, "blocks": blocks })).await.map(|_| ())
    }

    /// Add or take off a reaction. Best effort: the app may lack `reactions:write`.
    async fn react(&self, channel: &str, ts: &str, name: &str, on: bool) {
        let method = if on { "reactions.add" } else { "reactions.remove" };
        if let Err(e) = self.call(method, &json!({ "channel": channel, "timestamp": ts, "name": name })).await {
            tracing::debug!(error = %e, "could not change a reaction in Slack");
        }
    }

    /// A Slack account's name and language, as far as Slack says.
    async fn profile(&self, user: &str) -> (Option<String>, Option<String>) {
        match self.call_get("users.info", &[("user", user), ("include_locale", "true")]).await {
            Ok(info) => {
                let u = &info["user"];
                let name = [&u["profile"]["display_name"], &u["real_name"], &u["name"]]
                    .into_iter()
                    .filter_map(|v| v.as_str())
                    .find(|s| !s.trim().is_empty())
                    .map(str::to_string);
                (name, u["locale"].as_str().map(str::to_string))
            }
            Err(e) => {
                tracing::debug!(error = %e, "could not look up a Slack user");
                (None, None)
            }
        }
    }

    // -- link codes -----------------------------------------------------------------------------

    /// The code for a Slack account to enter here: the one it was already sent while that is
    /// still good, else a new one.
    fn code_for(&self, team: &str, team_name: Option<&str>, slack_user: &str, name: Option<&str>) -> String {
        let mut codes = self.codes.lock().unwrap();
        let now = Instant::now();
        codes.retain(|_, p| p.expires > now);
        if let Some((code, _)) = codes.iter().find(|(_, p)| p.team == team && p.slack_user == slack_user) {
            return code.clone();
        }
        if codes.len() >= MAX_CODES {
            if let Some(oldest) = codes.iter().min_by_key(|(_, p)| p.expires).map(|(c, _)| c.clone()) {
                codes.remove(&oldest);
            }
        }
        let code = loop {
            let code = new_code();
            if !codes.contains_key(&code) {
                break code;
            }
        };
        codes.insert(
            code.clone(),
            PendingLink {
                team: team.to_string(),
                team_name: team_name.map(str::to_string),
                slack_user: slack_user.to_string(),
                name: name.map(str::to_string),
                expires: now + CODE_TTL,
            },
        );
        code
    }

    /// Take a code someone entered: the account it was sent to, once, if it is still good.
    pub fn redeem(&self, entered: &str) -> Option<PendingLink> {
        let code = normalize_code(entered)?;
        let mut codes = self.codes.lock().unwrap();
        let pending = codes.remove(&code)?;
        (pending.expires > Instant::now()).then_some(pending)
    }

    /// Whether an event was handled already; notes it if not.
    fn seen_before(&self, id: &str) -> bool {
        let mut seen = self.seen.lock().unwrap();
        let (order, set) = &mut *seen;
        if !set.insert(id.to_string()) {
            return true;
        }
        order.push_back(id.to_string());
        while order.len() > MAX_SEEN {
            if let Some(old) = order.pop_front() {
                set.remove(&old);
            }
        }
        false
    }
}

/// A new code: eight letters from `CODE_ALPHABET` (40 bits), shown as `ABCD-EFGH`.
fn new_code() -> String {
    let mut buf = [0u8; 8];
    if openssl::rand::rand_bytes(&mut buf).is_err() {
        // Never expected; a v4 UUID is random too.
        buf.copy_from_slice(&Uuid::new_v4().as_bytes()[..8]);
    }
    let letters: String = buf.iter().map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char).collect();
    format!("{}-{}", &letters[..4], &letters[4..])
}

/// A code as someone typed it (any case, with or without the dash or spaces) in its `ABCD-EFGH`
/// form; `None` for what can't be one.
fn normalize_code(entered: &str) -> Option<String> {
    let letters: String = entered.chars().filter(|c| !matches!(c, '-' | ' ')).map(|c| c.to_ascii_uppercase()).collect();
    (letters.len() == 8 && letters.bytes().all(|b| CODE_ALPHABET.contains(&b))).then(|| format!("{}-{}", &letters[..4], &letters[4..]))
}

// -- the connection -------------------------------------------------------------------------------

/// Keep a Socket Mode connection to Slack open until `stop`, opening it again when Slack ends it or
/// it breaks, and with the new app when Settings changes it. Waits while there is no app.
pub fn start(state: AppState, stop: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = state.slack.load_saved(&state.store).await {
            tracing::warn!(error = %e, "could not read the Slack app's saved tokens");
        }
        let mut changes = state.slack.changed.subscribe();
        loop {
            changes.borrow_and_update();
            let current = state.slack.get();
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = changes.changed() => continue,
                _ = async {
                    match current {
                        Some(slack) => keep_connected(&state, &slack, &stop).await,
                        None => std::future::pending().await,
                    }
                } => return,
            }
        }
    })
}

/// Connect with one app until `stop`, again and again.
async fn keep_connected(state: &AppState, slack: &Arc<Slack>, stop: &CancellationToken) {
    let mut failures: u32 = 0;
    let mut resumed = false;
    loop {
        match connect(state, slack, stop, &mut resumed).await {
            Ok(()) => failures = 0,
            Err(e) => {
                failures += 1;
                if failures == 1 || failures.is_multiple_of(10) {
                    tracing::warn!(error = %e, failures, "Slack connection failed; retrying");
                }
            }
        }
        if stop.is_cancelled() {
            return;
        }
        let wait = Duration::from_secs(1 << failures.min(6));
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// One connection: open it, acknowledge and hand off each envelope, until Slack asks to reconnect,
/// the connection ends, or `stop`.
async fn connect(state: &AppState, slack: &Arc<Slack>, stop: &CancellationToken, resumed: &mut bool) -> Result<()> {
    let me = slack.identify().await.context("checking CHAT_SLACK_BOT_TOKEN")?;
    if !*resumed {
        *resumed = true;
        tracing::info!(team = ?me.team_name, bot = ?me.name, "Slack app connected");
        resume(state, slack).await;
    }
    let url = slack.open_connection().await.context("opening a Socket Mode connection with CHAT_SLACK_APP_TOKEN")?;
    let (mut ws, _) = tokio_tungstenite::connect_async(url.as_str()).await.context("connecting to Slack's WebSocket")?;
    let mut pinged = false;
    loop {
        let next = tokio::select! {
            _ = stop.cancelled() => {
                let _ = ws.close(None).await;
                return Ok(());
            }
            next = tokio::time::timeout(QUIET, ws.next()) => next,
        };
        let msg = match next {
            Err(_) if !pinged => {
                pinged = true;
                ws.send(WsMessage::Ping(Vec::new().into())).await?;
                continue;
            }
            Err(_) => bail!("Slack's connection went quiet"),
            Ok(None) => return Ok(()),
            Ok(Some(msg)) => msg?,
        };
        pinged = false;
        let text = match msg {
            WsMessage::Text(text) => text,
            WsMessage::Close(_) => return Ok(()),
            _ => continue,
        };
        let Ok(envelope) = serde_json::from_str::<Value>(text.as_str()) else { continue };
        // Slack waits three seconds for this, so it goes before any work.
        if let Some(id) = envelope["envelope_id"].as_str() {
            ws.send(WsMessage::Text(json!({ "envelope_id": id }).to_string().into())).await?;
        }
        match envelope["type"].as_str().unwrap_or_default() {
            "disconnect" => return Ok(()),
            "events_api" => {
                let (state, slack) = (state.clone(), slack.clone());
                let payload = envelope["payload"].clone();
                tokio::spawn(async move { on_event(&state, &slack, &payload).await });
            }
            "interactive" => {
                let (state, slack) = (state.clone(), slack.clone());
                let payload = envelope["payload"].clone();
                tokio::spawn(async move { on_interaction(&state, &slack, &payload).await });
            }
            _ => {}
        }
    }
}

/// After a restart: answer the Slack turns whose runs ended meanwhile, and follow the rest again.
async fn resume(state: &AppState, slack: &Arc<Slack>) {
    let open = match state.store.unanswered_slack_turns().await {
        Ok(open) => open,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the Slack turns to resume");
            return;
        }
    };
    for (st, user) in open {
        let state = state.as_user(user);
        let Ok(Some(turn)) = state.store.get_turn(st.turn_id).await else { continue };
        match api::refresh(&state, turn).await {
            Ok(turn) if cedar::is_terminal(&turn.status) => turn_finished(&state, &turn).await,
            Ok(turn) => watch(state.clone(), slack.clone(), turn, st),
            Err(e) => tracing::warn!(turn_id = %st.turn_id, error = %e, "could not resume a Slack turn"),
        }
    }
}

// -- messages -------------------------------------------------------------------------------------

/// An event Slack sent: a message for the app, or nothing to do.
async fn on_event(state: &AppState, slack: &Arc<Slack>, payload: &Value) {
    if let Some(id) = payload["event_id"].as_str() {
        if slack.seen_before(id) {
            return;
        }
    }
    let Some(me) = slack.me().await else { return };
    let Some(incoming) = incoming(payload, &me.user_id) else { return };
    if let Err(e) = on_message(state, slack, &me, incoming.clone()).await {
        tracing::warn!(error = %e, channel = %incoming.channel, "could not handle a Slack message");
        let lang = Lang::of(None);
        let _ = slack.post(&incoming.channel, Some(incoming.root()), &texts(lang).start_failed.replace("{error}", &e.to_string()), None).await;
    }
}

/// The message in an `events_api` payload, if it is one for the app: a mention, a direct
/// message, or (to be checked against the threads) a reply in a channel thread. Edits, the
/// app's own messages and other bots' are left out.
fn incoming(payload: &Value, bot_user: &str) -> Option<Incoming> {
    let event = &payload["event"];
    let kind = event["type"].as_str()?;
    if event.get("subtype").is_some_and(|s| !s.is_null()) || event.get("bot_id").is_some_and(|b| !b.is_null()) {
        return None;
    }
    let user = event["user"].as_str()?;
    if user == bot_user {
        return None;
    }
    let team = payload["team_id"].as_str().or_else(|| event["team"].as_str())?;
    let direct = event["channel_type"].as_str() == Some("im");
    let mention = format!("<@{bot_user}");
    let text = event["text"].as_str().unwrap_or_default();
    let mentioned = match kind {
        "app_mention" => true,
        // A mention in a channel also comes as an app_mention, handled there.
        "message" if !direct && text.contains(&mention) => return None,
        "message" if !direct && event["thread_ts"].is_null() => return None,
        "message" => false,
        _ => return None,
    };
    Some(Incoming {
        team: team.to_string(),
        channel: event["channel"].as_str()?.to_string(),
        user: user.to_string(),
        text: text.to_string(),
        ts: event["ts"].as_str()?.to_string(),
        thread_ts: event["thread_ts"].as_str().map(str::to_string),
        mentioned,
        direct,
    })
}

async fn on_message(state: &AppState, slack: &Arc<Slack>, me: &Me, msg: Incoming) -> Result<()> {
    let root = msg.root().to_string();
    let thread = state.store.slack_thread(&msg.channel, &root).await?;
    let addressed = msg.mentioned || msg.direct;
    // A reply in a channel thread that isn't Cedar's, and doesn't mention the app, isn't for it.
    if thread.is_none() && !addressed {
        return Ok(());
    }

    let Some(user) = state.store.slack_user(&msg.team, &msg.user).await? else {
        if addressed {
            offer_link(slack, me, &msg).await?;
        }
        return Ok(());
    };
    let state = state.as_user(user);
    let lang = Lang::of(state.store.preferences().await?.locale.as_deref());
    let t = texts(lang);
    let content = plain_text(&msg.text, &me.user_id);

    let conversation_id = match thread {
        Some(thread) if thread.user_id == user && thread.slack_user == msg.user && thread.team == msg.team => thread.conversation_id,
        Some(_) => {
            if addressed {
                slack.post_ephemeral(&msg.channel, &msg.user, Some(&root), t.not_yours).await?;
            }
            return Ok(());
        }
        None if content.is_empty() => {
            slack.post(&msg.channel, Some(&root), t.help, None).await?;
            return Ok(());
        }
        None => {
            let conversation = state.store.create_conversation(Some(&crate::store::title_from(&content))).await?;
            state.store.add_slack_thread(&msg.channel, &root, conversation.id, &msg.team, &msg.user).await?;
            conversation.id
        }
    };

    if content.is_empty() {
        slack.post(&msg.channel, Some(&root), t.help, None).await?;
        return Ok(());
    }
    if is_stop(&content) {
        return stop_turn(&state, slack, &msg, &root, conversation_id, t).await;
    }
    let content = match api::clean_message("message", &content) {
        Ok(c) => c,
        Err(e) => {
            slack.post(&msg.channel, Some(&root), &t.start_failed.replace("{error}", &e.to_string()), None).await?;
            return Ok(());
        }
    };
    let message = NewMessage { content: &content, ..NewMessage::default() };
    match api::post_message(&state, ConversationRef::Found(conversation_id), message).await {
        Ok(turn) => {
            state.store.add_slack_turn(turn.id, &msg.channel, &root, &msg.ts).await?;
            slack.react(&msg.channel, &msg.ts, "eyes", true).await;
            let st = SlackTurn { turn_id: turn.id, channel: msg.channel.clone(), thread_ts: root, message_ts: msg.ts.clone(), answered: false };
            watch(state.clone(), slack.clone(), turn, st);
        }
        Err(AppError::Conflict(_)) => {
            slack.post(&msg.channel, Some(&root), t.busy, None).await?;
        }
        Err(e) => {
            slack.post(&msg.channel, Some(&root), &t.start_failed.replace("{error}", &e.to_string()), None).await?;
        }
    }
    Ok(())
}

/// Tell a Slack account the app doesn't know how to link it: in the direct message, or only to
/// them in a channel.
async fn offer_link(slack: &Slack, me: &Me, msg: &Incoming) -> Result<()> {
    let (name, locale) = slack.profile(&msg.user).await;
    let team_name = (me.team == msg.team).then(|| me.team_name.clone()).flatten();
    let code = slack.code_for(&msg.team, team_name.as_deref(), &msg.user, name.as_deref());
    let text = texts(Lang::of(locale.as_deref())).link_offer.replace("{code}", &code);
    if msg.direct {
        slack.post(&msg.channel, Some(msg.root()), &text, None).await?;
    } else {
        slack.post_ephemeral(&msg.channel, &msg.user, Some(msg.root()), &text).await?;
    }
    Ok(())
}

/// "stop" in a thread: cancel its running turn.
async fn stop_turn(state: &AppState, slack: &Slack, msg: &Incoming, root: &str, conversation_id: Uuid, t: &Texts) -> Result<()> {
    let last = state.store.list_turns(conversation_id).await?.into_iter().last();
    let running = match last {
        Some(turn) => Some(api::refresh(state, turn).await?).filter(|t| !cedar::is_terminal(&t.status)),
        None => None,
    };
    match running {
        Some(turn) => {
            state.cedar.cancel_run(turn.run_id).await?;
            slack.react(&msg.channel, &msg.ts, "ok_hand", true).await;
        }
        None => {
            slack.post(&msg.channel, Some(root), t.nothing_to_stop, None).await?;
        }
    }
    Ok(())
}

fn is_stop(text: &str) -> bool {
    let word = text.trim().trim_end_matches(['.', '!']).to_lowercase();
    matches!(word.as_str(), "stop" | "cancel" | "stopp" | "abbrechen" | "arrête" | "arrete" | "annuler" | "para" | "parar" | "cancelar")
}

// -- following a run --------------------------------------------------------------------------------

/// Follow a Slack turn's run in the background: post its plan and approvals in the thread as they
/// come, and the answer when it ends. `state` is the turn's owner's.
fn watch(state: AppState, slack: Arc<Slack>, turn: Turn, st: SlackTurn) {
    if !slack.watching.lock().unwrap().insert(turn.id) {
        return;
    }
    tokio::spawn(async move {
        let turn_id = turn.id;
        let shutdown = state.shutdown.clone();
        tokio::select! {
            _ = shutdown.cancelled() => {}
            _ = follow(&state, &slack, &turn, &st) => {}
        }
        slack.watching.lock().unwrap().remove(&turn_id);
    });
}

async fn follow(state: &AppState, slack: &Slack, turn: &Turn, st: &SlackTurn) {
    let mut after: i64 = 0;
    let mut failures: u32 = 0;
    let t = texts(Lang::of(state.store.preferences().await.ok().and_then(|p| p.locale).as_deref()));
    loop {
        match follow_once(state, slack, turn, st, t, &mut after).await {
            Ok(true) => break,
            Ok(false) => failures = 0,
            Err(e) => {
                failures += 1;
                if failures == 1 {
                    tracing::warn!(turn_id = %turn.id, error = %e, "could not follow a Slack turn's run; retrying");
                }
            }
        }
        // The stream closed early, or broke: the run may have ended meanwhile.
        if let Ok(fresh) = api::refresh(state, turn.clone()).await {
            if cedar::is_terminal(&fresh.status) {
                break;
            }
        }
        if failures > 20 {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1 << failures.min(5))).await;
    }
    if let Ok(fresh) = api::refresh(state, turn.clone()).await {
        turn_finished(state, &fresh).await;
    }
}

/// Read the run's events from `after` on. `true` once the run's last event has come.
async fn follow_once(state: &AppState, slack: &Slack, turn: &Turn, st: &SlackTurn, t: &Texts, after: &mut i64) -> Result<bool> {
    let resp = state.cedar.events(turn.run_id, *after).await.map_err(|e| anyhow!("{e}"))?;
    let mut body = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = body.next().await {
        buf.extend_from_slice(&chunk?);
        while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = buf.drain(..end + 2).collect();
            let Some(event) = crate::notify::parse_sse(&block) else { continue };
            if let Some(seq) = event["seq"].as_i64() {
                *after = (*after).max(seq);
            }
            let kind = event["type"].as_str().unwrap_or_default();
            if let Err(e) = on_run_event(state, slack, turn, st, t, kind, &event["data"]).await {
                tracing::warn!(turn_id = %turn.id, event = kind, error = %e, "could not post a run's event to Slack");
            }
            if matches!(kind, "run.completed" | "run.failed" | "run.cancelled") {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn on_run_event(state: &AppState, slack: &Slack, turn: &Turn, st: &SlackTurn, t: &Texts, kind: &str, data: &Value) -> Result<()> {
    match kind {
        "plan" if data["awaiting"] == true => {
            if state.store.slack_prompt(turn.id, "plan").await?.is_some() {
                return Ok(());
            }
            let (text, blocks) = plan_message(t, turn.id, &data["steps"], None);
            let ts = slack.post(&st.channel, Some(&st.thread_ts), &text, Some(&blocks)).await?;
            state.store.set_slack_prompt(turn.id, "plan", &ts, None).await?;
        }
        "plan.updated" => {
            if let Some((ts, _)) = state.store.slack_prompt(turn.id, "plan").await? {
                let note = if data["by"] == "timeout" { t.plan_timeout } else { t.plan_agreed };
                let (text, blocks) = plan_message(t, turn.id, &data["steps"], Some(note));
                slack.update(&st.channel, &ts, &text, &blocks).await?;
            }
        }
        "approval.required" => {
            let Some(id) = data["id"].as_str() else { return Ok(()) };
            let key = format!("approval:{id}");
            if state.store.slack_prompt(turn.id, &key).await?.is_some() {
                return Ok(());
            }
            let (text, blocks) = approval_message(t, turn.id, data, None);
            let ts = slack.post(&st.channel, Some(&st.thread_ts), &text, Some(&blocks)).await?;
            // The request is shown again with the decision.
            state.store.set_slack_prompt(turn.id, &key, &ts, Some(data)).await?;
        }
        "approval.resolved" => {
            let Some(id) = data["id"].as_str() else { return Ok(()) };
            let Some((ts, request)) = state.store.slack_prompt(turn.id, &format!("approval:{id}")).await? else { return Ok(()) };
            let request = request.unwrap_or_else(|| json!({ "id": id }));
            let note = match data["decision"].as_str().unwrap_or_default() {
                "allow" => t.allowed,
                "allow_always" => t.always_allowed,
                "deny" => t.denied,
                _ => t.timed_out,
            };
            let (text, blocks) = approval_message(t, turn.id, &request, Some(note));
            slack.update(&st.channel, &ts, &text, &blocks).await?;
        }
        _ => {}
    }
    Ok(())
}

/// A plan to agree to, with Go ahead / Stop; or, with `note`, as it was settled.
fn plan_message(t: &Texts, turn_id: Uuid, steps: &Value, note: Option<&str>) -> (String, Value) {
    let list: Vec<String> = steps
        .as_array()
        .map(|s| s.iter().enumerate().map(|(i, step)| format!("{}. {}", i + 1, escape(step["title"].as_str().unwrap_or_default()))).collect())
        .unwrap_or_default();
    let text = clip(&format!("*{}*\n{}", t.plan_title, list.join("\n")), MAX_SECTION_CHARS);
    let mut blocks = vec![json!({ "type": "section", "text": { "type": "mrkdwn", "text": text } })];
    match note {
        None => blocks.push(json!({
            "type": "actions",
            "elements": [
                button(t.go_ahead, ACTION_PLAN_GO, &turn_id.to_string(), Some("primary")),
                button(t.stop, ACTION_PLAN_STOP, &turn_id.to_string(), Some("danger")),
            ],
        })),
        Some(note) => blocks.push(json!({ "type": "context", "elements": [{ "type": "mrkdwn", "text": note }] })),
    }
    (text, Value::Array(blocks))
}

/// A tool call to approve, with Allow / Always / Deny; or, with `note`, as it was decided.
fn approval_message(t: &Texts, turn_id: Uuid, request: &Value, note: Option<&str>) -> (String, Value) {
    let tool = request["tool"].as_str().unwrap_or("a tool");
    let mut text = format!("*{}*", t.approval_title.replace("{tool}", &escape(tool)));
    if let Some(reason) = request["reason"].as_str().filter(|r| !r.trim().is_empty()) {
        text.push('\n');
        text.push_str(&escape(reason));
    }
    if !request["input"].is_null() {
        let input = serde_json::to_string_pretty(&request["input"]).unwrap_or_default();
        text.push_str(&format!("\n```{}```", escape(&clip(&input, MAX_INPUT_CHARS)).replace("```", "'''")));
    }
    let text = clip(&text, MAX_SECTION_CHARS);
    let mut blocks = vec![json!({ "type": "section", "text": { "type": "mrkdwn", "text": text } })];
    let value = format!("{turn_id}|{}", request["id"].as_str().unwrap_or_default());
    match note {
        None => {
            let mut buttons = vec![button(t.allow, ACTION_ALLOW, &value, Some("primary"))];
            // "Always" remembers the call's scope; a call without one can only be allowed once.
            if request["scope"].as_str().is_some_and(|s| !s.is_empty()) || request["scope"].as_array().is_some_and(|s| !s.is_empty()) {
                buttons.push(button(t.always, ACTION_ALWAYS, &value, None));
            }
            buttons.push(button(t.deny, ACTION_DENY, &value, Some("danger")));
            blocks.push(json!({ "type": "actions", "elements": buttons }));
        }
        Some(note) => blocks.push(json!({ "type": "context", "elements": [{ "type": "mrkdwn", "text": note }] })),
    }
    (text, Value::Array(blocks))
}

fn button(label: &str, action: &str, value: &str, style: Option<&str>) -> Value {
    let mut b = json!({ "type": "button", "text": { "type": "plain_text", "text": label }, "action_id": action, "value": value });
    if let Some(style) = style {
        b["style"] = json!(style);
    }
    b
}

/// A button pressed on a plan or an approval.
async fn on_interaction(state: &AppState, slack: &Arc<Slack>, payload: &Value) {
    if payload["type"] != "block_actions" {
        return;
    }
    let user = payload["user"]["id"].as_str().unwrap_or_default();
    let team = payload["team"]["id"].as_str().or_else(|| payload["user"]["team_id"].as_str()).unwrap_or_default();
    let channel = payload["channel"]["id"].as_str().unwrap_or_default();
    let thread = payload["message"]["thread_ts"].as_str().or_else(|| payload["container"]["thread_ts"].as_str());
    let Some(action) = payload["actions"].get(0) else { return };
    let action_id = action["action_id"].as_str().unwrap_or_default();
    let value = action["value"].as_str().unwrap_or_default();
    if let Err(e) = on_button(state, slack, team, user, channel, thread, action_id, value).await {
        tracing::warn!(error = %e, action = action_id, "could not handle a Slack button");
        let _ = slack.post_ephemeral(channel, user, thread, &texts(Lang::En).start_failed.replace("{error}", &e.to_string())).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn on_button(state: &AppState, slack: &Slack, team: &str, user: &str, channel: &str, thread: Option<&str>, action_id: &str, value: &str) -> Result<()> {
    let (turn_part, approval_id) = value.split_once('|').unwrap_or((value, ""));
    let Ok(turn_id) = Uuid::parse_str(turn_part) else { return Ok(()) };
    let Some(owner) = state.store.slack_user(team, user).await? else {
        return slack.post_ephemeral(channel, user, thread, texts(Lang::En).only_owner).await;
    };
    let state = state.as_user(owner);
    let t = texts(Lang::of(state.store.preferences().await?.locale.as_deref()));
    // Only the person whose turn it is: another user's turn isn't found for them.
    let Some(turn) = state.store.get_turn(turn_id).await? else {
        return slack.post_ephemeral(channel, user, thread, t.only_owner).await;
    };
    let answer = match action_id {
        ACTION_ALLOW | ACTION_ALWAYS | ACTION_DENY => {
            let decision = match action_id {
                ACTION_ALLOW => "allow",
                ACTION_ALWAYS => "allow_always",
                _ => "deny",
            };
            state.cedar.decide_approval(turn.run_id, approval_id, &json!({ "decision": decision })).await.map(|_| ())
        }
        ACTION_PLAN_GO => match last_plan(&state, turn.run_id).await? {
            Some(steps) => state.cedar.answer_plan(turn.run_id, &json!({ "steps": steps })).await.map(|_| ()),
            None => Err(AppError::Conflict("no plan".into())),
        },
        ACTION_PLAN_STOP => state.cedar.cancel_run(turn.run_id).await,
        _ => return Ok(()),
    };
    match answer {
        Ok(()) => Ok(()),
        Err(AppError::Conflict(_)) | Err(AppError::Upstream { status: reqwest::StatusCode::CONFLICT, .. }) => {
            slack.post_ephemeral(channel, user, thread, t.already_answered).await
        }
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// The steps of the latest plan a run proposed, as cedar takes them back.
async fn last_plan(state: &AppState, run_id: Uuid) -> Result<Option<Value>> {
    let trace = state.cedar.trace(run_id).await.map_err(|e| anyhow!("{e}"))?;
    let steps = trace["events"]
        .as_array()
        .and_then(|events| events.iter().rev().find(|e| e["type"] == "plan"))
        .map(|e| e["data"]["steps"].clone())
        .filter(Value::is_array);
    Ok(steps)
}

/// Post the answer to a Slack turn whose run ended, once: whichever of `watch` and `notify`
/// gets here first. `state` is the turn's owner's. Nothing for a turn Slack didn't start.
pub async fn turn_finished(state: &AppState, turn: &Turn) {
    let Some(slack) = state.slack.get() else { return };
    if !cedar::is_terminal(&turn.status) {
        return;
    }
    let st = match state.store.slack_turn(turn.id).await {
        Ok(Some(st)) if !st.answered => st,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(turn_id = %turn.id, error = %e, "could not read a Slack turn");
            return;
        }
    };
    match state.store.claim_slack_answer(turn.id).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            tracing::warn!(turn_id = %turn.id, error = %e, "could not claim a Slack turn's answer");
            return;
        }
    }
    let t = texts(Lang::of(state.store.preferences().await.ok().and_then(|p| p.locale).as_deref()));
    let link = conversation_link(&state.cfg.public_url, turn.conversation_id, t);
    // A turn stored as finished already (it ended before it was saved) has no text yet.
    let mut turn = turn.clone();
    if turn.status == "completed" && turn.final_text.is_none() {
        if let Ok(Some(run)) = state.cedar.get_run(turn.run_id).await {
            turn.final_text = run.final_text;
            turn.stop_reason = run.stop_reason;
        }
    }
    let messages = answer_messages(t, &turn, link.as_deref());
    for text in &messages {
        if let Err(e) = slack.post(&st.channel, Some(&st.thread_ts), text, None).await {
            tracing::warn!(turn_id = %turn.id, error = %e, "could not post an answer to Slack");
            break;
        }
    }
    slack.react(&st.channel, &st.message_ts, "eyes", false).await;
    match turn.status.as_str() {
        "completed" => slack.react(&st.channel, &st.message_ts, "white_check_mark", true).await,
        "failed" => slack.react(&st.channel, &st.message_ts, "x", true).await,
        _ => {}
    }
}

/// What is posted for a finished turn: the answer in pieces Slack takes, or why there is none.
fn answer_messages(t: &Texts, turn: &Turn, link: Option<&str>) -> Vec<String> {
    let mut messages = match turn.status.as_str() {
        "completed" => {
            let text = turn.final_text.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(mrkdwn).unwrap_or_else(|| t.done.to_string());
            let mut chunks = chunks(&text, MAX_CHUNK_CHARS);
            if turn.stop_reason.as_deref() == Some("ask_user") {
                if let Some(last) = chunks.last_mut() {
                    last.push_str(&format!("\n\n_{}_", t.reply_to_answer));
                }
            }
            chunks
        }
        "cancelled" => vec![t.cancelled.to_string()],
        _ => vec![t.failed.replace("{error}", &escape(turn.error.as_deref().unwrap_or("unknown error")))],
    };
    if let (Some(link), Some(last)) = (link, messages.last_mut()) {
        if last.chars().count() + link.chars().count() + 2 <= MAX_CHUNK_CHARS {
            last.push_str("\n\n");
            last.push_str(link);
        } else {
            messages.push(link.to_string());
        }
    }
    messages
}

/// A link to the conversation on the page, when the page has an address others can open (not a
/// loopback one).
fn conversation_link(public_url: &str, conversation_id: Uuid, t: &Texts) -> Option<String> {
    let host = public_url.split("://").nth(1).unwrap_or_default().split(['/', ':']).next().unwrap_or_default();
    if matches!(host, "127.0.0.1" | "localhost" | "[" | "0.0.0.0") || host.starts_with("[::1") || host.is_empty() {
        return None;
    }
    Some(format!("<{public_url}/#/c/{conversation_id}|{}>", t.open_in_cedar))
}

// -- text -----------------------------------------------------------------------------------------

/// Slack's escaping for text it shows: `&`, `<` and `>`.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// What someone wrote in Slack as plain text: the app's own mention left out, other people and
/// channels by name, links as their address, and Slack's escaping undone.
fn plain_text(text: &str, bot_user: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let inner = &after[..end];
        rest = &after[end + 1..];
        let (target, label) = match inner.split_once('|') {
            Some((target, label)) => (target, Some(label)),
            None => (inner, None),
        };
        if let Some(user) = target.strip_prefix('@') {
            if user == bot_user {
                continue;
            }
            out.push('@');
            out.push_str(label.unwrap_or(user));
        } else if let Some(channel) = target.strip_prefix('#') {
            out.push('#');
            out.push_str(label.unwrap_or(channel));
        } else if let Some(special) = target.strip_prefix('!') {
            match label {
                Some(label) => out.push_str(label),
                None => {
                    out.push('@');
                    out.push_str(special.split('^').next().unwrap_or(special));
                }
            }
        } else if let Some(address) = target.strip_prefix("mailto:") {
            out.push_str(label.unwrap_or(address));
        } else {
            match label {
                Some(label) if label != target => {
                    out.push_str(label);
                    out.push_str(" (");
                    out.push_str(target);
                    out.push(')');
                }
                _ => out.push_str(target),
            }
        }
    }
    out.push_str(rest);
    let out = out.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
    // The mention's place leaves a space at the start, or two in the middle.
    out.split('\n').map(|l| l.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// The model's Markdown as Slack's mrkdwn: bold, strikethrough, links, headings and lists in
/// Slack's way, tables as preformatted text, code left alone.
pub fn mrkdwn(markdown: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_code = false;
    let mut table: Vec<String> = Vec::new();
    let flush_table = |out: &mut Vec<String>, table: &mut Vec<String>| {
        if !table.is_empty() {
            out.push("```".into());
            out.append(table);
            out.push("```".into());
        }
    };
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            flush_table(&mut out, &mut table);
            in_code = !in_code;
            out.push("```".into());
            continue;
        }
        if in_code {
            out.push(escape(line));
            continue;
        }
        if trimmed.starts_with('|') {
            // The |---|---| line under a table's head says nothing in plain text.
            if !trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                table.push(escape(line.trim()));
            }
            continue;
        }
        flush_table(&mut out, &mut table);
        let indent = &line[..line.len() - trimmed.len()];
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let heading = inline(trimmed[hashes..].trim()).replace('*', "");
            out.push(format!("*{heading}*"));
        } else if let Some(item) = ["- ", "* ", "+ "].iter().find_map(|m| trimmed.strip_prefix(m)) {
            out.push(format!("{indent}• {}", inline(item)));
        } else if trimmed.chars().all(|c| matches!(c, '-' | '*' | '_')) && trimmed.len() >= 3 {
            out.push("───".into());
        } else {
            out.push(format!("{indent}{}", inline(trimmed)));
        }
    }
    flush_table(&mut out, &mut table);
    if in_code {
        out.push("```".into());
    }
    out.join("\n")
}

/// One line's inline Markdown in mrkdwn, leaving `code` alone.
fn inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for (i, piece) in line.split('`').enumerate() {
        if i > 0 {
            out.push('`');
        }
        if i % 2 == 1 {
            out.push_str(&escape(piece));
            continue;
        }
        out.push_str(&inline_text(piece));
    }
    out
}

fn inline_text(text: &str) -> String {
    // Links first, while their text and address are still what was written.
    let mut linked = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let parsed = after.find("](").and_then(|close| {
            let url_start = close + 2;
            after[url_start..].find(')').map(|end| (&after[..close], &after[url_start..url_start + end], url_start + end + 1))
        });
        match parsed {
            Some((label, url, used)) if !url.contains(' ') && (url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:")) => {
                linked.push_str(&escape(&rest[..open]));
                linked.push_str(&format!("\u{0}{}|{}\u{1}", url.replace('&', "&amp;").replace('<', "%3C").replace('>', "%3E"), escape(label).replace('|', "¦")));
                rest = &after[used..];
            }
            _ => {
                linked.push_str(&escape(&rest[..open + 1]));
                rest = after;
            }
        }
    }
    linked.push_str(&escape(rest));
    let styled = linked.replace("**", "*").replace("__", "*").replace("~~", "~");
    styled.replace('\u{0}', "<").replace('\u{1}', ">")
}

/// `text` in pieces of at most `max` characters, split between lines where it can be; a code
/// block cut in two is closed and opened again.
fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_code = false;
    for line in text.split('\n') {
        let mut line = line.to_string();
        loop {
            let need = current.chars().count() + line.chars().count() + 1;
            if need <= max || current.is_empty() && line.chars().count() <= max {
                break;
            }
            if current.is_empty() {
                // One line longer than a message: cut it.
                let cut: String = line.chars().take(max - 4).collect();
                let rest: String = line.chars().skip(max - 4).collect();
                out.push(if in_code { format!("{cut}\n```") } else { cut });
                line = if in_code { format!("```\n{rest}") } else { rest };
                continue;
            }
            if in_code {
                current.push_str("\n```");
            }
            out.push(std::mem::take(&mut current));
            if in_code {
                current.push_str("```");
            }
        }
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(&line);
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

// -- the bot's own words --------------------------------------------------------------------------

/// What the app says in Slack, in the user's language (from Settings once they are linked, from
/// their Slack profile before).
pub struct Texts {
    pub link_offer: &'static str,
    pub linked: &'static str,
    not_yours: &'static str,
    busy: &'static str,
    nothing_to_stop: &'static str,
    help: &'static str,
    start_failed: &'static str,
    failed: &'static str,
    cancelled: &'static str,
    done: &'static str,
    reply_to_answer: &'static str,
    open_in_cedar: &'static str,
    plan_title: &'static str,
    go_ahead: &'static str,
    stop: &'static str,
    plan_agreed: &'static str,
    plan_timeout: &'static str,
    approval_title: &'static str,
    allow: &'static str,
    always: &'static str,
    deny: &'static str,
    allowed: &'static str,
    always_allowed: &'static str,
    denied: &'static str,
    timed_out: &'static str,
    only_owner: &'static str,
    already_answered: &'static str,
}

pub fn texts(lang: Lang) -> &'static Texts {
    match lang {
        Lang::En => &EN,
        Lang::De => &DE,
        Lang::Fr => &FR,
        Lang::Es => &ES,
    }
}

static EN: Texts = Texts {
    link_offer: "I don't know you in Cedar yet. To link your Slack account, open Cedar, go to *Settings → Slack* and enter this code within 15 minutes: *{code}*",
    linked: "Your Slack account is now linked to Cedar as *{name}*. Message me here or mention me in a channel to start. Each thread is one conversation: reply in it to continue.",
    not_yours: "This thread is someone else's conversation with Cedar. Mention me in a new message to start your own.",
    busy: "I'm still working on your last message. Wait for it to finish, or say *stop* to end it.",
    nothing_to_stop: "Nothing is running here.",
    help: "Ask me anything: mention me with a question or a task, or message me directly. Each thread is one conversation: reply in it to continue, or say *stop* to end what I'm doing.",
    start_failed: ":warning: I couldn't start on that: {error}",
    failed: ":warning: That didn't work: {error}",
    cancelled: "Stopped.",
    done: "Done.",
    reply_to_answer: "Reply in this thread to answer.",
    open_in_cedar: "Open in Cedar",
    plan_title: "Here's my plan:",
    go_ahead: "Go ahead",
    stop: "Stop",
    plan_agreed: "Going ahead with this plan.",
    plan_timeout: "No answer came, so I went ahead with this plan.",
    approval_title: "May I use {tool}?",
    allow: "Allow",
    always: "Always allow",
    deny: "Deny",
    allowed: ":white_check_mark: Allowed.",
    always_allowed: ":white_check_mark: Allowed, and from now on without asking.",
    denied: ":no_entry_sign: Denied: not done.",
    timed_out: ":hourglass: No answer came: not done.",
    only_owner: "Only the person who asked can answer this.",
    already_answered: "That was already answered.",
};

static DE: Texts = Texts {
    link_offer: "Ich kenne dich in Cedar noch nicht. Um dein Slack-Konto zu verknüpfen, öffne Cedar, gehe zu *Einstellungen → Slack* und gib innerhalb von 15 Minuten diesen Code ein: *{code}*",
    linked: "Dein Slack-Konto ist jetzt mit Cedar als *{name}* verknüpft. Schreib mir hier oder erwähne mich in einem Kanal, um zu beginnen. Jeder Thread ist ein Gespräch: Antworte darin, um weiterzumachen.",
    not_yours: "Dieser Thread ist das Gespräch einer anderen Person mit Cedar. Erwähne mich in einer neuen Nachricht, um dein eigenes zu beginnen.",
    busy: "Ich arbeite noch an deiner letzten Nachricht. Warte, bis sie fertig ist, oder schreib *stop*, um sie abzubrechen.",
    nothing_to_stop: "Hier läuft gerade nichts.",
    help: "Frag mich, was du willst: Erwähne mich mit einer Frage oder Aufgabe oder schreib mir direkt. Jeder Thread ist ein Gespräch: Antworte darin, um weiterzumachen, oder schreib *stop*, um abzubrechen.",
    start_failed: ":warning: Damit konnte ich nicht beginnen: {error}",
    failed: ":warning: Das hat nicht geklappt: {error}",
    cancelled: "Abgebrochen.",
    done: "Erledigt.",
    reply_to_answer: "Antworte in diesem Thread.",
    open_in_cedar: "In Cedar öffnen",
    plan_title: "Mein Plan:",
    go_ahead: "Los geht's",
    stop: "Abbrechen",
    plan_agreed: "Ich arbeite nach diesem Plan.",
    plan_timeout: "Es kam keine Antwort, also arbeite ich nach diesem Plan.",
    approval_title: "Darf ich {tool} verwenden?",
    allow: "Erlauben",
    always: "Immer erlauben",
    deny: "Ablehnen",
    allowed: ":white_check_mark: Erlaubt.",
    always_allowed: ":white_check_mark: Erlaubt, und ab jetzt ohne Nachfrage.",
    denied: ":no_entry_sign: Abgelehnt: nicht ausgeführt.",
    timed_out: ":hourglass: Keine Antwort: nicht ausgeführt.",
    only_owner: "Nur wer gefragt hat, kann das beantworten.",
    already_answered: "Das wurde schon beantwortet.",
};

static FR: Texts = Texts {
    link_offer: "Je ne vous connais pas encore dans Cedar. Pour associer votre compte Slack, ouvrez Cedar, allez dans *Réglages → Slack* et saisissez ce code dans les 15 minutes\u{a0}: *{code}*",
    linked: "Votre compte Slack est maintenant associé à Cedar en tant que *{name}*. Écrivez-moi ici ou mentionnez-moi dans un canal pour commencer. Chaque fil est une conversation\u{a0}: répondez-y pour continuer.",
    not_yours: "Ce fil est la conversation de quelqu'un d'autre avec Cedar. Mentionnez-moi dans un nouveau message pour commencer la vôtre.",
    busy: "Je travaille encore sur votre dernier message. Attendez qu'il soit terminé, ou écrivez *stop* pour l'arrêter.",
    nothing_to_stop: "Rien n'est en cours ici.",
    help: "Demandez-moi ce que vous voulez\u{a0}: mentionnez-moi avec une question ou une tâche, ou écrivez-moi directement. Chaque fil est une conversation\u{a0}: répondez-y pour continuer, ou écrivez *stop* pour arrêter.",
    start_failed: ":warning: Je n'ai pas pu commencer\u{a0}: {error}",
    failed: ":warning: Cela n'a pas marché\u{a0}: {error}",
    cancelled: "Arrêté.",
    done: "Terminé.",
    reply_to_answer: "Répondez dans ce fil.",
    open_in_cedar: "Ouvrir dans Cedar",
    plan_title: "Voici mon plan\u{a0}:",
    go_ahead: "Allez-y",
    stop: "Arrêter",
    plan_agreed: "Je suis ce plan.",
    plan_timeout: "Aucune réponse n'est venue, je suis donc ce plan.",
    approval_title: "Puis-je utiliser {tool}\u{a0}?",
    allow: "Autoriser",
    always: "Toujours autoriser",
    deny: "Refuser",
    allowed: ":white_check_mark: Autorisé.",
    always_allowed: ":white_check_mark: Autorisé, et désormais sans demander.",
    denied: ":no_entry_sign: Refusé\u{a0}: pas fait.",
    timed_out: ":hourglass: Aucune réponse\u{a0}: pas fait.",
    only_owner: "Seule la personne qui a demandé peut répondre.",
    already_answered: "La réponse a déjà été donnée.",
};

static ES: Texts = Texts {
    link_offer: "Todavía no te conozco en Cedar. Para vincular tu cuenta de Slack, abre Cedar, ve a *Ajustes → Slack* e introduce este código en los próximos 15 minutos: *{code}*",
    linked: "Tu cuenta de Slack está vinculada a Cedar como *{name}*. Escríbeme aquí o mencióname en un canal para empezar. Cada hilo es una conversación: responde en él para seguir.",
    not_yours: "Este hilo es la conversación de otra persona con Cedar. Mencióname en un mensaje nuevo para empezar la tuya.",
    busy: "Sigo trabajando en tu último mensaje. Espera a que termine o escribe *stop* para detenerlo.",
    nothing_to_stop: "Aquí no hay nada en marcha.",
    help: "Pregúntame lo que quieras: mencióname con una pregunta o una tarea, o escríbeme directamente. Cada hilo es una conversación: responde en él para seguir, o escribe *stop* para detenerme.",
    start_failed: ":warning: No pude empezar con eso: {error}",
    failed: ":warning: No funcionó: {error}",
    cancelled: "Detenido.",
    done: "Hecho.",
    reply_to_answer: "Responde en este hilo.",
    open_in_cedar: "Abrir en Cedar",
    plan_title: "Este es mi plan:",
    go_ahead: "Adelante",
    stop: "Detener",
    plan_agreed: "Sigo este plan.",
    plan_timeout: "No llegó ninguna respuesta, así que sigo este plan.",
    approval_title: "¿Puedo usar {tool}?",
    allow: "Permitir",
    always: "Permitir siempre",
    deny: "Denegar",
    allowed: ":white_check_mark: Permitido.",
    always_allowed: ":white_check_mark: Permitido, y a partir de ahora sin preguntar.",
    denied: ":no_entry_sign: Denegado: no se hizo.",
    timed_out: ":hourglass: Sin respuesta: no se hizo.",
    only_owner: "Solo quien preguntó puede responder.",
    already_answered: "Eso ya tiene respuesta.",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_markup_becomes_plain_text() {
        assert_eq!(plain_text("<@UBOT> what's up?", "UBOT"), "what's up?");
        assert_eq!(plain_text("hey <@UBOT|cedar>, ask <@U1|ann> in <#C1|general>", "UBOT"), "hey , ask @ann in #general");
        assert_eq!(plain_text("see <https://a.example/x?a=1&amp;b=2> and <https://b.example|B>", "UBOT"), "see https://a.example/x?a=1&b=2 and B (https://b.example)");
        assert_eq!(plain_text("mail <mailto:a@b.example|a@b.example> <!here> 1 &lt; 2", "UBOT"), "mail a@b.example @here 1 < 2");
        assert_eq!(plain_text("<@UBOT>", "UBOT"), "");
        assert_eq!(plain_text("line one\n<@UBOT>  line two", "UBOT"), "line one\nline two");
    }

    #[test]
    fn markdown_becomes_mrkdwn() {
        assert_eq!(mrkdwn("# Title\nSome **bold** and ~~gone~~ and [a link](https://x.example/?a=1&b=2)."), "*Title*\nSome *bold* and ~gone~ and <https://x.example/?a=1&amp;b=2|a link>.");
        assert_eq!(mrkdwn("- one\n  * two\n1 < 2 & `a<b`"), "• one\n  • two\n1 &lt; 2 &amp; `a&lt;b`");
        assert_eq!(mrkdwn("```rust\nlet x = **y**;\n```"), "```\nlet x = **y**;\n```");
        assert_eq!(mrkdwn("| a | b |\n|---|---|\n| 1 | 2 |\nafter"), "```\n| a | b |\n| 1 | 2 |\n```\nafter");
        assert_eq!(mrkdwn("[not a link](javascript:alert(1))"), "[not a link](javascript:alert(1))");
        assert_eq!(mrkdwn("```\nunclosed"), "```\nunclosed\n```");
    }

    #[test]
    fn long_answers_are_split_between_lines_and_code_stays_closed() {
        let text = format!("{}\n```\n{}\n{}\n```\nend", "a".repeat(30), "b".repeat(30), "c".repeat(30));
        let parts = chunks(&text, 70);
        assert!(parts.iter().all(|p| p.chars().count() <= 70), "{parts:?}");
        assert!(parts.iter().all(|p| p.matches("```").count() % 2 == 0), "{parts:?}");
        assert_eq!(parts.join("\n").replace("\n```\n```", "").replace("```\n```\n", ""), text);
        let long = "x".repeat(200);
        let parts = chunks(&long, 70);
        assert!(parts.iter().all(|p| p.chars().count() <= 70));
        assert_eq!(parts.concat(), long);
        assert!(chunks("", 70).is_empty());
    }

    #[test]
    fn codes_are_read_however_they_are_typed() {
        let code = new_code();
        assert_eq!(code.len(), 9);
        assert_eq!(normalize_code(&code.to_lowercase().replace('-', " ")), Some(code.clone()));
        assert_eq!(normalize_code("ABCD-EFG0"), None, "0 isn't in the alphabet");
        assert_eq!(normalize_code("ABC"), None);
    }

    #[test]
    fn a_code_is_taken_once_and_reused_until_then() {
        let slack = Slack::new(SlackConfig { app_token: "xapp-1".into(), bot_token: "xoxb-1".into(), api_url: "http://127.0.0.1:1".into() }).unwrap();
        let code = slack.code_for("T1", Some("Team"), "U1", Some("Ann"));
        assert_eq!(slack.code_for("T1", None, "U1", None), code, "the same account gets the code it was sent");
        assert_ne!(slack.code_for("T1", None, "U2", None), code);
        let pending = slack.redeem(&code.to_lowercase()).unwrap();
        assert_eq!((pending.team.as_str(), pending.slack_user.as_str(), pending.name.as_deref()), ("T1", "U1", Some("Ann")));
        assert!(slack.redeem(&code).is_none(), "a code works once");
    }

    #[test]
    fn events_for_the_app_are_told_apart() {
        let payload = |event: Value| json!({ "team_id": "T1", "event": event });
        let mention = incoming(&payload(json!({"type": "app_mention", "user": "U1", "channel": "C1", "text": "<@UBOT> hi", "ts": "1.1"})), "UBOT").unwrap();
        assert!(mention.mentioned && !mention.direct);
        assert_eq!(mention.root(), "1.1");
        let dm = incoming(&payload(json!({"type": "message", "channel_type": "im", "user": "U1", "channel": "D1", "text": "hi", "ts": "2.1", "thread_ts": "2.0"})), "UBOT").unwrap();
        assert!(dm.direct && !dm.mentioned);
        assert_eq!(dm.root(), "2.0");
        // The same mention as a channel message comes twice; only the app_mention counts.
        assert!(incoming(&payload(json!({"type": "message", "channel_type": "channel", "user": "U1", "channel": "C1", "text": "<@UBOT> hi", "ts": "1.1"})), "UBOT").is_none());
        // A channel message outside a thread isn't for the app; one in a thread may be.
        assert!(incoming(&payload(json!({"type": "message", "channel_type": "channel", "user": "U1", "channel": "C1", "text": "hi", "ts": "3.1"})), "UBOT").is_none());
        assert!(incoming(&payload(json!({"type": "message", "channel_type": "channel", "user": "U1", "channel": "C1", "text": "more", "ts": "3.2", "thread_ts": "1.1"})), "UBOT").is_some());
        // Edits, bots and the app itself are left out.
        assert!(incoming(&payload(json!({"type": "message", "subtype": "message_changed", "channel_type": "im", "channel": "D1", "ts": "4"})), "UBOT").is_none());
        assert!(incoming(&payload(json!({"type": "message", "bot_id": "B1", "user": "U2", "channel_type": "im", "channel": "D1", "text": "x", "ts": "4"})), "UBOT").is_none());
        assert!(incoming(&payload(json!({"type": "message", "user": "UBOT", "channel_type": "im", "channel": "D1", "text": "x", "ts": "4"})), "UBOT").is_none());
    }

    #[test]
    fn stop_words() {
        assert!(is_stop("Stop!"));
        assert!(is_stop(" abbrechen "));
        assert!(!is_stop("stop the server"));
    }

    #[test]
    fn approvals_offer_always_only_with_a_scope() {
        let id = Uuid::nil();
        let (_, blocks) = approval_message(&EN, id, &json!({"id": "a1", "tool": "bash", "input": {"command": "rm -rf x"}, "scope": null}), None);
        let labels: Vec<_> = blocks[1]["elements"].as_array().unwrap().iter().map(|b| b["text"]["text"].as_str().unwrap()).collect();
        assert_eq!(labels, ["Allow", "Deny"]);
        assert_eq!(blocks[1]["elements"][0]["value"], format!("{id}|a1"));
        let (_, blocks) = approval_message(&EN, id, &json!({"id": "a1", "tool": "download_file", "scope": "example.com"}), None);
        assert_eq!(blocks[1]["elements"].as_array().unwrap().len(), 3);
        let (_, blocks) = approval_message(&EN, id, &json!({"id": "a1", "tool": "bash"}), Some(EN.denied));
        assert_eq!(blocks[1]["type"], "context");
    }

    #[test]
    fn links_only_to_a_page_others_can_open() {
        let id = Uuid::nil();
        assert_eq!(conversation_link("http://127.0.0.1:3000", id, &EN), None);
        assert_eq!(conversation_link("http://localhost:3000", id, &EN), None);
        assert_eq!(conversation_link("https://chat.example", id, &EN).unwrap(), format!("<https://chat.example/#/c/{id}|Open in Cedar>"));
    }
}
