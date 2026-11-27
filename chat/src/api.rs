//! The HTTP API and the embedded web UI.

use crate::cedar::{self, Attachment, CedarClient, RunOptions};
use crate::channels::{self, ApnsEnvironment, Method, Reach, Scope, Security};
use crate::config::{Config, ModelsAccess};
use crate::error::AppError;
use crate::i18n::{self, Lang};
use crate::prompt::{self, Exchange};
use crate::schedule::{self, Timing};
use crate::notify;
use crate::pairing;
use crate::update;
use crate::contrib;
use crate::store::{AttachmentInfo, Contribution, Conversation, InboxItem, NewSchedule, Preferences, Schedule, ScheduleRun, SearchHit, StarredConversation, Store, Turn};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use chrono::Utc;
use futures_util::Stream;
use std::convert::Infallible;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Query, RawQuery, Request, State};
use axum::http::header::{self, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

mod actions;
mod agents;
mod asks;
mod outcome;
mod search;
mod timeline;
mod handover;
mod slack;
mod users;
use users::{create_user, delete_user, get_me, list_users, rename_user, rotate_user_token};
pub mod hooks;

const MAX_BODY_BYTES: usize = 128 * 1024;
const MAX_MESSAGE_BYTES: usize = 32_000;
const MAX_TITLE_CHARS: usize = 200;
const MAX_CONVERSATIONS: u32 = 500;
/// How many conversations a search returns, and how long what is searched for may be.
const MAX_SEARCH_RESULTS: u32 = 50;
const MAX_SEARCH_CHARS: usize = 200;
const MAX_SCHEDULES: i64 = 100;
const MAX_SCHEDULE_RUNS: u32 = 100;
const MAX_TOOLS: usize = 100;
const MAX_SYSTEM_BYTES: usize = 20_000;
const MAX_TURNS: u32 = 1000;
/// How many finished responses the inbox lists.
const MAX_INBOX: u32 = 50;
/// Attachments, mirroring cedar's limits: how many per message, how big each and all together
/// (decoded), and the body limit that leaves room for their base64 on the messages route only.
const MAX_ATTACHMENTS: usize = 10;
const MAX_ATTACHMENT_BYTES: usize = 20 << 20;
const MAX_ATTACHMENTS_TOTAL_BYTES: usize = 32 << 20;
const MAX_MESSAGE_BODY_BYTES: usize = 48 << 20;
/// A contribution as the dialog sends it back (the bundle plus the outcome); the corpus takes
/// 8 MiB bundles.
const MAX_BUNDLE_BODY_BYTES: usize = 9 << 20;
const MAX_ATTACHMENT_NAME_CHARS: usize = 200;
const ATTACHMENT_TYPES: [&str; 7] = ["image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf", "text/csv", "application/json"];
/// Where Google sends the browser back after "Connect Google" (under `CHAT_PUBLIC_URL`).
pub const GOOGLE_CALLBACK_PATH: &str = "/api/oauth/google/callback";
const ADMIN_HEADER: &str = "x-admin-token";

/// Setting Cedar up the first time it is opened (finding or adding a model, then a first task).
const SETUP_JS: &str = include_str!("../static/setup.js");
const SETUP_CSS: &str = include_str!("../static/setup.css");
/// `cedarConfirm`, asking in the page because the macOS app's webview has no `confirm()`; every UI loads it.
const CONFIRM_JS: &str = include_str!("../static/confirm.js");
/// The hosted providers and the presets of model servers people run themselves, for Settings'
/// "Add a model". The desktop app compiles the same file in for its setup page.
const MODEL_SERVERS_JSON: &str = include_str!("../static/model-servers.json");
/// Which model to suggest at each hosted provider, once it has listed its models.
const SUGGESTED_MODELS_JSON: &str = include_str!("../static/suggested-models.json");
/// Shows the images the agent produces in the response; both UIs load it, before the viewer.
const IMAGE_VIEW_JS: &str = include_str!("../static/image-view.js");
const IMAGE_VIEW_CSS: &str = include_str!("../static/image-view.css");
/// Previews Markdown files, PDFs and images; both UIs load it.
const FILE_VIEWER_JS: &str = include_str!("../static/file-viewer.js");
/// Draws the `show_map` tool's result as a map; both UIs load it, and it loads Leaflet on demand.
const MAP_VIEW_JS: &str = include_str!("../static/map-view.js");
const MAP_VIEW_CSS: &str = include_str!("../static/map-view.css");
/// Draws the `show_recipe` tool's result as a recipe card with the steps as a timeline; both UIs load it.
const RECIPE_VIEW_JS: &str = include_str!("../static/recipe-view.js");
const RECIPE_VIEW_CSS: &str = include_str!("../static/recipe-view.css");
/// Draws the `weather_forecast` tool's report as a weather card with a row per day; every UI loads it.
const WEATHER_VIEW_JS: &str = include_str!("../static/weather-view.js");
const WEATHER_VIEW_CSS: &str = include_str!("../static/weather-view.css");
/// Draws the `ask_user` tool's result as a form with choices and a free-form answer; both UIs load it.
const CLARIFY_VIEW_JS: &str = include_str!("../static/clarify-view.js");
const CLARIFY_VIEW_CSS: &str = include_str!("../static/clarify-view.css");
/// The card for a follow-up the model suggested, with the buttons that decide about it; every UI loads it.
const FOLLOWUP_VIEW_JS: &str = include_str!("../static/followup-view.js");
const FOLLOWUP_VIEW_CSS: &str = include_str!("../static/followup-view.css");
/// The cards for what Cedar could not do and for a call that waits for approval, the outbox on the
/// Agents page and the Feedback page of Settings; every UI loads it.
const HANDOVER_VIEW_JS: &str = include_str!("../static/handover-view.js");
const HANDOVER_VIEW_CSS: &str = include_str!("../static/handover-view.css");
/// Draws the `plan_trip` tool's result as a route map with the options; it needs the map view.
const TRANSIT_VIEW_JS: &str = include_str!("../static/transit-view.js");
const TRANSIT_VIEW_CSS: &str = include_str!("../static/transit-view.css");
/// Draws the `compare_stocks` tool's result as quotes, a chart, the ratings and the news; both UIs load it.
const STOCK_VIEW_JS: &str = include_str!("../static/stock-view.js");
const STOCK_VIEW_CSS: &str = include_str!("../static/stock-view.css");
/// Draws the `search_flights` tool's result as a flights card (prices, times, stops, segments) or the
/// places to book a chosen flight; every UI loads it.
const FLIGHT_VIEW_JS: &str = include_str!("../static/flight-view.js");
const FLIGHT_VIEW_CSS: &str = include_str!("../static/flight-view.css");
/// Draws a `generate_chart` call's data as an interactive chart card; every UI loads it.
const CHART_VIEW_JS: &str = include_str!("../static/chart-view.js");
const CHART_VIEW_CSS: &str = include_str!("../static/chart-view.css");
/// Draws the `search_products` tool's result as product tiles, a detail panel with a photo gallery and
/// the reading of the reviews, and the comparable options; every UI loads it.
const PRODUCT_VIEW_JS: &str = include_str!("../static/product-view.js");
const PRODUCT_VIEW_CSS: &str = include_str!("../static/product-view.css");
/// The Tools, Model and Integrations sections of Settings and the composer's tools menu; both UIs load it.
const MODELS_JS: &str = include_str!("../static/models.js");
const MODELS_CSS: &str = include_str!("../static/models.css");
const SETTINGS_SHELL_JS: &str = include_str!("../static/settings-shell.js");
const MEMORY_JS: &str = include_str!("../static/memory.js");
const MEMORY_GRAPH_JS: &str = include_str!("../static/memory-graph.js");
const MEMORY_GRAPH_CSS: &str = include_str!("../static/memory-graph.css");
const RECORDS_JS: &str = include_str!("../static/records.js");
const TRACE_VIEW_JS: &str = include_str!("../static/trace-view.js");
const TRACE_VIEW_CSS: &str = include_str!("../static/trace-view.css");
/// Contributing a conversation (contrib.rs): the review dialog and Settings → Contribute.
const CONTRIB_VIEW_JS: &str = include_str!("../static/contrib-view.js");
const CONTRIB_VIEW_CSS: &str = include_str!("../static/contrib-view.css");
const SETTINGS_JS: &str = include_str!("../static/settings.js");
const SETTINGS_CSS: &str = include_str!("../static/settings.css");
/// Attaching images and PDFs to a message (button, drag-and-drop, paste); both UIs load it.
/// The pages' translations: the lookup every page loads first, and the language question and
/// Settings page. The catalogs in `static/i18n/` hold the text, one file per area.
const I18N_JS: &str = include_str!("../static/i18n.js");
const LANGUAGE_JS: &str = include_str!("../static/language.js");
const LANGUAGE_CSS: &str = include_str!("../static/language.css");
const I18N_CATALOGS: &[(&str, &str)] = &[
    ("common.js", include_str!("../static/i18n/common.js")),
    ("settings.js", include_str!("../static/i18n/settings.js")),
    ("cards.js", include_str!("../static/i18n/cards.js")),
    ("agents.js", include_str!("../static/i18n/agents.js")),
    ("actions.js", include_str!("../static/i18n/actions.js")),
    ("v3.js", include_str!("../static/i18n/v3.js")),
    ("today.js", include_str!("../static/i18n/today.js")),
];
const ATTACH_JS: &str = include_str!("../static/attach.js");
const ATTACH_CSS: &str = include_str!("../static/attach.css");
/// The Agents page (every agent and its runs, the activity feed, the form that makes one), the link
/// to it in the sidebar and the badge on a run's conversation; every UI loads it.
const AGENTS_JS: &str = include_str!("../static/agents.js");
const AGENTS_CSS: &str = include_str!("../static/agents.css");
/// The Actions page: what the person showed Cedar how to do, the screen recorder that makes a
/// new one, and the link to it in the sidebar; every UI loads it.
const ACTIONS_JS: &str = include_str!("../static/actions.js");
const ACTIONS_CSS: &str = include_str!("../static/actions.css");
/// The search box of the conversations sidebar and its results; every UI loads it.
const SEARCH_JS: &str = include_str!("../static/search.js");
const SEARCH_CSS: &str = include_str!("../static/search.css");
/// The star button and the Starred list in the conversations sidebar; every UI loads it.
const STAR_JS: &str = include_str!("../static/star.js");
const PRIVATE_JS: &str = include_str!("../static/private.js");
const PRIVATE_CSS: &str = include_str!("../static/private.css");
const MORE_MENU_JS: &str = include_str!("../static/more-menu.js");
const STAR_CSS: &str = include_str!("../static/star.css");
/// The inbox of finished responses, the unread count in the title, the Notifications settings and
/// the push subscription; both UIs load it.
const NOTIFY_JS: &str = include_str!("../static/notify.js");
const NOTIFY_CSS: &str = include_str!("../static/notify.css");
const UPDATE_JS: &str = include_str!("../static/update.js");
const UPDATE_CSS: &str = include_str!("../static/update.css");
/// The Phone page of Settings: the QR code and token the iOS and Android apps connect with; every UI loads it.
const PAIRING_JS: &str = include_str!("../static/pairing.js");
const SLACK_JS: &str = include_str!("../static/slack.js");
/// The service worker that shows Web Push messages, served at the root so its scope is the whole site.
const SW_JS: &str = include_str!("../static/sw.js");
/// The plan, approval cards, folded reasoning, sources and citations.
const TRUST_VIEW_JS: &str = include_str!("../static/trust-view.js");
const TRUST_VIEW_CSS: &str = include_str!("../static/trust-view.css");
/// Leaflet 1.9.4 (BSD-2-Clause, see `static/vendor/leaflet/LICENSE`), served from here so no
/// third-party code runs in the page. Only the map tiles come from another server.
const LEAFLET_JS: &str = include_str!("../static/vendor/leaflet/leaflet.js");
const LEAFLET_CSS: &str = include_str!("../static/vendor/leaflet/leaflet.css");
/// The page's typefaces (SIL Open Font License 1.1, see `static/fonts/OFL-*.txt`), served from
/// here so the pages load no fonts from another host. `fonts.css` holds their `@font-face` rules.
const FONTS_CSS: &str = include_str!("../static/fonts/fonts.css");
const FONTS: &[(&str, &[u8])] = &[
    ("figtree-latin-ext.woff2", include_bytes!("../static/fonts/figtree-latin-ext.woff2")),
    ("figtree-latin.woff2", include_bytes!("../static/fonts/figtree-latin.woff2")),
    ("instrument-sans-latin-ext.woff2", include_bytes!("../static/fonts/instrument-sans-latin-ext.woff2")),
    ("instrument-sans-latin.woff2", include_bytes!("../static/fonts/instrument-sans-latin.woff2")),
    ("instrument-serif-italic-latin-ext.woff2", include_bytes!("../static/fonts/instrument-serif-italic-latin-ext.woff2")),
    ("instrument-serif-italic-latin.woff2", include_bytes!("../static/fonts/instrument-serif-italic-latin.woff2")),
    ("instrument-serif-latin-ext.woff2", include_bytes!("../static/fonts/instrument-serif-latin-ext.woff2")),
    ("instrument-serif-latin.woff2", include_bytes!("../static/fonts/instrument-serif-latin.woff2")),
    ("jetbrains-mono-latin-ext.woff2", include_bytes!("../static/fonts/jetbrains-mono-latin-ext.woff2")),
    ("jetbrains-mono-latin.woff2", include_bytes!("../static/fonts/jetbrains-mono-latin.woff2")),
    ("lora-italic-latin-ext.woff2", include_bytes!("../static/fonts/lora-italic-latin-ext.woff2")),
    ("lora-italic-latin.woff2", include_bytes!("../static/fonts/lora-italic-latin.woff2")),
    ("lora-latin-ext.woff2", include_bytes!("../static/fonts/lora-latin-ext.woff2")),
    ("lora-latin.woff2", include_bytes!("../static/fonts/lora-latin.woff2")),
];
/// The logo and icons, shared with the README and the landing page in `docs/assets`. The mark has a
/// light and a dark variant; the pages pick one with `prefers-color-scheme`.
const MARK_SVG: &str = include_str!("../../docs/assets/cedar-mark.svg");
const MARK_ON_DARK_SVG: &str = include_str!("../../docs/assets/cedar-mark-on-dark.svg");
const FAVICON_SVG: &str = include_str!("../../docs/assets/favicon.svg");
const FAVICON_32_PNG: &[u8] = include_bytes!("../../docs/assets/favicon-32.png");
const FAVICON_16_PNG: &[u8] = include_bytes!("../../docs/assets/favicon-16.png");
const APPLE_TOUCH_ICON_PNG: &[u8] = include_bytes!("../../docs/assets/apple-touch-icon.png");

/// One of the front ends, served under its own prefix.
struct Ui {
    prefix: &'static str,
    index: &'static str,
    js: &'static str,
    css: &'static str,
}

/// The page: the conversation in a narrow column, and what Cedar produced beside it (on a
/// phone, the results are the page and the conversation a sheet over them). It lives at `/v3/`,
/// the address the phone apps and bookmarks know.
const V3: Ui = Ui {
    prefix: "/v3",
    index: include_str!("../static/v3/index.html"),
    js: include_str!("../static/v3/app.js"),
    css: include_str!("../static/v3/style.css"),
};

/// The layout's own script and stylesheet, served next to the page's.
const LAYOUT_FILES: &[(&str, &str, &str)] = &[
    ("/v3/ledger.js", JS, include_str!("../static/v3/ledger.js")),
    ("/v3/ledger.css", CSS, include_str!("../static/v3/ledger.css")),
];

/// The Today page: one day as a river, and the box to ask with a When (`/api/timeline`,
/// `/api/asks`). An alternative home, at `/today/`; the conversations stay at `/v3/`.
const TODAY_FILES: &[(&str, &str, &str)] = &[
    ("/today/", HTML, include_str!("../static/today/index.html")),
    ("/today/today.js", JS, include_str!("../static/today/today.js")),
    ("/today/today.css", CSS, include_str!("../static/today/today.css")),
];

/// Send the browser to the page, keeping the query (the outcome of "Connect Google"); the
/// browser keeps the `#/c/<id>` of a bookmarked conversation itself.
async fn to_page(RawQuery(query): RawQuery) -> Redirect {
    match query {
        Some(q) if !q.is_empty() => Redirect::temporary(&format!("/v3/?{q}")),
        _ => Redirect::temporary("/v3/"),
    }
}

/// The home screen is Today. `/` opens it; with a query (the outcome of "Connect Google", which
/// Settings on the main page reads) it opens the main page as before. The `#/c/<id>` of a
/// notification's link stays with the browser, and the Today page hands it to the main page.
async fn to_home(RawQuery(query): RawQuery) -> Redirect {
    match query {
        Some(q) if !q.is_empty() => Redirect::temporary(&format!("/v3/?{q}")),
        _ => Redirect::temporary("/today/"),
    }
}

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    /// The database and cedar, for one user: the primary user in the state the server starts
    /// with, and whoever a request or a piece of background work is for in the copy
    /// [`AppState::as_user`] makes for it.
    pub store: Store,
    pub cedar: CedarClient,
    /// The primary user: whoever presents `CHAT_ACCESS_TOKEN`. Its cedar runs are the key's own.
    pub primary: Uuid,
    /// SHA-256 of the access token, if one is required.
    token_digest: Option<[u8; 32]>,
    /// SHA-256 of the admin token; `None` hides the Integrations settings.
    admin_digest: Option<[u8; 32]>,
    /// Serializes sending messages, so two tabs can't start overlapping runs in one conversation.
    send_lock: Arc<Mutex<()>>,
    /// Serializes starting the runs events ask for, so a webhook call and a check can't both find
    /// an agent free (`schedule::start_event`).
    pub firing: Arc<Mutex<()>>,
    /// How many webhook calls were taken lately, per hook and in all (`hooks`).
    pub hook_limits: Arc<hooks::Limits>,
    /// Live notices for the open tabs (`GET /api/events`): a finished response, as `notify` learns
    /// of one from cedar.
    pub notices: broadcast::Sender<(Uuid, Value)>,
    /// Messages for the Cedar desktop app (`GET /api/notifications/desktop/events`), as
    /// `channels` sends them.
    pub desktop: broadcast::Sender<(Uuid, Value)>,
    /// For Pushover and the push services (`channels`).
    pub http: reqwest::Client,
    /// The APNs connection and its provider token (`channels`).
    pub apns: Arc<channels::Apns>,
    /// The Google access token FCM (Android push) is sent with (`channels`).
    pub fcm: Arc<channels::Fcm>,
    /// The latest release, as the update check last found it (`update`).
    pub updates: Arc<update::Updates>,
    /// Cancelled when the server begins to shut down. Ends the event streams, which would
    /// otherwise hold the graceful shutdown open for as long as a client stays connected.
    pub shutdown: CancellationToken,
    /// The `state` of each "Connect Google" the desktop app sent to the default browser
    /// (`?browser=1`). Google's redirect lands in that browser, not in the app, so the callback
    /// shows a page that says to go back to the app instead of loading the chat there.
    google_in_browser: Arc<std::sync::Mutex<Vec<String>>>,
    /// The Slack app people talk to Cedar through, when one is set up (`slack`).
    pub slack: Arc<crate::slack::SlackHub>,
}

