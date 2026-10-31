//! Contributing a conversation to a training corpus (`docs/design/contributions.md`).
//!
//! A bundle is built here, on the person's own server, from the traces of the thread's runs
//! (`GET /v1/runs/{id}/trace`), and shown to them before anything leaves:
//!
//! 1. **Structural** removals, always: the memory note is cut from the system prompt, attached
//!    files' bytes and names go, fetched page bodies become stubs (unless chosen), reasoning is
//!    left out (unless chosen), run and conversation ids and times go, events are reduced to
//!    what trains (plans, approvals, how the run ended).
//! 2. The **pattern scan** of `cedar_contrib_schema::scan` replaces keys, emails, phone and
//!    account numbers, IP addresses and home directories with placeholders.
//! 3. The **consistency checks** both sides run, and a second look for secrets: a bundle that
//!    fails either is shown with its problems and cannot be sent.
//!
//! Sending (`CHAT_CONTRIBUTIONS_URL` set, Settings → Contribute turned on) posts exactly the
//! document the dialog holds, with the license and pseudonym from Settings, and keeps the
//! receipt so the contribution can be withdrawn.

use crate::api::AppState;
use crate::error::AppError;
use crate::store::{Contribution, ContributionSettings, Conversation, Turn};
use cedar_contrib_schema::scan::{self, Kind, Redactor};
use cedar_contrib_schema::validate::{self, Limits};
use cedar_contrib_schema::{Bundle, Call, Dropped, License, Message, ModelInfo, Outcome, Usage, CANARY, SCHEMA_VERSION};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

/// What the person may include beyond the default.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Options {
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub fetched: bool,
}

/// The bundle as the dialog shows it, with why it cannot be sent, if it can't.
#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub bundle: Bundle,
    /// Reasons nothing can be sent: `scheduled`, `active`, `no_turns`, `private_tools`,
    /// `run_gone`, `secrets`, `not_consistent`, `model_not_allowed`.
    pub blocked: Vec<String>,
    /// Where a secret still is, for the person to fix.
    pub secrets: Vec<String>,
    /// What the consistency checks found.
    pub problems: Vec<String>,
    /// Whether this server can send at all (`CHAT_CONTRIBUTIONS_URL`), and whether the person
    /// turned contributing on in Settings.
    pub can_send: bool,
    pub accepted: bool,
    /// Tools that made the thread not contributable, by name.
    pub private_tools: Vec<String>,
    /// The chat's id of each turn in the bundle, in order, for the dialog's rating buttons.
    /// Beside the bundle, never in it.
    pub turn_ids: Vec<uuid::Uuid>,
}

/// Tools whose results are other people's data: a thread that called one cannot be contributed.
fn is_private_tool(name: &str) -> bool {
    name.starts_with("gmail_") || name.starts_with("calendar_")
}

/// Tools whose results are pages and files from elsewhere: stubbed unless the person includes them.
fn is_fetching_tool(name: &str) -> bool {
    matches!(name, "fetch_url" | "download_file" | "browser")
}

/// The events a bundle keeps, and the fields of their data.
fn keep_event(ty: &str, data: &Value) -> Option<Value> {
    let pick = |keys: &[&str]| -> Value {
        let mut out = serde_json::Map::new();
        for k in keys {
            if let Some(v) = data.get(*k) {
                out.insert((*k).to_string(), v.clone());
            }
        }
        Value::Object(out)
    };
    Some(match ty {
        "plan" => pick(&["steps", "editable", "origin"]),
        "plan.updated" => pick(&["steps", "by"]),
        "approval.required" => pick(&["id", "tool", "rule", "reason", "scope"]),
        "approval.resolved" => pick(&["id", "decision", "by"]),
        "sandbox.denied" => pick(&["program", "host", "port", "what", "reason"]),
        "run.completed" => pick(&["stop_reason"]),
        "run.failed" => pick(&["error"]),
        "run.cancelled" => pick(&["reason"]),
        _ => return None,
    })
}

/// How the memory note opens, for a cedar from before the trace marked it.
const MEMORY_NOTE_STARTS: [&str; 2] = ["What you remember about this user from earlier conversations.", "You don't remember anything about this user from earlier conversations yet."];

