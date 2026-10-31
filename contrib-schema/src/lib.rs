//! The bundle a conversation is contributed as (see `docs/design/contributions.md`), shared by
//! the two sides that must agree on it: cedar-chat builds one from a thread's traces, redacts it,
//! shows it and sends it; cedar-contrib (the ingest) checks it the same way before storing it.
//!
//! Three parts:
//!
//! - [`Bundle`] and its pieces: schema version 1, serialized as JSON.
//! - [`validate`]: the deterministic consistency checks of the ingest's request path (shape,
//!   limits, tool-call pairing, call counts, token bands, event order, placeholder numbering).
//!   Both sides run them, so a bundle the chat shows is one the ingest will take.
//! - [`scan`]: the pattern scan for secrets and personal data, and the [`scan::Redactor`] that
//!   replaces what it finds with typed placeholders (`[EMAIL_1]`), the same for the same value
//!   throughout a bundle.
//!
//! Nothing here does I/O.

pub mod scan;
pub mod validate;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The schema this crate reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// Every bundle and every release carries this string, so the corpus can be recognised and
/// filtered out of what should not train on it (cedar-bench's own data, for one).
pub const CANARY: &str = "CEDAR-CONTRIB CANARY 7c1f5a2e-4b8d-4a3e-9f61-2d0c8e5b7a94";

/// The licenses a contribution can be offered under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum License {
    #[serde(rename = "cc0")]
    Cc0,
    #[serde(rename = "cc-by-4.0")]
    CcBy4,
}

impl License {
    pub const ALL: [License; 2] = [License::Cc0, License::CcBy4];

    pub fn id(self) -> &'static str {
        match self {
            License::Cc0 => "cc0",
            License::CcBy4 => "cc-by-4.0",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }
}

/// What the person said the agent achieved, asked when they contribute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub result: OutcomeResult,
    /// What was wrong, in their words (redacted like everything else).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeResult {
    Succeeded,
    Partly,
    Failed,
}

/// The model the thread ran on, as cedar names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// cedar's registry id (`deepseek`).
    pub id: String,
    /// The model's name on its server (`deepseek-ai/DeepSeek-V4-Flash`).
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

/// What a contribution leaves out, counted so a reader knows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dropped {
    /// Files attached to messages (their bytes and names never go).
    #[serde(default)]
    pub attachments: u32,
    /// Whether a memory note was cut from the system prompt.
    #[serde(default)]
    pub memory: bool,
    /// Bodies of fetched pages, downloads and browser reads replaced by stubs.
    #[serde(default)]
    pub fetched_bodies: u32,
    /// Reasoning the model returned, left out of the calls.
    #[serde(default)]
    pub reasoning: u32,
}

/// One recorded call to the model, as the trace keeps it, minus what the bundle leaves out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub seq: u32,
    /// The agent turn; 0 for the planning call.
    pub turn: u32,
    /// `agent` or `plan`.
    pub purpose: String,
    /// The call was sent the first `message_count` messages of the turn's conversation.
    pub message_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// The model's reasoning, when the person chose to include it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

/// A message of the conversation as the model saw it: a role and content blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<Value>,
}

/// One exchange: a prompt and the cedar run that answered it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    /// 1-based position in the thread.
    pub seq: u32,
    pub prompt: String,
    /// The system prompt the run's calls were sent, memory note cut out.
    pub system: String,
    pub tool_definitions: Vec<Value>,
    pub messages: Vec<Message>,
    pub calls: Vec<Call>,
    /// The run's events worth training on: the plan and its edits, approvals, denials, how the
    /// run ended. Each `{type, data}`.
    #[serde(default)]
    pub events: Vec<Value>,
    /// The person's thumbs on this response: 1, -1, or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<i8>,
    /// `completed`, `failed` or `cancelled`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// The day the run started, `YYYY-MM-DD` (no time, no zone).
    pub day: String,
}

