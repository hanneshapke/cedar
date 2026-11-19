//! A transcript becomes a bundle the way cedar-chat makes one from a thread
//! (`chat/src/contrib.rs`), so the ingest, the screener and the trainer read both alike:
//!
//! - Each exchange (a prompt and everything the model did about it) is a turn, and a turn's
//!   messages are the whole conversation up to the end of that exchange, as a cedar run that
//!   continues a conversation starts from a copy of its parent's.
//! - Uploaded files and images become stubs with their type and size; pages fetched from the web
//!   become stubs unless the person keeps them; files the model searched always do; reasoning is
//!   left out unless the person includes it.
//! - The pattern scan replaces keys, emails, phone and account numbers, IP addresses and home
//!   directories with stand-ins of the same shape, and the person's own phrases with what they
//!   chose; then the consistency checks the ingest runs, and a last look for secrets.

use crate::transcript::{tool_kind, Item, Source, ToolKind, Transcript};
use cedar_contrib_schema::scan::{self, Redactor};
use cedar_contrib_schema::validate::{self, Limits};
use cedar_contrib_schema::{Bundle, Call, Dropped, License, Message, ModelInfo, Outcome, Turn, CANARY, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The most exchanges a bundle holds (the schema's limit on turns).
pub const MAX_EXCHANGES: usize = 50;

/// A phrase the person wants replaced, and by what (a neutral `redacted-…` when nothing).
#[derive(Debug, Clone, Deserialize)]
pub struct Phrase {
    pub phrase: String,
    #[serde(default)]
    pub replacement: Option<String>,
}

/// What the person chose in the review.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Options {
    /// Include the model's reasoning.
    #[serde(default)]
    pub reasoning: bool,
    /// Keep the bodies of pages fetched from the web.
    #[serde(default)]
    pub fetched: bool,
    /// Contribute only the first this many exchanges.
    #[serde(default)]
    pub upto: Option<usize>,
    /// The model's name after the vendor prefix, when the export doesn't say (Claude's).
    #[serde(default)]
    pub model: Option<String>,
    /// The conversation's language; guessed when absent.
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    /// The person's pseudonym, or none: unlinked.
    #[serde(default)]
    pub contributor: Option<String>,
    #[serde(default)]
    pub outcome: Option<Outcome>,
    /// Thumbs per exchange, by its 1-based position.
    #[serde(default)]
    pub ratings: BTreeMap<String, i8>,
    #[serde(default)]
    pub phrases: Vec<Phrase>,
    /// The ingest's model list (`GET /v1/policy`); absent when the policy couldn't be read.
    #[serde(default)]
    pub models_allowed: Option<Vec<String>>,
    #[serde(default)]
    pub max_bundle_bytes: Option<usize>,
    /// Today, `YYYY-MM-DD`, for an export that carries no dates.
    #[serde(default)]
    pub today: Option<String>,
}

/// The bundle as the review shows it, and why it can't be sent, if it can't.
#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub bundle: Bundle,
    /// The exact document Send posts.
    pub json: String,
    pub size_bytes: usize,
    /// Exchanges with an answer in the conversation, and how many the bundle holds.
    pub exchanges_total: usize,
    pub exchanges_included: usize,
    pub language_guess: String,
    /// Whether the export named the model (Claude's often doesn't: the person picks it).
    pub model_known: bool,
    /// `empty`, `private_tools`, `secrets`, `not_consistent`, `too_large`, `model_not_allowed`.
    pub blocked: Vec<String>,
    pub problems: Vec<String>,
    pub secrets: Vec<String>,
    pub private_tools: Vec<String>,
}

struct Exchange<'a> {
    prompt: &'a Item,
    rest: Vec<&'a Item>,
}

/// The messages being built, and the calls of the exchange at hand.
struct Builder {
    messages: Vec<Message>,
    /// Calls of the current exchange: (index of the assistant message, its reasoning).
    calls: Vec<(usize, Vec<String>)>,
    /// tool_use ids of the last assistant message still waiting for a result.
    open: Vec<String>,
    /// Reasoning that came before the assistant message it belongs to.
    pending_reasoning: Vec<String>,
}