/// How many "Connect Google" sign-ins started in the browser are remembered at once.
const GOOGLE_IN_BROWSER_MAX: usize = 16;

impl AppState {
    pub fn new(cfg: Config, store: Store) -> anyhow::Result<Self> {
        let cedar = CedarClient::new(&cfg.cedar_url, &cfg.cedar_api_key)?;
        let token_digest = cfg.access_token.as_deref().map(|t| Sha256::digest(t.as_bytes()).into());
        let admin_digest = cfg.admin_token.as_deref().map(|t| Sha256::digest(t.as_bytes()).into());
        let (notices, _) = broadcast::channel(notify::NOTICE_BUFFER);
        let (desktop, _) = broadcast::channel(channels::DESKTOP_BUFFER);
        let http = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).build()?;
        let apns = Arc::new(channels::Apns::new(cfg.apns_url.clone())?);
        let primary = store.user();
        let slack = Arc::new(crate::slack::SlackHub::new(cfg.slack.clone(), cfg.slack_api_url.clone())?);
        Ok(Self { cfg: Arc::new(cfg), store, cedar, primary, token_digest, admin_digest, send_lock: Arc::default(), firing: Arc::default(), hook_limits: Arc::default(), notices, desktop, http, apns, fcm: Arc::default(), updates: Arc::default(), shutdown: CancellationToken::new(), google_in_browser: Arc::default(), slack })
    }
}

impl AppState {
    /// This state for `user`: what it stores and asks cedar for is theirs. Each of the two tags
    /// its notices with the user, so a tab only hears of its own.
    pub fn as_user(&self, user: Uuid) -> Self {
        Self { store: self.store.as_user(user), cedar: self.cedar.as_user((user != self.primary).then_some(user)), ..self.clone() }
    }

    /// The user this state is for.
    pub fn user(&self) -> Uuid {
        self.store.user()
    }
}

/// Who a request is from, as [`require_token`] worked out.
#[derive(Debug, Clone, Copy)]
struct CurrentUser(Uuid);

/// The state for the user the request is from. Every handler behind the access token takes this
/// rather than `State<AppState>`, so that nothing it does can touch another user's data.
pub(crate) struct Scoped(pub(crate) AppState);

impl FromRequestParts<AppState> for Scoped {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, state: &AppState) -> Result<Self, AppError> {
        match parts.extensions.get::<CurrentUser>() {
            Some(CurrentUser(user)) => Ok(Scoped(state.as_user(*user))),
            // A route outside `require_token` asked for a user's state: a mistake in the router.
            None => Err(AppError::Internal(anyhow::anyhow!("no user on a request that needs one"))),
        }
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/conversations", get(list_conversations).post(create_conversation))
        .route("/conversations/search", get(search_conversations))
        .route("/conversations/{id}", get(get_conversation).patch(rename_conversation).delete(delete_conversation))
        // Only this route takes attachments, so only it gets the room for them.
        .route("/conversations/{id}/messages", post(send_message).layer(DefaultBodyLimit::max(MAX_MESSAGE_BODY_BYTES)))
        .route("/conversations/{id}/tools", put(set_conversation_tools))
        .route("/conversations/{id}/compact", post(compact_conversation))
        .route("/conversations/{id}/contribution", get(contribution_preview).post(send_contribution).layer(DefaultBodyLimit::max(MAX_BUNDLE_BODY_BYTES)))
        .route("/contributions", get(list_contributions))
        .route("/contributions/refresh", post(refresh_contributions))
        .route("/contributions/{id}", axum::routing::delete(withdraw_contribution))
        .route("/settings/contributions", get(get_contribution_settings).put(put_contribution_settings))
        .route("/turns/{id}/rating", put(rate_turn))
        .route("/conversations/{id}/seen", post(mark_seen))
        .route("/conversations/{id}/star", put(star_conversation).delete(unstar_conversation))
        .route("/starred", get(list_starred))
        .route("/inbox", get(inbox))
        .route("/timeline", get(timeline::timeline))
        .route("/search", get(search::search))
        .route("/asks", post(asks::ask).layer(DefaultBodyLimit::max(MAX_MESSAGE_BODY_BYTES)))
        .route("/asks/draft", post(asks::draft))
        .route("/events", get(notice_events))
        .route("/notifications", get(get_notifications).put(put_notifications))
        .route("/notifications/test", post(test_notification))
        .route("/notifications/push/subscriptions", post(subscribe_push).delete(unsubscribe_push))
        .route("/notifications/push/subscriptions/{id}", axum::routing::delete(delete_push_subscription))
        .route("/notifications/desktop/events", get(desktop_events))
        .route("/notifications/apns/devices", post(register_apns_device).delete(unregister_apns_device))
        .route("/notifications/apns/devices/{id}", axum::routing::delete(delete_apns_device))
        .route("/notifications/android/devices", post(register_android_device).delete(unregister_android_device))
        .route("/notifications/android/devices/{id}", axum::routing::delete(delete_android_device))
        .route("/conversations/{id}/files", get(list_files))
        .route("/conversations/{id}/files/{*path}", get(download_file))
        .route("/turns/{id}/events", get(turn_events))
        .route("/turns/{id}/trace", get(turn_trace))
        .route("/turns/{id}/outcome", get(outcome::outcome))
        .route("/turns/{id}/cancel", post(cancel_turn))
        .route("/turns/{id}/plan", post(approve_plan))
        .route("/turns/{id}/approvals/{approval_id}", post(decide_approval))
        .route("/turns/{id}/followups/{call_id}", put(agents::decide_followup))
        .route("/turns/{id}/limitations/{call_id}", put(handover::decide_limitation))
        .route("/feedback", get(handover::list_feedback))
        .route("/feedback/{id}", axum::routing::delete(handover::delete_feedback))
        .route("/outbox", get(handover::list_outbox))
        .route("/outbox/{id}", get(handover::get_outbox).post(handover::decide_outbox))
        .route("/actions", get(actions::list).post(actions::create).layer(DefaultBodyLimit::max(actions::MAX_ACTION_BODY_BYTES)))
        .route("/actions/draft", post(actions::draft).layer(DefaultBodyLimit::max(actions::MAX_RECORDING_BODY_BYTES)))
        .route("/actions/{id}", get(actions::get).put(actions::update).delete(actions::delete).layer(DefaultBodyLimit::max(actions::MAX_ACTION_BODY_BYTES)))
        .route("/actions/{id}/frames", get(actions::frames))
        .route("/schedules", get(list_schedules).post(create_schedule))
        .route("/schedules/{id}", get(get_schedule).patch(update_schedule).delete(delete_schedule))
        .route("/schedules/{id}/run", post(run_schedule_now))
        .route("/agents", get(agents::list).post(agents::create))
        .route("/agents/test", post(agents::test))
        .route("/agents/activity", get(agents::activity))
        .route("/agents/{id}", get(agents::get).patch(agents::update).delete(agents::delete))
        .route("/agents/{id}/run", post(agents::run_now))
        .route("/agents/{id}/trigger", post(agents::from_device))
        .route("/agents/{id}/hook/rotate", post(agents::rotate_hook))
        .route("/slack", get(slack::get_slack))
        .route("/slack/link", post(slack::link_slack))
        .route("/slack/app", put(slack::put_slack_app).delete(slack::delete_slack_app))
        .route("/slack/links/{team}/{slack_user}", axum::routing::delete(slack::unlink_slack))
        .route("/me", get(get_me))
        .route("/users", get(list_users).post(create_user))
        .route("/users/{id}", axum::routing::patch(rename_user).delete(delete_user))
        .route("/users/{id}/token", post(rotate_user_token))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/settings/locale", put(put_locale))
        .route("/pairing", get(get_pairing))
        .route("/update", get(get_update).post(install_update))
        .route("/update/check", post(check_update))
        .route("/tools", get(list_tools))
        .route("/models", get(list_models))
        .route("/memory", get(list_memory).post(add_memory).delete(delete_all_memory))
        .route("/memory/profile", get(get_memory_profile).put(put_memory_profile))
        .route("/memory/export", get(export_memory))
        .route("/memory/graph", get(memory_graph))
        .route("/memory/{id}", axum::routing::patch(patch_memory).delete(delete_memory))
        .route("/memory/{id}/history", get(memory_history))
        .route("/records", get(list_records).delete(delete_all_records))
        .route("/records/{name}", get(get_record_set).delete(delete_record_set))
        .route("/integrations", get(get_integrations))
        .route("/integrations/brave", put(put_brave_key).delete(delete_brave_key))
        .route("/integrations/bestbuy", put(put_bestbuy_key).delete(delete_bestbuy_key))
        .route("/integrations/serpapi", put(put_serpapi_key).delete(delete_serpapi_key))
        .route("/integrations/slack", put(put_slack_token).delete(delete_slack_token))
        .route("/integrations/models", get(get_models))
        .route("/integrations/server-models", post(list_server_models))
        .route("/integrations/models/{id}", put(put_model).delete(delete_model))
        .route("/integrations/models/{id}/stats", get(get_model_stats))
        .route("/integrations/settings", get(get_server_settings).put(put_server_settings))
        .route("/integrations/model-selection", put(put_model_selection))
        .route("/google", get(get_own_google).delete(delete_own_google))
        .route("/google/connect", post(connect_own_google))
        .route("/integrations/google", axum::routing::delete(delete_google))
        .route("/integrations/google/client", put(put_google_client).delete(delete_google_client))
        .route("/integrations/google/connect", post(connect_google))
        .route("/integrations/tools", get(get_personal_tools))
        .route("/integrations/tools/{name}/setup", post(setup_tool))
        .route("/integrations/tools/{name}/secrets/{secret}", put(put_tool_secret).delete(delete_tool_secret))
        .route("/integrations/marketplace", get(get_marketplace))
        .route("/integrations/marketplace/{name}", axum::routing::delete(uninstall_tool))
        .route("/integrations/marketplace/{name}/install", post(install_tool))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        // Public: the page needs it to know whether to ask for a token.
        .route("/config", get(client_config))
        // Public too: Google redirects the browser here, which can't add the token. The single-use
        // `state` cedar issued for an admin's "Connect Google" is what authorizes it.
        .route("/oauth/google/callback", get(google_callback))
        // Public as well: whoever calls an agent's webhook has the hook's own secret, not the
        // access token (see `hooks`).
        .route("/hooks/{hook_id}", post(hooks::receive).layer(DefaultBodyLimit::max(hooks::MAX_BODY_BYTES)));