/// A contribution: one conversation, as it leaves the person's server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    pub schema_version: u32,
    pub canary: String,
    pub license: License,
    /// The person's pseudonym (random, chosen on their server), or none for an unlinked bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributor: Option<String>,
    pub cedar_version: String,
    /// The language of the conversation, a two- or three-letter tag (`de`, not `de-CH`).
    pub language: String,
    pub model: ModelInfo,
    /// The names of the tools the agent called, sorted, each once.
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    pub turns: Vec<Turn>,
    /// Placeholders applied, by kind (`EMAIL` → 2 means `[EMAIL_1]` and `[EMAIL_2]` occur).
    #[serde(default)]
    pub redactions: BTreeMap<String, u32>,
    #[serde(default)]
    pub dropped: Dropped,
}

impl Bundle {
    /// The hash the ingest deduplicates by: the content, without who sent it, what they said
    /// about it, or the canary.
    pub fn content_sha256(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut copy = self.clone();
        copy.contributor = None;
        copy.outcome = None;
        copy.canary = String::new();
        for t in &mut copy.turns {
            t.rating = None;
        }
        let bytes = serde_json::to_vec(&copy).expect("a bundle serializes");
        hex::encode(Sha256::digest(bytes))
    }

    /// A short fingerprint of what the conversation is about (the prompts), for grouping bundles
    /// that look alike: a 64-bit SimHash over word 3-grams, as 16 hex characters. Bundles about
    /// the same thing share a prefix.
    pub fn fingerprint(&self) -> String {
        let text: String = self.turns.iter().map(|t| t.prompt.as_str()).collect::<Vec<_>>().join("\n");
        format!("{:016x}", simhash(&text))
    }
}

/// A 64-bit SimHash over the word 3-grams of `text` (lowercased, punctuation stripped).
pub fn simhash(text: &str) -> u64 {
    use sha2::{Digest, Sha256};
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    let mut acc = [0i64; 64];
    let grams: Vec<String> = if words.len() < 3 {
        vec![words.join(" ")]
    } else {
        words.windows(3).map(|w| w.join(" ")).collect()
    };
    for g in grams {
        let h = Sha256::digest(g.as_bytes());
        let v = u64::from_le_bytes(h[..8].try_into().unwrap());
        for (i, slot) in acc.iter_mut().enumerate() {
            *slot += if (v >> i) & 1 == 1 { 1 } else { -1 };
        }
    }
    acc.iter().enumerate().fold(0u64, |out, (i, &s)| if s > 0 { out | (1 << i) } else { out })
}

/// Every string in a JSON value, in place.
pub fn walk_strings(value: &mut Value, f: &mut dyn FnMut(&mut String)) {
    match value {
        Value::String(s) => f(s),
        Value::Array(items) => items.iter_mut().for_each(|v| walk_strings(v, f)),
        Value::Object(map) => map.values_mut().for_each(|v| walk_strings(v, f)),
        _ => {}
    }
}

/// Every string of a bundle that came from the conversation, in place: prompts, system prompts,
/// message content, reasoning, errors, event data and the outcome's notes. Not the tool
/// definitions (cedar's), the model's name, or the metadata.
pub fn walk_bundle_text(bundle: &mut Bundle, f: &mut dyn FnMut(&mut String)) {
    for t in &mut bundle.turns {
        f(&mut t.prompt);
        f(&mut t.system);
        for m in &mut t.messages {
            for b in &mut m.content {
                walk_strings(b, f);
            }
        }
        for c in &mut t.calls {
            if let Some(r) = &mut c.reasoning {
                f(r);
            }
            if let Some(e) = &mut c.error {
                f(e);
            }
        }
        for e in &mut t.events {
            walk_strings(e, f);
        }
    }
    if let Some(o) = &mut bundle.outcome {
        if let Some(n) = &mut o.notes {
            f(n);
        }
    }
}

