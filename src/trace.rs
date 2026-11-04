//! The trace of a run, for a client's expert view: every call to the model with what it was sent
//! and what came back, beside the run's conversation and event log (`GET /v1/runs/{id}/trace`).
//!
//! An agent call sends the run's system prompt, its tool definitions and the first
//! `message_count` messages of its conversation. Those are kept once (the prompt and tools on the
//! run, the conversation in `messages`) and the call records only the count. A planning call has a
//! prompt and messages of its own, so it records them in full.

use crate::llm::{LlmResponse, MessageRequest};
use crate::models::Run;
use crate::store::Store;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

/// The model call of an agent turn.
pub const AGENT: &str = "agent";
/// The call that asks for a plan before the first turn.
pub const PLAN: &str = "plan";

/// The most events a trace lists.
const MAX_EVENTS: u32 = 10_000;

/// One recorded call to the model.
#[derive(Debug, Clone, Serialize)]
pub struct LlmCall {
    pub seq: i64,
    /// The agent turn; 0 for the planning call.
    pub turn: u32,
    pub purpose: String,
    pub request: Value,
    /// `null` when the call failed.
    pub response: Option<Value>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
}

/// A call to record; the store numbers it.
pub struct NewCall<'a> {
    pub turn: u32,
    pub purpose: &'a str,
    pub request: Value,
    pub response: Option<Value>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
}

/// What a call sent. An agent call's prompt, tools and messages are kept elsewhere, so only their
/// count is recorded; any other call keeps them here.
pub fn request(req: &MessageRequest<'_>, purpose: &str) -> Value {
    let mut out = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "reasoning_effort": req.reasoning_effort,
        "message_count": req.messages.len(),
    });
    if purpose != AGENT {
        out["system"] = json!(req.system);
        out["tools"] = json!(req.tools);
        out["messages"] = without_file_data(&Value::Array(req.messages.to_vec()));
    }
    out
}

/// What a call got back.
pub fn response(resp: &LlmResponse) -> Value {
    json!({
        "content": resp.content,
        "stop_reason": resp.stop_reason,
        "usage": resp.usage,
        "reasoning": resp.reasoning,
    })
}

/// `value` with the base64 of attached files (`source.data` of `image` and `document` blocks)
/// replaced by its decoded size in `source.bytes`, so a trace stays small enough to read.
pub fn without_file_data(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(without_file_data).collect()),
        Value::Object(map) => {
            let mut out: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), without_file_data(v)))
                .collect();
            if let Some(source) = out.get_mut("source").and_then(Value::as_object_mut) {
                if source.get("type").and_then(Value::as_str) == Some("base64") {
                    if let Some(data) = source.remove("data") {
                        let len = data.as_str().map_or(0, |d| d.trim_end_matches('=').len());
                        source.insert("bytes".into(), json!(len * 3 / 4));
                    }
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// The whole trace of `run`: the run, the system prompt and tool definitions its agent calls were
/// sent (`null` for a run that hasn't reached its first turn, or ran before traces were kept), its
/// conversation, its model calls and its events. `memory_note` is where the user's memories sit
/// in `system` (`{start, end}`, byte offsets), `null` when it holds none, so a client that passes
/// the prompt on without them can cut exactly that.
pub async fn build(store: &Store, run: &Run) -> Result<Value> {
    let (system, tools) = store.sent_prompt(run.id).await?;
    let memory_note = system
        .as_deref()
        .and_then(crate::memory::note_span)
        .map(|r| json!({ "start": r.start, "end": r.end }));
    let messages: Vec<Value> = store
        .list_messages(run.id)
        .await?
        .into_iter()
        .map(|m| json!({ "seq": m.seq, "role": m.role, "content": without_file_data(&m.content) }))
        .collect();
    let calls = store.list_llm_calls(run.id).await?;
    let events = store.events_after(run.id, 0, MAX_EVENTS).await?;
    Ok(json!({
        "run": run,
        "system": system,
        "memory_note": memory_note,
        "tools": tools,
        "messages": messages,
        "calls": calls,
        "events": events,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_data_is_replaced_by_its_size() {
        let msg = json!([{ "role": "user", "content": [
            { "type": "text", "text": "look" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "aGVsbG8=" } },
        ] }]);
        let out = without_file_data(&msg);
        assert_eq!(out[0]["content"][0], msg[0]["content"][0]);
        assert_eq!(
            out[0]["content"][1]["source"],
            json!({ "type": "base64", "media_type": "image/png", "bytes": 5 })
        );
    }

    #[test]
    fn an_agent_call_records_counts_and_a_planning_call_its_prompt() {
        let tools = [json!({ "name": "bash" })];
        let messages = [json!({ "role": "user", "content": "hi" })];
        let req = MessageRequest {
            model: "m",
            max_tokens: 100,
            reasoning_effort: Some("high"),
            system: "sys",
            tools: &tools,
            messages: &messages,
        };
        let agent = request(&req, AGENT);
        assert_eq!(
            agent,
            json!({ "model": "m", "max_tokens": 100, "reasoning_effort": "high", "message_count": 1 })
        );
        let plan = request(&req, PLAN);
        assert_eq!(plan["system"], "sys");
        assert_eq!(plan["tools"], json!(tools));
        assert_eq!(plan["messages"], json!(messages));
    }
}