impl Builder {
    fn last_role(&self) -> Option<&str> {
        self.messages.last().map(|m| m.role.as_str())
    }

    /// Calls left without a result get one that says so: the schema wants every call answered.
    fn close_open(&mut self) {
        if self.open.is_empty() {
            return;
        }
        let missing: Vec<Value> = self.open.drain(..).map(|id| json!({ "type": "tool_result", "tool_use_id": id, "content": "[the export has no result for this call]" })).collect();
        if self.last_role() == Some("assistant") {
            self.messages.push(Message { role: "user".into(), content: missing });
        } else if let Some(m) = self.messages.last_mut() {
            m.content.extend(missing);
        }
    }

    fn assistant(&mut self) -> &mut Message {
        if self.last_role() != Some("assistant") {
            self.close_open();
            self.messages.push(Message { role: "assistant".into(), content: Vec::new() });
            let reasoning = std::mem::take(&mut self.pending_reasoning);
            self.calls.push((self.messages.len() - 1, reasoning));
        }
        self.messages.last_mut().expect("just pushed")
    }
}

fn file_block(f: &crate::transcript::FileStub) -> Value {
    json!({ "type": f.kind, "source": { "type": "base64", "media_type": f.media_type, "bytes": f.bytes } })
}

/// Stopwords for the languages the corpus's screener tells apart (`contrib/src/screen.rs`).
const STOPWORDS: [(&str, &[&str]); 4] = [
    ("en", &["the", "and", "is", "of", "to", "in", "that", "it", "for", "with", "you", "this", "what", "how", "can"]),
    ("de", &["der", "die", "das", "und", "ist", "nicht", "ich", "mit", "sie", "ein", "eine", "zu", "auf", "wie", "kannst"]),
    ("fr", &["le", "la", "les", "et", "est", "pas", "je", "avec", "une", "des", "pour", "que", "dans", "comment", "vous"]),
    ("es", &["el", "la", "los", "las", "y", "es", "no", "con", "una", "para", "que", "por", "como", "del", "puedes"]),
];

pub fn guess_language(text: &str) -> String {
    let words: Vec<String> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    STOPWORDS
        .iter()
        .map(|(lang, list)| (*lang, words.iter().filter(|w| list.contains(&w.as_str())).count()))
        .max_by_key(|(_, n)| *n)
        .filter(|(_, n)| *n > 0)
        .map_or("en", |(l, _)| l)
        .to_string()
}

/// The ingest's rule (`CONTRIB_MODELS`): exact names, or prefixes ending in `*`; none means any.
pub fn model_allowed(allowed: &[String], name: &str) -> bool {
    allowed.is_empty() || allowed.iter().any(|m| m.strip_suffix('*').map_or(m == name, |p| name.starts_with(p)))
}

/// What may follow the vendor in a model name.
fn clean_model(s: &str) -> Option<String> {
    let s: String = s.trim().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')).take(80).collect();
    (!s.is_empty()).then_some(s)
}

