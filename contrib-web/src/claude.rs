//! Claude's data export (Settings → Privacy → Export data): `conversations.json`, an array of
//! conversations, each with `chat_messages` from `human` and `assistant`. Newer exports link
//! messages with `parent_message_uuid`, because an edited prompt or a retried answer starts a
//! branch; then the newest message's chain is the conversation as the person last saw it.
//! Assistant messages carry content blocks (`text`, `thinking`, `tool_use`, `tool_result`); the
//! tools are Claude's own (web search, artifacts, analysis), run on Anthropic's side, so a call
//! and its result sit in the same message.

use crate::transcript::{day_of_iso, tool_kind, FileStub, Item, Source, ToolKind, Transcript};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// The messages on the branch last looked at, oldest first.
fn branch(conv: &Value) -> Vec<&Value> {
    let all: Vec<&Value> = conv["chat_messages"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    if !all.iter().any(|m| m["parent_message_uuid"].is_string()) {
        return all;
    }
    let by_id: HashMap<&str, &Value> = all.iter().filter_map(|m| Some((m["uuid"].as_str()?, *m))).collect();
    let Some(leaf) = all.iter().max_by(|a, b| a["created_at"].as_str().unwrap_or("").cmp(b["created_at"].as_str().unwrap_or(""))) else {
        return Vec::new();
    };
    let mut out = vec![*leaf];
    let mut seen: HashSet<&str> = HashSet::new();
    let mut at = leaf["parent_message_uuid"].as_str();
    while let Some(id) = at {
        if !seen.insert(id) {
            break;
        }
        let Some(m) = by_id.get(id) else { break };
        out.push(m);
        at = m["parent_message_uuid"].as_str();
    }
    out.reverse();
    out
}

/// The text of a block list's text blocks.
fn texts(blocks: &[Value]) -> String {
    blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// A tool result's content as text.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| match i {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o
                    .get("text")
                    .and_then(Value::as_str)
                    .map(String::from)
                    // Web search results: a title and an address each.
                    .or_else(|| {
                        let title = o.get("title").and_then(Value::as_str)?;
                        Some(format!("{title} {}", o.get("url").and_then(Value::as_str).unwrap_or("")).trim().to_string())
                    }),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn title(conv: &Value) -> String {
    conv["name"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("Untitled").to_string()
}

pub fn import(conv: &Value) -> Transcript {
    let mut t = Transcript::new(Source::Claude);
    t.model = conv["model"].as_str().map(String::from);
    t.day = conv["created_at"].as_str().and_then(day_of_iso);
    let mut open: Vec<(String, String)> = Vec::new();
    // Calls cut because they read or wrote memory, by id: their results go too.
    let mut cut: HashSet<String> = HashSet::new();
    let mut next_id = 0usize;

    for m in branch(conv) {
        let blocks: Vec<Value> = match m["content"].as_array() {
            Some(b) if !b.is_empty() => b.clone(),
            _ => m["text"].as_str().map(|s| vec![json!({ "type": "text", "text": s })]).unwrap_or_default(),
        };
        match m["sender"].as_str() {
            Some("human") => {
                let mut files = Vec::new();
                for a in m["attachments"].as_array().into_iter().flatten() {
                    // Text extracted from an uploaded document: the person's document, never sent.
                    files.push(FileStub {
                        kind: "document",
                        media_type: a["file_type"].as_str().filter(|s| !s.is_empty()).unwrap_or("text/plain").to_string(),
                        bytes: a["file_size"].as_u64().unwrap_or_else(|| a["extracted_content"].as_str().map_or(0, |s| s.len() as u64)),
                    });
                    t.dropped.attachments += 1;
                }
                for f in m["files"].as_array().into_iter().flatten() {
                    let kind = f["file_kind"].as_str().unwrap_or("image");
                    files.push(FileStub {
                        kind: if kind == "image" { "image" } else { "document" },
                        media_type: if kind == "image" { "image/*".into() } else { "application/octet-stream".into() },
                        bytes: f["file_size"].as_u64().unwrap_or(0),
                    });
                    t.dropped.attachments += 1;
                }
                let text = texts(&blocks);
                if text.is_empty() && files.is_empty() {
                    continue;
                }
                open.clear();
                t.items.push(Item::Prompt { text, day: m["created_at"].as_str().and_then(day_of_iso), files });
            }
            Some("assistant") => {
                for b in &blocks {
                    match b["type"].as_str() {
                        Some("text") => {
                            if let Some(s) = b["text"].as_str().filter(|s| !s.trim().is_empty()) {
                                t.items.push(Item::Text { text: s.to_string() });
                            }
                        }
                        Some("thinking") => {
                            if let Some(s) = b["thinking"].as_str().filter(|s| !s.trim().is_empty()) {
                                t.items.push(Item::Reasoning { text: s.to_string() });
                            }
                        }
                        Some("tool_use") => {
                            let name = b["name"].as_str().unwrap_or("tool").to_string();
                            next_id += 1;
                            let id = b["id"].as_str().filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| format!("toolu_{next_id}"));
                            match tool_kind(&name) {
                                ToolKind::Memory => {
                                    t.dropped.memory = true;
                                    cut.insert(id);
                                    continue;
                                }
                                ToolKind::Private => {
                                    t.private_tools.insert(name.clone());
                                }
                                _ => {}
                            }
                            let input = if b["input"].is_object() { b["input"].clone() } else { json!({ "input": b["input"] }) };
                            open.push((id.clone(), name.clone()));
                            t.items.push(Item::ToolCall { id, name, input });
                        }
                        Some("tool_result") => {
                            let given = b["tool_use_id"].as_str().unwrap_or_default();
                            if cut.contains(given) {
                                continue;
                            }
                            let name = b["name"].as_str().unwrap_or_default();
                            let at = open
                                .iter()
                                .position(|(id, _)| id == given)
                                .or_else(|| open.iter().position(|(_, n)| n == name))
                                .or(if open.is_empty() { None } else { Some(0) });
                            let Some(at) = at else {
                                if tool_kind(name) == ToolKind::Memory {
                                    t.dropped.memory = true;
                                }
                                continue;
                            };
                            let (id, called) = open.remove(at);
                            match tool_kind(&called) {
                                ToolKind::Files => {
                                    t.files.insert(id.clone());
                                }
                                ToolKind::Fetching => {
                                    t.fetched.insert(id.clone());
                                }
                                _ => {}
                            }
                            t.items.push(Item::ToolResult { id, text: result_text(&b["content"]), is_error: b["is_error"].as_bool() == Some(true) });
                        }
                        _ => {} // token budgets, citations' metadata and the like
                    }
                }
            }
            _ => {}
        }
    }
    t
}

pub fn looks_like(v: &Value) -> bool {
    v.get("chat_messages").is_some_and(Value::is_array)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_blocks_and_attachments() {
        let conv = json!({
            "uuid": "c1", "name": "Primes", "created_at": "2025-05-01T10:00:00Z",
            "chat_messages": [
                { "uuid": "a", "sender": "human", "created_at": "2025-05-01T10:00:00Z", "text": "find primes",
                  "content": [{ "type": "text", "text": "find primes" }],
                  "attachments": [{ "file_name": "taxes-2024.pdf", "file_size": 5000, "file_type": "application/pdf", "extracted_content": "SSN 123" }],
                  "files": [] },
                { "uuid": "b", "sender": "assistant", "created_at": "2025-05-01T10:00:05Z", "text": "",
                  "content": [
                    { "type": "thinking", "thinking": "Let me search." },
                    { "type": "tool_use", "name": "web_search", "input": { "query": "primes" } },
                    { "type": "tool_result", "name": "web_search", "content": [{ "type": "knowledge", "title": "Prime number", "url": "https://en.wikipedia.org/wiki/Prime_number" }] },
                    { "type": "text", "text": "2, 3, 5, 7." }
                  ] }
            ]
        });
        let t = import(&conv);
        assert_eq!(t.dropped.attachments, 1);
        assert!(!format!("{:?}", t.items).contains("taxes"));
        assert!(!format!("{:?}", t.items).contains("SSN"));
        assert!(matches!(&t.items[0], Item::Prompt { text, files, day: Some(d) } if text == "find primes" && files[0].bytes == 5000 && d == "2025-05-01"));
        assert!(matches!(&t.items[1], Item::Reasoning { .. }));
        assert!(matches!(&t.items[2], Item::ToolCall { name, .. } if name == "web_search"));
        assert!(matches!(&t.items[3], Item::ToolResult { text, .. } if text.contains("Prime number")));
        assert_eq!(t.fetched.len(), 1);
        assert!(matches!(&t.items[4], Item::Text { text } if text == "2, 3, 5, 7."));
    }

    #[test]
    fn the_newest_branch_wins() {
        let root = "00000000-0000-4000-8000-000000000000";
        let conv = json!({
            "chat_messages": [
                { "uuid": "a", "parent_message_uuid": root, "sender": "human", "created_at": "2025-05-01T10:00:00Z", "text": "first try" },
                { "uuid": "b", "parent_message_uuid": "a", "sender": "assistant", "created_at": "2025-05-01T10:00:01Z", "text": "answer to first" },
                { "uuid": "c", "parent_message_uuid": root, "sender": "human", "created_at": "2025-05-01T10:01:00Z", "text": "edited prompt" },
                { "uuid": "d", "parent_message_uuid": "c", "sender": "assistant", "created_at": "2025-05-01T10:01:01Z", "text": "answer to edit" }
            ]
        });
        let t = import(&conv);
        let s = format!("{:?}", t.items);
        assert!(s.contains("edited prompt") && s.contains("answer to edit"));
        assert!(!s.contains("first try"));
    }
}
