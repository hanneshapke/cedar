//! ChatGPT's data export (Settings → Data controls → Export data): `conversations.json`, an array
//! of conversations. Each is a tree of messages (`mapping`), because an edited prompt or a
//! regenerated answer starts a branch; `current_node` is the leaf the person last looked at, and
//! the path from the root to it is the conversation as they saw it.

use crate::transcript::{day_of_epoch, tool_kind, FileStub, Item, Source, ToolKind, Transcript};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

/// The messages on the branch the person last looked at, root first.
fn branch(conv: &Value) -> Vec<&Value> {
    let Some(mapping) = conv["mapping"].as_object() else { return Vec::new() };
    let leaf = conv["current_node"]
        .as_str()
        .filter(|id| mapping.contains_key(*id))
        .map(String::from)
        .or_else(|| {
            // No current node: the newest leaf.
            mapping
                .iter()
                .filter(|(_, n)| n["children"].as_array().is_none_or(|c| c.is_empty()))
                .max_by(|a, b| {
                    let t = |n: &Value| n["message"]["create_time"].as_f64().unwrap_or(0.0);
                    t(a.1).total_cmp(&t(b.1))
                })
                .map(|(id, _)| id.clone())
        });
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut at = leaf;
    while let Some(id) = at {
        if !seen.insert(id.clone()) {
            break; // a cycle: not a tree, stop rather than loop
        }
        let Some(node) = mapping.get(&id) else { break };
        if node["message"].is_object() {
            out.push(&node["message"]);
        }
        at = node["parent"].as_str().map(String::from);
    }
    out.reverse();
    out
}

/// The text of a `parts` array: strings, and audio transcriptions; images become stubs.
fn parts(content: &Value, files: &mut Vec<FileStub>, dropped: &mut cedar_contrib_schema::Dropped) -> String {
    let mut text = Vec::new();
    for p in content["parts"].as_array().into_iter().flatten() {
        match p {
            Value::String(s) => text.push(s.clone()),
            Value::Object(o) => match o.get("content_type").and_then(Value::as_str) {
                Some("image_asset_pointer") => {
                    files.push(FileStub {
                        kind: "image",
                        media_type: "image/*".into(),
                        bytes: o.get("size_bytes").and_then(Value::as_u64).unwrap_or(0),
                    });
                    dropped.attachments += 1;
                }
                Some("audio_transcription") => {
                    if let Some(t) = o.get("text").and_then(Value::as_str) {
                        text.push(t.to_string());
                    }
                }
                Some(_) | None => {
                    if let Some(t) = o.get("text").and_then(Value::as_str) {
                        text.push(t.to_string());
                    }
                }
            },
            _ => {}
        }
    }
    text.join("\n").trim().to_string()
}