pub fn build(t: &Transcript, opts: &Options, seed: u64) -> Preview {
    // Exchanges: a prompt and what followed it, those with an answer only (a prompt left
    // unanswered was usually retried, and teaches nothing).
    let mut exchanges: Vec<Exchange> = Vec::new();
    for item in &t.items {
        match item {
            Item::Prompt { .. } => exchanges.push(Exchange { prompt: item, rest: Vec::new() }),
            other => {
                if let Some(e) = exchanges.last_mut() {
                    e.rest.push(other);
                }
            }
        }
    }
    exchanges.retain(|e| e.rest.iter().any(|i| matches!(i, Item::Text { .. } | Item::ToolCall { .. })));
    let total = exchanges.len();
    let upto = opts.upto.unwrap_or(MAX_EXCHANGES).clamp(1, MAX_EXCHANGES);
    exchanges.truncate(upto);

    let mut dropped = Dropped { memory: t.dropped.memory, fetched_bodies: t.dropped.fetched_bodies, ..Default::default() };
    let mut tools: BTreeSet<String> = BTreeSet::new();
    let mut private: BTreeSet<String> = BTreeSet::new();
    for e in &exchanges {
        for i in &e.rest {
            if let Item::ToolCall { name, .. } = i {
                tools.insert(name.clone());
                if tool_kind(name) == ToolKind::Private {
                    private.insert(name.clone());
                }
            }
        }
    }
    let tool_definitions: Vec<Value> = tools.iter().map(|n| json!({ "name": n, "description": t.source.tool_description(), "input_schema": { "type": "object" } })).collect();

    let fallback_day = t.day.clone().or_else(|| opts.today.clone()).unwrap_or_else(|| "2000-01-01".into());
    let mut b = Builder { messages: Vec::new(), calls: Vec::new(), open: Vec::new(), pending_reasoning: Vec::new() };
    let mut turns = Vec::new();
    let mut prompts_text = String::new();
    for (k, e) in exchanges.iter().enumerate() {
        let Item::Prompt { text, day, files } = e.prompt else { continue };
        b.close_open();
        b.calls.clear();
        b.pending_reasoning.clear();
        let mut content = Vec::new();
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
        for f in files {
            dropped.attachments += 1;
            content.push(file_block(f));
        }
        b.messages.push(Message { role: "user".into(), content });
        prompts_text.push_str(text);
        prompts_text.push('\n');

        for item in &e.rest {
            match item {
                Item::Text { text } => b.assistant().content.push(json!({ "type": "text", "text": text })),
                Item::ToolCall { id, name, input } => {
                    b.assistant().content.push(json!({ "type": "tool_use", "id": id, "name": name, "input": input }));
                    b.open.push(id.clone());
                }
                Item::Reasoning { text } => {
                    if b.last_role() == Some("assistant") {
                        if let Some(c) = b.calls.last_mut() {
                            c.1.push(text.clone());
                        }
                    } else {
                        b.pending_reasoning.push(text.clone());
                    }
                }
                Item::ToolResult { id, text, is_error } => {
                    let Some(at) = b.open.iter().position(|o| o == id) else { continue };
                    b.open.remove(at);
                    let body = if t.files.contains(id) {
                        dropped.attachments += 1;
                        format!("[FILE CONTENT LEFT OUT: {} bytes]", text.len())
                    } else if t.fetched.contains(id) && !opts.fetched {
                        dropped.fetched_bodies += 1;
                        format!("[FETCHED BODY LEFT OUT: {} bytes, sha256 {}]", text.len(), hex::encode(Sha256::digest(text.as_bytes())))
                    } else if text.is_empty() {
                        "[empty]".to_string()
                    } else {
                        text.clone()
                    };
                    let mut block = json!({ "type": "tool_result", "tool_use_id": id, "content": body });
                    if *is_error {
                        block["is_error"] = json!(true);
                    }
                    if b.last_role() == Some("assistant") {
                        b.messages.push(Message { role: "user".into(), content: vec![block] });
                    } else if let Some(m) = b.messages.last_mut() {
                        m.content.push(block);
                    }
                }
                Item::Prompt { .. } => {}
            }
        }
        b.close_open();

        let mut calls = Vec::new();
        for (n, (index, reasoning)) in b.calls.iter().enumerate() {
            let has_reasoning = reasoning.iter().any(|r| !r.trim().is_empty());
            if has_reasoning && !opts.reasoning {
                dropped.reasoning += 1;
            }
            let uses_tools = b.messages[*index].content.iter().any(|c| c["type"] == "tool_use");
            calls.push(Call {
                seq: n as u32 + 1,
                turn: n as u32 + 1,
                purpose: "agent".into(),
                message_count: *index as u32,
                stop_reason: Some(if uses_tools { "tool_use" } else { "end_turn" }.into()),
                usage: None,
                reasoning: (opts.reasoning && has_reasoning).then(|| reasoning.join("\n\n")),
                error: None,
            });
        }
        let seq = k as u32 + 1;
        turns.push(Turn {
            seq,
            prompt: text.clone(),
            system: String::new(),
            tool_definitions: tool_definitions.clone(),
            messages: b.messages.clone(),
            calls,
            events: vec![json!({ "type": "run.completed", "data": { "stop_reason": "end_turn" } })],
            rating: opts.ratings.get(&seq.to_string()).copied().filter(|r| *r == 1 || *r == -1),
            status: "completed".into(),
            stop_reason: Some("end_turn".into()),
            day: day.clone().unwrap_or_else(|| fallback_day.clone()),
        });
    }

    let language_guess = guess_language(&prompts_text);
    let language = opts
        .language
        .as_deref()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| (2..=3).contains(&l.len()) && l.bytes().all(|c| c.is_ascii_lowercase()))
        .unwrap_or_else(|| language_guess.clone());
    let model_known = t.model.is_some();
    let suffix = t.model.as_deref().and_then(clean_model).or_else(|| opts.model.as_deref().and_then(clean_model)).unwrap_or_else(|| "unknown".into());
    let model_name = format!("{}/{suffix}", t.source.vendor());
    let contributor = opts.contributor.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(String::from);

    let mut bundle = Bundle {
        schema_version: SCHEMA_VERSION,
        canary: CANARY.into(),
        license: opts.license.as_deref().and_then(License::parse).unwrap_or(License::Cc0),
        contributor,
        cedar_version: format!("contrib-web {}", env!("CARGO_PKG_VERSION")),
        language,
        model: ModelInfo { id: t.source.model_id().into(), name: model_name, reasoning: None },
        tools: tools.into_iter().collect(),
        outcome: opts.outcome.clone().map(|mut o| {
            o.notes = o.notes.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            o
        }),
        turns,
        redactions: BTreeMap::new(),
        dropped,
    };

    // Redaction: the person's phrases, then the pattern scan, over every string from the
    // conversation (prompts, messages, tool inputs and results, reasoning, the outcome's notes).
    let mut redactor = Redactor::with_seed(seed);
    // A user name the scan finds in a home directory (`/Users/hannah/...`) is usually elsewhere
    // too (`whoami` printing `hannah`, "notes for hannah"): replace it everywhere, by the stand-in
    // the path gets.
    let mut names: BTreeSet<String> = BTreeSet::new();
    {
        let mut copy = bundle.clone();
        cedar_contrib_schema::walk_bundle_text(&mut copy, &mut |s| {
            names.extend(scan::scan(s).into_iter().filter(|f| f.kind == scan::Kind::User && f.value.chars().count() >= 3).map(|f| f.value));
        });
    }
    let mut user_stand_ins: BTreeSet<String> = BTreeSet::new();
    for name in &names {
        let mut probe = format!("/home/{name}/");
        redactor.redact(&mut probe);
        if let Some(stand_in) = probe.strip_prefix("/home/").and_then(|p| p.strip_suffix('/')) {
            if stand_in != name {
                let stand_in = stand_in.to_string();
                redactor.redact_phrase(name, Some(&stand_in));
                user_stand_ins.insert(stand_in);
            }
        }
    }
    for p in &opts.phrases {
        if p.phrase.trim().chars().count() >= 2 {
            redactor.redact_phrase(&p.phrase, p.replacement.as_deref());
        }
    }
    cedar_contrib_schema::walk_bundle_text(&mut bundle, &mut |s| {
        redactor.redact(s);
    });
    // Declare only the stand-ins that are in the text (a phrase that occurs nowhere has none).
    let mut text = String::new();
    {
        let mut copy = bundle.clone();
        cedar_contrib_schema::walk_bundle_text(&mut copy, &mut |s| {
            text.push_str(s);
            text.push('\n');
        });
    }
    // The user names spread from paths are declared as USER, not as the person's own phrases.
    let mut declared: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (kind, values) in redactor.surrogates() {
        for v in values {
            if v.is_empty() || !text.contains(v.as_str()) {
                continue;
            }
            let kind = if kind == "CUSTOM" && user_stand_ins.contains(&v) { "USER".to_string() } else { kind.clone() };
            let list = declared.entry(kind).or_default();
            if !list.contains(&v) {
                list.push(v);
            }
        }
    }
    bundle.redactions = declared;

    let json = serde_json::to_string(&bundle).expect("a bundle serializes");
    let mut blocked: BTreeSet<&str> = BTreeSet::new();
    if bundle.turns.is_empty() {
        blocked.insert("empty");
    }
    if !private.is_empty() {
        blocked.insert("private_tools");
    }
    let secrets = scan::secrets_in(&bundle);
    if !secrets.is_empty() {
        blocked.insert("secrets");
    }
    let mut problems: Vec<String> = if bundle.turns.is_empty() {
        Vec::new()
    } else {
        validate::validate(&bundle, &Limits::default()).into_iter().map(|p| format!("{}: {}", p.code.as_str(), p.detail)).collect()
    };
    if !problems.is_empty() {
        blocked.insert("not_consistent");
    }
    let max = opts.max_bundle_bytes.unwrap_or(Limits::default().max_bundle_bytes);
    if json.len() > max {
        blocked.insert("too_large");
        problems.push(format!("too_large: {} bytes; at most {max}", json.len()));
    }
    if let Some(allowed) = &opts.models_allowed {
        if !model_allowed(allowed, &bundle.model.name) {
            blocked.insert("model_not_allowed");
        }
    }
    Preview {
        size_bytes: json.len(),
        json,
        exchanges_total: total,
        exchanges_included: bundle.turns.len(),
        language_guess,
        model_known,
        blocked: blocked.into_iter().map(String::from).collect(),
        problems,
        secrets,
        private_tools: private.into_iter().collect(),
        bundle,
    }
}