/// A small, consistent example bundle, for tests on both sides.
#[doc(hidden)]
pub mod example {
    use super::*;
    use serde_json::json;

    /// One turn: a prompt, a `bash` call and its result, an answer; two model calls.
    pub fn bundle() -> Bundle {
        let tools = vec![json!({ "name": "bash", "description": "run a command", "input_schema": { "type": "object" } })];
        Bundle {
            schema_version: SCHEMA_VERSION,
            canary: CANARY.into(),
            license: License::Cc0,
            contributor: Some("p_0123456789abcdef".into()),
            cedar_version: "0.3.5".into(),
            language: "en".into(),
            model: ModelInfo { id: "deepseek".into(), name: "deepseek-ai/DeepSeek-V4-Flash".into(), reasoning: Some("high".into()) },
            tools: vec!["bash".into()],
            outcome: Some(Outcome { result: OutcomeResult::Succeeded, notes: None }),
            turns: vec![Turn {
                seq: 1,
                prompt: "list the files".into(),
                system: "You are helpful.".into(),
                tool_definitions: tools,
                messages: vec![
                    Message { role: "user".into(), content: vec![json!({ "type": "text", "text": "list the files" })] },
                    Message {
                        role: "assistant".into(),
                        content: vec![json!({ "type": "tool_use", "id": "t1", "name": "bash", "input": { "command": "ls" } })],
                    },
                    Message {
                        role: "user".into(),
                        content: vec![json!({ "type": "tool_result", "tool_use_id": "t1", "content": "a.txt\n" })],
                    },
                    Message { role: "assistant".into(), content: vec![json!({ "type": "text", "text": "One file: a.txt" })] },
                ],
                calls: vec![
                    Call { seq: 1, turn: 1, purpose: "agent".into(), message_count: 1, stop_reason: Some("tool_use".into()), usage: Some(Usage { input_tokens: 40, output_tokens: 20 }), reasoning: None, error: None },
                    Call { seq: 2, turn: 2, purpose: "agent".into(), message_count: 3, stop_reason: Some("end_turn".into()), usage: Some(Usage { input_tokens: 80, output_tokens: 10 }), reasoning: None, error: None },
                ],
                events: vec![json!({ "type": "run.completed", "data": { "stop_reason": "end_turn" } })],
                rating: Some(1),
                status: "completed".into(),
                stop_reason: Some("end_turn".into()),
                day: "2026-10-31".into(),
            }],
            redactions: BTreeMap::new(),
            dropped: Dropped::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_content_hash_ignores_who_sent_it_and_what_they_said() {
        let a = example::bundle();
        let mut b = a.clone();
        b.contributor = None;
        b.outcome = Some(Outcome { result: OutcomeResult::Failed, notes: Some("no".into()) });
        b.turns[0].rating = Some(-1);
        assert_eq!(a.content_sha256(), b.content_sha256());
        b.turns[0].prompt.push('!');
        assert_ne!(a.content_sha256(), b.content_sha256());
    }

    #[test]
    fn fingerprints_of_similar_prompts_share_most_bits() {
        let a = simhash("please write a python script that prints the first ten primes and run it");
        let b = simhash("please write a python script that prints the first twelve primes and run it");
        let c = simhash("what is the weather in berlin tomorrow morning, and should I bring an umbrella");
        let distance = |x: u64, y: u64| (x ^ y).count_ones();
        assert!(distance(a, b) < distance(a, c), "{} vs {}", distance(a, b), distance(a, c));
    }

    #[test]
    fn a_bundle_round_trips_through_json() {
        let b = example::bundle();
        let text = serde_json::to_string(&b).unwrap();
        let back: Bundle = serde_json::from_str(&text).unwrap();
        assert_eq!(b, back);
        assert!(text.contains("\"license\":\"cc0\""));
        assert!(text.contains("\"result\":\"succeeded\""));
    }
}
