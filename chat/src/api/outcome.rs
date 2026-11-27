//! `GET /api/turns/{id}/outcome`: what one ask came to, for the Today page's outcome view.
//!
//! An outcome is the answer, not the transcript: what was asked, the answer with its one-line
//! summary, and what the turn left behind — the files it wrote or changed in the workspace, what
//! it remembered, the record rows it saved, the follow-ups it suggested — with how it got there
//! (its steps and the sources it read) folded behind that.
//!
//! Every part is read from what cedar already keeps for the run: its events (steps, sources,
//! follow-ups), its memories and record rows (each knows the run that saved it) and the
//! workspace (files changed since the turn was sent). A part cedar can't give (an older cedar,
//! a workspace that is gone) is left empty; the outcome still opens.

use super::*;
use chrono::DateTime;
use futures_util::StreamExt;
use std::collections::HashSet;
use std::time::Duration;

/// How long reading a finished run's events may take, and how much of them is read.
const EVENTS_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;
/// How many record sets are looked into, and how many rows and sources are listed.
const MAX_RECORD_SETS: usize = 10;
const MAX_RECORDS: usize = 20;
const MAX_SOURCES: usize = 50;
/// A file counts as this turn's when it changed after the turn was sent, give or take clocks.
const FILE_SLACK_MS: i64 = 2_000;
/// The memory kinds about the user, as `/api/memory` lists them by default.
const USER_KINDS: &str = "fact,preference,instruction,temporary";

pub(super) async fn outcome(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let turn = refresh(&state, find_turn(&state, &id).await?).await?;
    let conversation = state.store.get_conversation(turn.conversation_id).await?.ok_or(AppError::NotFound("turn not found"))?;
    let turns = state.store.list_turns(conversation.id).await?;
    let later = turns.iter().filter(|t| t.seq > turn.seq).count();
    let agent = match conversation.schedule_id {
        Some(sid) => state.store.get_schedule(sid).await?.map(|s| json!({ "id": s.id, "title": s.title, "trigger_kind": s.trigger_kind })),
        None => None,
    };
    let finished = cedar::is_terminal(&turn.status);
    let asking = turn.status == "completed" && turn.stop_reason.as_deref() == Some("ask_user") && later == 0;
    let outcome_state = if asking {
        "asking"
    } else {
        match turn.status.as_str() {
            "queued" | "running" => "working",
            "completed" => "done",
            "failed" => "failed",
            _ => "stopped",
        }
    };

    let run = if finished { read_run(&state, turn.run_id).await } else { RunParts::default() };
    // Nothing of a private conversation is remembered or recorded; its files are its own.
    let memories = if conversation.private { Vec::new() } else { memories_of(&state, turn.run_id).await };
    let records = if conversation.private { Vec::new() } else { records_of(&state, &turn).await };
    let files = files_of(&state, &turn).await;
    let decisions: Vec<_> = state.store.list_followups(conversation.id).await?.into_iter().filter(|f| f.turn_id == turn.id).collect();

    Ok(Json(json!({
        "turn": turn,
        "conversation": { "id": conversation.id, "title": conversation.title, "private": conversation.private },
        "agent": agent,
        "state": outcome_state,
        "summary": turn.final_text.as_deref().and_then(timeline::summary),
        "later_turns": later,
        "steps": run.steps,
        "sources": run.sources,
        "followups": run.followups,
        "followup_decisions": decisions,
        "memories": memories,
        "records": records,
        "files": files,
    })))
}

/// What a finished run's events say: its tool calls, the sources it read and its follow-ups.
#[derive(Default)]
struct RunParts {
    steps: Vec<Value>,
    sources: Vec<Value>,
    followups: Vec<Value>,
}

/// Read a finished run's events, from the first to the last. A stream that fails or takes too
/// long gives what was read so far.
async fn read_run(state: &AppState, run_id: Uuid) -> RunParts {
    let mut events: Vec<Value> = Vec::new();
    let read = async {
        let resp = state.cedar.events(run_id, 0).await?;
        let mut body = resp.bytes_stream();
        let (mut buf, mut total) = (Vec::new(), 0usize);
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| AppError::BadGateway(format!("cedar's event stream broke: {e}")))?;
            total += chunk.len();
            buf.extend_from_slice(&chunk);
            while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
                let block: Vec<u8> = buf.drain(..end + 2).collect();
                if let Some(event) = crate::notify::parse_sse(&block) {
                    let last = matches!(event["type"].as_str(), Some("run.completed" | "run.failed" | "run.cancelled"));
                    events.push(event);
                    if last {
                        return Ok::<(), AppError>(());
                    }
                }
            }
            if total > MAX_EVENT_BYTES {
                break;
            }
        }
        Ok(())
    };
    match tokio::time::timeout(EVENTS_TIMEOUT, read).await {
        Ok(Err(e)) => tracing::debug!(%run_id, error = %e, "could not read a run's events for its outcome"),
        Err(_) => tracing::debug!(%run_id, "reading a run's events for its outcome took too long"),
        Ok(Ok(())) => {}
    }
    parts_of(&events)
}

