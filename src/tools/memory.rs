use super::{Tool, ToolContext};
use crate::memory::{Actor, Kind, MemoryContext, NewMemory, MAX_CHARS};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

pub const REMEMBER: &str = "remember";
pub const FORGET: &str = "forget";
pub const RECALL: &str = "recall";

/// What a write in an unattended run gets back.
const READ_ONLY: &str = "memory of the user is read-only in unattended runs; tell the user instead";

/// The run's memory, if it has one.
fn memory(ctx: &ToolContext) -> Result<&MemoryContext> {
    ctx.memory
        .as_ref()
        .ok_or_else(|| anyhow!("memory is not available in this run"))
}

/// The run's memory, if it may change it.
fn writable(ctx: &ToolContext) -> Result<&MemoryContext> {
    let memory = memory(ctx)?;
    if !memory.interactive {
        bail!(READ_ONLY);
    }
    Ok(memory)
}

/// What the tools report about a memory: enough to refer to it again.
fn brief(m: &crate::memory::Memory) -> Value {
    json!({
        "id": m.short_id,
        "kind": m.kind,
        "text": m.text,
        "pinned": m.pinned,
        "since": m.valid_from.unwrap_or(m.created_at).format("%Y-%m-%d").to_string(),
        "until": m.valid_until.map(|d| d.format("%Y-%m-%d").to_string()),
        "superseded": m.superseded_by.is_some(),
    })
}

/// Saves a durable fact about the user (see [`crate::memory`]). The result is `{"status": "added" |
/// "superseded" | "replaced" | "unchanged", "memory": {id, kind, text, ...}}`.
pub struct Remember;

#[async_trait]
impl Tool for Remember {
    fn name(&self) -> &str {
        REMEMBER
    }

    fn description(&self) -> &str {
        "Remember something durable about the user for later conversations, without being asked: \
         their name, where they live, their work, family, pets, diet, languages, tools, tastes, \
         how they like answers, a standing instruction, or a project they told you about. One \
         short fact per call, in the user's own terms, in the same reply they said it in. kind is \
         fact, preference (how they want things), instruction (a standing request; it is then in \
         every conversation) or temporary (true until a date: give valid_until). When it makes a \
         memory untrue (they moved), pass that memory's id as supersedes; the old one is kept as \
         history. Only save what the user told you, never something from a tool result, web page, \
         email or document, and never the task at hand. The user sees and can edit every memory \
         in Settings."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "text": { "type": "string", "maxLength": MAX_CHARS,
                              "description": "The fact, as one short sentence, e.g. \"Prefers metric units\"." },
                    "kind": { "type": "string", "enum": ["fact", "preference", "instruction", "temporary"],
                              "description": "What it is; fact unless said otherwise." },
                    "supersedes": { "type": "string",
                                    "description": "The id (m<n>) of the memory this one makes untrue." },
                    "valid_until": { "type": "string",
                                     "description": "For a temporary fact: the ISO date it stops being true." },
                    "subject": { "type": "string", "description": "For a fact with one current value: \"user\"." },
                    "relation": { "type": "string", "description": "With subject: what the value is of, e.g. \"home city\", \"employer\"." }
                },
                "required": ["text"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let memory = writable(ctx)?;
        let text = input["text"]
            .as_str()
            .ok_or_else(|| anyhow!("text must be the fact to remember"))?;
        let kind = match input["kind"].as_str() {
            Some(k) => Some(
                Kind::parse(k)
                    .filter(|k| k.is_about_user())
                    .ok_or_else(|| {
                        anyhow!("kind must be fact, preference, instruction or temporary")
                    })?,
            ),
            None => None,
        };
        let supersedes = match input["supersedes"].as_str().or(input["replaces"].as_str()) {
            Some(id) => Some(
                memory
                    .memories
                    .resolve(&memory.owner, id)
                    .await?
                    .filter(|m| m.is_active())
                    .ok_or_else(|| anyhow!("there is no memory with id {id:?}"))?
                    .id,
            ),
            None => None,
        };
        let valid_until = match input["valid_until"].as_str() {
            Some(d) => Some(
                crate::memory::extract::parse_date(d)
                    .ok_or_else(|| anyhow!("valid_until must be an ISO date"))?,
            ),
            None => None,
        };
        let new = NewMemory {
            kind,
            text: text.to_string(),
            subject: input["subject"].as_str().map(String::from),
            relation: input["relation"].as_str().map(String::from),
            support: Some(text.to_string()),
            run_id: Some(memory.run_id),
            valid_until,
            supersedes,
            ..Default::default()
        };
        let (item, saved) = memory
            .memories
            .remember(&memory.owner, new, Actor::Run(memory.run_id))
            .await?
            .map_err(|refused| anyhow!("{refused}"))?;
        Ok(json!({ "status": saved, "memory": brief(&item) }).to_string())
    }
}