    let pages = Router::new()
        .route("/", get(to_home))
        .route("/setup.js", get(|| async { asset(JS, SETUP_JS) }))
        .route("/setup.css", get(|| async { asset(CSS, SETUP_CSS) }))
        .route("/i18n.js", get(|| async { asset(JS, I18N_JS) }))
        .route("/language.js", get(|| async { asset(JS, LANGUAGE_JS) }))
        .route("/language.css", get(|| async { asset(CSS, LANGUAGE_CSS) }))
        .route("/confirm.js", get(|| async { asset(JS, CONFIRM_JS) }))
        .route("/model-servers.json", get(|| async { asset(JSON, MODEL_SERVERS_JSON) }))
        .route("/suggested-models.json", get(|| async { asset(JSON, SUGGESTED_MODELS_JSON) }))
        .route("/image-view.js", get(|| async { asset(JS, IMAGE_VIEW_JS) }))
        .route("/image-view.css", get(|| async { asset(CSS, IMAGE_VIEW_CSS) }))
        .route("/file-viewer.js", get(|| async { asset(JS, FILE_VIEWER_JS) }))
        .route("/map-view.js", get(|| async { asset(JS, MAP_VIEW_JS) }))
        .route("/map-view.css", get(|| async { asset(CSS, MAP_VIEW_CSS) }))
        .route("/recipe-view.js", get(|| async { asset(JS, RECIPE_VIEW_JS) }))
        .route("/recipe-view.css", get(|| async { asset(CSS, RECIPE_VIEW_CSS) }))
        .route("/weather-view.js", get(|| async { asset(JS, WEATHER_VIEW_JS) }))
        .route("/weather-view.css", get(|| async { asset(CSS, WEATHER_VIEW_CSS) }))
        .route("/clarify-view.js", get(|| async { asset(JS, CLARIFY_VIEW_JS) }))
        .route("/clarify-view.css", get(|| async { asset(CSS, CLARIFY_VIEW_CSS) }))
        .route("/followup-view.js", get(|| async { asset(JS, FOLLOWUP_VIEW_JS) }))
        .route("/followup-view.css", get(|| async { asset(CSS, FOLLOWUP_VIEW_CSS) }))
        .route("/handover-view.js", get(|| async { asset(JS, HANDOVER_VIEW_JS) }))
        .route("/handover-view.css", get(|| async { asset(CSS, HANDOVER_VIEW_CSS) }))
        .route("/transit-view.js", get(|| async { asset(JS, TRANSIT_VIEW_JS) }))
        .route("/transit-view.css", get(|| async { asset(CSS, TRANSIT_VIEW_CSS) }))
        .route("/stock-view.js", get(|| async { asset(JS, STOCK_VIEW_JS) }))
        .route("/stock-view.css", get(|| async { asset(CSS, STOCK_VIEW_CSS) }))
        .route("/flight-view.js", get(|| async { asset(JS, FLIGHT_VIEW_JS) }))
        .route("/flight-view.css", get(|| async { asset(CSS, FLIGHT_VIEW_CSS) }))
        .route("/chart-view.js", get(|| async { asset(JS, CHART_VIEW_JS) }))
        .route("/chart-view.css", get(|| async { asset(CSS, CHART_VIEW_CSS) }))
        .route("/product-view.js", get(|| async { asset(JS, PRODUCT_VIEW_JS) }))
        .route("/product-view.css", get(|| async { asset(CSS, PRODUCT_VIEW_CSS) }))
        .route("/models.js", get(|| async { asset(JS, MODELS_JS) }))
        .route("/models.css", get(|| async { asset(CSS, MODELS_CSS) }))
        .route("/settings-shell.js", get(|| async { asset(JS, SETTINGS_SHELL_JS) }))
        .route("/memory.js", get(|| async { asset(JS, MEMORY_JS) }))
        .route("/memory-graph.js", get(|| async { asset(JS, MEMORY_GRAPH_JS) }))
        .route("/memory-graph.css", get(|| async { asset(CSS, MEMORY_GRAPH_CSS) }))
        .route("/records.js", get(|| async { asset(JS, RECORDS_JS) }))
        .route("/trace-view.js", get(|| async { asset(JS, TRACE_VIEW_JS) }))
        .route("/trace-view.css", get(|| async { asset(CSS, TRACE_VIEW_CSS) }))
        .route("/contrib-view.js", get(|| async { asset(JS, CONTRIB_VIEW_JS) }))
        .route("/contrib-view.css", get(|| async { asset(CSS, CONTRIB_VIEW_CSS) }))
        .route("/settings.js", get(|| async { asset(JS, SETTINGS_JS) }))
        .route("/settings.css", get(|| async { asset(CSS, SETTINGS_CSS) }))
        .route("/attach.js", get(|| async { asset(JS, ATTACH_JS) }))
        .route("/attach.css", get(|| async { asset(CSS, ATTACH_CSS) }))
        .route("/agents.js", get(|| async { asset(JS, AGENTS_JS) }))
        .route("/agents.css", get(|| async { asset(CSS, AGENTS_CSS) }))
        .route("/actions.js", get(|| async { asset(JS, ACTIONS_JS) }))
        .route("/actions.css", get(|| async { asset(CSS, ACTIONS_CSS) }))
        .route("/star.js", get(|| async { asset(JS, STAR_JS) }))
        .route("/private.js", get(|| async { asset(JS, PRIVATE_JS) }))
        .route("/private.css", get(|| async { asset(CSS, PRIVATE_CSS) }))
        .route("/search.js", get(|| async { asset(JS, SEARCH_JS) }))
        .route("/search.css", get(|| async { asset(CSS, SEARCH_CSS) }))
        .route("/more-menu.js", get(|| async { asset(JS, MORE_MENU_JS) }))
        .route("/star.css", get(|| async { asset(CSS, STAR_CSS) }))
        .route("/notify.js", get(|| async { asset(JS, NOTIFY_JS) }))
        .route("/notify.css", get(|| async { asset(CSS, NOTIFY_CSS) }))
        .route("/update.js", get(|| async { asset(JS, UPDATE_JS) }))
        .route("/update.css", get(|| async { asset(CSS, UPDATE_CSS) }))
        .route("/pairing.js", get(|| async { asset(JS, PAIRING_JS) }))
        .route("/slack.js", get(|| async { asset(JS, SLACK_JS) }))
        .route("/sw.js", get(|| async { asset(JS, SW_JS) }))
        .route("/trust-view.js", get(|| async { asset(JS, TRUST_VIEW_JS) }))
        .route("/trust-view.css", get(|| async { asset(CSS, TRUST_VIEW_CSS) }))
        .route("/vendor/leaflet/leaflet.js", get(|| async { asset(JS, LEAFLET_JS) }))
        .route("/vendor/leaflet/leaflet.css", get(|| async { asset(CSS, LEAFLET_CSS) }))
        .route("/assets/cedar-mark.svg", get(|| async { asset(SVG, MARK_SVG) }))
        .route("/assets/cedar-mark-on-dark.svg", get(|| async { asset(SVG, MARK_ON_DARK_SVG) }))
        .route("/assets/favicon.svg", get(|| async { asset(SVG, FAVICON_SVG) }))
        .route("/assets/favicon-32.png", get(|| async { asset(PNG, FAVICON_32_PNG) }))
        .route("/assets/favicon-16.png", get(|| async { asset(PNG, FAVICON_16_PNG) }))
        .route("/assets/apple-touch-icon.png", get(|| async { asset(PNG, APPLE_TOUCH_ICON_PNG) }))
        .route("/fonts/fonts.css", get(|| async { asset(CSS, FONTS_CSS) }))
        .route("/healthz", get(|| async { "ok" }));
    let pages = FONTS.iter().fold(pages, |pages, &(name, bytes)| {
        pages.route(&format!("/fonts/{name}"), get(move || async move { asset(WOFF2, bytes) }))
    });
    let pages = I18N_CATALOGS.iter().fold(pages, |pages, &(name, js)| {
        pages.route(&format!("/i18n/{name}"), get(move || async move { asset(JS, js) }))
    });
    let pages = LAYOUT_FILES.iter().fold(pages, |pages, &(path, content_type, body)| {
        pages.route(path, get(move || async move { asset(content_type, body) }))
    });
    let pages = TODAY_FILES.iter().fold(pages, |pages, &(path, content_type, body)| {
        pages.route(path, get(move || async move { asset(content_type, body) }))
    });
    let pages = pages.route("/today", get(|| async { Redirect::permanent("/today/") }));
    // There is one interface. The addresses of the retired ones ("Claude style" at /v1/, "Cedar
    // style" at /v2/, "Dossier" at /v4/) open it.
    let pages = ["/v1", "/v1/", "/v2", "/v2/", "/v4", "/v4/"].iter().fold(with_ui(pages, V3), |pages, path| pages.route(path, get(to_page)));
    pages
        .nest("/api", api)
        .fallback(|| async { AppError::NotFound("no such route") })
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

const HTML: &str = "text/html; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JSON: &str = "application/json; charset=utf-8";
const SVG: &str = "image/svg+xml";
const PNG: &str = "image/png";
const WOFF2: &str = "font/woff2";

/// Serve a front end's page at `/vN` and `/vN/`, and its script and stylesheet next to it.
fn with_ui(router: Router<AppState>, ui: Ui) -> Router<AppState> {
    let Ui { prefix, index, js, css } = ui;
    router
        .route(prefix, get(move || async move { asset(HTML, index) }))
        .route(&format!("{prefix}/"), get(move || async move { asset(HTML, index) }))
        .route(&format!("{prefix}/app.js"), get(move || async move { asset(JS, js) }))
        .route(&format!("{prefix}/style.css"), get(move || async move { asset(CSS, css) }))
}

fn asset(content_type: &'static str, body: impl IntoResponse) -> Response {
    ([(CONTENT_TYPE, content_type), (CACHE_CONTROL, "no-cache")], body).into_response()
}

/// Who is asking. The shared access token (`CHAT_ACCESS_TOKEN`) is the primary user's, and so is
/// a request with no token when none is set; any other token is looked up among the users' own.
async fn require_token(State(state): State<AppState>, mut req: Request, next: Next) -> Result<Response, AppError> {
    let presented: Option<[u8; 32]> = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| Sha256::digest(token.trim().as_bytes()).into());
    let user = match (&state.token_digest, presented) {
        (Some(expected), Some(digest)) if bool::from(digest.as_slice().ct_eq(expected)) => state.primary,
        (None, None) => state.primary,
        // Found by the token's hash: a user's token is random and long, so the lookup gives
        // nothing away that guessing wouldn't.
        (expected, Some(digest)) => match state.store.user_by_token(&hex::encode(digest)).await? {
            Some(user) => user.id,
            // A server with no access token takes anyone, whatever token an app still sends.
            None if expected.is_none() => state.primary,
            None => return Err(AppError::Unauthorized),
        },
        (Some(_), None) => return Err(AppError::Unauthorized),
    };
    req.extensions_mut().insert(CurrentUser(user));
    Ok(next.run(req).await)
}

#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
struct ApiJson<T>(T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(AppError))]
struct ApiQuery<T>(T);

fn parse_id(raw: &str, what: &'static str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw).map_err(|_| AppError::NotFound(what))
}

async fn find_conversation(state: &AppState, raw_id: &str) -> Result<Conversation, AppError> {
    let id = parse_id(raw_id, "conversation not found")?;
    state.store.get_conversation(id).await?.ok_or(AppError::NotFound("conversation not found"))
}

/// A conversation that may be contributed: any but a private one.
async fn open_conversation(state: &AppState, raw_id: &str) -> Result<Conversation, AppError> {
    let conversation = find_conversation(state, raw_id).await?;
    if conversation.private {
        return Err(AppError::Conflict("a private conversation can't be contributed: nothing of it is kept or sent".into()));
    }
    Ok(conversation)
}

async fn find_turn(state: &AppState, raw_id: &str) -> Result<Turn, AppError> {
    let id = parse_id(raw_id, "turn not found")?;
    state.store.get_turn(id).await?.ok_or(AppError::NotFound("turn not found"))
}

/// Bring a turn's status up to date with its cedar run, until the run is over.
pub(crate) async fn refresh(state: &AppState, turn: Turn) -> Result<Turn, AppError> {
    if cedar::is_terminal(&turn.status) {
        return Ok(turn);
    }
    let (status, final_text, error, stop_reason) = match state.cedar.get_run(turn.run_id).await? {
        Some(run) => (run.status, run.final_text, run.error, run.stop_reason),
        None => ("failed".to_string(), None, Some("the run no longer exists on the cedar server".to_string()), None),
    };
    if status != turn.status {
        state.store.update_turn(turn.id, &status, final_text.as_deref(), error.as_deref(), stop_reason.as_deref()).await?;
    }
    let finished_at = cedar::is_terminal(&status).then(|| turn.finished_at.unwrap_or_else(Utc::now));
    Ok(Turn { status, final_text, error, stop_reason, finished_at, ..turn })
}

/// Refresh every unfinished turn. If cedar can't be reached, keep the last known state.
async fn refresh_all(state: &AppState, turns: Vec<Turn>) -> Vec<Turn> {
    let mut out = Vec::with_capacity(turns.len());
    for turn in turns {
        out.push(match refresh(state, turn.clone()).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(turn_id = %turn.id, error = %e, "could not refresh turn");
                turn
            }
        });
    }
    out
}

// -- config ---------------------------------------------------------------------------------

async fn client_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "auth_required": state.token_digest.is_some(),
        "admin_enabled": state.admin_digest.is_some(),
        // Whether the models in Settings -> Server are open to everyone with the access token
        // ("users", the default) or need the admin token ("admins", CHAT_MODELS_ACCESS).
        "models_access": state.cfg.models_access.as_str(),
        // This chat server has the /api/memory routes (whether cedar has memory is in their answer).
        "memory": true,
        // Likewise the /api/records routes (whether cedar has records is in their answer).
        "records": true,
        // Whether a conversation can be sent to a corpus (CHAT_CONTRIBUTIONS_URL); the review
        // dialog exports to a file either way.
        "contributions": state.cfg.contributions_url.is_some(),
        // This chat server has private conversations (`"private": true` on a new one), so the
        // phone apps offer them; on an older server they don't.
        "private_chats": true,
        // Where the page opens an issue for a report the person wants the project to see
        // (CHAT_FEEDBACK_ISSUES_URL); null when that isn't offered. They file it themselves.
        "feedback_issues_url": state.cfg.feedback_issues_url,
    }))
}

/// Settings -> Phone: the address, token and QR code the iOS and Android apps connect with, and
/// whether Tailscale is ready to serve the chat to them (see `pairing`).
async fn get_pairing(Scoped(state): Scoped, headers: HeaderMap) -> Json<pairing::Pairing> {
    // The primary user's phone gets the shared token; anyone else's gets the token they signed
    // in with, never the shared one.
    let token = if state.user() == state.primary {
        state.cfg.access_token.clone()
    } else {
        headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split_once(' '))
            .map(|(_, token)| token.trim().to_string())
    };
    Json(pairing::pairing(&state, token).await)
}

// -- updates --------------------------------------------------------------------------------

/// The update banner: this version, the latest release, and how far an install got.
async fn get_update(Scoped(state): Scoped) -> Json<update::Status> {
    Json(update::status(&state, state.admin_digest.is_some()))
}

/// "Check now": looks the latest release up (at most once a minute) and answers like `GET`.
async fn check_update(Scoped(state): Scoped) -> Json<update::Status> {
    if let Err(e) = update::check(&state, false).await {
        tracing::warn!(error = %format!("{e:#}"), "update check failed");
    }
    Json(update::status(&state, state.admin_digest.is_some()))
}

/// The banner's Update: hands the latest release to the root `cedar-update` unit (admin only).
async fn install_update(Scoped(state): Scoped, headers: HeaderMap) -> Result<(StatusCode, Json<update::Status>), AppError> {
    if state.admin_digest.is_none() {
        return Err(AppError::Forbidden("updates are installed by hand here (CHAT_ADMIN_TOKEN is not set)".into()));
    }
    require_admin(&state, &headers)?;
    match update::request_install(&state) {
        Ok(_) => Ok((StatusCode::ACCEPTED, Json(update::status(&state, true)))),
        Err(e @ update::RequestError::NotSetUp) => Err(AppError::Forbidden(e.to_string())),
        Err(e @ (update::RequestError::NothingNewer | update::RequestError::Running)) => Err(AppError::Conflict(e.to_string())),
        Err(update::RequestError::Io(e)) => Err(AppError::Internal(anyhow::anyhow!(e))),
    }
}

// -- conversations --------------------------------------------------------------------------

#[derive(Serialize)]
struct ConversationList {
    conversations: Vec<Conversation>,
}

