//! Keeping a run within its model's context window.
//!
//! A model server takes a prompt and a reply together in one window of tokens. A run whose
//! conversation grows towards that size first gets replies cut short and then none at all; with
//! a small window (a local model with 32k tokens, say) a single page or a few search results can
//! be enough. [`Budget`] tracks how much of the window a request takes and how many tokens the
//! reply may still have, and [`fit`] cuts a conversation down to a size before it is sent: old
//! tool results first, then the largest new ones, then the oldest messages. The conversation as
//! stored is never changed, only what is sent.

use crate::models::Usage;
use serde_json::{json, Value};

/// A window at or below this many tokens counts as small: a run then gets only the tools its
/// request seems to need, a note on working within the window, and no whole listing of its
/// owner's memory beside the profile.
pub const SMALL_WINDOW: u32 = 65_536;

/// Bytes of a request's JSON per prompt token, until a reply's usage says better. Low on
/// purpose: estimating too many tokens only trims a little early.
const DEFAULT_BYTES_PER_TOKEN: f64 = 3.0;
/// Tokens kept free for the difference between the estimate and the real count, and for the
/// chat template's own tokens.
const SAFETY_TOKENS: u32 = 512;
/// The least a reply is given; below it a call is not worth making.
pub const MIN_REPLY_TOKENS: u32 = 1_024;
/// The least a tool result is given when tool output is shared out among a turn's calls, in
/// bytes.
const MIN_TOOL_OUTPUT_BYTES: usize = 4 * 1024;
/// What an earlier tool result is cut to once the conversation no longer fits, in characters.
const OLD_RESULT_CHARS: usize = 1_500;
/// What the newest tool results may be cut to at the most, in characters.
const NEW_RESULT_MIN_CHARS: usize = 2_000;

/// How much of a model's window a run's requests take and leave for the reply.
#[derive(Debug, Clone)]
pub struct Budget {
    /// The model's context window in tokens, prompt and reply together; `None` when nobody said
    /// and it could not be found out, in which case nothing is trimmed.
    window: Option<u32>,
    /// The registered limit on one reply.
    max_reply: u32,
    bytes_per_token: f64,
}

impl Budget {
    pub fn new(window: Option<u32>, max_reply: u32) -> Self {
        Self {
            window: window.filter(|w| *w > 0),
            max_reply: max_reply.max(1),
            bytes_per_token: DEFAULT_BYTES_PER_TOKEN,
        }
    }

    pub fn window(&self) -> Option<u32> {
        self.window
    }

    /// The registered limit on one reply.
    pub fn max_reply(&self) -> u32 {
        self.max_reply
    }

    /// Change the limit on one reply (an admin may change it while a run goes on).
    pub fn set_max_reply(&mut self, max_reply: u32) {
        self.max_reply = max_reply.max(1);
    }

    /// Whether the window is small enough to work differently (see [`SMALL_WINDOW`]).
    pub fn is_small(&self) -> bool {
        self.window.is_some_and(|w| w <= SMALL_WINDOW)
    }

    /// The tokens a request of `bytes` (its JSON: system prompt, tools and messages) is likely
    /// to take.
    pub fn estimate(&self, bytes: usize) -> u32 {
        (bytes as f64 / self.bytes_per_token).ceil().min(u32::MAX as f64) as u32
    }

    fn bytes_for(&self, tokens: u32) -> usize {
        (f64::from(tokens) * self.bytes_per_token) as usize
    }

    /// Learn from a reply how many bytes a token is for this model and this kind of content. A
    /// ratio no tokenizer gives (a server that counts nothing, or a prompt mostly of images) is
    /// ignored.
    pub fn calibrate(&mut self, bytes_sent: usize, usage: Usage) {
        if bytes_sent >= 1_000 && usage.input_tokens > 0 {
            let ratio = bytes_sent as f64 / usage.input_tokens as f64;
            if (1.0..=8.0).contains(&ratio) {
                self.bytes_per_token = ratio;
            }
        }
    }

    /// When a reply stopped at its limit before reaching the `max_tokens` it asked for, the
    /// prompt and the reply filled the window: that is how large it is. Returns whether that
    /// was news (no window known, or a larger one).
    pub fn learn_window(&mut self, requested: u32, stop_reason: Option<&str>, usage: Usage) -> bool {
        if stop_reason != Some("max_tokens") || usage.output_tokens >= u64::from(requested) {
            return false;
        }
        let filled = usage.input_tokens + usage.output_tokens;
        let Ok(filled) = u32::try_from(filled) else {
            return false;
        };
        if filled == 0 || self.window.is_some_and(|w| w <= filled) {
            return false;
        }
        self.window = Some(filled);
        true
    }