/// The system prompt without the user's memories, and whether any were cut.
fn without_memory(system: &str, span: Option<&Value>) -> (String, bool) {
    if let Some(span) = span {
        if let (Some(start), Some(end)) = (span["start"].as_u64(), span["end"].as_u64()) {
            let (start, end) = (start as usize, end as usize);
            if start <= end && end <= system.len() && system.is_char_boundary(start) && system.is_char_boundary(end) {
                return (format!("{}{}", &system[..start], &system[end..]).trim_end().to_string(), true);
            }
        }
    }
    if let Some(start) = MEMORY_NOTE_STARTS.iter().filter_map(|m| system.find(m)).min() {
        return (system[..start].trim_end().to_string(), true);
    }
    (system.to_string(), false)
}

/// A message's content as blocks, whether cedar stored it as text or as blocks.
fn blocks_of(content: &Value) -> Vec<Value> {
    match content {
        Value::Array(a) => a.clone(),
        Value::String(s) => vec![json!({ "type": "text", "text": s })],
        Value::Null => vec![],
        other => vec![json!({ "type": "text", "text": other.to_string() })],
    }
}

/// A tool result's content as text, for a stub's size and hash.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        other => other.to_string(),
    }
}

/// Build the bundle of `conversation` as it would be sent, and say whether it can be.
pub async fn preview(state: &AppState, conversation: &Conversation, opts: Options) -> Result<Preview, AppError> {
    let settings = state.store.contribution_settings().await?;
    let prefs = state.store.preferences().await?;
    let opts = Options {
        reasoning: opts.reasoning || settings.include_reasoning,
        fetched: opts.fetched || settings.include_fetched,
    };
    let mut blocked: BTreeSet<String> = BTreeSet::new();
    if conversation.schedule_id.is_some() {
        blocked.insert("scheduled".into());
    }
    let mut turns = Vec::new();
    for turn in state.store.list_turns(conversation.id).await? {
        turns.push(crate::api::refresh(state, turn).await?);
    }
    if turns.iter().any(|t| !crate::cedar::is_terminal(&t.status)) {
        blocked.insert("active".into());
    }
    let turns: Vec<Turn> = turns.into_iter().filter(|t| crate::cedar::is_terminal(&t.status)).collect();
    if !turns.iter().any(|t| t.status == "completed") {
        blocked.insert("no_turns".into());
    }

    let mut redactor = Redactor::new();
    let mut dropped = Dropped::default();
    let mut tools_used: BTreeSet<String> = BTreeSet::new();
    let mut private_tools: BTreeSet<String> = BTreeSet::new();
    let mut model: Option<ModelInfo> = None;
    let mut out_turns = Vec::new();
    let mut turn_ids = Vec::new();

    for turn in &turns {
        let trace = match state.cedar.trace(turn.run_id).await {
            Ok(t) => t,
            Err(AppError::Upstream { status: StatusCode::NOT_FOUND, .. }) => {
                blocked.insert("run_gone".into());
                continue;
            }
            Err(e) => return Err(e),
        };
        let run = &trace["run"];
        if let (Some(id), Some(name)) = (run["model_id"].as_str(), run["model"].as_str()) {
            model = Some(ModelInfo { id: id.into(), name: name.into(), reasoning: run["reasoning"].as_str().map(String::from) });
        }
        for a in &turn.attachments {
            redactor.redact_exact(Kind::File, &a.name, false);
            dropped.attachments += 1;
        }
        let (system, cut) = without_memory(trace["system"].as_str().unwrap_or_default(), trace.get("memory_note"));
        dropped.memory |= cut;
        let tool_definitions: Vec<Value> = trace["tools"].as_array().cloned().unwrap_or_default();

        // Which tool each call id named, so results can be judged by their tool.
        let mut tool_of: HashMap<String, String> = HashMap::new();
        let mut messages = Vec::new();
        for m in trace["messages"].as_array().into_iter().flatten() {
            let role = m["role"].as_str().unwrap_or("user").to_string();
            let mut content = Vec::new();
            for b in blocks_of(&m["content"]) {
                match b["type"].as_str() {
                    Some("tool_use") => {
                        let name = b["name"].as_str().unwrap_or_default().to_string();
                        if let Some(id) = b["id"].as_str() {
                            tool_of.insert(id.to_string(), name.clone());
                        }
                        if is_private_tool(&name) {
                            private_tools.insert(name.clone());
                        }
                        tools_used.insert(name);
                        content.push(b);
                    }
                    Some("tool_result") => {
                        let id = b["tool_use_id"].as_str().unwrap_or_default();
                        let fetched = tool_of.get(id).is_some_and(|n| is_fetching_tool(n));
                        if fetched && !opts.fetched {
                            let text = result_text(&b["content"]);
                            dropped.fetched_bodies += 1;
                            content.push(json!({
                                "type": "tool_result",
                                "tool_use_id": id,
                                "is_error": b["is_error"],
                                "content": format!("[FETCHED BODY LEFT OUT: {} bytes, sha256 {}]", text.len(), hex::encode(Sha256::digest(text.as_bytes()))),
                            }));
                        } else {
                            content.push(b);
                        }
                    }
                    Some("image") | Some("document") => {
                        // cedar's trace already replaced the base64 by its size.
                        content.push(json!({
                            "type": b["type"],
                            "source": { "type": "base64", "media_type": b["source"]["media_type"], "bytes": b["source"]["bytes"] },
                        }));
                    }
                    _ => content.push(b),
                }
            }
            messages.push(Message { role, content });
        }

        let mut calls = Vec::new();
        for c in trace["calls"].as_array().into_iter().flatten() {
            let response = &c["response"];
            let reasoning = response["reasoning"].as_str().filter(|r| !r.trim().is_empty()).map(String::from);
            if reasoning.is_some() && !opts.reasoning {
                dropped.reasoning += 1;
            }
            calls.push(Call {
                seq: c["seq"].as_u64().unwrap_or(0) as u32,
                turn: c["turn"].as_u64().unwrap_or(0) as u32,
                purpose: c["purpose"].as_str().unwrap_or("agent").to_string(),
                message_count: c["request"]["message_count"].as_u64().unwrap_or(0) as u32,
                stop_reason: response["stop_reason"].as_str().map(String::from),
                usage: response["usage"].as_object().map(|u| Usage {
                    input_tokens: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
                    output_tokens: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
                }),
                reasoning: if opts.reasoning { reasoning } else { None },
                error: c["error"].as_str().map(String::from),
            });
        }

        let events: Vec<Value> = trace["events"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let ty = e["type"].as_str()?;
                keep_event(ty, &e["data"]).map(|data| json!({ "type": ty, "data": data }))
            })
            .collect();

        turn_ids.push(turn.id);
        out_turns.push(cedar_contrib_schema::Turn {
            seq: out_turns.len() as u32 + 1,
            prompt: turn.prompt.clone(),
            system,
            tool_definitions,
            messages,
            calls,
            events,
            rating: turn.rating,
            status: turn.status.clone(),
            stop_reason: turn.stop_reason.clone(),
            day: turn.created_at.format("%Y-%m-%d").to_string(),
        });
    }
    if !private_tools.is_empty() {
        blocked.insert("private_tools".into());
    }

    let language = prefs
        .locale
        .as_deref()
        .and_then(|l| l.split(['-', '_']).next())
        .map(|l| l.to_ascii_lowercase())
        .filter(|l| (2..=3).contains(&l.len()))
        .unwrap_or_else(|| "en".into());
    let mut bundle = Bundle {
        schema_version: SCHEMA_VERSION,
        canary: CANARY.into(),
        license: License::parse(&settings.license).unwrap_or(License::Cc0),
        contributor: settings.pseudonym.clone(),
        cedar_version: crate::update::VERSION.to_string(),
        language,
        model: model.unwrap_or(ModelInfo { id: conversation.model.clone().unwrap_or_default(), name: conversation.model.clone().unwrap_or_default(), reasoning: conversation.reasoning.clone() }),
        tools: tools_used.into_iter().collect(),
        outcome: None,
        turns: out_turns,
        redactions: BTreeMap::new(),
        dropped,
    };
    cedar_contrib_schema::walk_bundle_text(&mut bundle, &mut |s| {
        redactor.redact(s);
    });
    bundle.redactions = redactor.counts();

    let secrets = scan::secrets_in(&bundle);
    if !secrets.is_empty() {
        blocked.insert("secrets".into());
    }
    let problems: Vec<String> = if bundle.turns.is_empty() {
        vec![]
    } else {
        validate::validate(&bundle, &Limits::default()).into_iter().map(|p| format!("{}: {}", p.code.as_str(), p.detail)).collect()
    };
    if !problems.is_empty() {
        blocked.insert("not_consistent".into());
    }
    if !bundle.turns.is_empty() {
        if let Some(policy) = policy(state).await {
            if !model_allowed(&policy, &bundle.model.name) {
                blocked.insert("model_not_allowed".into());
            }
        }
    }
    Ok(Preview {
        can_send: state.cfg.contributions_url.is_some(),
        accepted: settings.accepted_at.is_some(),
        bundle,
        blocked: blocked.into_iter().collect(),
        secrets,
        problems,
        private_tools: private_tools.into_iter().collect(),
        turn_ids,
    })
}