async fn list_conversations(Scoped(state): Scoped) -> Result<Json<ConversationList>, AppError> {
    Ok(Json(ConversationList { conversations: state.store.list_conversations(MAX_CONVERSATIONS).await? }))
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

#[derive(Serialize)]
struct SearchResults {
    conversations: Vec<SearchHit>,
}

/// The conversations that have every word of `q` in their title or messages, most recently
/// active first, chats and scheduled runs alike: the search box of every client (the web pages,
/// the desktop app, the iOS and Android apps). An empty `q` finds nothing.
async fn search_conversations(Scoped(state): Scoped, ApiQuery(query): ApiQuery<SearchQuery>) -> Result<Json<SearchResults>, AppError> {
    if query.q.chars().count() > MAX_SEARCH_CHARS {
        return Err(AppError::BadRequest(format!("q is longer than {MAX_SEARCH_CHARS} characters")));
    }
    Ok(Json(SearchResults { conversations: state.store.search_conversations(&query.q, MAX_SEARCH_RESULTS).await? }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleBody {
    title: Option<String>,
}

fn clean_title(title: Option<String>) -> Result<Option<String>, AppError> {
    match title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) if t.chars().count() > MAX_TITLE_CHARS => {
            Err(AppError::BadRequest(format!("title is longer than {MAX_TITLE_CHARS} characters")))
        }
        other => Ok(other),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewConversationBody {
    title: Option<String>,
    /// A private conversation: in no list, kept out of memory, records and traces, announced
    /// nowhere, and deleted when it is left (see `private`). Only at the start: a conversation
    /// can't be made private, or open, later.
    #[serde(default)]
    private: bool,
}

async fn create_conversation(
    Scoped(state): Scoped,
    ApiJson(body): ApiJson<NewConversationBody>,
) -> Result<(StatusCode, Json<Conversation>), AppError> {
    let title = clean_title(body.title)?;
    let conversation = match body.private {
        true => state.store.create_private_conversation(title.as_deref()).await?,
        false => state.store.create_conversation(title.as_deref()).await?,
    };
    Ok((StatusCode::CREATED, Json(conversation)))
}

#[derive(Serialize)]
struct ConversationDetail {
    conversation: Conversation,
    turns: Vec<Turn>,
    /// The agent whose run this conversation is: `{id, title, trigger_kind}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<Value>,
    /// What was decided about the follow-ups suggested in its turns; one not listed is open.
    followups: Vec<crate::store::Followup>,
    /// What was decided about what the model reported it could not do; one not listed is open.
    limitations: Vec<crate::store::Feedback>,
}

async fn get_conversation(
    Scoped(state): Scoped,
    Path(id): Path<String>,
) -> Result<Json<ConversationDetail>, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    let turns = refresh_all(&state, state.store.list_turns(conversation.id).await?).await;
    let agent = match conversation.schedule_id {
        Some(id) => state.store.get_schedule(id).await?.map(|s| json!({ "id": s.id, "title": s.title, "trigger_kind": s.trigger_kind })),
        None => None,
    };
    let followups = state.store.list_followups(conversation.id).await?;
    let limitations = state.store.list_conversation_feedback(conversation.id).await?;
    Ok(Json(ConversationDetail { conversation, turns, agent, followups, limitations }))
}

async fn rename_conversation(
    Scoped(state): Scoped,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<TitleBody>,
) -> Result<Json<Conversation>, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    let title = clean_title(body.title)?.ok_or_else(|| AppError::BadRequest("title must not be empty".into()))?;
    state.store.rename_conversation(conversation.id, &title).await?;
    Ok(Json(Conversation { title: Some(title), ..conversation }))
}

async fn delete_conversation(Scoped(state): Scoped, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    // Leaving a private conversation ends it, a response in flight included: there is no list
    // to come back to it from and stop that first.
    if conversation.private {
        crate::private::end(&state, &conversation).await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let mut turns = Vec::new();
    for turn in state.store.list_turns(conversation.id).await? {
        turns.push(refresh(&state, turn).await?);
    }
    if turns.iter().any(|t| !cedar::is_terminal(&t.status)) {
        return Err(AppError::Conflict("a response is still being generated; stop it first".into()));
    }
    for turn in &turns {
        // Best effort: the conversation is gone either way.
        if let Err(e) = state.cedar.delete_run(turn.run_id).await {
            tracing::warn!(run_id = %turn.run_id, error = %e, "could not delete cedar run");
        }
    }
    state.store.delete_conversation(conversation.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct StarredList {
    conversations: Vec<StarredConversation>,
}

/// Starred conversations, most recently starred first: chats and scheduled runs alike, so every
/// client (the web pages, the desktop app, the iOS app) shows the same list.
async fn list_starred(Scoped(state): Scoped) -> Result<Json<StarredList>, AppError> {
    Ok(Json(StarredList { conversations: state.store.list_starred(MAX_CONVERSATIONS).await? }))
}

async fn star_conversation(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Conversation>, AppError> {
    set_starred(&state, &id, true).await
}

async fn unstar_conversation(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Conversation>, AppError> {
    set_starred(&state, &id, false).await
}

/// Both are idempotent, so a client that retries, or two clients that star the same
/// conversation, end up in the same state.
async fn set_starred(state: &AppState, id: &str, starred: bool) -> Result<Json<Conversation>, AppError> {
    let conversation = find_conversation(state, id).await?;
    if conversation.private && starred {
        return Err(AppError::Conflict("a private conversation can't be starred: it is not kept".into()));
    }
    state.store.set_starred(conversation.id, starred).await?;
    Ok(Json(state.store.get_conversation(conversation.id).await?.unwrap_or(conversation)))
}

/// The conversation has been looked at: its finished responses leave the inbox. Unfinished
/// turns are refreshed first, so a response the page just watched end counts as seen.
async fn mark_seen(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Conversation>, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    refresh_all(&state, state.store.list_turns(conversation.id).await?).await;
    state.store.mark_seen(conversation.id).await?;
    Ok(Json(state.store.get_conversation(conversation.id).await?.unwrap_or(conversation)))
}

#[derive(Serialize)]
struct Inbox {
    items: Vec<InboxItem>,
    /// How many there are in all (the list is capped).
    unread: i64,
}

/// Finished responses nobody has looked at, newest first, scheduled runs included.
async fn inbox(Scoped(state): Scoped) -> Result<Json<Inbox>, AppError> {
    Ok(Json(Inbox { items: state.store.inbox(MAX_INBOX).await?, unread: state.store.count_unseen().await? }))
}

/// Server-Sent Events for the page: a `turn.finished` event for every response that ends, as
/// cedar tells this service. It has no replay; the page reads `/api/inbox` when it connects.
async fn notice_events(Scoped(state): Scoped) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    relay(state.notices.subscribe(), state.user(), state.shutdown.clone())
}

/// Server-Sent Events for the Cedar desktop app: a `notification` event for every message
/// sent by the `desktop` method, which the app shows in Notification Center (a toast on Windows). Connecting is what
/// sets the method up; the first time, it also becomes the default if none was chosen.
async fn desktop_events(Scoped(state): Scoped) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, AppError> {
    // Listening before saving, so a test send right after the app connects reaches it.
    let rx = state.desktop.subscribe();
    let mut settings = state.store.notification_settings().await?;
    if settings.desktop_connected_at.is_none() && settings.default_method == Method::None {
        settings.default_method = Method::Desktop;
    }
    settings.desktop_connected_at = Some(Utc::now());
    state.store.set_notification_settings(&settings).await?;
    Ok(relay(rx, state.user(), state.shutdown.clone()))
}

/// What a broadcast channel of JSON has for `user`, as Server-Sent Events named by each value's
/// `type`, until `shutdown` is cancelled.
fn relay(rx: broadcast::Receiver<(Uuid, Value)>, user: Uuid, shutdown: CancellationToken) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    Sse::new(relay_stream(rx, user, shutdown)).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
}

fn relay_stream(rx: broadcast::Receiver<(Uuid, Value)>, user: Uuid, shutdown: CancellationToken) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    futures_util::stream::unfold((rx, shutdown), move |(mut rx, shutdown)| async move {
        loop {
            let received = tokio::select! {
                _ = shutdown.cancelled() => return None,
                received = rx.recv() => received,
            };
            match received {
                // Someone else's.
                Ok((to, _)) if to != user => continue,
                Ok((_, notice)) => {
                    let event = SseEvent::default()
                        .event(notice["type"].as_str().unwrap_or("notice"))
                        .data(notice.to_string());
                    return Some((Ok(event), (rx, shutdown)));
                }
                // Behind: some were missed (the page re-reads the inbox on the next notice); keep going.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

// -- notifications out of the app (channels) ------------------------------------------------

/// The settings as the page sees them (no secrets), with the subscribed browsers and the
/// registered iPhones and Android phones.
async fn notification_status(state: &AppState) -> Result<Value, AppError> {
    let settings = state.store.notification_settings().await?;
    let subscriptions = state.store.push_subscriptions().await?;
    let devices = state.store.apns_devices().await?;
    let androids = state.store.android_devices().await?;
    let mut status = settings.status(&subscriptions, &devices, &androids);
    status["desktop"]["listening"] = json!(state.desktop.receiver_count() > 0);
    Ok(status)
}

/// How many browsers, iPhones and Android phones a push could reach right now.
async fn reach(state: &AppState) -> Result<Reach, AppError> {
    Ok(Reach::new(&state.store.push_subscriptions().await?, &state.store.apns_devices().await?, &state.store.android_devices().await?))
}

/// The page subscribes a browser with the server's public key, so the keys are made here.
async fn get_notifications(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    channels::ensure_vapid(&state.store).await?;
    Ok(Json(notification_status(&state).await?))
}

/// Any subset of the settings. A secret left out keeps its value; an empty string clears it.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct NotificationsBody {
    /// For conversations, and for scheduled agents without a method of their own.
    default_method: Option<String>,
    scope: Option<String>,
    pushover: Option<PushoverBody>,
    email: Option<EmailBody>,
    ios: Option<ApnsBody>,
    android: Option<AndroidBody>,
    /// The contact the push services may write to about this server (an email address).
    contact: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ApnsBody {
    team_id: Option<String>,
    key_id: Option<String>,
    /// The `.p8` file's contents.
    private_key: Option<String>,
    topic: Option<String>,
    environment: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AndroidBody {
    /// The Firebase service account's JSON file, as pasted.
    service_account: Option<String>,
}

/// A service account's JSON file is a little over 2000 characters.
const MAX_SERVICE_ACCOUNT_CHARS: usize = 16384;

/// A `.p8` file is a few hundred characters; this leaves room for one pasted with its
/// surroundings.
const MAX_KEY_CHARS: usize = 4096;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PushoverBody {
    token: Option<String>,
    user: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct EmailBody {
    host: Option<String>,
    port: Option<u16>,
    security: Option<String>,
    username: Option<String>,
    password: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

const MAX_SETTING_CHARS: usize = 500;

/// A trimmed field; too long is refused.
fn short(what: &str, value: String) -> Result<String, AppError> {
    let value = value.trim().to_string();
    if value.chars().count() > MAX_SETTING_CHARS {
        return Err(AppError::BadRequest(format!("{what} is longer than {MAX_SETTING_CHARS} characters")));
    }
    Ok(value)
}

async fn put_notifications(Scoped(state): Scoped, ApiJson(body): ApiJson<NotificationsBody>) -> Result<Json<Value>, AppError> {
    let mut settings = state.store.notification_settings().await?;
    if let Some(m) = &body.default_method {
        settings.default_method = Method::parse(m).ok_or_else(|| AppError::BadRequest(format!("default_method must be none, {}, not {m:?}", Method::NAMES)))?;
    }
    if let Some(s) = &body.scope {
        settings.scope = Scope::parse(s).ok_or_else(|| AppError::BadRequest(format!("scope must be attention or all, not {s:?}")))?;
    }
    if let Some(p) = body.pushover {
        if let Some(token) = p.token {
            settings.pushover.token = short("the Pushover token", token)?;
        }
        if let Some(user) = p.user {
            settings.pushover.user = short("the Pushover user key", user)?;
        }
    }
    if let Some(e) = body.email {
        if let Some(host) = e.host {
            settings.email.host = short("the SMTP host", host)?;
        }
        if let Some(port) = e.port {
            settings.email.port = port;
        }
        if let Some(security) = &e.security {
            settings.email.security = Security::parse(security).ok_or_else(|| AppError::BadRequest(format!("security must be tls, starttls or none, not {security:?}")))?;
        }
        if let Some(username) = e.username {
            settings.email.username = short("the SMTP user name", username)?;
        }
        if let Some(password) = e.password {
            settings.email.password = short("the SMTP password", password)?;
        }
        if let Some(from) = e.from {
            let from = short("the sender address", from)?;
            if !from.is_empty() {
                from.parse::<lettre::message::Mailbox>().map_err(|err| AppError::BadRequest(format!("the sender address is invalid: {err}")))?;
            }
            settings.email.from = from;
        }
        if let Some(to) = e.to {
            let to = short("the recipient address", to)?;
            if !to.is_empty() {
                to.parse::<lettre::message::Mailbox>().map_err(|err| AppError::BadRequest(format!("the recipient address is invalid: {err}")))?;
            }
            settings.email.to = to;
        }
    }
    if let Some(a) = body.ios {
        if let Some(team_id) = a.team_id {
            settings.apns.team_id = short("the team id", team_id)?;
        }
        if let Some(key_id) = a.key_id {
            settings.apns.key_id = short("the key id", key_id)?;
        }
        if let Some(key) = a.private_key {
            let key = key.trim().to_string();
            if key.chars().count() > MAX_KEY_CHARS {
                return Err(AppError::BadRequest(format!("the APNs key is longer than {MAX_KEY_CHARS} characters")));
            }
            if !key.is_empty() && !key.contains("-----BEGIN PRIVATE KEY-----") {
                return Err(AppError::BadRequest("the APNs key must be the contents of the .p8 file, starting with -----BEGIN PRIVATE KEY-----".into()));
            }
            settings.apns.private_key = key;
        }
        if let Some(topic) = a.topic {
            let topic = short("the bundle id", topic)?;
            settings.apns.topic = if topic.is_empty() { channels::DEFAULT_APNS_TOPIC.into() } else { topic };
        }
        if let Some(environment) = &a.environment {
            settings.apns.environment = ApnsEnvironment::parse(environment).ok_or_else(|| AppError::BadRequest(format!("environment must be production or sandbox, not {environment:?}")))?;
        }
    }
    if let Some(a) = body.android {
        if let Some(json) = a.service_account {
            let json = json.trim().to_string();
            if json.chars().count() > MAX_SERVICE_ACCOUNT_CHARS {
                return Err(AppError::BadRequest(format!("the service-account key is longer than {MAX_SERVICE_ACCOUNT_CHARS} characters")));
            }
            if !json.is_empty() {
                channels::ServiceAccount::parse(&json).and_then(|account| account.signing_key()).map_err(AppError::BadRequest)?;
            }
            settings.android.service_account = json;
        }
    }
    if let Some(contact) = body.contact {
        let contact = short("the contact", contact)?;
        match settings.vapid.as_mut() {
            Some(v) => v.contact = contact,
            None if !contact.is_empty() => {
                // Keys are made when a browser subscribes; keep the contact for then.
                state.store.set_notification_settings(&settings).await?;
                settings = channels::ensure_vapid(&state.store).await?;
                settings.vapid.as_mut().expect("just made").contact = contact;
            }
            None => {}
        }
    }
    // The default method must be able to send, so a half-filled form can't silence everything.
    settings.check(settings.default_method, reach(&state).await?).map_err(|why| AppError::BadRequest(format!("the default method can't send: {why}")))?;
    state.store.set_notification_settings(&settings).await?;
    notification_status(&state).await.map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestBody {
    method: String,
}

/// Send a test message by `method` with the saved settings. The outcome is recorded like a
/// real delivery and returned: `{ok, detail}`, as a 502 when it failed.
async fn test_notification(Scoped(state): Scoped, ApiJson(body): ApiJson<TestBody>) -> Result<Json<Value>, AppError> {
    let method = Method::parse(&body.method).ok_or_else(|| AppError::BadRequest(format!("unknown method {:?}", body.method)))?;
    if method == Method::None {
        return Err(AppError::BadRequest("pick a method to test".into()));
    }
    let settings = state.store.notification_settings().await?;
    let lang = Lang::of(state.store.preferences().await?.locale.as_deref());
    let message = channels::Message {
        title: lang.test_title().into(),
        body: lang.test_body().into(),
        url: format!("{}/", state.cfg.public_url),
        tag: "test".into(),
        conversation_id: None,
        open_label: lang.open_in_cedar().into(),
    };
    let outcome = channels::send(&state, &settings, method, &message).await;
    channels::record(&state.store, method, true, &outcome).await?;
    match outcome {
        Ok(detail) => Ok(Json(json!({ "ok": true, "detail": detail }))),
        Err(detail) => Err(AppError::BadGateway(detail)),
    }
}

/// A browser's `PushSubscription`, as `JSON.stringify` gives it.
#[derive(Deserialize)]
struct SubscriptionBody {
    endpoint: String,
    keys: SubscriptionKeys,
    #[serde(default)]
    #[allow(dead_code)]
    expiration_time: Option<Value>,
}

#[derive(Deserialize)]
struct SubscriptionKeys {
    p256dh: String,
    auth: String,
}

/// A browser turned push on: keep its subscription. The server's public key is returned with
/// the settings for the page to subscribe with (`GET /api/notifications` has it too).
async fn subscribe_push(Scoped(state): Scoped, headers: HeaderMap, ApiJson(body): ApiJson<SubscriptionBody>) -> Result<Json<Value>, AppError> {
    if !(body.endpoint.starts_with("https://") || body.endpoint.starts_with("http://")) || body.endpoint.len() > 2048 {
        return Err(AppError::BadRequest("the subscription's endpoint must be a URL".into()));
    }
    if body.keys.p256dh.is_empty() || body.keys.auth.is_empty() {
        return Err(AppError::BadRequest("the subscription has no keys".into()));
    }
    channels::ensure_vapid(&state.store).await?;
    let agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|a| a.chars().take(200).collect::<String>());
    let sub = state.store.add_push_subscription(&body.endpoint, &body.keys.p256dh, &body.keys.auth, agent.as_deref()).await?;
    let mut status = notification_status(&state).await?;
    status["subscription_id"] = json!(sub.id);
    Ok(Json(status))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnsubscribeBody {
    endpoint: String,
}

/// The browser turned push off itself: forget its endpoint.
async fn unsubscribe_push(Scoped(state): Scoped, ApiJson(body): ApiJson<UnsubscribeBody>) -> Result<Json<Value>, AppError> {
    state.store.delete_push_subscription_by_endpoint(&body.endpoint).await?;
    notification_status(&state).await.map(Json)
}

async fn delete_push_subscription(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let id = parse_id(&id, "subscription not found")?;
    if !state.store.delete_push_subscription(id).await? {
        return Err(AppError::NotFound("subscription not found"));
    }
    notification_status(&state).await.map(Json)
}

/// What the iOS app sends once APNs has given it a device token.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApnsDeviceBody {
    /// The token as hex, the way the app reads it from `deviceToken`.
    token: String,
    /// The device's name, for Settings.
    name: Option<String>,
}

/// The iOS app registered for push: keep its device token.
async fn register_apns_device(Scoped(state): Scoped, ApiJson(body): ApiJson<ApnsDeviceBody>) -> Result<Json<Value>, AppError> {
    let token = body.token.trim().to_ascii_lowercase();
    if token.len() < 32 || token.len() > 256 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest("the device token must be the hex APNs gave the app".into()));
    }
    let name = body.name.map(|n| short("the device name", n)).transpose()?.filter(|n| !n.is_empty());
    let device = state.store.add_apns_device(&token, name.as_deref()).await?;
    let mut status = notification_status(&state).await?;
    status["device_id"] = json!(device.id);
    Ok(Json(status))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnregisterApnsBody {
    token: String,
}

/// The app turned push off, or left for another server: forget its token.
async fn unregister_apns_device(Scoped(state): Scoped, ApiJson(body): ApiJson<UnregisterApnsBody>) -> Result<Json<Value>, AppError> {
    state.store.delete_apns_device_by_token(&body.token.trim().to_ascii_lowercase()).await?;
    notification_status(&state).await.map(Json)
}

async fn delete_apns_device(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let id = parse_id(&id, "device not found")?;
    if !state.store.delete_apns_device(id).await? {
        return Err(AppError::NotFound("device not found"));
    }
    notification_status(&state).await.map(Json)
}

/// What the Android app sends once it has a way to be pushed to: an FCM registration token, or
/// the endpoint its UnifiedPush distributor (ntfy, say) gave it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AndroidDeviceBody {
    /// `fcm` or `unifiedpush`.
    transport: String,
    /// The FCM registration token, or the UnifiedPush endpoint URL.
    token: String,
    /// The device's name, for Settings.
    name: Option<String>,
}

/// An FCM registration token is about 160 characters; a UnifiedPush endpoint a URL.
const MAX_ANDROID_TOKEN_CHARS: usize = 2048;

/// The Android app registered for push: keep its token or endpoint. Returns the settings with
/// the phone as `device` (and its id as `device_id`, as for an iPhone).
async fn register_android_device(Scoped(state): Scoped, ApiJson(body): ApiJson<AndroidDeviceBody>) -> Result<Json<Value>, AppError> {
    let transport = body.transport.trim().to_ascii_lowercase();
    let token = body.token.trim().to_string();
    match transport.as_str() {
        channels::FCM => {
            if token.is_empty() || token.len() > MAX_ANDROID_TOKEN_CHARS || !token.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(AppError::BadRequest("the FCM token must be the registration token Firebase gave the app".into()));
            }
        }
        channels::UNIFIEDPUSH => {
            // The payload goes unencrypted (it only names a turn), so at least not in the clear.
            let url = reqwest::Url::parse(&token).ok().filter(|u| u.scheme() == "https" && u.host_str().is_some());
            if url.is_none() || token.len() > MAX_ANDROID_TOKEN_CHARS {
                return Err(AppError::BadRequest("the UnifiedPush endpoint must be an https:// URL; only https endpoints are accepted".into()));
            }
        }
        _ => return Err(AppError::BadRequest(format!("transport must be fcm or unifiedpush, not {:?}", body.transport))),
    }
    let name = body.name.map(|n| short("the device name", n)).transpose()?.filter(|n| !n.is_empty());
    let device = state.store.add_android_device(&transport, &token, name.as_deref()).await?;
    let mut status = notification_status(&state).await?;
    status["device_id"] = json!(device.id);
    status["device"] = json!(device);
    Ok(Json(status))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnregisterAndroidBody {
    token: String,
}

/// The app turned push off, or left for another server: forget its token or endpoint.
async fn unregister_android_device(Scoped(state): Scoped, ApiJson(body): ApiJson<UnregisterAndroidBody>) -> Result<Json<Value>, AppError> {
    state.store.delete_android_device_by_token(body.token.trim()).await?;
    notification_status(&state).await.map(Json)
}

async fn delete_android_device(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let id = parse_id(&id, "device not found")?;
    if !state.store.delete_android_device(id).await? {
        return Err(AppError::NotFound("device not found"));
    }
    notification_status(&state).await.map(Json)
}

// -- messages -------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageBody {
    content: String,
    /// A registered model's id. Absent: the default model from Settings.
    model: Option<String>,
    /// One of the model's reasoning levels. Absent: the model's default.
    reasoning: Option<String>,
    /// PDFs for any model; images for one registered as accepting them.
    attachments: Option<Vec<Attachment>>,
    /// Whether the run plans: `off`, `on`, `auto` or `tool`. Absent: the plan setting from Settings.
    plan: Option<String>,
    /// The language the page or app is shown in (a BCP 47 tag). Absent: the one from Settings.
    locale: Option<String>,
    /// The device's time zone (an IANA name such as `America/Los_Angeles`). Absent: the one a
    /// message last sent.
    timezone: Option<String>,
}

/// The files sent with a message: checked the way cedar checks them (count, name, type, base64,
/// sizes) so a mistake is reported before anything is sent, and summarized for the turn. The
/// size comes from the base64 length; cedar does the real decoding.
fn clean_attachments(attachments: Vec<Attachment>) -> Result<(Vec<Attachment>, Vec<AttachmentInfo>), AppError> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(AppError::BadRequest(format!("at most {MAX_ATTACHMENTS} files per message")));
    }
    let mut infos = Vec::with_capacity(attachments.len());
    let mut total = 0usize;
    for (i, a) in attachments.iter().enumerate() {
        let which = |what: &str| AppError::BadRequest(format!("file {} ({:?}): {what}", i + 1, a.name));
        let name = a.name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest(format!("file {}: name must not be empty", i + 1)));
        }
        if name.chars().count() > MAX_ATTACHMENT_NAME_CHARS || name.chars().any(char::is_control) {
            return Err(which("name is malformed"));
        }
        if !ATTACHMENT_TYPES.contains(&a.media_type.as_str()) {
            return Err(which("only PNG, JPEG, GIF, WebP and PDF files can be attached"));
        }
        let data = a.data.as_bytes();
        if data.is_empty() {
            return Err(which("the file is empty"));
        }
        if data.len() % 4 != 0 || !data.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')) {
            return Err(which("data is not valid base64"));
        }
        let padding = data.iter().rev().take_while(|b| **b == b'=').count();
        let size = data.len() / 4 * 3 - padding;
        if size > MAX_ATTACHMENT_BYTES {
            return Err(which(&format!("larger than {} MiB", MAX_ATTACHMENT_BYTES >> 20)));
        }
        total += size;
        if total > MAX_ATTACHMENTS_TOTAL_BYTES {
            return Err(AppError::BadRequest(format!("the files add up to more than {} MiB", MAX_ATTACHMENTS_TOTAL_BYTES >> 20)));
        }
        infos.push(AttachmentInfo { name: name.to_string(), media_type: a.media_type.clone(), size: size as u64 });
    }
    Ok((attachments, infos))
}

/// A user message or a schedule's prompt: trimmed, non-empty, and not too long for cedar.
pub(crate) fn clean_message(what: &str, content: &str) -> Result<String, AppError> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::BadRequest(format!("{what} must not be empty")));
    }
    if content.len() > MAX_MESSAGE_BYTES {
        return Err(AppError::BadRequest(format!("{what} is longer than {MAX_MESSAGE_BYTES} bytes")));
    }
    Ok(content.to_string())
}

/// Blank means absent, for the model and reasoning fields.
fn blank_is_none(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(String::from)
}

/// The registered model `model` names (by id, or by its name on the server), if any.
fn find_model<'a>(models: &'a [Value], model: &str) -> Option<&'a Value> {
    models.iter().find(|m| m["id"] == model).or_else(|| models.iter().find(|m| m["model"] == model))
}

