//! Domain types and API payloads.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => RunStatus::Queued,
            "running" => RunStatus::Running,
            "completed" => RunStatus::Completed,
            "failed" => RunStatus::Failed,
            "cancelled" => RunStatus::Cancelled,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, RunStatus::Queued | RunStatus::Running)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub id: Uuid,
    /// Identifier of the API key that created the run. Never serialized.
    #[serde(skip)]
    pub owner: String,
    pub status: RunStatus,
    pub prompt: String,
    pub system: String,
    /// The registry entry the run uses (see [`ModelEntry`]).
    pub model_id: String,
    /// The model's name on its server, as sent in each request.
    pub model: String,
    /// The `reasoning_effort` sent with each request, if the run asked for one.
    pub reasoning: Option<String>,
    pub max_turns: u32,
    pub turns: u32,
    pub final_text: Option<String>,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
    pub usage: Usage,
    /// The run this one continues, if any (and if it hasn't been deleted since).
    pub parent_id: Option<Uuid>,
    /// The sandbox workspace (files under /workspace) this run uses.
    pub workspace_id: Uuid,
    /// True if the run picked up its parent's workspace; false if it started with an empty one.
    pub workspace_reused: bool,
    /// The tool groups and tools the run may use besides the sandbox tools; `None` means all.
    pub tools: Option<Vec<String>>,
    /// Where a notification is POSTed when the run ends (see [`crate::webhooks`]). The secret
    /// its deliveries are signed with is never returned.
    pub callback_url: Option<String>,
    /// Whether the run plans its steps before it starts.
    pub plan: PlanMode,
    /// The client answers what the run asks while it goes: it waits for the plan to be agreed and
    /// for risky tool calls to be approved. Otherwise the plan is only shown and nothing waits.
    pub interactive: bool,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewRun {
    pub owner: String,
    pub prompt: String,
    pub system: String,
    pub model_id: String,
    pub model: String,
    pub reasoning: Option<String>,
    pub max_turns: u32,
    /// Continue this run: its conversation, and its workspace if that hasn't expired.
    pub parent_id: Option<Uuid>,
    pub tools: Option<Vec<String>>,
    /// Files sent with the prompt (images only for a multimodal model).
    pub attachments: Vec<Attachment>,
    /// A webhook to call when the run ends, and the secret to sign the call with.
    pub callback_url: Option<String>,
    pub callback_secret: Option<String>,
    pub plan: PlanMode,
    pub interactive: bool,
}

/// Whether a run makes a plan before it starts: always, never, or for prompts long enough to
/// need one (see [`crate::plan::wanted`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanMode {
    #[default]
    Off,
    On,
    Auto,
}

impl PlanMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanMode::Off => "off",
            PlanMode::On => "on",
            PlanMode::Auto => "auto",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "off" => PlanMode::Off,
            "on" => PlanMode::On,
            "auto" => PlanMode::Auto,
            _ => return None,
        })
    }
}

/// A registered model: a name on an OpenAI-compatible server, and what it accepts. Admins manage
/// the registry through `/v1/admin/models`; its API key, if any, lives in the encrypted secrets.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelEntry {
    pub id: String,
    /// What the pickers show.
    pub label: String,
    /// The model's name on its server, sent as `model` in each request.
    pub model: String,
    /// The server's base URL, including the version prefix (`http://localhost:8000/v1`).
    pub base_url: String,
    /// The `reasoning_effort` values the model accepts, in the order to offer them. Empty: the
    /// model takes none, and a run may not ask for one.
    pub reasoning_levels: Vec<String>,
    /// The level a run gets when it names none; `None` sends no `reasoning_effort`.
    pub default_reasoning: Option<String>,
    /// Accepts images with a prompt, and is shown attached PDFs' first pages as images (see
    /// [`crate::attachments`]). Without it, a run with images is refused; PDFs are fine.
    pub multimodal: bool,
    /// The most tokens one reply may take, a reasoning model's thinking included. A reply that
    /// reaches it ends with `stop_reason` `max_tokens`.
    pub max_tokens: u32,
    /// The entry a run gets when it names no model. Exactly one entry has it while any exist.
    pub is_default: bool,
    /// What its tokens cost; `None` when no one said.
    pub price: Option<Price>,
    /// Order in the pickers.
    pub position: i64,
    /// What the server listed at `/models` when the entry was last checked.
    pub listed_models: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ModelEntry {
    /// The public view: what any key may see through `GET /v1/models`.
    pub fn public(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "model": self.model,
            "reasoning_levels": self.reasoning_levels,
            "default_reasoning": self.default_reasoning,
            "multimodal": self.multimodal,
            "max_tokens": self.max_tokens,
            "is_default": self.is_default,
            "price": self.price,
        })
    }
}

/// The reply limit a model gets when it is registered without one.
pub const DEFAULT_MAX_TOKENS: u32 = 16384;
/// The highest reply limit a model may be given.
pub const MAX_MAX_TOKENS: u32 = 1_000_000;

/// A model's price in US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub input_usd_per_m: f64,
    pub output_usd_per_m: f64,
}

