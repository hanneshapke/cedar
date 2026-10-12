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
    pub model: String,
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
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewRun {
    pub owner: String,
    pub prompt: String,
    pub system: String,
    pub model: String,
    pub max_turns: u32,
    /// Continue this run: its conversation, and its workspace if that hasn't expired.
    pub parent_id: Option<Uuid>,
}

/// Event type names, as sent in the `type` field and the SSE `event:` line.
pub mod event {
    pub const RUN_STARTED: &str = "run.started";
    pub const ASSISTANT_TEXT: &str = "assistant.text";
    pub const TOOL_CALL: &str = "tool.call";
    pub const TOOL_RESULT: &str = "tool.result";
    pub const WEB_SEARCH: &str = "web_search";
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
    pub model: Option<String>,
    pub system: Option<String>,
    pub max_turns: Option<u32>,
    /// Continue an earlier, finished run instead of starting a fresh conversation.
    pub parent_id: Option<Uuid>,
}