/// A model and reasoning level to save (for the defaults or a schedule), checked against
/// cedar's registry so a stale choice can't break every later run: the model must be
/// registered, and the level one the model (or, without a model, the default one) offers.
async fn clean_model_choice(
    state: &AppState,
    model: Option<&str>,
    reasoning: Option<&str>,
) -> Result<(Option<String>, Option<String>), AppError> {
    let model = blank_is_none(model);
    let reasoning = blank_is_none(reasoning);
    if model.is_none() && reasoning.is_none() {
        return Ok((None, None));
    }
    let models = state.cedar.models().await?;
    let entry = match &model {
        Some(m) => find_model(&models, m).ok_or_else(|| AppError::BadRequest(format!("model {m:?} is not registered")))?,
        None => models
            .iter()
            .find(|m| m["is_default"] == true)
            .ok_or_else(|| AppError::BadRequest("no model is registered yet".into()))?,
    };
    if let Some(level) = &reasoning {
        let offered = entry["reasoning_levels"].as_array().is_some_and(|ls| ls.iter().any(|l| l == level));
        if !offered {
            return Err(AppError::BadRequest(format!("model {:?} offers no reasoning level {level:?}", entry["id"])));
        }
    }
    Ok((model.map(|_| entry["id"].as_str().unwrap_or_default().to_string()), reasoning))
}

/// A saved default model, if cedar still has it. `None` lets cedar use its own default. Also
/// `None` while users don't choose models (cedar's `model_selection` is off): cedar then chooses.
async fn still_registered(state: &AppState, model: Option<&str>) -> Option<String> {
    let model = model?;
    match state.cedar.model_listing().await {
        Ok((_, false)) => None,
        Ok((models, true)) => find_model(&models, model).map(|m| m["id"].as_str().unwrap_or(model).to_string()),
        // Can't tell: let cedar decide.
        Err(_) => Some(model.to_string()),
    }
}

/// The conversation's turns, brought up to date. `Err(Conflict)` if one is still going.
async fn settled_turns(state: &AppState, conversation: &Conversation) -> Result<Vec<Turn>, AppError> {
    let mut turns = Vec::new();
    for turn in state.store.list_turns(conversation.id).await? {
        turns.push(refresh(state, turn).await?);
    }
    if turns.iter().any(|t| !cedar::is_terminal(&t.status)) {
        return Err(AppError::Conflict("wait for the current response to finish, or stop it".into()));
    }
    Ok(turns)
}

/// The finished exchanges of a conversation, for resending as text.
fn exchanges(turns: &[Turn]) -> Vec<Exchange<'_>> {
    turns
        .iter()
        .filter(|t| t.status == "completed")
        .filter_map(|t| Some(Exchange { user: &t.prompt, assistant: t.final_text.as_deref()? }))
        .collect()
}

async fn send_message(
    Scoped(state): Scoped,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<MessageBody>,
) -> Result<(StatusCode, Json<Turn>), AppError> {
    let content = clean_message("message", &body.content)?;
    // The model and level go to cedar as given: it checks them against its registry.
    let requested = blank_is_none(body.model.as_deref());
    let reasoning = blank_is_none(body.reasoning.as_deref());
    let plan = clean_plan(body.plan)?;
    let locale = i18n::clean_locale(body.locale.as_deref()).map_err(AppError::BadRequest)?;
    let timezone = agents::clean_zone(body.timezone.as_deref())?;
    // Whether the model takes them is cedar's call (its registry says), relayed as it answers.
    let (attachments, infos) = clean_attachments(body.attachments.unwrap_or_default())?;
    let message = NewMessage { content: &content, model: requested, reasoning, plan, locale, timezone, attachments, infos };
    let turn = post_message(&state, ConversationRef::Id(&id), message).await?;
    // Should the response wait for an OK while nobody looks at the page, they are told. Not
    // about a private conversation, though: nothing of it leaves the app.
    let private = state.store.get_conversation(turn.conversation_id).await?.is_some_and(|c| c.private);
    if !private {
        crate::attention::watch(state.clone(), turn.clone());
    }
    Ok((StatusCode::CREATED, Json(turn)))
}

/// A message to a conversation, already checked: what [`post_message`] sends.
#[derive(Default)]
pub(crate) struct NewMessage<'a> {
    pub content: &'a str,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub plan: Option<String>,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub attachments: Vec<Attachment>,
    pub infos: Vec<AttachmentInfo>,
}

/// Which conversation a message goes to: one the client named, or one already looked up.
pub(crate) enum ConversationRef<'a> {
    Id(&'a str),
    Found(Uuid),
}

/// Send a message to one of `state`'s user's conversations as a new turn: a cedar run that
/// continues the conversation's last one, with someone reading along (the run may wait for its
/// plan to be agreed to and risky calls to be approved). The page and Slack (`slack`) both send
/// messages this way. `409` while the conversation's last response is still going.
pub(crate) async fn post_message(state: &AppState, conversation: ConversationRef<'_>, message: NewMessage<'_>) -> Result<Turn, AppError> {
    let NewMessage { content, model: requested, reasoning, plan, locale, timezone, attachments, infos } = message;
    let mut prefs = state.store.preferences().await?;
    // The device's zone is kept for the runs nobody sends a message for (agents, Slack), so
    // their "today" is the user's too.
    if timezone.is_some() && timezone != prefs.timezone {
        prefs.timezone = timezone.clone();
        state.store.set_preferences(&prefs).await?;
    }

    let _guard = state.send_lock.lock().await;
    let conversation = match conversation {
        ConversationRef::Id(id) => find_conversation(state, id).await?,
        ConversationRef::Found(id) => state.store.get_conversation(id).await?.ok_or(AppError::NotFound("conversation not found"))?,
    };
    let turns = settled_turns(state, &conversation).await?;
    let settings = RunSettings::resolve(state, &prefs, requested, reasoning, conversation.tools.clone()).await;
    let model = settings.model.as_deref();
    // Someone is reading along, so the run may ask: agree to its plan, approve a risky call.
    let options = |parent| RunOptions {
        attachments: Some(&attachments),
        plan: Some(plan.as_deref().or(settings.plan.as_deref()).unwrap_or(DEFAULT_PLAN)),
        interactive: true,
        // No memory, no records and no trace for a private conversation's runs.
        private: conversation.private,
        locale: locale.as_deref().or(settings.locale.as_deref()),
        timezone: timezone.as_deref().or(settings.timezone.as_deref()),
        ..settings.options(parent)
    };

    // Continue the previous run, so the agent keeps the whole conversation (tool calls and
    // earlier attachments included) and the files in its sandbox workspace.
    let run = match turns.last() {
        None => state.cedar.create_run(content, &options(None)).await?,
        Some(last) => match state.cedar.create_run(content, &options(Some(last.run_id))).await {
            // cedar no longer has the previous run (deleted, or a reset database). Start over,
            // carrying what this service remembers of the conversation in the prompt. Earlier
            // attachments went with the run; only this message's come along.
            Err(AppError::Upstream { status: StatusCode::NOT_FOUND, .. }) => {
                tracing::warn!(run_id = %last.run_id, "previous run is gone from cedar; resending history as text");
                let run_prompt = prompt::compose(&exchanges(&turns), content, state.cfg.history_max_bytes);
                state.cedar.create_run(&run_prompt, &options(None)).await?
            }
            other => other?,
        },
    };
    let turn = state
        .store
        .insert_turn(conversation.id, run.id, content, &run.status, model, settings.reasoning.as_deref(), &infos)
        .await?;
    Ok(turn)
}

// -- settings and tools ---------------------------------------------------------------------

/// How a new run is started, from the Settings defaults and a conversation's or schedule's own
/// tools.
pub struct RunSettings {
    pub model: Option<String>,
    /// The reasoning level asked for; `None` leaves the model's default to cedar.
    pub reasoning: Option<String>,
    pub system: Option<String>,
    pub max_turns: Option<u32>,
    /// `*` when every tool is meant, so a continuation doesn't keep its parent's narrower choice.
    pub tools: Vec<String>,
    /// Whether a message's run plans first; only messages use it, not schedules.
    pub plan: Option<String>,
    /// The language from Settings; a message may name its own (the page's or app's language).
    pub locale: Option<String>,
    /// The time zone a message last sent; a message may name its own (the device's).
    pub timezone: Option<String>,
}

/// What a message's run does about a plan when Settings doesn't say.
const DEFAULT_PLAN: &str = "auto";

/// A plan setting as cedar takes it: `off`, `on`, `auto` (for longer requests) or `tool` (when the
/// model judges a request needs one). Blank is `None`.
fn clean_plan(plan: Option<String>) -> Result<Option<String>, AppError> {
    let plan = plan.map(|p| p.trim().to_ascii_lowercase()).filter(|p| !p.is_empty());
    if plan.as_deref().is_some_and(|p| !matches!(p, "off" | "on" | "auto" | "tool")) {
        return Err(AppError::BadRequest("plan must be off, on, auto or tool".into()));
    }
    Ok(plan)
}

impl RunSettings {
    /// `model`, `reasoning` and `tools` win over the defaults. A default model that is no longer
    /// registered is dropped (cedar then uses its own default). While users don't choose models
    /// (cedar's `model_selection` is off), no model or level is named at all, whatever a
    /// conversation, a schedule or the defaults saved earlier: cedar chooses.
    pub async fn resolve(
        state: &AppState,
        prefs: &Preferences,
        model: Option<String>,
        reasoning: Option<String>,
        tools: Option<Vec<String>>,
    ) -> Self {
        let saved = |models: &[Value]| {
            let saved = prefs.model.as_deref()?;
            find_model(models, saved).map(|m| m["id"].as_str().unwrap_or(saved).to_string())
        };
        let (model, reasoning) = match (state.cedar.model_listing().await, model) {
            (Ok((_, false)), _) => (None, None),
            (_, Some(named)) => (Some(named), reasoning),
            (Ok((models, _)), None) => (saved(&models), reasoning),
            // Can't tell whether the saved default is still there: let cedar decide.
            (Err(_), None) => (prefs.model.clone(), reasoning),
        };
        let tools = tools.or_else(|| prefs.tools.clone()).unwrap_or_else(|| vec!["*".to_string()]);
        Self {
            model,
            reasoning,
            system: prefs.system.clone(),
            max_turns: prefs.max_turns,
            tools,
            plan: prefs.plan.clone(),
            locale: prefs.locale.clone(),
            timezone: prefs.timezone.clone(),
        }
    }

    pub fn options(&self, parent: Option<Uuid>) -> RunOptions<'_> {
        RunOptions {
            model: self.model.as_deref(),
            reasoning: self.reasoning.as_deref(),
            system: self.system.as_deref(),
            max_turns: self.max_turns,
            tools: Some(&self.tools),
            parent,
            attachments: None,
            plan: None,
            interactive: false,
            hold_writes: false,
            private: false,
            locale: self.locale.as_deref(),
            timezone: self.timezone.as_deref(),
        }
    }
}

/// A tool selection from the client, checked against what cedar offers so a typo can't break
/// every later message. Groups are accepted even when cedar has no tools in them right now.
async fn clean_tools(state: &AppState, tools: Option<Vec<String>>) -> Result<Option<Vec<String>>, AppError> {
    let Some(mut tools) = tools else { return Ok(None) };
    if tools.len() > MAX_TOOLS {
        return Err(AppError::BadRequest(format!("tools may name at most {MAX_TOOLS} entries")));
    }
    tools.sort();
    tools.dedup();
    let catalog = state.cedar.tools().await?;
    let groups = catalog["groups"].as_array().cloned().unwrap_or_default();
    let known = |name: &str| {
        name == "*"
            || groups.iter().any(|g| {
                g["id"] == name
                    || g["tools"].as_array().is_some_and(|ts| ts.iter().any(|t| t["name"] == name))
                    // MCP servers list their tools only once a run has connected to them.
                    || g["discovered"] == false && name.strip_prefix(g["id"].as_str().unwrap_or("-")).is_some_and(|r| r.starts_with('_'))
            })
    };
    if let Some(bad) = tools.iter().find(|t| !known(t)) {
        return Err(AppError::BadRequest(format!("cedar offers no tool or tool group {bad:?}")));
    }
    Ok(Some(tools))
}

