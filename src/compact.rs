//! Compacting a conversation into one self-contained prompt.
//!
//! A client that wants to re-run what a conversation achieved (on a schedule, say) needs a single
//! prompt rather than the back-and-forth that led there. This asks the model to write that prompt
//! from a finished run's stored conversation, in one call with no tools and no sandbox. Tool
//! results are trimmed first so a long run doesn't cost a long context window.

use crate::agent::opening_message;
use crate::llm::{LlmClient, MessageRequest};
use crate::models::{StoredMessage, Usage};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// The system prompt for the compaction call. The run's own system prompt is not used: the model
/// is not acting as the agent here, it is writing instructions for one.
pub const SYSTEM: &str = "You turn a conversation between a user and an AI agent into a single, \
    self-contained task prompt for that agent. You write prompts, you do not carry out tasks.";

/// The request that closes the conversation.
pub const INSTRUCTION: &str = "Review this whole conversation and write ONE self-contained prompt \
    that a fresh instance of the agent could carry out unattended, with the same tools but no \
    memory of this conversation and an empty workspace, to produce the final outcome the user \
    ended up wanting. Fold in everything that was clarified or corrected along the way; leave out \
    dead ends and the parts the user dropped. Include the concrete inputs (names, URLs, places, \
    numbers), the constraints, the format of the result, and the names of any files to produce. \
    Write it in the second person imperative, as the user would. Do not ask questions, do not add \
    a preamble or commentary, and do not mention this conversation. Reply with the prompt text \
    only.";

/// How much of each tool result is kept, in characters.
pub const TOOL_RESULT_MAX_CHARS: usize = 2_000;
/// How much conversation is sent in total, in bytes of its JSON form; older messages go first.
pub const HISTORY_MAX_BYTES: usize = 60_000;

/// The stored messages, as the chat API sends them (`{"role", "content"}`).
pub fn to_messages(stored: Vec<StoredMessage>) -> Vec<Value> {
    stored
        .into_iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect()
}

/// Cut each tool result to `per_result` characters and drop the oldest messages (but never the
/// first one, which carries the task) until the JSON form fits in `total` bytes.
pub fn trim_messages(mut messages: Vec<Value>, per_result: usize, total: usize) -> Vec<Value> {
    for message in &mut messages {
        if let Some(blocks) = message["content"].as_array_mut() {
            // Images and PDFs sent with the prompt would fill the budget on their own, and the
            // prompt they belong to is never dropped; a note in their place says what was there.
            for block in blocks.iter_mut() {
                let kind = match block["type"].as_str() {
                    Some(kind @ ("image" | "document")) => kind,
                    _ => continue,
                };
                let title = block["title"]
                    .as_str()
                    .or_else(|| block["source"]["media_type"].as_str())
                    .unwrap_or("file");
                *block = json!({ "type": "text", "text": format!("[{kind} omitted: {title}]\n") });
            }
            for block in blocks.iter_mut().filter(|b| b["type"] == "tool_result") {
                let text = match &block["content"] {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts
                        .iter()
                        .filter(|p| p["type"] == "text")
                        .filter_map(|p| p["text"].as_str())
                        .collect(),
                    _ => continue,
                };
                block["content"] = Value::String(cut(&text, per_result));
            }
        }
    }
    let size = |m: &Value| m.to_string().len();
    let mut used: usize = messages.iter().map(size).sum();
    let mut dropped = 0;
    while used > total && messages.len() > 2 {
        used -= size(&messages[1]);
        messages.remove(1);
        dropped += 1;
    }
    if dropped > 0 {
        let note = format!("({dropped} earlier message(s) omitted to fit the context window)");
        messages.insert(1, json!({ "role": "user", "content": note }));
        // Two user messages in a row are fine for the chat format; a dangling tool_use is not.
        if messages
            .get(2)
            .and_then(|m| m["content"].as_array())
            .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
        {
            // The results now answer nothing; drop them rather than confuse the model.
            messages.remove(2);
        }
    }
    messages
}

fn cut(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}\n[… {} characters omitted]", total - max_chars)
}

/// The messages for a compaction call: the (trimmed) conversation closed by the instruction.
pub fn messages_for_run(stored: Vec<StoredMessage>) -> Vec<Value> {
    let mut messages = trim_messages(
        to_messages(stored),
        TOOL_RESULT_MAX_CHARS,
        HISTORY_MAX_BYTES,
    );
    let closing = opening_message(&messages, INSTRUCTION, false);
    messages.push(json!({ "role": "user", "content": closing }));
    messages
}