    /// What is kept free for the reply when deciding how much prompt fits: a quarter of the
    /// window, but no more than one reply may take.
    fn reply_reserve(&self, window: u32) -> u32 {
        self.max_reply
            .min((window / 4).max(MIN_REPLY_TOKENS * 2))
            .max(MIN_REPLY_TOKENS)
    }

    /// How many tokens the prompt may take, `None` without a known window. `tight` leaves
    /// more room, for a second try after a reply that got no room at all.
    pub fn input_budget(&self, tight: bool) -> Option<u32> {
        let window = self.window?;
        let mut budget = window
            .saturating_sub(self.reply_reserve(window))
            .saturating_sub(SAFETY_TOKENS);
        if tight {
            budget = budget * 3 / 5;
        }
        Some(budget.max(MIN_REPLY_TOKENS))
    }

    /// The bytes of messages that fit beside a system prompt and tools of `fixed_bytes`, `None`
    /// without a known window.
    pub fn message_bytes(&self, fixed_bytes: usize, tight: bool) -> Option<usize> {
        let budget = self.bytes_for(self.input_budget(tight)?);
        Some(budget.saturating_sub(fixed_bytes))
    }

    /// The `max_tokens` to ask for with a prompt of about `input_tokens`: the registered limit,
    /// or what is left of the window when that is less.
    pub fn reply_tokens(&self, input_tokens: u32) -> u32 {
        match self.window {
            None => self.max_reply,
            Some(window) => self
                .max_reply
                .min(window.saturating_sub(input_tokens).saturating_sub(SAFETY_TOKENS))
                .max(MIN_REPLY_TOKENS.min(self.max_reply)),
        }
    }

    /// How many bytes each of `calls` tool results may take when the prompt already takes
    /// `used_tokens`: an equal share of what is left of the input budget, between a floor and
    /// `cap` (the server's own limit).
    pub fn tool_output_bytes(&self, used_tokens: u32, calls: usize, cap: usize) -> usize {
        let Some(budget) = self.input_budget(false) else {
            return cap;
        };
        let left = self.bytes_for(budget.saturating_sub(used_tokens));
        (left / calls.max(1)).clamp(MIN_TOOL_OUTPUT_BYTES.min(cap), cap)
    }
}

/// The note for the system prompt of a run with a small window.
pub fn small_window_note(window: u32) -> String {
    format!(
        "This model's context window is small (about {window} tokens for the whole conversation \
         and your reply), so long tool results are cut short and older ones are cut down as the \
         conversation grows. Work through a task one source at a time: after reading a long \
         result, write down what you need from it (briefly in your reply, or in a workspace \
         file) before going on, and ask tools for no more than you need (fewer results, a \
         narrower search, a date filter)."
    )
}

/// The size of `messages` as sent, in bytes of their JSON.
pub fn size(messages: &[Value]) -> usize {
    messages.iter().map(|m| m.to_string().len()).sum()
}

/// What [`fit`] did.
#[derive(Debug, Default, PartialEq)]
pub struct Fitted {
    pub messages: Vec<Value>,
    /// Tool results that were cut down.
    pub trimmed: usize,
    /// Messages left out from the start of the conversation.
    pub dropped: usize,
}