/// A tool call's input from what the assistant addressed to the tool.
fn call_input(recipient: &str, content: &Value) -> Value {
    let text = content["text"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| content["parts"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"));
    if recipient == "python" || content["language"].as_str().is_some_and(|l| l.contains("python")) {
        return json!({ "code": text });
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(o)) => Value::Object(o),
        _ => json!({ "text": text }),
    }
}

/// A tool message's output as text.
fn tool_output(content: &Value) -> String {
    match content["content_type"].as_str() {
        Some("execution_output") | Some("system_error") | Some("code") => content["text"].as_str().unwrap_or_default().to_string(),
        Some("tether_browsing_display") => {
            let mut s = content["summary"].as_str().unwrap_or_default().to_string();
            if let Some(r) = content["result"].as_str() {
                if !s.is_empty() {
                    s.push('\n');
                }
                s.push_str(r);
            }
            s
        }
        Some("tether_quote") => format!(
            "{}\n{}\n{}",
            content["title"].as_str().unwrap_or_default(),
            content["url"].as_str().unwrap_or_default(),
            content["text"].as_str().unwrap_or_default()
        )
        .trim()
        .to_string(),
        _ => {
            let mut out = Vec::new();
            for p in content["parts"].as_array().into_iter().flatten() {
                match p {
                    Value::String(s) => out.push(s.clone()),
                    Value::Object(o) if o.get("content_type").and_then(Value::as_str) == Some("image_asset_pointer") => {
                        out.push("[an image the tool made, left out]".into());
                    }
                    Value::Object(o) => {
                        if let Some(t) = o.get("text").and_then(Value::as_str) {
                            out.push(t.to_string());
                        }
                    }
                    _ => {}
                }
            }
            if out.is_empty() {
                content["text"].as_str().unwrap_or_default().to_string()
            } else {
                out.join("\n")
            }
        }
    }
}

/// The conversation's title, for the picker only (it never goes into a bundle).
pub fn title(conv: &Value) -> String {
    conv["title"].as_str().unwrap_or("Untitled").to_string()
}

pub fn import(conv: &Value) -> Transcript {
    let mut t = Transcript::new(Source::ChatGpt);
    t.model = conv["default_model_slug"].as_str().map(String::from);
    t.day = conv["create_time"].as_f64().and_then(day_of_epoch);
    // Calls awaiting their result: (id, tool name).
    let mut open: Vec<(String, String)> = Vec::new();
    let mut next_id = 0usize;

    for m in branch(conv) {
        let role = m["author"]["role"].as_str().unwrap_or_default();
        let meta = &m["metadata"];
        let content = &m["content"];
        let ctype = content["content_type"].as_str().unwrap_or_default();
        if meta["is_visually_hidden_from_conversation"].as_bool() == Some(true) && role != "tool" {
            if matches!(ctype, "user_editable_context" | "model_editable_context") {
                t.dropped.memory = true;
            }
            continue;
        }
        if matches!(ctype, "user_editable_context" | "model_editable_context") {
            // Custom instructions and saved memories: the most personal thing in the export.
            t.dropped.memory = true;
            continue;
        }
        match role {
            "user" => {
                let mut files = Vec::new();
                let text = parts(content, &mut files, &mut t.dropped);
                for a in meta["attachments"].as_array().into_iter().flatten() {
                    // Uploaded files: the export has their names and sizes, never their bytes in
                    // the message. Names are not kept; the stub says what kind of file it was.
                    let media = a["mime_type"].as_str().or(a["mimeType"].as_str()).unwrap_or("application/octet-stream");
                    if media.starts_with("image/") && files.iter().any(|f| f.kind == "image") {
                        continue; // already counted from the image part
                    }
                    files.push(FileStub {
                        kind: if media.starts_with("image/") { "image" } else { "document" },
                        media_type: media.to_string(),
                        bytes: a["size"].as_u64().unwrap_or(0),
                    });
                    t.dropped.attachments += 1;
                }
                if text.is_empty() && files.is_empty() {
                    continue;
                }
                open.clear();
                t.items.push(Item::Prompt { text, day: m["create_time"].as_f64().and_then(day_of_epoch), files });
            }
            "assistant" => {
                if let Some(slug) = meta["model_slug"].as_str() {
                    t.model = Some(slug.to_string());
                }
                let recipient = m["recipient"].as_str().unwrap_or("all");
                match ctype {
                    "thoughts" => {
                        let text: Vec<String> = content["thoughts"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|th| {
                                let s = th["content"].as_str().or(th["summary"].as_str())?;
                                (!s.trim().is_empty()).then(|| s.to_string())
                            })
                            .collect();
                        if !text.is_empty() {
                            t.items.push(Item::Reasoning { text: text.join("\n\n") });
                        }
                    }
                    "reasoning_recap" => {}
                    _ if recipient != "all" => {
                        let kind = tool_kind(recipient);
                        next_id += 1;
                        let id = format!("call_{next_id}");
                        open.push((id.clone(), recipient.to_string()));
                        match kind {
                            ToolKind::Memory => {
                                t.dropped.memory = true;
                                continue;
                            }
                            ToolKind::Private => {
                                t.private_tools.insert(recipient.to_string());
                            }
                            _ => {}
                        }
                        t.items.push(Item::ToolCall { id, name: recipient.to_string(), input: call_input(recipient, content) });
                    }
                    _ => {
                        let mut files = Vec::new();
                        let text = if ctype == "code" { content["text"].as_str().unwrap_or_default().to_string() } else { parts(content, &mut files, &mut t.dropped) };
                        if !text.trim().is_empty() {
                            t.items.push(Item::Text { text });
                        }
                    }
                }
            }
            "tool" => {
                let name = m["author"]["name"].as_str().unwrap_or_default();
                // The open call this answers: the first with the same tool name (or the same
                // family, `web` for `web.run`), else the oldest open one.
                let family = |n: &str| n.split('.').next().unwrap_or(n).to_string();
                let at = open.iter().position(|(_, n)| n == name).or_else(|| open.iter().position(|(_, n)| family(n) == family(name))).or(if open.is_empty() { None } else { Some(0) });
                let Some(at) = at else {
                    // A result nothing asked for (search results ChatGPT shows on its own): left
                    // out, since there is no call to pair it with.
                    if !tool_output(content).trim().is_empty() {
                        t.dropped.fetched_bodies += 1;
                    }
                    continue;
                };
                let (id, called) = open.remove(at);
                let kind = tool_kind(&called);
                if kind == ToolKind::Memory {
                    continue;
                }
                match kind {
                    ToolKind::Files => {
                        t.files.insert(id.clone());
                    }
                    ToolKind::Fetching => {
                        t.fetched.insert(id.clone());
                    }
                    _ => {}
                }
                let is_error = content["content_type"].as_str() == Some("system_error") || m["status"].as_str() == Some("failed");
                t.items.push(Item::ToolResult { id, text: tool_output(content), is_error });
            }
            _ => {} // system messages: the provider's, and usually empty
        }
    }
    t
}

/// Whether a JSON value looks like one ChatGPT conversation.
pub fn looks_like(v: &Value) -> bool {
    v.get("mapping").is_some_and(Value::is_object)
}

/// A minimal conversation for tests: `(role, recipient or author name, content)` in order.
#[doc(hidden)]
pub fn fixture(title: &str, msgs: &[(&str, &str, Value)]) -> Value {
    let mut mapping = Map::new();
    let mut parent: Option<String> = None;
    mapping.insert("root".into(), json!({ "id": "root", "message": null, "parent": null, "children": [] }));
    parent.replace("root".into());
    for (i, (role, who, content)) in msgs.iter().enumerate() {
        let id = format!("m{i}");
        let author = if *role == "tool" { json!({ "role": "tool", "name": who }) } else { json!({ "role": role }) };
        let recipient = if *role == "assistant" { *who } else { "all" };
        mapping.insert(
            id.clone(),
            json!({
                "id": id,
                "parent": parent,
                "children": [],
                "message": {
                    "id": id, "author": author, "recipient": recipient,
                    "create_time": 1_730_419_200.0 + i as f64,
                    "content": content,
                    "metadata": { "model_slug": "gpt-4o" },
                },
            }),
        );
        if let Some(p) = &parent {
            mapping[p]["children"].as_array_mut().unwrap().push(json!(id));
        }
        parent = Some(id);
    }
    json!({ "title": title, "create_time": 1_730_419_200.0, "mapping": mapping, "current_node": parent })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Value {
        json!({ "content_type": "text", "parts": [s] })
    }

    #[test]
    fn the_branch_last_looked_at_is_the_conversation() {
        let mut conv = fixture("t", &[("user", "", text("first")), ("assistant", "all", text("answer one"))]);
        // An edited prompt: a sibling of m0 that current_node does not lead to.
        conv["mapping"]["x"] = json!({ "id": "x", "parent": "root", "children": [], "message": { "author": { "role": "user" }, "content": text("abandoned"), "metadata": {} } });
        let t = import(&conv);
        assert_eq!(t.items.len(), 2);
        assert!(matches!(&t.items[0], Item::Prompt { text, day: Some(d), .. } if text == "first" && d == "2024-11-01"));
        assert_eq!(t.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn tool_calls_pair_with_their_results_and_custom_instructions_go() {
        let conv = fixture(
            "t",
            &[
                ("user", "", json!({ "content_type": "user_editable_context", "user_profile": "I live in Springfield" })),
                ("user", "", text("plot primes")),
                ("assistant", "python", json!({ "content_type": "code", "language": "unknown", "text": "print(2)" })),
                ("tool", "python", json!({ "content_type": "execution_output", "text": "2" })),
                ("assistant", "bio", json!({ "content_type": "code", "text": "User likes primes" })),
                ("tool", "bio", json!({ "content_type": "text", "parts": ["Model set context updated."] })),
                ("assistant", "browser", json!({ "content_type": "code", "text": "search(\"primes\")" })),
                ("tool", "browser", json!({ "content_type": "tether_browsing_display", "result": "page text" })),
                ("assistant", "all", text("Done.")),
            ],
        );
        let t = import(&conv);
        assert!(t.dropped.memory);
        let kinds: Vec<&str> = t
            .items
            .iter()
            .map(|i| match i {
                Item::Prompt { .. } => "prompt",
                Item::Text { .. } => "text",
                Item::Reasoning { .. } => "reasoning",
                Item::ToolCall { .. } => "call",
                Item::ToolResult { .. } => "result",
            })
            .collect();
        assert_eq!(kinds, ["prompt", "call", "result", "call", "result", "text"]);
        assert!(matches!(&t.items[1], Item::ToolCall { name, input, .. } if name == "python" && input["code"] == "print(2)"));
        assert_eq!(t.fetched.len(), 1);
        assert!(!format!("{:?}", t.items).contains("Springfield"));
        assert!(!format!("{:?}", t.items).contains("likes primes"));
    }

    #[test]
    fn images_become_stubs_and_connectors_are_private() {
        let conv = fixture(
            "t",
            &[
                ("user", "", json!({ "content_type": "multimodal_text", "parts": [{ "content_type": "image_asset_pointer", "size_bytes": 1234 }, "what is this"] })),
                ("assistant", "api_tool.call_tool", json!({ "content_type": "code", "text": "{\"path\": \"/gmail/search\"}" })),
                ("tool", "api_tool.call_tool", text("mail from bob")),
                ("assistant", "all", text("A cat.")),
            ],
        );
        let t = import(&conv);
        assert_eq!(t.dropped.attachments, 1);
        assert!(matches!(&t.items[0], Item::Prompt { files, .. } if files.len() == 1 && files[0].bytes == 1234));
        assert!(t.private_tools.contains("api_tool.call_tool"));
    }
}
