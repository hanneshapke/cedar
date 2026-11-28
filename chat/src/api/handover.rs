//! Handing work over from the agent to the person: the outbox (`/api/outbox`), where what an
//! unattended run would have sent or changed waits for approval, and feedback on what the agent
//! could not do (`/api/turns/{id}/limitations/{call_id}`, `/api/feedback`).
//!
//! Both follow the rule the follow-up suggestions set: the model proposes, the page shows a card,
//! and nothing happens until the person presses a button. The outbox's calls are kept by cedar,
//! exactly as the model wrote them (see `hold_writes` in cedar's README); this service only
//! relays the decision and says which conversation or agent a call came from. Feedback is kept
//! here, on this server, for whoever runs it: nothing is sent anywhere else.

use super::*;
use crate::store::{Feedback, NewFeedback};

/// The most feedback one listing returns.
const MAX_FEEDBACK_LISTED: u32 = 500;
const MAX_SUMMARY_CHARS: usize = 300;
const MAX_NEEDED_CHARS: usize = 200;
const MAX_COMMENT_CHARS: usize = 1000;
/// The categories cedar's `report_limitation` knows.
const CATEGORIES: &[&str] = &["missing_tool", "not_connected", "blocked", "limit", "other"];

// -- the outbox -------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub(super) struct OutboxQuery {
    /// `pending` (the default), `sent`, `failed`, `discarded`, or `all`.
    status: Option<String>,
}

/// A held call with where it came from: the conversation its run is in, and the agent whose run
/// that is. A call of a run this service doesn't know (another client's on the same key) is
/// shown without.
pub(super) async fn with_origin(state: &AppState, mut held: Value) -> Result<Value, AppError> {
    let run_id = held["run_id"].as_str().and_then(|s| Uuid::parse_str(s).ok());
    let turn = match run_id {
        Some(run_id) => state.store.get_turn_by_run(run_id).await?.filter(|(_, user)| *user == state.user()).map(|(turn, _)| turn),
        None => None,
    };
    let Some(turn) = turn else { return Ok(held) };
    held["turn_id"] = json!(turn.id);
    held["conversation_id"] = json!(turn.conversation_id);
    if let Some(conversation) = state.store.get_conversation(turn.conversation_id).await? {
        held["title"] = json!(conversation.title);
        if let Some(agent) = conversation.schedule_id {
            if let Some(schedule) = state.store.get_schedule(agent).await? {
                held["agent"] = json!({ "id": schedule.id, "title": schedule.title });
            }
        }
    }
    Ok(held)
}

/// The calls held for the person's approval with where each came from: those of one run, or
/// (`run_id` `None`) every one with `status`. For the Today page, which must open even when
/// cedar can't say (one from before the outbox): a failure is logged and gives none.
pub(super) async fn held_with_origin(state: &AppState, status: Option<&str>, run_id: Option<Uuid>) -> Vec<Value> {
    let held = match state.cedar.held(status, run_id).await {
        Ok(held) => held,
        Err(e) => {
            tracing::debug!(error = %e, "could not list the outbox");
            return Vec::new();
        }
    };
    let mut items = Vec::with_capacity(held.len());
    for call in held {
        match with_origin(state, call.clone()).await {
            Ok(item) => items.push(item),
            Err(e) => {
                tracing::debug!(error = %e, "could not say where a held call came from");
                items.push(call);
            }
        }
    }
    items
}

/// `GET /api/outbox`: what waits for the person's approval, newest first.
pub(super) async fn list_outbox(Scoped(state): Scoped, Query(q): Query<OutboxQuery>) -> Result<Json<Value>, AppError> {
    let status = match q.status.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Some("pending"),
        Some("all") => None,
        Some(s @ ("pending" | "sent" | "failed" | "discarded" | "sending")) => Some(s),
        Some(_) => return Err(AppError::BadRequest("status must be pending, sent, failed, discarded or all".into())),
    };
    let mut items = Vec::new();
    for held in state.cedar.held(status, None).await? {
        items.push(with_origin(&state, held).await?);
    }
    Ok(Json(json!({ "items": items })))
}

fn held_id(raw: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw).map_err(|_| AppError::NotFound("nothing in the outbox by that id"))
}