/// `messages` cut down to fit in `max_bytes`: earlier tool results to a short head each, then the
/// newest ones (the largest first, never below a floor), then the oldest messages after the
/// first, which carries the task. Unchanged when they fit.
pub fn fit(messages: &[Value], max_bytes: usize) -> Fitted {
    let mut out: Vec<Value> = messages.to_vec();
    let mut used = size(&out);
    if used <= max_bytes {
        return Fitted { messages: out, ..Fitted::default() };
    }
    let mut trimmed = 0;
    // The newest message's tool results are what the model is about to read; they are cut last.
    let newest = out
        .iter()
        .rposition(has_tool_results)
        .filter(|i| *i + 1 == out.len());
    for (i, message) in out.iter_mut().enumerate() {
        if used <= max_bytes {
            break;
        }
        if Some(i) == newest {
            continue;
        }
        let before = message.to_string().len();
        let n = cut_results(message, OLD_RESULT_CHARS);
        if n > 0 {
            trimmed += n;
            used = used - before + message.to_string().len();
        }
    }
    if let Some(i) = newest {
        while used > max_bytes {
            let Some((b, len)) = largest_result(&out[i]) else { break };
            if len <= NEW_RESULT_MIN_CHARS {
                break;
            }
            let excess = used - max_bytes;
            let keep = len.saturating_sub(excess + 200).max(NEW_RESULT_MIN_CHARS);
            let before = out[i].to_string().len();
            let block = &mut out[i]["content"][b];
            let text = result_text(block);
            block["content"] = Value::String(cut(&text, keep));
            trimmed += 1;
            let after = out[i].to_string().len();
            if after >= before {
                break;
            }
            used = used - before + after;
        }
    }
    // Then the oldest messages after the task, never the newest exchange (the model's last
    // calls and their results), and never leaving results whose calls are gone.
    let mut dropped = 0;
    while used > max_bytes && out.len() > 3 {
        used -= out[1].to_string().len();
        out.remove(1);
        dropped += 1;
        if out.len() > 3 && out[1]["role"] == "user" && has_tool_results(&out[1]) {
            used -= out[1].to_string().len();
            out.remove(1);
            dropped += 1;
        }
    }
    if dropped > 0 {
        let note = format!("({dropped} earlier message(s) left out to fit the model's context window)");
        out.insert(1, json!({ "role": "user", "content": note }));
        used = size(&out);
    }
    // Last, the newest results go below their floor rather than be lost.
    if used > max_bytes {
        let last = out.len() - 1;
        let n = out[last]["content"].as_array().map_or(0, |b| b.len()).max(1);
        let each = (max_bytes.saturating_sub(used - out[last].to_string().len()) / n).max(LAST_RESORT_CHARS);
        trimmed += cut_results(&mut out[last], each);
    }
    Fitted { messages: out, trimmed, dropped }
}

/// What the newest tool results are cut to when nothing else is left to cut, in characters.
const LAST_RESORT_CHARS: usize = 500;

fn has_tool_results(message: &Value) -> bool {
    message["content"]
        .as_array()
        .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
}

/// Cut every tool result of `message` longer than `max_chars`; returns how many were cut.
fn cut_results(message: &mut Value, max_chars: usize) -> usize {
    let Some(blocks) = message["content"].as_array_mut() else {
        return 0;
    };
    let mut n = 0;
    for block in blocks.iter_mut().filter(|b| b["type"] == "tool_result") {
        let text = result_text(block);
        if text.chars().count() > max_chars || !block["content"].is_string() {
            block["content"] = Value::String(cut(&text, max_chars));
            n += 1;
        }
    }
    n
}

/// The index and length in characters of the longest tool result of `message`.
fn largest_result(message: &Value) -> Option<(usize, usize)> {
    message["content"]
        .as_array()?
        .iter()
        .enumerate()
        .filter(|(_, b)| b["type"] == "tool_result")
        .map(|(i, b)| (i, result_text(b).chars().count()))
        .max_by_key(|(_, len)| *len)
}

