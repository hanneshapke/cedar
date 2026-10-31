//! The consistency checks a bundle must pass before it is stored: the ingest's request path
//! (`docs/design/contrib-ingest-cloud-run.md`, §4), run by the chat too so what it shows is
//! what the ingest takes. They refuse the obvious, deterministically and in milliseconds; what
//! a careful forger can still pass is the screener's and the release job's business.

use crate::{Bundle, License, CANARY, SCHEMA_VERSION};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

/// The bundle limits both sides agree on.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_turns: usize,
    pub max_messages_per_turn: usize,
    /// Any single string.
    pub max_string_bytes: usize,
    pub max_events_per_turn: usize,
    pub max_calls_per_turn: usize,
    /// Of the whole bundle, serialized.
    pub max_bundle_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_turns: 50,
            max_messages_per_turn: 400,
            max_string_bytes: 256 * 1024,
            max_events_per_turn: 2000,
            max_calls_per_turn: 200,
            max_bundle_bytes: 8 << 20,
        }
    }
}

/// Why a bundle was refused. `code` is the coarse reason a client can act on (the ingest's
/// `400` carries it); `detail` says where, for the person fixing their bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub code: Code,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Wrong shape, version, canary, license, language or limit.
    Schema,
    TooLarge,
    /// The bundle contradicts itself: a run cedar could not have produced.
    NotConsistent,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Schema => "schema",
            Code::TooLarge => "too_large",
            Code::NotConsistent => "not_consistent",
        }
    }
}

fn problem(code: Code, detail: impl Into<String>) -> Problem {
    Problem { code, detail: detail.into() }
}

/// The placeholders a redaction leaves: `[KIND_n]`.
pub fn placeholder_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[([A-Z]{2,12})_(\d{1,6})\]").unwrap())
}

/// Check `bundle` against `limits`; every problem found, or none.
pub fn validate(bundle: &Bundle, limits: &Limits) -> Vec<Problem> {
    let mut out = Vec::new();
    if bundle.schema_version != SCHEMA_VERSION {
        out.push(problem(Code::Schema, format!("schema_version {} is not {SCHEMA_VERSION}", bundle.schema_version)));
    }
    if bundle.canary != CANARY {
        out.push(problem(Code::Schema, "canary is missing or not the current one"));
    }
    if !License::ALL.contains(&bundle.license) {
        out.push(problem(Code::Schema, "license is not offered"));
    }
    let lang_ok = (2..=3).contains(&bundle.language.len()) && bundle.language.bytes().all(|b| b.is_ascii_lowercase());
    if !lang_ok {
        out.push(problem(Code::Schema, format!("language {:?} is not a two- or three-letter tag", bundle.language)));
    }
    if let Some(c) = &bundle.contributor {
        let ok = (8..=64).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !ok {
            out.push(problem(Code::Schema, "contributor must be 8 to 64 letters, digits, _ or -"));
        }
    }
    if bundle.model.id.is_empty() || bundle.model.name.is_empty() {
        out.push(problem(Code::Schema, "model id and name are required"));
    }
    if bundle.turns.is_empty() {
        out.push(problem(Code::Schema, "a bundle needs at least one turn"));
    }
    if bundle.turns.len() > limits.max_turns {
        out.push(problem(Code::TooLarge, format!("{} turns; at most {}", bundle.turns.len(), limits.max_turns)));
    }
    let mut sorted = bundle.tools.clone();
    sorted.sort();
    sorted.dedup();
    if sorted != bundle.tools {
        out.push(problem(Code::Schema, "tools must be sorted and unique"));
    }
    if let Some(o) = &bundle.outcome {
        if o.notes.as_ref().is_some_and(|n| n.len() > 4000) {
            out.push(problem(Code::TooLarge, "outcome notes are longer than 4000 bytes"));
        }
    }

    // Strings anywhere over the limit.
    let mut longest = 0usize;
    {
        let mut copy = bundle.clone();
        crate::walk_bundle_text(&mut copy, &mut |s| longest = longest.max(s.len()));
        for t in &mut copy.turns {
            for d in &mut t.tool_definitions {
                crate::walk_strings(d, &mut |s| longest = longest.max(s.len()));
            }
        }
    }
    if longest > limits.max_string_bytes {
        out.push(problem(Code::TooLarge, format!("a string of {longest} bytes; at most {}", limits.max_string_bytes)));
    }

    for (i, turn) in bundle.turns.iter().enumerate() {
        let at = format!("turn {}", i + 1);
        if turn.seq as usize != i + 1 {
            out.push(problem(Code::NotConsistent, format!("{at}: seq is {}", turn.seq)));
        }
        if !matches!(turn.status.as_str(), "completed" | "failed" | "cancelled") {
            out.push(problem(Code::NotConsistent, format!("{at}: status {:?} is not terminal", turn.status)));
        }
        if !day_ok(&turn.day) {
            out.push(problem(Code::Schema, format!("{at}: day {:?} is not YYYY-MM-DD", turn.day)));
        }
        if turn.messages.len() > limits.max_messages_per_turn {
            out.push(problem(Code::TooLarge, format!("{at}: {} messages; at most {}", turn.messages.len(), limits.max_messages_per_turn)));
        }
        if turn.events.len() > limits.max_events_per_turn {
            out.push(problem(Code::TooLarge, format!("{at}: {} events; at most {}", turn.events.len(), limits.max_events_per_turn)));
        }
        if turn.calls.len() > limits.max_calls_per_turn {
            out.push(problem(Code::TooLarge, format!("{at}: {} calls; at most {}", turn.calls.len(), limits.max_calls_per_turn)));
        }
        if turn.status == "cancelled" && i + 1 != bundle.turns.len() {
            out.push(problem(Code::NotConsistent, format!("{at}: a cancelled turn must be the last")));
        }
        if let Some(r) = turn.rating {
            if r != 1 && r != -1 {
                out.push(problem(Code::Schema, format!("{at}: rating must be 1 or -1")));
            }
        }
        check_messages(turn, &at, &mut out);
        check_calls(turn, &at, &mut out);
        check_events(turn, &at, &mut out);
    }
    check_placeholders(bundle, &mut out);
    out
}