impl Price {
    /// What `usage` cost at this price.
    pub fn cost_usd(&self, usage: Usage) -> f64 {
        (usage.input_tokens as f64 * self.input_usd_per_m
            + usage.output_tokens as f64 * self.output_usd_per_m)
            / 1_000_000.0
    }
}

/// The media types a run's prompt may come with: images and PDFs.
pub const ALLOWED_ATTACHMENT_TYPES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "application/pdf",
];

/// A file sent with a run's prompt, base64 as received. It is saved in the workspace when the run
/// starts, and an image is shown to the model in the opening user message (see
/// [`crate::attachments`]); cedar does not check that the model can read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub name: String,
    pub media_type: String,
    /// Standard base64 (with padding, no data: prefix) of the file's bytes.
    pub data: String,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.media_type.starts_with("image/")
    }

    /// The content block the attachment becomes in the conversation: `image` for images,
    /// `document` for PDFs, each with a base64 `source` and the file name as `title`.
    pub fn to_block(&self) -> Value {
        serde_json::json!({
            "type": if self.is_image() { "image" } else { "document" },
            "source": { "type": "base64", "media_type": self.media_type, "data": self.data },
            "title": self.name,
        })
    }
}

/// Event type names, as sent in the `type` field and the SSE `event:` line.
pub mod event {
    pub const RUN_STARTED: &str = "run.started";
    pub const ASSISTANT_TEXT: &str = "assistant.text";
    pub const TOOL_CALL: &str = "tool.call";
    pub const TOOL_RESULT: &str = "tool.result";
    pub const ASSISTANT_REASONING: &str = "assistant.reasoning";
    pub const PLAN: &str = "plan";
    pub const PLAN_UPDATED: &str = "plan.updated";
    pub const APPROVAL_REQUIRED: &str = "approval.required";
    pub const APPROVAL_RESOLVED: &str = "approval.resolved";
    pub const USAGE: &str = "usage";
    pub const RUN_COMPLETED: &str = "run.completed";
    pub const RUN_FAILED: &str = "run.failed";
    pub const RUN_CANCELLED: &str = "run.cancelled";

    pub fn is_terminal(kind: &str) -> bool {
        matches!(kind, RUN_COMPLETED | RUN_FAILED | RUN_CANCELLED)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub seq: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

impl Event {
    pub fn is_terminal(&self) -> bool {
        event::is_terminal(&self.kind)
    }
}

/// That a run ended: one per finished run, numbered across all runs of an owner so a client can
/// resume `GET /v1/events` from the last `id` it saw. The same JSON is POSTed to a run's
/// `callback_url` (see [`crate::webhooks`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notification {
    pub id: i64,
    /// The terminal event's type: `run.completed`, `run.failed` or `run.cancelled`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub run_id: Uuid,
    /// The terminal event's seq in the run's event stream.
    pub seq: i64,
    pub status: RunStatus,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
    /// The run this one continued, if any.
    pub parent_id: Option<Uuid>,
    pub finished_at: DateTime<Utc>,
}

impl RunStatus {
    /// The terminal event a run in this status ended with; `None` while it is still going.
    pub fn terminal_event(self) -> Option<&'static str> {
        Some(match self {
            RunStatus::Completed => event::RUN_COMPLETED,
            RunStatus::Failed => event::RUN_FAILED,
            RunStatus::Cancelled => event::RUN_CANCELLED,
            RunStatus::Queued | RunStatus::Running => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredMessage {
    pub seq: i64,
    pub role: String,
    pub content: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRunRequest {
    pub prompt: String,
    /// A registered model's id, or its name on the server when only one entry has it. Absent:
    /// the parent's for a continuation, else the default entry.
    pub model: Option<String>,
    /// One of the model's reasoning levels, sent as `reasoning_effort`. Absent: the parent's
    /// for a continuation of the same model, else the model's default level.
    pub reasoning: Option<String>,
    pub system: Option<String>,
    pub max_turns: Option<u32>,
    /// Continue an earlier, finished run instead of starting a fresh conversation.
    pub parent_id: Option<Uuid>,
    /// Tool groups (`web_search`, `gmail`, ...) and tool names the run may use, on top of the
    /// sandbox tools; `*` means all. Absent: the parent's selection for a continuation, else all.
    pub tools: Option<Vec<String>>,
    /// Images and PDFs to show the model with the prompt (see [`Attachment`]).
    pub attachments: Option<Vec<Attachment>>,
    /// An http(s) URL that is POSTed a notification when the run ends (see
    /// [`crate::webhooks`]). Absent: the parent's for a continuation, else none.
    pub callback_url: Option<String>,
    /// Signs those notifications (`X-Cedar-Signature`). Only with `callback_url`; absent for a
    /// continuation keeps the parent's.
    pub callback_secret: Option<String>,
    /// Plan before starting: `on`, `off` or `auto`. Absent: the server's `PLAN` setting.
    pub plan: Option<PlanMode>,
    /// The client will answer the run's questions while it goes: agree to its plan
    /// (`POST /v1/runs/{id}/plan`) and approve risky tool calls
    /// (`POST /v1/runs/{id}/approvals/{approval_id}`). Absent: false, and nothing waits.
    pub interactive: Option<bool>,
}