/// The source of a JSON conversation, by its shape.
pub fn detect(v: &Value) -> Option<Source> {
    if crate::chatgpt::looks_like(v) {
        Some(Source::ChatGpt)
    } else if crate::claude::looks_like(v) {
        Some(Source::Claude)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatgpt;
    use serde_json::json;

    fn text(s: &str) -> Value {
        json!({ "content_type": "text", "parts": [s] })
    }

    fn sample() -> Value {
        chatgpt::fixture(
            "Trip",
            &[
                ("user", "", text("Mail maya.real@gmail.com my key sk-proj-abcdefghijklmnopqrstuvwx please, I live in Springfield")),
                ("assistant", "python", json!({ "content_type": "code", "text": "open('/Users/hannah/notes.txt').read()" })),
                ("tool", "python", json!({ "content_type": "execution_output", "text": "notes for hannah" })),
                ("assistant", "all", text("Done, Springfield is lovely.")),
                ("user", "", text("and the weather?")),
                ("assistant", "browser", json!({ "content_type": "code", "text": "search(\"weather\")" })),
                ("tool", "browser", json!({ "content_type": "tether_browsing_display", "result": "Sunny, 21 degrees, from weather.example" })),
                ("assistant", "all", text("Sunny.")),
                ("user", "", text("never answered")),
            ],
        )
    }

    #[test]
    fn a_chatgpt_conversation_becomes_a_bundle_the_ingest_takes() {
        let t = chatgpt::import(&sample());
        let opts = Options { phrases: vec![Phrase { phrase: "Springfield".into(), replacement: Some("Riverton".into()) }], ..Default::default() };
        let p = build(&t, &opts, 7);
        assert_eq!(p.problems, Vec::<String>::new());
        assert_eq!(p.blocked, Vec::<String>::new());
        assert_eq!(p.exchanges_total, 2);
        let b = &p.bundle;
        assert_eq!(b.model.name, "openai/gpt-4o");
        assert_eq!(b.model.id, "chatgpt-export");
        assert_eq!(b.tools, vec!["browser", "python"]);
        assert_eq!(b.turns.len(), 2);
        // Turns hold the conversation so far.
        assert_eq!(b.turns[0].messages.len(), 4);
        assert_eq!(b.turns[1].messages.len(), 8);
        assert_eq!(b.turns[1].calls.len(), 2);
        assert_eq!(b.turns[1].calls[0].message_count, 5);
        // Nothing of the originals is left; the stand-ins are declared.
        for gone in ["maya.real@gmail.com", "sk-proj-abcdefghij", "hannah", "Springfield", "Sunny, 21 degrees"] {
            assert!(!p.json.contains(gone), "{gone} is still in the bundle");
        }
        assert!(p.json.contains("Riverton"));
        assert!(p.json.contains("FETCHED BODY LEFT OUT"));
        assert_eq!(b.dropped.fetched_bodies, 1);
        for kind in ["EMAIL", "KEY", "USER", "CUSTOM"] {
            assert!(b.redactions.contains_key(kind), "{kind} not declared: {:?}", b.redactions);
        }
        // Only the person's phrase is CUSTOM; the user name spread from the path is a USER.
        assert_eq!(b.redactions["CUSTOM"], vec!["Riverton"]);
        assert_eq!(b.redactions["USER"].len(), 1);
        assert_eq!(b.language, "en");
        // The bundle parses back and passes the ingest's checks as sent.
        let back: Bundle = serde_json::from_str(&p.json).unwrap();
        assert!(validate::validate(&back, &Limits::default()).is_empty());
        assert!(scan::secrets_in(&back).is_empty());
    }

    #[test]
    fn the_same_seed_gives_the_same_stand_ins_and_another_seed_others() {
        let t = chatgpt::import(&sample());
        let a = build(&t, &Options::default(), 1);
        let b = build(&t, &Options::default(), 1);
        let c = build(&t, &Options::default(), 2);
        assert_eq!(a.json, b.json);
        assert_ne!(a.bundle.redactions, c.bundle.redactions);
    }

    #[test]
    fn options_shape_the_bundle() {
        let t = chatgpt::import(&sample());
        let mut ratings = BTreeMap::new();
        ratings.insert("1".to_string(), -1i8);
        let opts = Options {
            fetched: true,
            upto: Some(1),
            license: Some("cc-by-4.0".into()),
            contributor: Some("p_0123456789abcdef".into()),
            outcome: Some(Outcome { result: cedar_contrib_schema::OutcomeResult::Partly, notes: Some("write to theo@corp.io".into()) }),
            ratings,
            ..Default::default()
        };
        let p = build(&t, &opts, 3);
        assert!(p.blocked.is_empty(), "{:?} {:?}", p.blocked, p.problems);
        assert_eq!(p.bundle.turns.len(), 1);
        assert_eq!(p.bundle.turns[0].rating, Some(-1));
        assert_eq!(p.bundle.license, License::CcBy4);
        assert!(!p.json.contains("theo@corp.io"));
        let p = build(&t, &Options { fetched: true, ..Default::default() }, 3);
        assert!(p.json.contains("Sunny, 21 degrees"));
    }

    #[test]
    fn private_tools_block_and_the_model_list_is_honoured() {
        let conv = chatgpt::fixture(
            "Mail",
            &[
                ("user", "", text("summarise my inbox")),
                ("assistant", "gmail.search", json!({ "content_type": "code", "text": "{}" })),
                ("tool", "gmail.search", text("from bob: hi")),
                ("assistant", "all", text("Bob says hi.")),
            ],
        );
        let p = build(&chatgpt::import(&conv), &Options { models_allowed: Some(vec!["deepseek-ai/*".into()]), ..Default::default() }, 1);
        assert!(p.blocked.contains(&"private_tools".to_string()));
        assert!(p.blocked.contains(&"model_not_allowed".to_string()));
        assert!(model_allowed(&["openai/*".into()], "openai/gpt-4o"));
        assert!(model_allowed(&[], "anything"));
    }

    #[test]
    fn claude_without_a_model_name_takes_the_persons_choice_under_the_vendor() {
        let conv = json!({ "chat_messages": [
            { "sender": "human", "created_at": "2025-01-02T00:00:00Z", "text": "Wie spät ist es in Berlin und was ist das Wetter?" },
            { "sender": "assistant", "created_at": "2025-01-02T00:00:01Z", "text": "Es ist zwölf Uhr." }
        ]});
        let t = crate::claude::import(&conv);
        let p = build(&t, &Options { model: Some("claude-sonnet-4-5 / deepseek".into()), ..Default::default() }, 1);
        assert!(p.blocked.is_empty(), "{:?}", p.problems);
        assert_eq!(p.bundle.model.name, "anthropic/claude-sonnet-4-5deepseek");
        assert!(!p.model_known);
        assert_eq!(p.bundle.language, "de");
        assert_eq!(p.bundle.turns[0].day, "2025-01-02");
    }
}