/// The messages for a compaction call over a plain-text transcript.
pub fn messages_for_transcript(transcript: &str) -> Vec<Value> {
    vec![json!({
        "role": "user",
        "content": format!("<conversation>\n{}\n</conversation>\n\n{INSTRUCTION}", transcript.trim()),
    })]
}

pub struct Compacted {
    pub prompt: String,
    pub usage: Usage,
}

/// One tool-free model call. Fails if the model returned no text.
pub async fn compact(
    llm: &dyn LlmClient,
    model: &str,
    max_tokens: u32,
    messages: &[Value],
) -> Result<Compacted> {
    let req = MessageRequest {
        model,
        max_tokens,
        reasoning_effort: None,
        system: SYSTEM,
        tools: &[],
        messages,
    };
    let resp = llm.create_message(&req).await?;
    let prompt: String = resp
        .content
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    if prompt.is_empty() {
        return Err(anyhow!("the model returned no text"));
    }
    Ok(Compacted {
        prompt,
        usage: resp.usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(role: &str, content: Value) -> StoredMessage {
        StoredMessage {
            seq: 0,
            role: role.into(),
            content,
        }
    }

    #[test]
    fn tool_results_are_cut_with_a_marker() {
        let long = "x".repeat(50);
        let messages = vec![json!({ "role": "user", "content": [
            { "type": "tool_result", "tool_use_id": "t", "content": long },
            { "type": "tool_result", "tool_use_id": "u", "content": [{ "type": "text", "text": "short" }] },
        ] })];
        let out = trim_messages(messages, 10, 10_000);
        let blocks = out[0]["content"].as_array().unwrap();
        assert_eq!(
            blocks[0]["content"],
            format!("{}\n[… 40 characters omitted]", "x".repeat(10))
        );
        assert_eq!(blocks[1]["content"], "short");
    }

    #[test]
    fn attachments_become_placeholders_before_sizing() {
        let big = "A".repeat(200_000);
        let mut messages = vec![json!({ "role": "user", "content": [
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": big }, "title": "a.png" },
            { "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": "x" } },
            { "type": "text", "text": "what is this?" },
        ] })];
        for i in 0..4 {
            messages.push(json!({ "role": if i % 2 == 0 { "assistant" } else { "user" }, "content": format!("m{i}") }));
        }
        let out = trim_messages(messages, 10, 60_000);
        assert_eq!(
            out.len(),
            5,
            "nothing had to be dropped once the image was replaced"
        );
        let blocks = out[0]["content"].as_array().unwrap();
        assert_eq!(
            blocks[0],
            json!({ "type": "text", "text": "[image omitted: a.png]\n" })
        );
        assert_eq!(
            blocks[1],
            json!({ "type": "text", "text": "[document omitted: application/pdf]\n" })
        );
        assert_eq!(blocks[2]["text"], "what is this?");
    }

    #[test]
    fn oldest_messages_after_the_first_are_dropped_to_fit() {
        let messages: Vec<Value> = (0..6)
            .map(|i| json!({ "role": if i % 2 == 0 { "user" } else { "assistant" }, "content": format!("m{i} {}", "y".repeat(100)) }))
            .collect();
        let out = trim_messages(messages, 10, 400);
        assert_eq!(out[0]["content"].as_str().unwrap()[..2], *"m0");
        assert!(out[1]["content"].as_str().unwrap().contains("omitted"));
        assert_eq!(out.last().unwrap()["content"].as_str().unwrap()[..2], *"m5");
        assert!(out.len() < 7);
    }

    #[test]
    fn the_instruction_closes_the_conversation_and_answers_dangling_calls() {
        let history = vec![
            stored("user", json!("do it")),
            stored(
                "assistant",
                json!([{ "type": "tool_use", "id": "t1", "name": "bash", "input": {} }]),
            ),
        ];
        let messages = messages_for_run(history);
        assert_eq!(messages.len(), 3);
        let closing = messages[2]["content"].as_array().unwrap();
        assert_eq!(closing[0]["tool_use_id"], "t1");
        assert_eq!(closing[1]["text"], INSTRUCTION);

        let plain = messages_for_transcript("  <user>\nhi\n</user>  ");
        let text = plain[0]["content"].as_str().unwrap();
        assert!(text.starts_with("<conversation>\n<user>\nhi\n</user>\n</conversation>"));
        assert!(text.ends_with(INSTRUCTION));
    }
}