/// The saved defaults, plus cedar's own (`defaults`: its model, system prompt and turn limit, or
/// `null` if cedar can't be reached) so the page can show what an unset field means.
async fn get_settings(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    let prefs = state.store.preferences().await?;
    let defaults = match state.cedar.defaults().await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "could not read cedar's defaults");
            Value::Null
        }
    };
    let mut body = serde_json::to_value(prefs).map_err(anyhow::Error::from)?;
    body["defaults"] = defaults;
    Ok(Json(body))
}

async fn put_settings(Scoped(state): Scoped, ApiJson(body): ApiJson<SettingsBody>) -> Result<Json<Preferences>, AppError> {
    let system = body.system.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if system.as_ref().is_some_and(|s| s.len() > MAX_SYSTEM_BYTES) {
        return Err(AppError::BadRequest(format!("system is longer than {MAX_SYSTEM_BYTES} bytes")));
    }
    if body.max_turns.is_some_and(|n| n == 0 || n > MAX_TURNS) {
        return Err(AppError::BadRequest(format!("max_turns must be between 1 and {MAX_TURNS}")));
    }
    let (model, _) = clean_model_choice(&state, body.model.as_deref(), None).await?;
    let plan = clean_plan(body.plan)?;
    // The language is saved on its own (`PUT /api/settings/locale`) and the time zone comes with
    // messages, so both are kept here.
    let saved = state.store.preferences().await?;
    let (locale, timezone) = (saved.locale, saved.timezone);
    let prefs = Preferences {
        tools: clean_tools(&state, body.tools).await?,
        model,
        system,
        max_turns: body.max_turns,
        plan,
        locale,
        timezone,
    };
    state.store.set_preferences(&prefs).await?;
    Ok(Json(prefs))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocaleBody {
    /// A BCP 47 tag such as `de` or `fr-CH`; `null` goes back to cedar's default.
    locale: Option<String>,
}

/// Save the user's language: what scheduled runs and notifications use, and what the pages and
/// apps start in on a device that hasn't picked one.
async fn put_locale(Scoped(state): Scoped, ApiJson(body): ApiJson<LocaleBody>) -> Result<Json<Preferences>, AppError> {
    let locale = i18n::clean_locale(body.locale.as_deref()).map_err(AppError::BadRequest)?;
    let mut prefs = state.store.preferences().await?;
    prefs.locale = locale;
    state.store.set_preferences(&prefs).await?;
    Ok(Json(prefs))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsBody {
    tools: Option<Vec<String>>,
    model: Option<String>,
    system: Option<String>,
    max_turns: Option<u32>,
    plan: Option<String>,
}

/// cedar's tool groups, for the Tools settings and the composer's tools menu.
async fn list_tools(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    Ok(Json(state.cedar.tools().await?))
}

/// The models cedar has registered, for the pickers, and whether there are pickers at all:
/// `selection` is cedar's `model_selection` setting, and while it is off cedar chooses the model
/// of each run.
async fn list_models(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    let (models, selection) = state.cedar.model_listing().await?;
    Ok(Json(json!({ "models": models, "selection": selection })))
}

// -- memory ---------------------------------------------------------------------------------

/// Settings -> Memory: what cedar remembers about the user, `{supported, items, profile, limits}`,
/// each item with the `conversation_id` and `title` of the conversation it was saved in, when
/// that still exists here. `?archived=true`, `?kind=` and `?q=` go through to cedar. A cedar from
/// before memory gives `supported: false` and no items.
async fn list_memory(Scoped(state): Scoped, Query(q): Query<HashMap<String, String>>) -> Result<Json<Value>, AppError> {
    let mut query: Vec<(&str, &str)> = q.iter().filter(|(k, _)| ["archived", "kind", "q"].contains(&k.as_str())).map(|(k, v)| (k.as_str(), v.as_str())).collect();
    // Without a kind, the rows about the user: what the phone apps, which predate kinds, list.
    if !q.contains_key("kind") && !q.contains_key("archived") {
        query.push(("kind", "fact,preference,instruction,temporary"));
    }
    let Some(mut body) = state.cedar.memory(&query).await? else {
        return Ok(Json(json!({ "supported": false, "items": [], "profile": null, "limits": null })));
    };
    let mut items = body["items"].as_array().cloned().unwrap_or_default();
    let runs: Vec<Uuid> = items.iter().filter_map(|i| i["run_id"].as_str()?.parse().ok()).collect();
    let sources = state.store.conversations_of_runs(&runs).await?;
    for item in &mut items {
        let source = item["run_id"].as_str().and_then(|r| r.parse::<Uuid>().ok()).and_then(|r| sources.get(&r));
        item["conversation_id"] = json!(source.map(|(id, _)| id));
        item["title"] = json!(source.and_then(|(_, title)| title.clone()));
    }
    Ok(Json(json!({ "supported": true, "items": items, "profile": body["profile"].take(), "limits": body["limits"].take() })))
}

/// Settings -> Memory -> Graph: cedar's memory as a network, `{supported, nodes, edges, total,
/// left_out}`, each node a memory with the `conversation_id` and `title` of the conversation it
/// was saved in, as in the list. `?archived=true` and `?limit=` go through to cedar. A cedar from
/// before the graph gives `supported: false`.
async fn memory_graph(Scoped(state): Scoped, Query(q): Query<HashMap<String, String>>) -> Result<Json<Value>, AppError> {
    let query: Vec<(&str, &str)> = q.iter().filter(|(k, _)| ["archived", "limit"].contains(&k.as_str())).map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let Some(mut body) = state.cedar.memory_graph(&query).await? else {
        return Ok(Json(json!({ "supported": false, "nodes": [], "edges": [], "total": 0, "left_out": 0 })));
    };
    let mut nodes = body["nodes"].as_array().cloned().unwrap_or_default();
    let runs: Vec<Uuid> = nodes.iter().filter_map(|i| i["run_id"].as_str()?.parse().ok()).collect();
    let sources = state.store.conversations_of_runs(&runs).await?;
    for node in &mut nodes {
        let source = node["run_id"].as_str().and_then(|r| r.parse::<Uuid>().ok()).and_then(|r| sources.get(&r));
        node["conversation_id"] = json!(source.map(|(id, _)| id));
        node["title"] = json!(source.and_then(|(_, title)| title.clone()));
    }
    body["nodes"] = json!(nodes);
    body["supported"] = json!(true);
    Ok(Json(body))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryBody {
    text: String,
    kind: Option<String>,
    pinned: Option<bool>,
    valid_until: Option<String>,
}

async fn add_memory(Scoped(state): Scoped, ApiJson(body): ApiJson<MemoryBody>) -> Result<Json<Value>, AppError> {
    let mut req = json!({ "text": body.text });
    if let Some(kind) = body.kind {
        req["kind"] = json!(kind);
    }
    if let Some(pinned) = body.pinned {
        req["pinned"] = json!(pinned);
    }
    if let Some(until) = body.valid_until {
        req["valid_until"] = json!(until);
    }
    Ok(Json(state.cedar.add_memory(&req).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryChange {
    text: Option<String>,
    kind: Option<String>,
    pinned: Option<bool>,
    archived: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_some")]
    valid_until: Option<Option<String>>,
}

/// `null` as `Some(None)`, absent as `None`.
fn deserialize_some<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

async fn patch_memory(Scoped(state): Scoped, Path(id): Path<String>, ApiJson(body): ApiJson<MemoryChange>) -> Result<Json<Value>, AppError> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::NotFound("memory not found"))?;
    let mut req = json!({});
    if let Some(v) = body.text {
        req["text"] = json!(v);
    }
    if let Some(v) = body.kind {
        req["kind"] = json!(v);
    }
    if let Some(v) = body.pinned {
        req["pinned"] = json!(v);
    }
    if let Some(v) = body.archived {
        req["archived"] = json!(v);
    }
    if let Some(v) = body.valid_until {
        req["valid_until"] = json!(v);
    }
    Ok(Json(state.cedar.patch_memory(id, &req).await?))
}

async fn memory_history(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::NotFound("memory not found"))?;
    Ok(Json(state.cedar.memory_history(id).await?))
}

async fn get_memory_profile(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    Ok(Json(state.cedar.memory_profile().await?.unwrap_or_else(|| json!({ "profile": null }))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileBody {
    text: String,
}

async fn put_memory_profile(Scoped(state): Scoped, ApiJson(body): ApiJson<ProfileBody>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.cedar.set_memory_profile(&body.text).await?))
}

async fn export_memory(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    Ok(Json(state.cedar.memory_export().await?))
}

async fn delete_memory(Scoped(state): Scoped, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::NotFound("memory not found"))?;
    state.cedar.delete_memory(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_all_memory(Scoped(state): Scoped) -> Result<StatusCode, AppError> {
    state.cedar.delete_all_memory().await?;
    Ok(StatusCode::NO_CONTENT)
}

// -- records --------------------------------------------------------------------------------

/// Settings -> Records: the record sets cedar's runs keep for the user, `{supported, sets,
/// limits}`. A cedar from before records gives `supported: false` and no sets.
async fn list_records(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    let Some(mut body) = state.cedar.records().await? else {
        return Ok(Json(json!({ "supported": false, "sets": [], "limits": null })));
    };
    Ok(Json(json!({ "supported": true, "sets": body["sets"].take(), "limits": body["limits"].take() })))
}

/// One set with its records, each with the `conversation_id` and `title` of the conversation
/// whose run saved it last, when that still exists here.
async fn get_record_set(Scoped(state): Scoped, Path(name): Path<String>) -> Result<Json<Value>, AppError> {
    let mut body = state.cedar.record_set(&set_name(&name)?).await?;
    let mut records = body["records"].as_array().cloned().unwrap_or_default();
    let runs: Vec<Uuid> = records.iter().filter_map(|r| r["run_id"].as_str()?.parse().ok()).collect();
    let sources = state.store.conversations_of_runs(&runs).await?;
    for record in &mut records {
        let source = record["run_id"].as_str().and_then(|r| r.parse::<Uuid>().ok()).and_then(|r| sources.get(&r));
        record["conversation_id"] = json!(source.map(|(id, _)| id));
        record["title"] = json!(source.and_then(|(_, title)| title.clone()));
    }
    Ok(Json(json!({ "set": body["set"].take(), "records": records })))
}

async fn delete_record_set(Scoped(state): Scoped, Path(name): Path<String>) -> Result<StatusCode, AppError> {
    state.cedar.delete_record_set(&set_name(&name)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_all_records(Scoped(state): Scoped) -> Result<StatusCode, AppError> {
    state.cedar.delete_all_records().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A set name as cedar accepts it, so nothing else can be put in the path sent on.
fn set_name(name: &str) -> Result<String, AppError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok { Ok(name.to_string()) } else { Err(AppError::NotFound("record set not found")) }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolsBody {
    /// `null` returns the conversation to the default tools.
    tools: Option<Vec<String>>,
}

async fn set_conversation_tools(
    Scoped(state): Scoped,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ToolsBody>,
) -> Result<Json<Conversation>, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    let tools = clean_tools(&state, body.tools).await?;
    state.store.set_conversation_tools(conversation.id, tools.as_deref()).await?;
    Ok(Json(Conversation { tools, ..conversation }))
}

// -- integrations (admin) -------------------------------------------------------------------

/// The Integrations settings need the admin token (`X-Admin-Token`) on top of the access token.
fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let Some(expected) = &state.admin_digest else {
        return Err(AppError::Forbidden("integrations are managed in cedar's environment here (CHAT_ADMIN_TOKEN is not set)".into()));
    };
    let presented = headers.get(ADMIN_HEADER).and_then(|v| v.to_str().ok()).map(|t| Sha256::digest(t.trim().as_bytes()));
    match presented {
        Some(digest) if bool::from(digest.as_slice().ct_eq(expected)) => Ok(()),
        _ => Err(AppError::Forbidden("wrong or missing admin token".into())),
    }
}

/// The model routes: by default the access token is enough (a personal setup, where whoever chats
/// also picks the models); with `CHAT_MODELS_ACCESS=admins` they need the admin token like the
/// rest of Settings -> Server.
fn require_models_access(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    match state.cfg.models_access {
        ModelsAccess::Users => Ok(()),
        ModelsAccess::Admins => require_admin(state, headers),
    }
}

/// Where Google sends the browser back after "Connect Google". Normally under `CHAT_PUBLIC_URL`;
/// but a browser on this computer that opened the chat at a loopback address (the desktop app's
/// window, or `http://localhost:…`) is sent back to that address, which an OAuth client of the
/// *Desktop app* kind accepts without registering it. The page's own address comes from the
/// `Origin` header (sent with the POST that starts the sign-in) or `Referer` (sent with the GET
/// that shows it), which a reverse proxy doesn't rewrite the way it may rewrite `Host`.
fn redirect_uri(state: &AppState, headers: &HeaderMap) -> String {
    let loopback = headers
        .get(header::ORIGIN)
        .or_else(|| headers.get(header::REFERER))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| reqwest::Url::parse(v).ok())
        .filter(|u| {
            matches!(u.scheme(), "http" | "https")
                && matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        })
        .map(|u| u.origin().ascii_serialization());
    format!("{}{GOOGLE_CALLBACK_PATH}", loopback.as_deref().unwrap_or(&state.cfg.public_url))
}

async fn get_integrations(Scoped(state): Scoped, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    let mut status = state.cedar.integrations().await?;
    // To register with the Google OAuth client.
    status["google"]["redirect_uri"] = json!(redirect_uri(&state, &headers));
    Ok(Json(status))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoogleClientBody {
    client_id: String,
    client_secret: String,
}

async fn put_google_client(
    Scoped(state): Scoped,
    headers: HeaderMap,
    ApiJson(body): ApiJson<GoogleClientBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    let mut status = state.cedar.set_google_client(&body.client_id, &body.client_secret).await?;
    status["google"]["redirect_uri"] = json!(redirect_uri(&state, &headers));
    Ok(Json(status))
}

async fn delete_google_client(Scoped(state): Scoped, headers: HeaderMap) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_google_client().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BraveKeyBody {
    api_key: String,
}

async fn put_brave_key(
    Scoped(state): Scoped,
    headers: HeaderMap,
    ApiJson(body): ApiJson<BraveKeyBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.set_brave_key(&body.api_key).await?))
}

async fn get_models(Scoped(state): Scoped, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    require_models_access(&state, &headers)?;
    Ok(Json(state.cedar.models_admin().await?))
}

/// The models a server lists (`{base_url, api_key?, id?}`, cedar's body, passed through), so the
/// form offers names it serves.
async fn list_server_models(Scoped(state): Scoped, headers: HeaderMap, ApiJson(body): ApiJson<Value>) -> Result<Json<Value>, AppError> {
    require_models_access(&state, &headers)?;
    if !body.is_object() {
        return Err(AppError::BadRequest("the body must be a JSON object".into()));
    }
    Ok(Json(state.cedar.server_models(&body).await?))
}

/// Register a model or change one. The body is cedar's (`{base_url, model, label?, api_key?,
/// reasoning_levels?, default_reasoning?, multimodal?, max_tokens?, is_default?, price?}`) and
/// goes through as it is; cedar checks it and the server it names.
async fn put_model(
    Scoped(state): Scoped,
    headers: HeaderMap,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<Value>,
) -> Result<Json<Value>, AppError> {
    require_models_access(&state, &headers)?;
    if !body.is_object() {
        return Err(AppError::BadRequest("the body must be a JSON object".into()));
    }
    Ok(Json(state.cedar.set_model(&id, &body).await?))
}

/// What a model's server reports about itself, for its row in Settings: cedar's `{available,
/// ...}`, with llama.cpp's speed and token counts when the server offers them.
async fn get_model_stats(Scoped(state): Scoped, headers: HeaderMap, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    require_models_access(&state, &headers)?;
    Ok(Json(state.cedar.model_stats(&id).await?))
}

/// cedar's server-wide settings (`{max_image_side, sandbox_idle_minutes, defaults}`): the longer
/// side attached images are downscaled to, and how long an idle conversation's sandbox runs
/// before it is paused.
async fn get_server_settings(Scoped(state): Scoped, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.server_settings().await?))
}

/// Change cedar's server-wide settings; the body (`{max_image_side?, sandbox_idle_minutes?, sandbox_runtime?, run_timeout_secs?, model_selection?, computer_runtime?}`)
/// goes through as it is.
async fn put_server_settings(Scoped(state): Scoped, headers: HeaderMap, ApiJson(body): ApiJson<Value>) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    if !body.is_object() {
        return Err(AppError::BadRequest("the body must be a JSON object".into()));
    }
    Ok(Json(state.cedar.set_server_settings(&body).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSelectionBody {
    enabled: bool,
}

/// Turn choosing models on or off (cedar's `model_selection` setting; Settings -> Advanced).
/// It is about the models, so whoever may manage them may change it (see
/// `require_models_access`). Answers `{selection}`.
async fn put_model_selection(
    Scoped(state): Scoped,
    headers: HeaderMap,
    ApiJson(body): ApiJson<ModelSelectionBody>,
) -> Result<Json<Value>, AppError> {
    require_models_access(&state, &headers)?;
    let saved = state.cedar.set_server_settings(&json!({ "model_selection": body.enabled })).await?;
    Ok(Json(json!({ "selection": saved["model_selection"].as_bool().unwrap_or(body.enabled) })))
}

async fn delete_model(Scoped(state): Scoped, headers: HeaderMap, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    require_models_access(&state, &headers)?;
    state.cedar.remove_model(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_brave_key(Scoped(state): Scoped, headers: HeaderMap) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_brave_key().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn put_bestbuy_key(
    Scoped(state): Scoped,
    headers: HeaderMap,
    ApiJson(body): ApiJson<BraveKeyBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.set_bestbuy_key(&body.api_key).await?))
}

async fn delete_bestbuy_key(Scoped(state): Scoped, headers: HeaderMap) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_bestbuy_key().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn put_serpapi_key(
    Scoped(state): Scoped,
    headers: HeaderMap,
    ApiJson(body): ApiJson<BraveKeyBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.set_serpapi_key(&body.api_key).await?))
}

async fn delete_serpapi_key(Scoped(state): Scoped, headers: HeaderMap) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_serpapi_key().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn put_slack_token(
    Scoped(state): Scoped,
    headers: HeaderMap,
    ApiJson(body): ApiJson<BraveKeyBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.set_slack_token(&body.api_key).await?))
}