fn day_ok(day: &str) -> bool {
    let b = day.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && (1..=12).contains(&day[5..7].parse::<u8>().unwrap_or(0))
        && (1..=31).contains(&day[8..10].parse::<u8>().unwrap_or(0))
}

/// Tool calls and results pair up, in order, and every tool called is defined.
fn check_messages(turn: &crate::Turn, at: &str, out: &mut Vec<Problem>) {
    let defined: HashSet<&str> = turn.tool_definitions.iter().filter_map(|d| d["name"].as_str()).collect();
    let mut seen_uses: HashSet<String> = HashSet::new();
    let mut open: Vec<String> = Vec::new(); // tool_use ids awaiting a result in the next message
    for (mi, m) in turn.messages.iter().enumerate() {
        let where_ = format!("{at}, message {}", mi + 1);
        if !matches!(m.role.as_str(), "user" | "assistant") {
            out.push(problem(Code::NotConsistent, format!("{where_}: role {:?}", m.role)));
        }
        if m.content.is_empty() {
            out.push(problem(Code::NotConsistent, format!("{where_}: no content")));
        }
        let mut results_here: HashSet<String> = HashSet::new();
        let mut uses_here: Vec<String> = Vec::new();
        for b in &m.content {
            match b["type"].as_str() {
                Some("tool_use") => {
                    if m.role != "assistant" {
                        out.push(problem(Code::NotConsistent, format!("{where_}: a tool_use in a {} message", m.role)));
                    }
                    let id = b["id"].as_str().unwrap_or_default().to_string();
                    let name = b["name"].as_str().unwrap_or_default();
                    if id.is_empty() || !seen_uses.insert(id.clone()) {
                        out.push(problem(Code::NotConsistent, format!("{where_}: tool_use id {id:?} is missing or repeated")));
                    }
                    if !defined.contains(name) {
                        out.push(problem(Code::NotConsistent, format!("{where_}: tool {name:?} is not among the turn's tool definitions")));
                    }
                    uses_here.push(id);
                }
                Some("tool_result") => {
                    if m.role != "user" {
                        out.push(problem(Code::NotConsistent, format!("{where_}: a tool_result in a {} message", m.role)));
                    }
                    let id = b["tool_use_id"].as_str().unwrap_or_default().to_string();
                    if !open.contains(&id) {
                        out.push(problem(Code::NotConsistent, format!("{where_}: tool_result {id:?} answers no tool_use of the message before")));
                    }
                    results_here.insert(id);
                }
                Some("text") | Some("image") | Some("document") => {}
                other => out.push(problem(Code::NotConsistent, format!("{where_}: content block type {other:?}"))),
            }
        }
        if !open.is_empty() {
            for id in &open {
                if !results_here.contains(id) {
                    out.push(problem(Code::NotConsistent, format!("{where_}: no tool_result for tool_use {id:?}")));
                }
            }
        }
        open = uses_here;
    }
    // A run that ended mid tool call (cancelled, failed) legitimately leaves the last uses open.
    if !open.is_empty() && turn.status == "completed" {
        out.push(problem(Code::NotConsistent, format!("{at}: the conversation ends with tool calls that have no result")));
    }
}