/// The ingest's rules (`GET /v1/policy`), if it answers; a failure only means no check here.
async fn policy(state: &AppState) -> Option<Value> {
    let url = state.cfg.contributions_url.as_ref()?;
    let resp = state.http.get(format!("{url}/v1/policy")).timeout(Duration::from_secs(10)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// Whether a policy's `models` allow `name`: exact names, or a trailing `*` as a prefix wildcard.
/// No list means every model.
fn model_allowed(policy: &Value, name: &str) -> bool {
    let Some(models) = policy["models"].as_array() else { return true };
    models.iter().filter_map(Value::as_str).any(|m| match m.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => m == name,
    })
}

/// What the ingest answers a contribution with.
#[derive(Debug, Clone, Deserialize)]
struct Receipt {
    receipt_id: String,
    withdraw_token: String,
}

/// Send `bundle`, exactly as given but for the license, pseudonym, canary and schema version,
/// which are this server's to set. Checked again first; the ingest checks a third time.
pub async fn send(state: &AppState, conversation: &Conversation, mut bundle: Bundle, outcome: Option<Outcome>) -> Result<Contribution, AppError> {
    let Some(url) = state.cfg.contributions_url.as_deref() else {
        return Err(AppError::Conflict("this server has no corpus to contribute to (CHAT_CONTRIBUTIONS_URL)".into()));
    };
    let settings = state.store.contribution_settings().await?;
    if settings.accepted_at.is_none() {
        return Err(AppError::Conflict("turn contributing on in Settings → Contribute first".into()));
    }
    bundle.schema_version = SCHEMA_VERSION;
    bundle.canary = CANARY.into();
    bundle.license = License::parse(&settings.license).unwrap_or(License::Cc0);
    bundle.contributor = settings.pseudonym.clone();
    if let Some(o) = outcome {
        bundle.outcome = Some(o);
    }
    let problems = validate::validate(&bundle, &Limits::default());
    if let Some(p) = problems.first() {
        return Err(AppError::BadRequest(format!("the bundle doesn't check out: {}", p.detail)));
    }
    let secrets = scan::secrets_in(&bundle);
    if let Some(s) = secrets.first() {
        return Err(AppError::BadRequest(format!("a secret is still in the bundle ({s}); redact it first")));
    }
    let body = serde_json::to_vec(&bundle).map_err(anyhow::Error::from)?;
    if body.len() > Limits::default().max_bundle_bytes {
        return Err(AppError::PayloadTooLarge(format!("the bundle is {} bytes; the corpus takes at most {}", body.len(), Limits::default().max_bundle_bytes)));
    }
    let resp = state
        .http
        .post(format!("{url}/v1/contributions"))
        .header("content-type", "application/json")
        .header("x-cedar-chat-version", crate::update::VERSION)
        .body(body)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| AppError::BadGateway(format!("could not reach the corpus: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let message = parsed["error"]["message"].as_str().map(String::from).unwrap_or_else(|| format!("the corpus answered {status}"));
        let reason = parsed["error"]["code"].as_str().unwrap_or("refused");
        return Err(match status {
            StatusCode::CONFLICT => AppError::Conflict(message),
            StatusCode::TOO_MANY_REQUESTS => AppError::Conflict(format!("{message} (try again later)")),
            s if s.is_client_error() => AppError::BadRequest(format!("{reason}: {message}")),
            _ => AppError::BadGateway(message),
        });
    }
    let receipt: Receipt = serde_json::from_str(&text).map_err(|e| AppError::BadGateway(format!("unreadable receipt from the corpus: {e}")))?;
    let saved = state
        .store
        .insert_contribution(&receipt.receipt_id, conversation.id, conversation.title.as_deref(), &receipt.withdraw_token, &bundle.content_sha256(), bundle.license.id(), url)
        .await?;
    Ok(saved)
}

/// Withdraw a contribution: the ingest deletes it; the receipt stays, marked withdrawn, so the
/// list in Settings says what happened. A corpus that no longer knows it counts as withdrawn.
pub async fn withdraw(state: &AppState, id: &str) -> Result<Contribution, AppError> {
    let Some(c) = state.store.get_contribution(id).await? else {
        return Err(AppError::NotFound("no such contribution"));
    };
    if c.withdrawn_at.is_none() {
        let resp = state
            .http
            .delete(format!("{}/v1/contributions/{}", c.url, c.id))
            .bearer_auth(&c.withdraw_token)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| AppError::BadGateway(format!("could not reach the corpus: {e}")))?;
        if !(resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND) {
            return Err(AppError::BadGateway(format!("the corpus answered {} to the withdrawal", resp.status())));
        }
        state.store.mark_withdrawn(&c.id).await?;
    }
    Ok(state.store.get_contribution(id).await?.expect("still there"))
}

/// The settings page's view of things: the choices, whether this server can send, and where.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    #[serde(flatten)]
    pub settings: ContributionSettings,
    pub available: bool,
    /// The corpus's host, for the page to name.
    pub host: Option<String>,
    pub licenses: Vec<&'static str>,
}