async fn delete_slack_token(Scoped(state): Scoped, headers: HeaderMap) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_slack_token().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Tool and secret names go into cedar's URLs, so only the characters cedar allows pass.
fn path_name(value: &str, upper: bool) -> Result<&str, AppError> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|c| c.is_ascii_digit() || c == b'_' || if upper { c.is_ascii_uppercase() } else { c.is_ascii_lowercase() });
    if ok { Ok(value) } else { Err(AppError::BadRequest(format!("{value:?} is not a tool or secret name"))) }
}

async fn get_personal_tools(Scoped(state): Scoped, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.personal_tools().await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolSecretBody {
    value: String,
}

async fn put_tool_secret(
    Scoped(state): Scoped,
    headers: HeaderMap,
    Path((name, secret)): Path<(String, String)>,
    ApiJson(body): ApiJson<ToolSecretBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.set_tool_secret(path_name(&name, false)?, path_name(&secret, true)?, &body.value).await?))
}

async fn delete_tool_secret(Scoped(state): Scoped, headers: HeaderMap, Path((name, secret)): Path<(String, String)>) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_tool_secret(path_name(&name, false)?, path_name(&secret, true)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn setup_tool(Scoped(state): Scoped, headers: HeaderMap, Path(name): Path<String>) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.setup_tool(path_name(&name, false)?).await?))
}

#[derive(Deserialize)]
struct MarketplaceQuery {
    #[serde(default)]
    refresh: bool,
}

async fn get_marketplace(Scoped(state): Scoped, headers: HeaderMap, Query(q): Query<MarketplaceQuery>) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    Ok(Json(state.cedar.marketplace(q.refresh).await?))
}

/// Install or update a community tool. The body is cedar's (`{accept: {network, secrets, pip,
/// health_data}}`) and goes through as it is; cedar checks it against the release.
async fn install_tool(Scoped(state): Scoped, headers: HeaderMap, Path(name): Path<String>, ApiJson(body): ApiJson<Value>) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    if !body.is_object() {
        return Err(AppError::BadRequest("the body must be a JSON object".into()));
    }
    Ok(Json(state.cedar.install_tool(path_name(&name, false)?, &body).await?))
}