/// The steps, sources and follow-ups in a run's events.
fn parts_of(events: &[Value]) -> RunParts {
    let mut parts = RunParts::default();
    let mut calls: Vec<(String, String, Value)> = Vec::new(); // (id, name, input)
    let mut seen: HashSet<String> = HashSet::new();
    let mut push_source = |parts: &mut RunParts, source: Value| {
        let key = source["url"].as_str().or(source["path"].as_str()).unwrap_or_default().to_string();
        if !key.is_empty() && parts.sources.len() < MAX_SOURCES && seen.insert(key) {
            parts.sources.push(source);
        }
    };
    for event in events {
        let d = &event["data"];
        match event["type"].as_str() {
            Some("tool.call") => {
                let (id, name) = (d["id"].as_str().unwrap_or_default(), d["name"].as_str().unwrap_or_default());
                calls.push((id.to_string(), name.to_string(), d["input"].clone()));
                parts.steps.push(json!({ "id": id, "name": name, "is_error": false }));
            }
            Some("tool.result") => {
                let id = d["id"].as_str().unwrap_or_default();
                let is_error = d["is_error"].as_bool().unwrap_or(false);
                if let Some(step) = parts.steps.iter_mut().find(|s| s["id"] == id) {
                    step["is_error"] = json!(is_error);
                }
                if is_error {
                    continue;
                }
                let (name, input) = calls.iter().find(|(cid, ..)| cid == id).map(|(_, n, i)| (n.as_str(), i)).unwrap_or(("", &Value::Null));
                // What cedar numbered as the run's sources, else the pages it fetched.
                if let Some(sources) = d["sources"].as_array() {
                    for s in sources {
                        push_source(&mut parts, json!({ "url": s["url"], "path": s["path"], "title": s["title"] }));
                    }
                } else if name == "fetch_url" {
                    if let Some(url) = input["url"].as_str() {
                        push_source(&mut parts, json!({ "url": url, "path": null, "title": null }));
                    }
                }
                if name == "suggest_follow_up" {
                    parts.followups.push(json!({ "call_id": id, "output": d["output"] }));
                }
            }
            _ => {}
        }
    }
    parts
}

/// What the run remembered about the user.
async fn memories_of(state: &AppState, run_id: Uuid) -> Vec<Value> {
    match state.cedar.memory(&[("kind", USER_KINDS)]).await {
        Ok(Some(body)) => body["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["run_id"].as_str() == Some(run_id.to_string().as_str()))
            .map(|m| json!({ "id": m["id"], "text": m["text"], "kind": m["kind"] }))
            .collect(),
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::debug!(%run_id, error = %e, "could not read what a run remembered");
            Vec::new()
        }
    }
}

/// The record rows the run saved last, in the sets changed since the turn was sent.
async fn records_of(state: &AppState, turn: &Turn) -> Vec<Value> {
    let sets = match state.cedar.records().await {
        Ok(Some(body)) => body["sets"].as_array().cloned().unwrap_or_default(),
        Ok(None) => return Vec::new(),
        Err(e) => {
            tracing::debug!(turn_id = %turn.id, error = %e, "could not list record sets for an outcome");
            return Vec::new();
        }
    };
    let run = turn.run_id.to_string();
    let mut rows = Vec::new();
    let changed = sets.iter().filter(|s| {
        s["updated_at"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| t.with_timezone(&Utc) >= turn.created_at)
    });
    for set in changed.take(MAX_RECORD_SETS) {
        let Some(name) = set["name"].as_str() else { continue };
        let Ok(body) = state.cedar.record_set(name).await else { continue };
        for record in body["records"].as_array().into_iter().flatten().filter(|r| r["run_id"].as_str() == Some(run.as_str())) {
            if rows.len() >= MAX_RECORDS {
                return rows;
            }
            rows.push(json!({ "set": name, "key": record["key"], "data": record["data"] }));
        }
    }
    rows
}

/// The workspace files that changed after the turn was sent.
async fn files_of(state: &AppState, turn: &Turn) -> Vec<Value> {
    let since = turn.created_at.timestamp_millis() - FILE_SLACK_MS;
    match state.cedar.list_files(turn.run_id).await {
        Ok(body) => body["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|f| f["modified_at"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| t.timestamp_millis() >= since))
            .map(|f| json!({ "path": f["path"], "size": f["size"], "modified_at": f["modified_at"] }))
            .collect(),
        // A workspace that is gone (expired, deleted) has nothing to show.
        Err(e) => {
            tracing::debug!(turn_id = %turn.id, error = %e, "could not list an outcome's files");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, data: Value) -> Value {
        json!({ "type": kind, "data": data })
    }

    #[test]
    fn the_events_give_steps_sources_and_follow_ups() {
        let events = vec![
            ev("tool.call", json!({ "id": "t1", "name": "web_search", "input": { "query": "lisbon" } })),
            ev("tool.result", json!({ "id": "t1", "output": "…", "sources": [{ "n": 1, "url": "https://a.org", "title": "A" }, { "n": 2, "url": "https://a.org", "title": "A again" }] })),
            ev("tool.call", json!({ "id": "t2", "name": "fetch_url", "input": { "url": "https://b.org" } })),
            ev("tool.result", json!({ "id": "t2", "output": "<html>" })),
            ev("tool.call", json!({ "id": "t3", "name": "bash", "input": {} })),
            ev("tool.result", json!({ "id": "t3", "output": "boom", "is_error": true })),
            ev("tool.call", json!({ "id": "t4", "name": "suggest_follow_up", "input": {} })),
            ev("tool.result", json!({ "id": "t4", "output": "{\"follow_up\":{}}" })),
            ev("run.completed", json!({})),
        ];
        let parts = parts_of(&events);
        assert_eq!(parts.steps.len(), 4);
        assert_eq!(parts.steps[2], json!({ "id": "t3", "name": "bash", "is_error": true }));
        let urls: Vec<&str> = parts.sources.iter().filter_map(|s| s["url"].as_str()).collect();
        assert_eq!(urls, ["https://a.org", "https://b.org"], "deduplicated, in the order read");
        assert_eq!(parts.followups, vec![json!({ "call_id": "t4", "output": "{\"follow_up\":{}}" })]);
    }
}