pub fn settings_view(state: &AppState, settings: ContributionSettings) -> SettingsView {
    let host = state.cfg.contributions_url.as_deref().map(|u| {
        u.trim_start_matches("https://").trim_start_matches("http://").split('/').next().unwrap_or_default().to_string()
    });
    SettingsView { settings, available: state.cfg.contributions_url.is_some(), host, licenses: License::ALL.iter().map(|l| l.id()).collect() }
}

/// A fresh pseudonym: `p_` and 16 hex characters of randomness.
pub fn new_pseudonym() -> String {
    let id = uuid::Uuid::new_v4();
    format!("p_{}", &id.simple().to_string()[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_note_is_cut_by_span_or_by_its_opening() {
        let system = "Be brief.\n\nYou don't remember anything about this user from earlier conversations yet. Use remember.";
        let span = json!({ "start": 9, "end": system.len() });
        assert_eq!(without_memory(system, Some(&span)), ("Be brief.".into(), true));
        assert_eq!(without_memory(system, None), ("Be brief.".into(), true));
        assert_eq!(without_memory("Be brief.", None), ("Be brief.".into(), false));
        // A span that doesn't fit the text is ignored, and the opening still found.
        assert_eq!(without_memory(system, Some(&json!({ "start": 5000, "end": 9 }))), ("Be brief.".into(), true));
    }

    #[test]
    fn only_training_events_are_kept_and_only_their_useful_fields() {
        assert_eq!(keep_event("tool.call", &json!({ "name": "bash" })), None);
        assert_eq!(keep_event("usage", &json!({ "input_tokens": 1 })), None);
        assert_eq!(
            keep_event("approval.required", &json!({ "id": "a", "tool": "bash", "input": { "command": "rm" }, "rule": "r", "reason": "x", "scope": "s" })),
            Some(json!({ "id": "a", "tool": "bash", "rule": "r", "reason": "x", "scope": "s" }))
        );
        assert_eq!(keep_event("run.completed", &json!({ "final_text": "long", "stop_reason": "end_turn" })), Some(json!({ "stop_reason": "end_turn" })));
    }

    #[test]
    fn model_policies_take_names_and_prefixes() {
        let p = json!({ "models": ["deepseek-ai/*", "Qwen/Qwen3-8B"] });
        assert!(model_allowed(&p, "deepseek-ai/DeepSeek-V4-Flash"));
        assert!(model_allowed(&p, "Qwen/Qwen3-8B"));
        assert!(!model_allowed(&p, "openai/gpt-5"));
        assert!(model_allowed(&json!({}), "anything"));
    }

    #[test]
    fn pseudonyms_are_short_random_ids() {
        let p = new_pseudonym();
        assert_eq!(p.len(), 18);
        assert!(p.starts_with("p_"));
        assert_ne!(p, new_pseudonym());
    }
}