async fn uninstall_tool(Scoped(state): Scoped, headers: HeaderMap, Path(name): Path<String>) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.uninstall_tool(path_name(&name, false)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The Google account the caller connected themselves, which their own runs act for. The
/// primary user has none of their own (`server: true`): theirs is the server's, under
/// Settings -> Server.
async fn get_own_google(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    Ok(Json(state.cedar.own_google().await?))
}

/// Start connecting the caller's own Google account; no admin token needed. Google comes back
/// to the same callback as the server's account does.
async fn connect_own_google(Scoped(state): Scoped, headers: HeaderMap, Query(params): Query<HashMap<String, String>>) -> Result<Json<Value>, AppError> {
    let url = state.cedar.own_google_authorize(&redirect_uri(&state, &headers)).await?;
    remember_browser_sign_in(&state, &params, &url);
    Ok(Json(json!({ "url": url })))
}

async fn delete_own_google(Scoped(state): Scoped) -> Result<StatusCode, AppError> {
    state.cedar.remove_own_google().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn connect_google(Scoped(state): Scoped, headers: HeaderMap, Query(params): Query<HashMap<String, String>>) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    let url = state.cedar.google_authorize(&redirect_uri(&state, &headers)).await?;
    remember_browser_sign_in(&state, &params, &url);
    Ok(Json(json!({ "url": url })))
}

/// Note a sign-in the desktop app sent to the default browser (`?browser=1`), so the callback
/// shows that browser a page of its own.
fn remember_browser_sign_in(state: &AppState, params: &HashMap<String, String>, url: &str) {
    // The desktop app opens Google's page in the default browser: its embedded web view can't
    // offer every way to sign in (passkeys and Sign in with Apple, for one).
    if params.contains_key("browser") {
        if let Some(s) = google_state(url) {
            let mut pending = state.google_in_browser.lock().unwrap_or_else(|e| e.into_inner());
            if pending.len() >= GOOGLE_IN_BROWSER_MAX {
                pending.remove(0);
            }
            pending.push(s);
        }
    }
}

/// The `state` parameter of Google's sign-in URL.
fn google_state(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    url.query_pairs().find(|(k, _)| k == "state").map(|(_, v)| v.into_owned())
}

async fn delete_google(Scoped(state): Scoped, headers: HeaderMap) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.cedar.remove_google().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Google sends the browser here after "Connect Google". Hand the code to cedar, then go back to
/// the page, which reports the outcome from the query string. A sign-in the desktop app started
/// in the default browser gets a page of its own instead: the outcome, and a word to go back to
/// the app, whose Settings page checks again when its window comes forward.
async fn google_callback(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> Response {
    let in_browser = params.get("state").is_some_and(|s| {
        let mut pending = state.google_in_browser.lock().unwrap_or_else(|e| e.into_inner());
        pending.iter().position(|p| p == s).map(|i| pending.remove(i)).is_some()
    });
    let outcome = match (params.get("state"), params.get("code"), params.get("error")) {
        (_, _, Some(error)) => Err(format!("Google sign-in was not completed ({error})")),
        (Some(s), Some(code), None) => state.cedar.google_callback(s, code).await.map(|done| done["own"] == true).map_err(|e| e.to_string()),
        _ => Err("the sign-in response from Google is incomplete".to_string()),
    };
    if let Err(message) = &outcome {
        tracing::warn!(error = %message, "Connect Google failed");
    }
    if in_browser {
        return google_done_page(&outcome);
    }
    let mut url = reqwest::Url::parse("http://local/").expect("static URL");
    match outcome {
        // The page reopens Settings where the account is shown: a user's own under Tools, the
        // server's under Server.
        Ok(true) => url.query_pairs_mut().append_pair("integrations", "connected").append_pair("own", "1"),
        Ok(false) => url.query_pairs_mut().append_pair("integrations", "connected"),
        Err(message) => url.query_pairs_mut().append_pair("integrations", "error").append_pair("message", &message),
    };
    Redirect::to(&format!("/?{}", url.query().unwrap_or_default())).into_response()
}

/// What the browser shows after a "Connect Google" the desktop app started there.
fn google_done_page(outcome: &Result<bool, String>) -> Response {
    let (title, detail) = match outcome {
        Ok(_) => ("Google account connected".to_string(), "You can close this tab and go back to Cedar.".to_string()),
        Err(message) => ("Google was not connected".to_string(), format!("{}. Close this tab and try again in Cedar.", html_escape(message))),
    };
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Cedar</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:32rem;margin:15vh auto;padding:0 1rem;\
         color:#222;background:#fafaf7}}@media (prefers-color-scheme:dark){{body{{color:#eee;background:#1b1b19}}}}\
         h1{{font-size:1.4rem}}</style></head><body><h1>{title}</h1><p>{detail}</p></body></html>"
    );
    ([(CACHE_CONTROL, "no-store")], axum::response::Html(html)).into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

// -- compaction -----------------------------------------------------------------------------

#[derive(Serialize)]
struct Compacted {
    /// One self-contained prompt that stands for the whole conversation.
    prompt: String,
    /// `run` when cedar compacted the stored run (tool calls included); `history` when the run
    /// was gone and the exchanges this service remembers were used instead.
    source: &'static str,
}

/// Have the model turn the conversation so far into one prompt, for scheduling it.
async fn compact_conversation(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Compacted>, AppError> {
    let conversation = find_conversation(&state, &id).await?;
    let turns = settled_turns(&state, &conversation).await?;
    let last = turns
        .iter()
        .rev()
        .find(|t| t.status == "completed")
        .ok_or(AppError::NotFound("the conversation has no finished response to schedule yet"))?;
    // The conversation's model, if cedar still has it; else cedar's default.
    let model = still_registered(&state, conversation.model.as_deref()).await;
    match state.cedar.compact_run(last.run_id, model.as_deref()).await {
        Ok(prompt) => Ok(Json(Compacted { prompt, source: "run" })),
        Err(AppError::Upstream { status: StatusCode::NOT_FOUND, .. }) => {
            tracing::warn!(run_id = %last.run_id, "run is gone from cedar; compacting the remembered exchanges");
            let transcript = prompt::transcript(&exchanges(&turns), state.cfg.history_max_bytes);
            let prompt = state.cedar.compact_transcript(&transcript, model.as_deref()).await?;
            Ok(Json(Compacted { prompt, source: "history" }))
        }
        Err(e) => Err(e),
    }
}

// -- contributions --------------------------------------------------------------------------

/// The conversation as it would be contributed, with why it can't be, if it can't (see
/// `contrib::preview`). `?reasoning=1` and `?fetched=1` include what is left out by default.
async fn contribution_preview(
    Scoped(state): Scoped,
    Path(id): Path<String>,
    ApiQuery(opts): ApiQuery<contrib::Options>,
) -> Result<Json<contrib::Preview>, AppError> {
    let conversation = open_conversation(&state, &id).await?;
    Ok(Json(contrib::preview(&state, &conversation, opts).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendBody {
    /// The bundle as the dialog shows it, placeholders and all.
    bundle: cedar_contrib_schema::Bundle,
    #[serde(default)]
    outcome: Option<cedar_contrib_schema::Outcome>,
}

/// Send the bundle the dialog holds to the corpus; answers with the receipt.
async fn send_contribution(
    Scoped(state): Scoped,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SendBody>,
) -> Result<(StatusCode, Json<Contribution>), AppError> {
    let conversation = open_conversation(&state, &id).await?;
    let saved = contrib::send(&state, &conversation, body.bundle, body.outcome).await?;
    Ok((StatusCode::CREATED, Json(saved)))
}

#[derive(Serialize)]
struct ContributionList {
    contributions: Vec<Contribution>,
}

async fn list_contributions(Scoped(state): Scoped) -> Result<Json<ContributionList>, AppError> {
    Ok(Json(ContributionList { contributions: state.store.list_contributions().await? }))
}

/// Ask the corpus what became of the contributions (their screening and release status).
async fn refresh_contributions(Scoped(state): Scoped) -> Result<Json<ContributionList>, AppError> {
    Ok(Json(ContributionList { contributions: contrib::refresh_statuses(&state, 50).await? }))
}

async fn withdraw_contribution(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Contribution>, AppError> {
    if id.len() > 100 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(AppError::NotFound("no such contribution"));
    }
    Ok(Json(contrib::withdraw(&state, &id).await?))
}

async fn get_contribution_settings(Scoped(state): Scoped) -> Result<Json<contrib::SettingsView>, AppError> {
    let settings = state.store.contribution_settings().await?;
    Ok(Json(contrib::settings_view(&state, settings)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContributionSettingsBody {
    #[serde(default)]
    license: Option<String>,
    /// `"new"` makes a fresh pseudonym, `null` or `""` drops it, anything else keeps the current one.
    #[serde(default)]
    pseudonym: Option<String>,
    #[serde(default)]
    include_reasoning: Option<bool>,
    #[serde(default)]
    include_fetched: Option<bool>,
    /// Turn contributing on (the person read the page) or off.
    #[serde(default)]
    enabled: Option<bool>,
}

async fn put_contribution_settings(
    Scoped(state): Scoped,
    ApiJson(body): ApiJson<ContributionSettingsBody>,
) -> Result<Json<contrib::SettingsView>, AppError> {
    let mut settings = state.store.contribution_settings().await?;
    if let Some(l) = body.license {
        if cedar_contrib_schema::License::parse(&l).is_none() {
            return Err(AppError::BadRequest(format!("license {l:?} is not offered")));
        }
        settings.license = l;
    }
    match body.pseudonym.as_deref() {
        Some("new") => settings.pseudonym = Some(contrib::new_pseudonym()),
        Some("") => settings.pseudonym = None,
        Some(other) if Some(other) != settings.pseudonym.as_deref() => {
            return Err(AppError::BadRequest("pseudonym must be \"new\", \"\" or the current one".into()));
        }
        _ => {}
    }
    if let Some(r) = body.include_reasoning {
        settings.include_reasoning = r;
    }
    if let Some(f) = body.include_fetched {
        settings.include_fetched = f;
    }
    match body.enabled {
        Some(true) if settings.accepted_at.is_none() => settings.accepted_at = Some(Utc::now()),
        Some(false) => settings.accepted_at = None,
        _ => {}
    }
    state.store.set_contribution_settings(&settings).await?;
    Ok(Json(contrib::settings_view(&state, settings)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RatingBody {
    /// 1, -1, or null to clear.
    rating: Option<i8>,
}

/// The person's thumbs on a response.
async fn rate_turn(Scoped(state): Scoped, Path(id): Path<String>, ApiJson(body): ApiJson<RatingBody>) -> Result<Json<Turn>, AppError> {
    if body.rating.is_some_and(|r| r != 1 && r != -1) {
        return Err(AppError::BadRequest("rating must be 1, -1 or null".into()));
    }
    let turn = find_turn(&state, &id).await?;
    state.store.set_turn_rating(turn.id, body.rating).await?;
    // cedar learns from it too: what the run noted is promoted or archived (see cedar's memory).
    let rating = body.rating.map(|r| if r > 0 { "up" } else { "down" });
    if let Err(e) = state.cedar.run_feedback(turn.run_id, rating).await {
        tracing::warn!(run_id = %turn.run_id, error = %e, "could not pass the rating on to cedar");
    }
    Ok(Json(state.store.get_turn(turn.id).await?.ok_or(AppError::NotFound("no such turn"))?))
}

// -- schedules ------------------------------------------------------------------------------

/// Fields of a schedule as the client sends them. Creating needs all of them (but `model`,
/// `weekday`, `conversation_id` and `enabled`); updating takes any subset.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ScheduleBody {
    title: Option<String>,
    prompt: Option<String>,
    /// A registered model's id. Blank or absent means cedar's default model.
    model: Option<String>,
    /// One of the model's reasoning levels. Blank or absent means the model's default.
    reasoning: Option<String>,
    frequency: Option<String>,
    weekday: Option<u8>,
    time: Option<String>,
    timezone: Option<String>,
    enabled: Option<bool>,
    /// The chat the schedule was made from.
    conversation_id: Option<Uuid>,
    /// The tools its runs use; `null` means the defaults. When creating, absent means the
    /// conversation's own tools, if it has any.
    #[serde(default, deserialize_with = "nullable")]
    tools: Option<Option<Vec<String>>>,
    /// How a finished run is announced out of the app: a method set up under Settings →
    /// Notifications (`browser`, `pushover`, `email`, `ios`, `android`), `none`, or `null` for the default method
    /// there (the default when creating).
    #[serde(default, deserialize_with = "nullable")]
    notify: Option<Option<String>>,
}

/// A schedule's `notify`: a known method that can send right now, or none.
async fn clean_notify(state: &AppState, notify: Option<String>) -> Result<Option<String>, AppError> {
    let Some(raw) = notify else { return Ok(None) };
    let raw = raw.trim().to_ascii_lowercase();
    if raw.is_empty() || raw == "default" {
        return Ok(None);
    }
    let method = Method::parse(&raw).ok_or_else(|| AppError::BadRequest(format!("notify must be none, {} or null, not {raw:?}", Method::NAMES)))?;
    if method != Method::None {
        let settings = state.store.notification_settings().await?;
        settings.check(method, reach(state).await?).map_err(|why| AppError::BadRequest(format!("notify: {why}; set it up under Settings → Notifications first")))?;
    }
    Ok(Some(method.as_str().to_string()))
}

/// Tells an absent field (`None`) from an explicit `null` (`Some(None)`).
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// A run record with the turn that carries its cedar run, if it started.
#[derive(Serialize)]
struct RunDetail {
    #[serde(flatten)]
    run: ScheduleRun,
    turn: Option<Turn>,
}

#[derive(Serialize)]
struct ScheduleSummary {
    #[serde(flatten)]
    schedule: Schedule,
    last_run: Option<RunDetail>,
}

#[derive(Serialize)]
struct ScheduleDetail {
    schedule: Schedule,
    runs: Vec<RunDetail>,
}

#[derive(Serialize)]
struct ScheduleList {
    schedules: Vec<ScheduleSummary>,
}

/// A classic schedule, for `/api/schedules`: the apps from before agents had other triggers
/// read these routes, and would trip over an agent with no time of day. `/api/agents` has them all.
async fn find_schedule(state: &AppState, raw_id: &str) -> Result<Schedule, AppError> {
    let id = parse_id(raw_id, "schedule not found")?;
    state.store.get_schedule(id).await?.filter(|s| s.trigger_kind == "time").ok_or(AppError::NotFound("schedule not found"))
}

/// Whether an agent's latest run is still going.
pub(crate) async fn run_going(state: &AppState, schedule_id: Uuid) -> Result<bool, AppError> {
    let newest = state.store.list_schedule_runs(schedule_id, 1).await?;
    Ok(with_turns(state, newest).await?.pop().is_some_and(|last| last.turn.as_ref().is_some_and(|t| !cedar::is_terminal(&t.status))))
}

/// Delete an agent with its runs' conversations and their cedar runs, unless one is still going.
async fn remove_schedule(state: &AppState, schedule: &Schedule) -> Result<(), AppError> {
    let turns = refresh_all(state, state.store.list_schedule_turns(schedule.id).await?).await;
    if turns.iter().any(|t| !cedar::is_terminal(&t.status)) {
        return Err(AppError::Conflict("a run of this agent is still going; stop it first".into()));
    }
    for turn in &turns {
        // Best effort, as for a conversation: the agent is gone either way.
        if let Err(e) = state.cedar.delete_run(turn.run_id).await {
            tracing::warn!(run_id = %turn.run_id, error = %e, "could not delete cedar run");
        }
    }
    state.store.delete_schedule(schedule.id).await?;
    Ok(())
}

/// Start a run right now, outside what the trigger says. Doesn't move the next one.
async fn start_now(state: &AppState, schedule: &Schedule) -> Result<RunDetail, AppError> {
    if run_going(state, schedule.id).await? {
        return Err(AppError::Conflict("the previous run of this agent is still going".into()));
    }
    let run = schedule::fire(state, schedule, schedule::Firing::manual(Utc::now())).await?;
    Ok(with_turns(state, vec![run]).await?.pop().expect("one run"))
}

/// Attach each run's turn, refreshed from cedar while it is unfinished.
async fn with_turns(state: &AppState, runs: Vec<ScheduleRun>) -> Result<Vec<RunDetail>, AppError> {
    let mut out = Vec::with_capacity(runs.len());
    for run in runs {
        let turn = match run.turn_id {
            Some(id) => state.store.get_turn(id).await?,
            None => None,
        };
        let turn = match turn {
            Some(t) => refresh_all(state, vec![t]).await.pop(),
            None => None,
        };
        out.push(RunDetail { run, turn });
    }
    Ok(out)
}

async fn list_schedules(Scoped(state): Scoped) -> Result<Json<ScheduleList>, AppError> {
    let mut schedules = Vec::new();
    for schedule in state.store.list_time_schedules().await? {
        let last = state.store.list_schedule_runs(schedule.id, 1).await?;
        let last_run = with_turns(&state, last).await?.pop();
        schedules.push(ScheduleSummary { schedule, last_run });
    }
    Ok(Json(ScheduleList { schedules }))
}

async fn create_schedule(
    Scoped(state): Scoped,
    ApiJson(body): ApiJson<ScheduleBody>,
) -> Result<(StatusCode, Json<Schedule>), AppError> {
    let title = clean_title(body.title)?.ok_or_else(|| AppError::BadRequest("title must not be empty".into()))?;
    let prompt = clean_message("prompt", body.prompt.as_deref().unwrap_or_default())?;
    let (model, reasoning) = clean_model_choice(&state, body.model.as_deref(), body.reasoning.as_deref()).await?;
    let missing = |what: &str| AppError::BadRequest(format!("{what} is required"));
    let frequency = body.frequency.ok_or_else(|| missing("frequency"))?;
    let time = body.time.ok_or_else(|| missing("time"))?;
    let timezone = body.timezone.ok_or_else(|| missing("timezone"))?;
    let timing = Timing::parse(&frequency, body.weekday, &time, &timezone).map_err(AppError::BadRequest)?;
    if state.store.count_schedules().await? >= MAX_SCHEDULES {
        return Err(AppError::Conflict(format!("at most {MAX_SCHEDULES} schedules; delete one first")));
    }
    let tools = match body.tools {
        Some(tools) => clean_tools(&state, tools).await?,
        None => match body.conversation_id {
            Some(cid) => state.store.get_conversation(cid).await?.and_then(|c| c.tools),
            None => None,
        },
    };
    let notify = clean_notify(&state, body.notify.flatten()).await?;
    let schedule = state
        .store
        .create_schedule(&NewSchedule {
            title: &title,
            prompt: &prompt,
            model: model.as_deref(),
            reasoning: reasoning.as_deref(),
            source_conversation_id: body.conversation_id,
            tools: tools.as_deref(),
            notify: notify.as_deref(),
            frequency: timing.frequency.as_str(),
            weekday: timing.weekday.map(|d| d.num_days_from_monday() as u8),
            time_of_day: &timing.time.format("%H:%M").to_string(),
            timezone: timing.tz.name(),
            next_run_at: timing.next_after(Utc::now()),
            trigger_kind: "time",
            trigger: None,
            hook_id: None,
            hook_secret: None,
            min_interval_secs: None,
            max_runs_per_day: None,
            hold_writes: true,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(schedule)))
}

async fn get_schedule(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<ScheduleDetail>, AppError> {
    let schedule = find_schedule(&state, &id).await?;
    let runs = with_turns(&state, state.store.list_schedule_runs(schedule.id, MAX_SCHEDULE_RUNS).await?).await?;
    Ok(Json(ScheduleDetail { schedule, runs }))
}

async fn update_schedule(
    Scoped(state): Scoped,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ScheduleBody>,
) -> Result<Json<Schedule>, AppError> {
    let mut schedule = find_schedule(&state, &id).await?;
    if let Some(title) = body.title {
        schedule.title = clean_title(Some(title))?.ok_or_else(|| AppError::BadRequest("title must not be empty".into()))?;
    }
    if let Some(prompt) = &body.prompt {
        schedule.prompt = clean_message("prompt", prompt)?;
    }
    if body.model.is_some() || body.reasoning.is_some() {
        let model = body.model.as_deref().or(schedule.model.as_deref());
        let reasoning = body.reasoning.as_deref().or(schedule.reasoning.as_deref());
        (schedule.model, schedule.reasoning) = clean_model_choice(&state, model, reasoning).await?;
    }
    if let Some(tools) = body.tools {
        schedule.tools = clean_tools(&state, tools).await?;
    }
    if let Some(notify) = body.notify {
        schedule.notify = clean_notify(&state, notify).await?;
    }
    let retime = body.frequency.is_some() || body.weekday.is_some() || body.time.is_some() || body.timezone.is_some();
    if retime {
        let timing = Timing::parse(
            body.frequency.as_deref().unwrap_or(&schedule.frequency),
            body.weekday.or(schedule.weekday),
            body.time.as_deref().unwrap_or(&schedule.time_of_day),
            body.timezone.as_deref().unwrap_or(&schedule.timezone),
        )
        .map_err(AppError::BadRequest)?;
        schedule.frequency = timing.frequency.as_str().to_string();
        schedule.weekday = timing.weekday.map(|d| d.num_days_from_monday() as u8);
        schedule.time_of_day = timing.time.format("%H:%M").to_string();
        schedule.timezone = timing.tz.name().to_string();
        schedule.next_run_at = timing.next_after(Utc::now());
        // The columns are the trigger again (an agent made through `/api/agents` kept a copy).
        schedule.trigger = None;
    }
    if let Some(enabled) = body.enabled {
        if enabled && !schedule.enabled && !retime {
            // Resuming: don't fire for the slots missed while paused.
            let timing = Timing::of(&schedule).map_err(AppError::BadRequest)?;
            schedule.next_run_at = timing.next_after(Utc::now());
        }
        schedule.enabled = enabled;
    }
    if !state.store.update_schedule(&schedule, false).await? {
        return Err(AppError::NotFound("schedule not found"));
    }
    schedule.updated_at = Utc::now();
    Ok(Json(schedule))
}

async fn delete_schedule(Scoped(state): Scoped, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    let schedule = find_schedule(&state, &id).await?;
    remove_schedule(&state, &schedule).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Start a run right now, outside the schedule. Doesn't move the next scheduled run.
async fn run_schedule_now(Scoped(state): Scoped, Path(id): Path<String>) -> Result<(StatusCode, Json<RunDetail>), AppError> {
    let schedule = find_schedule(&state, &id).await?;
    Ok((StatusCode::ACCEPTED, Json(start_now(&state, &schedule).await?)))
}

// -- workspace files ------------------------------------------------------------------------

/// The run whose workspace holds the conversation's files: the latest one, since every turn
/// continues the previous run and keeps its workspace. `None` for a conversation with no turns.
async fn latest_run(state: &AppState, raw_id: &str) -> Result<Option<Uuid>, AppError> {
    let conversation = find_conversation(state, raw_id).await?;
    Ok(state.store.list_turns(conversation.id).await?.last().map(|t| t.run_id))
}

async fn list_files(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<serde_json::Value>, AppError> {
    match latest_run(&state, &id).await? {
        Some(run_id) => Ok(Json(state.cedar.list_files(run_id).await?)),
        None => Ok(Json(json!({ "workspace_id": null, "files": [], "truncated": false }))),
    }
}

/// Relay a file download from cedar, keeping the headers that describe the file.
async fn download_file(Scoped(state): Scoped, Path((id, path)): Path<(String, String)>) -> Result<Response, AppError> {
    let run_id = latest_run(&state, &id).await?.ok_or(AppError::NotFound("no such file in the workspace"))?;
    let upstream = state.cedar.download_file(run_id, &path).await?;
    let mut headers = axum::http::HeaderMap::new();
    for name in [CONTENT_TYPE, header::CONTENT_LENGTH, header::CONTENT_DISPOSITION] {
        if let Some(value) = upstream.headers().get(&name) {
            headers.insert(name, value.clone());
        }
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, Body::from_stream(upstream.bytes_stream())).into_response())
}

#[derive(Deserialize)]
struct EventsParams {
    after: Option<i64>,
}

/// Relay the cedar event stream for a turn's run: replay from `after`, then follow it live.
async fn turn_events(
    Scoped(state): Scoped,
    Path(id): Path<String>,
    ApiQuery(params): ApiQuery<EventsParams>,
) -> Result<Response, AppError> {
    let turn = find_turn(&state, &id).await?;
    let upstream = state.cedar.events(turn.run_id, params.after.unwrap_or(0).max(0)).await?;
    Ok((
        [(CONTENT_TYPE, "text/event-stream"), (CACHE_CONTROL, "no-cache")],
        [("x-accel-buffering", "no")],
        Body::from_stream(upstream.bytes_stream()),
    )
        .into_response())
}

/// The trace of a turn's run, for the expert view: every model call with what it was sent and
/// what came back, beside the run's conversation and events.
async fn turn_trace(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let turn = find_turn(&state, &id).await?;
    // cedar keeps none for a private run; say why instead of relaying its 404.
    if state.store.get_conversation(turn.conversation_id).await?.is_some_and(|c| c.private) {
        return Err(AppError::NotFound("a private conversation keeps no trace"));
    }
    Ok(Json(state.cedar.trace(turn.run_id).await?))
}

async fn cancel_turn(Scoped(state): Scoped, Path(id): Path<String>) -> Result<(StatusCode, Json<Turn>), AppError> {
    let turn = refresh(&state, find_turn(&state, &id).await?).await?;
    if cedar::is_terminal(&turn.status) {
        return Err(AppError::Conflict(format!("the response already {}", turn.status)));
    }
    state.cedar.cancel_run(turn.run_id).await?;
    Ok((StatusCode::ACCEPTED, Json(turn)))
}

/// Agree to the plan a turn's run is waiting on (`{"steps": [{id, title}, ...]}`); cedar checks
/// the steps and says whether the run is waiting.
async fn approve_plan(
    Scoped(state): Scoped,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<PlanBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let turn = find_turn(&state, &id).await?;
    let answer = state.cedar.answer_plan(turn.run_id, &json!({ "steps": body.steps })).await?;
    Ok((StatusCode::ACCEPTED, Json(answer)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanBody {
    steps: Vec<Value>,
}

/// Answer an approval a turn's run is waiting on: `allow`, `allow_always` or `deny`.
async fn decide_approval(
    Scoped(state): Scoped,
    Path((id, approval_id)): Path<(String, String)>,
    ApiJson(body): ApiJson<ApprovalBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let turn = find_turn(&state, &id).await?;
    if !matches!(body.decision.as_str(), "allow" | "allow_always" | "deny") {
        return Err(AppError::BadRequest("decision must be allow, allow_always or deny".into()));
    }
    let answer = state.cedar.decide_approval(turn.run_id, &approval_id, &json!({ "decision": body.decision })).await?;
    Ok((StatusCode::ACCEPTED, Json(answer)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalBody {
    decision: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    /// An open event stream held the graceful shutdown up until the launcher's SIGKILL.
    #[tokio::test]
    async fn event_streams_end_when_the_server_shuts_down() {
        let (tx, rx) = broadcast::channel(4);
        let shutdown = CancellationToken::new();
        let (me, other) = (Uuid::new_v4(), Uuid::new_v4());
        let mut stream = Box::pin(relay_stream(rx, me, shutdown.clone()));
        // Another user's notice is passed over.
        tx.send((other, json!({"type": "turn.finished", "title": "theirs"}))).unwrap();
        tx.send((me, json!({"type": "turn.finished", "title": "mine"}))).unwrap();
        let first = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next()).await.unwrap();
        assert!(format!("{first:?}").contains("mine"), "{first:?}");
        shutdown.cancel();
        let end = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next()).await;
        assert!(matches!(end, Ok(None)), "the stream kept going after shutdown");
        drop(tx);
    }
}

#[cfg(test)]
mod model_servers_tests {
    use super::{MODEL_SERVERS_JSON, SUGGESTED_MODELS_JSON};

    #[test]
    fn the_suggested_models_name_providers_on_the_list() {
        let list: serde_json::Value = serde_json::from_str(MODEL_SERVERS_JSON).unwrap();
        let suggested: serde_json::Value = serde_json::from_str(SUGGESTED_MODELS_JSON).unwrap();
        assert!(!suggested["default"].as_array().unwrap().is_empty());
        for (name, models) in suggested["providers"].as_object().unwrap() {
            assert!(list["providers"].as_array().unwrap().iter().any(|p| p["name"] == name.as_str()), "{name}");
            assert!(models.as_array().unwrap().iter().all(|m| m.is_string()), "{name}");
        }
        // The reasoning-level sets of Settings' dropdown, and the models that take them: levels
        // are tokens cedar accepts (src/integrations.rs, clean_reasoning_levels).
        let level = |l: &serde_json::Value| {
            let l = l.as_str().unwrap();
            !l.is_empty() && l.len() <= 32 && l.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        };
        let presets = suggested["reasoning"]["presets"].as_array().unwrap();
        assert!(!presets.is_empty());
        for set in presets {
            assert!(!set.as_array().unwrap().is_empty() && set.as_array().unwrap().iter().all(level));
        }
        // Which models read images: a name to look for and a yes or no.
        let images = suggested["images"].as_array().unwrap();
        assert!(!images.is_empty());
        for model in images {
            assert!(!model["match"].as_str().unwrap().trim().is_empty() && model["images"].is_boolean());
        }
        for model in suggested["reasoning"]["models"].as_array().unwrap() {
            assert!(!model["match"].as_str().unwrap().trim().is_empty());
            assert!(!model["levels"].as_array().unwrap().is_empty() && model["levels"].as_array().unwrap().iter().all(level));
        }
    }

    /// Settings' "Add a model" reads this file as it is served; the desktop app's tests check the
    /// addresses themselves (desktop/src/models.rs).
    #[test]
    fn the_served_list_has_providers_and_presets() {
        let list: serde_json::Value = serde_json::from_str(MODEL_SERVERS_JSON).unwrap();
        let providers = list["providers"].as_array().unwrap();
        let presets = list["presets"].as_array().unwrap();
        assert!(providers.len() >= 3 && presets.len() >= 4);
        for p in providers {
            assert!(p["name"].is_string() && p["docs"].as_str().unwrap().starts_with("https://"));
            assert!(p["base_url"].as_str().unwrap().starts_with("https://"));
        }
        for p in presets {
            assert!(p["name"].is_string() && p["base_url"].as_str().unwrap().starts_with("http://127.0.0.1:"));
            assert!(p["hint"].as_str().unwrap().starts_with("preset."));
        }
    }
}