/// A tool result's text; an image or other part in it becomes a short note.
fn result_text(block: &Value) -> String {
    match &block["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p["type"].as_str() {
                Some("text") => p["text"].as_str().unwrap_or_default().to_string(),
                Some(kind) => format!("[{kind} omitted]\n"),
                None => String::new(),
            })
            .collect(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn cut(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!(
        "{kept}\n[… {} more characters cut to fit the model's context window; call the tool \
         again (for fetch_url, with start) if you need them]",
        total - max_chars
    )
}

/// The event data for a request that was cut down to fit.
pub fn trimmed_event(turn: u32, before: u32, after: u32, fitted: &Fitted, window: u32) -> Value {
    json!({
        "turn": turn,
        "window": window,
        "estimated_tokens_before": before,
        "estimated_tokens_after": after,
        "trimmed_results": fitted.trimmed,
        "dropped_messages": fitted.dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(id: &str, text: &str) -> Value {
        json!({ "type": "tool_result", "tool_use_id": id, "content": text })
    }

    fn convo(old: &str, new: &[&str]) -> Vec<Value> {
        vec![
            json!({ "role": "user", "content": "the task" }),
            json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "a", "name": "t", "input": {} }] }),
            json!({ "role": "user", "content": [result("a", old)] }),
            json!({ "role": "assistant", "content": [
                { "type": "tool_use", "id": "b", "name": "t", "input": {} },
                { "type": "tool_use", "id": "c", "name": "t", "input": {} },
            ] }),
            json!({ "role": "user", "content": new.iter().enumerate().map(|(i, t)| result(["b", "c"][i], t)).collect::<Vec<_>>() }),
        ]
    }

    #[test]
    fn what_fits_is_left_alone() {
        let m = convo("old", &["x", "y"]);
        let f = fit(&m, 1_000_000);
        assert_eq!(f.messages, m);
        assert_eq!((f.trimmed, f.dropped), (0, 0));
    }

    #[test]
    fn earlier_results_go_first() {
        let m = convo(&"o".repeat(20_000), &[&"n".repeat(5_000), "short"]);
        let f = fit(&m, 10_000);
        assert!(size(&f.messages) <= 10_000, "{}", size(&f.messages));
        assert_eq!(f.dropped, 0);
        let old = f.messages[2]["content"][0]["content"].as_str().unwrap();
        assert!(old.contains("cut to fit"), "{old}");
        // The newest results were not touched: the old one made room enough.
        assert_eq!(f.messages[4], m[4]);
    }

    #[test]
    fn the_largest_new_result_is_cut_next() {
        let m = convo("old", &[&"n".repeat(30_000), &"s".repeat(3_000)]);
        let f = fit(&m, 12_000);
        assert!(size(&f.messages) <= 12_000, "{}", size(&f.messages));
        let big = f.messages[4]["content"][0]["content"].as_str().unwrap();
        assert!(big.contains("cut to fit"));
        assert_eq!(f.messages[4]["content"][1], m[4]["content"][1]);
    }

    #[test]
    fn old_messages_are_dropped_last_but_never_the_newest_exchange() {
        let m = convo(&"o".repeat(2_000), &[&"n".repeat(50_000), &"s".repeat(50_000)]);
        let f = fit(&m, 3_000);
        assert_eq!(f.dropped, 2);
        assert_eq!(f.messages[0], m[0]);
        assert!(f.messages[1]["content"].as_str().unwrap().contains("left out"));
        // The newest calls and their (cut) results are still there, in order.
        assert_eq!(f.messages[2], m[3]);
        let results = f.messages[3]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r["content"].as_str().unwrap().contains("cut to fit")));
    }

    #[test]
    fn replies_get_what_the_window_leaves() {
        let b = Budget::new(Some(32_768), 32_768);
        assert_eq!(b.reply_tokens(20_000), 32_768 - 20_000 - SAFETY_TOKENS);
        assert_eq!(b.reply_tokens(32_000), MIN_REPLY_TOKENS);
        // A quarter of the window is kept for the reply when deciding what fits.
        assert_eq!(b.input_budget(false), Some(32_768 - 8_192 - SAFETY_TOKENS));
        assert!(b.input_budget(true).unwrap() < b.input_budget(false).unwrap());
        assert!(b.is_small());
        let unknown = Budget::new(None, 16_384);
        assert_eq!(unknown.reply_tokens(100_000), 16_384);
        assert_eq!(unknown.message_bytes(0, false), None);
        assert_eq!(unknown.tool_output_bytes(0, 4, 32_768), 32_768);
    }

    #[test]
    fn tool_output_is_shared_out() {
        let b = Budget::new(Some(32_768), 8_192);
        let each = b.tool_output_bytes(8_000, 4, 32 * 1024);
        assert!((MIN_TOOL_OUTPUT_BYTES..32 * 1024).contains(&each), "{each}");
        assert_eq!(b.tool_output_bytes(40_000, 4, 32 * 1024), MIN_TOOL_OUTPUT_BYTES);
    }

    #[test]
    fn a_cut_off_reply_tells_the_window() {
        let mut b = Budget::new(None, 32_768);
        let usage = Usage { input_tokens: 31_555, output_tokens: 1_213 };
        assert!(b.learn_window(32_768, Some("max_tokens"), usage));
        assert_eq!(b.window(), Some(32_768));
        // A reply that used all it asked for says nothing about the window.
        let mut c = Budget::new(None, 1_000);
        assert!(!c.learn_window(1_000, Some("max_tokens"), Usage { input_tokens: 5, output_tokens: 1_000 }));
        assert!(!c.learn_window(1_000, Some("end_turn"), usage));
    }

    #[test]
    fn calibration_follows_the_usage() {
        let mut b = Budget::new(Some(10_000), 1_000);
        b.calibrate(40_000, Usage { input_tokens: 10_000, output_tokens: 0 });
        assert_eq!(b.estimate(4_000), 1_000);
        // A count no tokenizer gives is ignored.
        b.calibrate(40_000, Usage { input_tokens: 10, output_tokens: 0 });
        assert_eq!(b.estimate(4_000), 1_000);
    }
}