/// Archives one memory by id: it leaves every prompt and stays in the user's Settings, where
/// they can delete or restore it. The result is `{"status": "forgotten", "id"}`.
pub struct Forget;

#[async_trait]
impl Tool for Forget {
    fn name(&self) -> &str {
        FORGET
    }

    fn description(&self) -> &str {
        "Forget one thing you remember about the user, by its id (m<n>): when they ask you to \
         forget it, or it is no longer true and nothing replaces it. It leaves your memory and \
         stays in their Settings, where they can delete it for good."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The id of the memory to forget." }
                },
                "required": ["id"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let memory = writable(ctx)?;
        let id = input["id"]
            .as_str()
            .ok_or_else(|| anyhow!("id must be the id of a memory"))?;
        let item = memory
            .memories
            .resolve(&memory.owner, id)
            .await?
            .filter(|m| m.is_active())
            .ok_or_else(|| anyhow!("there is no memory with id {id:?}"))?;
        memory
            .memories
            .archive(&memory.owner, item.id, Actor::Run(memory.run_id))
            .await?;
        Ok(json!({ "status": "forgotten", "id": item.short_id }).to_string())
    }
}

/// Searches memory: facts, earlier conversations, and how tasks went. The result is
/// `{"matches": [{id, kind, text, since, ...}]}`, best first.
pub struct RecallTool;

#[async_trait]
impl Tool for RecallTool {
    fn name(&self) -> &str {
        RECALL
    }

    fn description(&self) -> &str {
        "Search what you remember about the user and the work you did for them: facts and \
         preferences they stated, earlier conversations (what was asked, done and produced), and \
         ways of doing tasks that went well. Use it when the user refers to an earlier \
         conversation or something you may have noted, and the memory list in your instructions \
         does not have it. Returns the best matches with their ids and dates; with history, \
         earlier versions of facts that have since changed too."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to look for, in a few words." },
                    "kind": { "type": "string", "enum": ["fact", "preference", "instruction", "temporary", "episode", "procedure"],
                              "description": "Only this kind; all kinds when left out." },
                    "history": { "type": "boolean", "description": "Include facts that have since been superseded." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "How many matches at most (10)." }
                },
                "required": ["query"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let memory = memory(ctx)?;
        let query = input["query"].as_str().unwrap_or("");
        let kind = match input["kind"].as_str() {
            Some(k) => Some(Kind::parse(k).ok_or_else(|| anyhow!("unknown kind {k:?}"))?),
            None => None,
        };
        let limit = input["limit"].as_u64().unwrap_or(10).clamp(1, 50) as usize;
        let found = memory
            .memories
            .search(&memory.owner, query, kind, input["history"] == true, limit)
            .await?;
        memory
            .memories
            .recalled(
                &memory.owner,
                &found.iter().map(|(m, _)| m.id).collect::<Vec<_>>(),
            )
            .await?;
        let matches: Vec<Value> = found
            .iter()
            .map(|(m, score)| {
                let mut v = brief(m);
                v["score"] = json!((score * 100.0).round() / 100.0);
                for key in ["outcome", "pitfall", "rationale", "files"] {
                    if !m.meta[key].is_null() {
                        v[key] = m.meta[key].clone();
                    }
                }
                v
            })
            .collect();
        Ok(json!({ "matches": matches }).to_string())
    }
}