/// Call counts grow with the conversation and token counts fit the bytes sent.
fn check_calls(turn: &crate::Turn, at: &str, out: &mut Vec<Problem>) {
    let mut last_count = 0u32;
    let mut last_seq = 0u32;
    let has_files = turn.messages.iter().flat_map(|m| &m.content).any(|b| matches!(b["type"].as_str(), Some("image") | Some("document")));
    for c in &turn.calls {
        let where_ = format!("{at}, call {}", c.seq);
        if c.seq <= last_seq {
            out.push(problem(Code::NotConsistent, format!("{where_}: calls are not in order")));
        }
        last_seq = c.seq;
        if !matches!(c.purpose.as_str(), "agent" | "plan") {
            out.push(problem(Code::NotConsistent, format!("{where_}: purpose {:?}", c.purpose)));
        }
        if c.purpose == "agent" {
            if c.message_count < last_count || c.message_count as usize > turn.messages.len() {
                out.push(problem(Code::NotConsistent, format!("{where_}: message_count {} after {last_count} of {} messages", c.message_count, turn.messages.len())));
            }
            last_count = c.message_count;
        }
        if let Some(u) = &c.usage {
            if c.purpose == "agent" && !has_files {
                let sent: usize = turn.system.len()
                    + turn.tool_definitions.iter().map(|d| d.to_string().len()).sum::<usize>()
                    + turn.messages.iter().take(c.message_count as usize).map(|m| serde_json::to_string(&m.content).map_or(0, |s| s.len())).sum::<usize>();
                // Tokens and bytes are within a factor of a few of each other for text; a wide
                // band, so odd tokenizers pass and made-up counts don't.
                let high = (sent as u64) * 2 + 8_192;
                let low = (sent as u64) / 64;
                if u.input_tokens > high || u.input_tokens < low {
                    out.push(problem(Code::NotConsistent, format!("{where_}: {} input tokens for about {sent} bytes sent", u.input_tokens)));
                }
            }
        }
    }
}

/// Events form a sequence a run could have produced.
fn check_events(turn: &crate::Turn, at: &str, out: &mut Vec<Problem>) {
    let mut terminal = 0;
    let mut planned = false;
    let mut approvals: HashSet<String> = HashSet::new();
    for (i, e) in turn.events.iter().enumerate() {
        let where_ = format!("{at}, event {}", i + 1);
        let ty = e["type"].as_str().unwrap_or_default();
        if !e["data"].is_object() {
            out.push(problem(Code::NotConsistent, format!("{where_}: no data")));
        }
        match ty {
            "run.completed" | "run.failed" | "run.cancelled" => {
                terminal += 1;
                let expected = &ty[4..];
                if expected != turn.status {
                    out.push(problem(Code::NotConsistent, format!("{where_}: {ty} in a {} turn", turn.status)));
                }
            }
            "plan" => planned = true,
            "plan.updated" if !planned => out.push(problem(Code::NotConsistent, format!("{where_}: plan.updated before plan"))),
            "approval.required" => {
                approvals.insert(e["data"]["id"].as_str().unwrap_or_default().to_string());
            }
            "approval.resolved" if !approvals.remove(e["data"]["id"].as_str().unwrap_or_default()) => {
                out.push(problem(Code::NotConsistent, format!("{where_}: approval.resolved for no approval.required")));
            }
            _ => {}
        }
        if terminal > 0 && i + 1 < turn.events.len() {
            out.push(problem(Code::NotConsistent, format!("{where_}: events after the run ended")));
            break;
        }
    }
    if terminal > 1 {
        out.push(problem(Code::NotConsistent, format!("{at}: {terminal} terminal events")));
    }
}

/// `[KIND_n]` placeholders are numbered 1..=n per kind, and `redactions` says so.
fn check_placeholders(bundle: &Bundle, out: &mut Vec<Problem>) {
    let mut seen: BTreeMap<String, HashSet<u32>> = BTreeMap::new();
    let mut copy = bundle.clone();
    crate::walk_bundle_text(&mut copy, &mut |s| {
        for cap in placeholder_regex().captures_iter(s) {
            let n: u32 = cap[2].parse().unwrap_or(0);
            seen.entry(cap[1].to_string()).or_default().insert(n);
        }
    });
    for (kind, numbers) in &seen {
        let max = numbers.iter().copied().max().unwrap_or(0);
        let dense = max as usize == numbers.len() && !numbers.contains(&0);
        if !dense {
            out.push(problem(Code::NotConsistent, format!("placeholders [{kind}_n] are not numbered 1 to {max}")));
        }
        if bundle.redactions.get(kind).copied() != Some(max) {
            out.push(problem(Code::NotConsistent, format!("redactions says {:?} for {kind}, the text has {max}", bundle.redactions.get(kind))));
        }
    }
    for (kind, count) in &bundle.redactions {
        if !seen.contains_key(kind) && *count > 0 {
            out.push(problem(Code::NotConsistent, format!("redactions lists {kind} but no [{kind}_n] occurs")));
        }
    }
}