/// `GET /api/outbox/{id}`: one held call, whatever became of it.
pub(super) async fn get_outbox(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let held = state.cedar.held_call(held_id(&id)?).await?.ok_or(AppError::NotFound("nothing in the outbox by that id"))?;
    Ok(Json(with_origin(&state, held).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OutboxDecision {
    /// `approve` or `discard`.
    decision: String,
    /// For `approve`: the call's input as the person changed it. Absent sends it as written.
    input: Option<Value>,
}

/// `POST /api/outbox/{id}`: approve a held call, which makes it now (as written, or with the
/// `input` the person edited it into), or discard it. Answers with what became of it; a call the
/// tool refused is `failed` and can be approved again. The person's other tabs are told.
pub(super) async fn decide_outbox(Scoped(state): Scoped, Path(id): Path<String>, ApiJson(body): ApiJson<OutboxDecision>) -> Result<Json<Value>, AppError> {
    let id = held_id(&id)?;
    if !matches!(body.decision.as_str(), "approve" | "discard") {
        return Err(AppError::BadRequest("decision must be approve or discard".into()));
    }
    let mut decision = json!({ "decision": body.decision });
    if let Some(input) = body.input {
        decision["input"] = input;
    }
    let held = state.cedar.decide_held(id, &decision).await?;
    let _ = state.notices.send((state.user(), json!({ "type": "outbox.changed", "id": id, "status": held["status"] })));
    Ok(Json(with_origin(&state, held).await?))
}

// -- feedback on what couldn't be done ----------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LimitationBody {
    /// `send` or `dismiss`.
    decision: String,
    /// For `send`: the report as the person saw it on the card (and may have changed it).
    summary: Option<String>,
    category: Option<String>,
    needed: Option<String>,
    /// What the person adds in their own words.
    comment: Option<String>,
}

/// One line, trimmed, at most `max` characters; blank is `None`.
fn one_line(what: &str, value: Option<&str>, max: usize) -> Result<Option<String>, AppError> {
    let Some(value) = value else { return Ok(None) };
    let line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return Ok(None);
    }
    if line.chars().count() > max {
        return Err(AppError::BadRequest(format!("{what} is longer than {max} characters")));
    }
    Ok(Some(line))
}

/// `PUT /api/turns/{id}/limitations/{call_id}`: decide about something the model reported it
/// could not do in a turn (cedar's `report_limitation`). `send` keeps the report on this server
/// as feedback; `dismiss` keeps only that it was seen. The first decision stands, on every
/// device. Nothing leaves this server either way.
pub(super) async fn decide_limitation(
    Scoped(state): Scoped,
    Path((turn_id, call_id)): Path<(String, String)>,
    ApiJson(body): ApiJson<LimitationBody>,
) -> Result<Json<Value>, AppError> {
    let turn = find_turn(&state, &turn_id).await?;
    if call_id.is_empty() || call_id.len() > 200 {
        return Err(AppError::BadRequest("the report's id is not one a tool call has".into()));
    }
    if let Some(done) = state.store.get_feedback_of(turn.id, &call_id).await? {
        return Ok(Json(json!({ "feedback": done })));
    }
    let done = match body.decision.as_str() {
        "dismiss" => state.store.decide_feedback(turn.id, &call_id, None).await?,
        "send" => {
            let summary = one_line("summary", body.summary.as_deref(), MAX_SUMMARY_CHARS)?.ok_or_else(|| AppError::BadRequest("summary must not be empty".into()))?;
            let category = body.category.as_deref().map(str::trim).filter(|c| !c.is_empty()).unwrap_or("other");
            if !CATEGORIES.contains(&category) {
                return Err(AppError::BadRequest(format!("category must be one of {}", CATEGORIES.join(", "))));
            }
            let needed = one_line("needed", body.needed.as_deref(), MAX_NEEDED_CHARS)?;
            let comment = body.comment.as_deref().map(str::trim).filter(|c| !c.is_empty());
            if comment.is_some_and(|c| c.chars().count() > MAX_COMMENT_CHARS) {
                return Err(AppError::BadRequest(format!("comment is longer than {MAX_COMMENT_CHARS} characters")));
            }
            let new = NewFeedback { summary: &summary, category, needed: needed.as_deref(), comment };
            state.store.decide_feedback(turn.id, &call_id, Some(&new)).await?
        }
        other => return Err(AppError::BadRequest(format!("decision must be \"send\" or \"dismiss\", not {other:?}"))),
    };
    Ok(Json(json!({ "feedback": done })))
}

#[derive(Deserialize, Default)]
pub(super) struct FeedbackQuery {
    /// `all` lists every user's, for whoever runs the server (needs the admin token).
    users: Option<String>,
}

/// Whether the request asks for everyone's feedback, and may.
fn everyone(state: &AppState, headers: &HeaderMap, q: &FeedbackQuery) -> Result<bool, AppError> {
    match q.users.as_deref() {
        None => Ok(false),
        Some("all") => require_admin(state, headers).map(|()| true),
        Some(_) => Err(AppError::BadRequest("users must be \"all\"".into())),
    }
}

/// What the reports say most, so the gaps people run into most often come first.
fn by_category(feedback: &[Feedback]) -> Value {
    let mut counts: Vec<(&str, usize)> = CATEGORIES.iter().map(|c| (*c, feedback.iter().filter(|f| f.category.as_deref() == Some(*c)).count())).filter(|(_, n)| *n > 0).collect();
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    Value::Array(counts.into_iter().map(|(category, count)| json!({ "category": category, "count": count })).collect())
}

/// `GET /api/feedback`: the reports sent as feedback, newest first: one's own, or with
/// `?users=all` and the admin token everyone's on this server, each with who sent it.
pub(super) async fn list_feedback(Scoped(state): Scoped, headers: HeaderMap, Query(q): Query<FeedbackQuery>) -> Result<Json<Value>, AppError> {
    let everyone = everyone(&state, &headers, &q)?;
    let feedback = state.store.list_feedback(everyone, MAX_FEEDBACK_LISTED).await?;
    Ok(Json(json!({ "by_category": by_category(&feedback), "feedback": feedback })))
}

/// `DELETE /api/feedback/{id}`: take one report back (one's own; with `?users=all` and the admin
/// token, anyone's).
pub(super) async fn delete_feedback(Scoped(state): Scoped, headers: HeaderMap, Path(id): Path<String>, Query(q): Query<FeedbackQuery>) -> Result<StatusCode, AppError> {
    let everyone = everyone(&state, &headers, &q)?;
    let id = parse_id(&id, "feedback not found")?;
    if !state.store.delete_feedback(id, everyone).await? {
        return Err(AppError::NotFound("feedback not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}