/// A text's content blocks as strings, for scans: text blocks, tool inputs and results.
pub fn block_texts(block: &Value) -> Vec<&str> {
    let mut out = Vec::new();
    fn walk<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
        match v {
            Value::String(s) => out.push(s),
            Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            Value::Object(o) => o.values().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    walk(block, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::example::bundle;
    use serde_json::json;

    fn codes(b: &Bundle) -> Vec<Code> {
        validate(b, &Limits::default()).into_iter().map(|p| p.code).collect()
    }

    #[test]
    fn the_fixture_passes() {
        assert_eq!(validate(&bundle(), &Limits::default()), vec![]);
    }

    #[test]
    fn version_canary_and_language_are_checked() {
        let mut b = bundle();
        b.schema_version = 2;
        b.canary = "nope".into();
        b.language = "de-CH".into();
        assert_eq!(codes(&b), vec![Code::Schema, Code::Schema, Code::Schema]);
    }

    #[test]
    fn a_tool_use_without_its_result_is_refused_unless_the_run_was_cut_short() {
        let mut b = bundle();
        b.turns[0].messages.truncate(2);
        b.turns[0].calls.truncate(1);
        assert!(codes(&b).contains(&Code::NotConsistent));
        b.turns[0].status = "cancelled".into();
        b.turns[0].events = vec![json!({ "type": "run.cancelled", "data": { "reason": "user" } })];
        assert_eq!(validate(&b, &Limits::default()), vec![]);
    }

    #[test]
    fn a_result_for_an_unknown_call_and_an_undefined_tool_are_refused() {
        let mut b = bundle();
        b.turns[0].messages[2].content[0]["tool_use_id"] = json!("other");
        let problems = validate(&b, &Limits::default());
        assert!(problems.iter().any(|p| p.detail.contains("answers no tool_use")), "{problems:?}");
        let mut b = bundle();
        b.turns[0].tool_definitions.clear();
        assert!(validate(&b, &Limits::default()).iter().any(|p| p.detail.contains("not among the turn's tool definitions")));
    }

    #[test]
    fn call_counts_must_grow_and_tokens_must_fit_the_bytes() {
        let mut b = bundle();
        b.turns[0].calls[1].message_count = 0;
        assert!(validate(&b, &Limits::default()).iter().any(|p| p.detail.contains("message_count")));
        let mut b = bundle();
        b.turns[0].calls[0].usage = Some(crate::Usage { input_tokens: 900_000, output_tokens: 1 });
        assert!(validate(&b, &Limits::default()).iter().any(|p| p.detail.contains("input tokens")));
    }

    #[test]
    fn events_must_be_in_a_legal_order() {
        let mut b = bundle();
        b.turns[0].events = vec![
            json!({ "type": "plan.updated", "data": { "by": "user" } }),
            json!({ "type": "approval.resolved", "data": { "id": "a1", "decision": "deny" } }),
            json!({ "type": "run.completed", "data": {} }),
            json!({ "type": "run.failed", "data": {} }),
        ];
        let details: Vec<String> = validate(&b, &Limits::default()).into_iter().map(|p| p.detail).collect();
        assert!(details.iter().any(|d| d.contains("plan.updated before plan")), "{details:?}");
        assert!(details.iter().any(|d| d.contains("no approval.required")), "{details:?}");
        assert!(details.iter().any(|d| d.contains("after the run ended")), "{details:?}");
    }

    #[test]
    fn placeholders_are_numbered_densely_and_counted() {
        let mut b = bundle();
        b.turns[0].prompt = "mail [EMAIL_1] and [EMAIL_3]".into();
        b.redactions.insert("EMAIL".into(), 3);
        assert!(validate(&b, &Limits::default()).iter().any(|p| p.detail.contains("not numbered 1 to 3")));
        b.turns[0].prompt = "mail [EMAIL_1] and [EMAIL_2]".into();
        assert!(validate(&b, &Limits::default()).iter().any(|p| p.detail.contains("redactions says")));
        b.redactions.insert("EMAIL".into(), 2);
        assert_eq!(validate(&b, &Limits::default()), vec![]);
    }

    #[test]
    fn limits_are_applied() {
        let mut b = bundle();
        b.turns[0].prompt = "x".repeat(300 * 1024);
        assert!(codes(&b).contains(&Code::TooLarge));
        let mut b = bundle();
        let t = b.turns[0].clone();
        for i in 1..60 {
            let mut t = t.clone();
            t.seq = i + 1;
            b.turns.push(t);
        }
        assert!(codes(&b).contains(&Code::TooLarge));
    }
}
