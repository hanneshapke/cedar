//! `POST /api/asks`: one sentence and a When. The river's single way to ask for something.
//!
//! - `when: "now"` (the default) starts a new conversation with the sentence as its first
//!   message, as `POST /api/conversations` and `/messages` do together. `private: true` makes it
//!   an ask off the record (see `private`).
//! - `when: "later"`, `"every"` or `"watch"` makes an agent of it: the sentence is the agent's
//!   prompt, `trigger` says when it runs (see `trigger::Trigger`), and the kind of trigger must fit
//!   the When: `later` is a `once`; `every` is a clock (`time`, `interval`, `monthly`); `watch` is
//!   something that happens (a condition, mail, the calendar, records, a Drive folder, a webhook
//!   or the person's device). Nothing runs now; the agent's runs come into the river when due.

use super::*;
use crate::trigger::Trigger;
use agents::AgentBody;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AskBody {
    content: String,
    /// `now`, `later`, `every` or `watch`. Absent: `now`.
    when: Option<String>,
    /// What starts the agent's runs, for every When but `now`.
    trigger: Option<Value>,
    /// The agent's name. Absent: taken from the sentence.
    title: Option<String>,
    /// Off the record: only with `now`.
    #[serde(default)]
    private: bool,
    model: Option<String>,
    reasoning: Option<String>,
    plan: Option<String>,
    locale: Option<String>,
    timezone: Option<String>,
    /// Only with `now`, as for a message.
    attachments: Option<Vec<Attachment>>,
}

/// Which triggers fit a When.
fn fits(when: &str, trigger: &Trigger) -> bool {
    match when {
        "later" => trigger.kind() == "once",
        "every" => matches!(trigger.kind(), "time" | "interval" | "monthly"),
        "watch" => trigger.by_event(),
        _ => false,
    }
}

pub(super) async fn ask(Scoped(state): Scoped, ApiJson(body): ApiJson<AskBody>) -> Result<(StatusCode, Json<Value>), AppError> {
    let when = body.when.as_deref().map(str::trim).filter(|w| !w.is_empty()).unwrap_or("now").to_ascii_lowercase();
    let content = clean_message("content", &body.content)?;
    match when.as_str() {
        "now" => {
            if body.trigger.is_some() {
                return Err(AppError::BadRequest("a trigger is for an ask that runs later, every so often or on a change, not now".into()));
            }
            let title = clean_title(body.title)?;
            let message = NewMessage {
                content: &content,
                model: blank_is_none(body.model.as_deref()),
                reasoning: blank_is_none(body.reasoning.as_deref()),
                plan: clean_plan(body.plan)?,
                locale: i18n::clean_locale(body.locale.as_deref()).map_err(AppError::BadRequest)?,
                timezone: agents::clean_zone(body.timezone.as_deref())?,
                ..Default::default()
            };
            let (attachments, infos) = clean_attachments(body.attachments.unwrap_or_default())?;
            let message = NewMessage { attachments, infos, ..message };
            let conversation = match body.private {
                true => state.store.create_private_conversation(title.as_deref()).await?,
                false => state.store.create_conversation(title.as_deref()).await?,
            };
            let turn = match post_message(&state, ConversationRef::Found(conversation.id), message).await {
                Ok(turn) => turn,
                Err(e) => {
                    // Nothing was asked: no empty conversation is left behind.
                    state.store.delete_conversation(conversation.id).await?;
                    return Err(e);
                }
            };
            if !conversation.private {
                crate::attention::watch(state.clone(), turn.clone());
            }
            let conversation = state.store.get_conversation(conversation.id).await?.unwrap_or(conversation);
            Ok((StatusCode::CREATED, Json(json!({ "when": "now", "conversation": conversation, "turn": turn }))))
        }
        "later" | "every" | "watch" => {
            if body.private {
                return Err(AppError::BadRequest("an ask off the record runs now only: nothing of it is kept for later".into()));
            }
            if body.attachments.as_ref().is_some_and(|a| !a.is_empty()) {
                return Err(AppError::BadRequest("files go with an ask that runs now".into()));
            }
            let raw = body.trigger.ok_or_else(|| AppError::BadRequest(format!("trigger is required for when: {when}")))?;
            let trigger = Trigger::parse(raw.clone()).map_err(AppError::BadRequest)?;
            if !fits(&when, &trigger) {
                return Err(AppError::BadRequest(format!("a {} trigger doesn't fit when: {when}", trigger.kind())));
            }
            let title = match clean_title(body.title)? {
                Some(title) => title,
                None => crate::store::title_from(&content),
            };
            let timezone = agents::clean_zone(body.timezone.as_deref())?;
            let agent = agents::create_agent(
                &state,
                AgentBody::for_ask(title, content, raw, timezone, blank_is_none(body.model.as_deref()), blank_is_none(body.reasoning.as_deref())),
            )
            .await?;
            Ok((StatusCode::CREATED, Json(json!({ "when": when, "agent": agent }))))
        }
        other => Err(AppError::BadRequest(format!("when must be now, later, every or watch, not {other:?}"))),
    }
}

// -- reading the When out of the sentence --------------------------------------------------------

/// How long the model may take to read a sentence, and how often its run is looked at.
const DRAFT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const DRAFT_POLL: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DraftBody {
    content: String,
    timezone: Option<String>,
}

/// `POST /api/asks/draft`: what When a sentence asks for, read by the model: `{when, trigger,
/// content}`, where `content` is the sentence without its timing. Nothing is saved or started;
/// the page shows the reading and the person sends it (as `POST /api/asks`) or changes it.
/// A sentence the model can't place, or an answer that isn't a trigger cedar-chat takes, is
/// `when: "now"` with the sentence as it was. The reading is a private run of one model call
/// with no tools: nothing of it is remembered, and the run is deleted afterwards.
pub(super) async fn draft(Scoped(state): Scoped, ApiJson(body): ApiJson<DraftBody>) -> Result<Json<Value>, AppError> {
    let content = clean_message("content", &body.content)?;
    let zone = match agents::clean_zone(body.timezone.as_deref())? {
        Some(zone) => zone,
        None => state.store.preferences().await?.timezone.unwrap_or_else(|| "UTC".into()),
    };
    let tz: chrono_tz::Tz = zone.parse().unwrap_or(chrono_tz::UTC);
    let now = Utc::now().with_timezone(&tz).format("%Y-%m-%dT%H:%M, a %A").to_string();
    let no_tools: [String; 0] = [];
    let options = RunOptions { tools: Some(&no_tools), max_turns: Some(1), plan: Some("off"), private: true, timezone: Some(&zone), ..Default::default() };
    let run = state.cedar.create_run(&draft_prompt(&content, &zone, &now), &options).await?;

    let deadline = tokio::time::Instant::now() + DRAFT_TIMEOUT;
    let mut text = run.final_text.clone().filter(|_| cedar::is_terminal(&run.status));
    let mut done = cedar::is_terminal(&run.status);
    while !done && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(DRAFT_POLL).await;
        match state.cedar.get_run(run.id).await? {
            Some(r) if cedar::is_terminal(&r.status) => {
                text = r.final_text;
                done = true;
            }
            Some(_) => {}
            None => done = true,
        }
    }
    if !done {
        let _ = state.cedar.cancel_run(run.id).await;
    }
    // The reading is nobody's conversation: it isn't kept.
    if let Err(e) = state.cedar.delete_run(run.id).await {
        tracing::debug!(run_id = %run.id, error = %e, "could not delete the run that read an ask's When");
    }

    Ok(Json(match text.as_deref().and_then(|t| read_draft(t, &zone)) {
        Some((when, trigger, task)) => json!({ "when": when, "trigger": trigger, "content": task.unwrap_or(content) }),
        None => json!({ "when": "now", "trigger": null, "content": content }),
    }))
}

/// What the model is asked. The shapes are `trigger::Trigger`'s, without the time zone, which is
/// filled in from the device's.
fn draft_prompt(content: &str, zone: &str, now: &str) -> String {
    format!(
        "Read the request below and decide when it should run. Do not carry it out. Reply with JSON only, no other text:\n\
         {{\"when\": \"now\" | \"later\" | \"every\" | \"watch\", \"trigger\": ..., \"task\": \"the request without its timing, in the request's own language\"}}\n\
         - now: no time or repetition is asked for. trigger is null.\n\
         - later: once, at a time. trigger: {{\"kind\": \"once\", \"at\": \"YYYY-MM-DDTHH:MM\"}} in local time.\n\
         - every: it repeats. trigger is one of {{\"kind\": \"time\", \"frequency\": \"daily\" | \"weekdays\" | \"weekly\", \"weekday\": 0 to 6 with 0 = Monday (weekly only), \"time\": \"HH:MM\"}}, \
         {{\"kind\": \"monthly\", \"day\": 1 to 31, \"time\": \"HH:MM\"}}, {{\"kind\": \"interval\", \"every_minutes\": 15 or more}}.\n\
         - watch: when something happens. trigger: {{\"kind\": \"email\", \"query\": \"a Gmail search\"}} for mail that arrives, or {{\"kind\": \"calendar\", \"minutes_before\": N}} before calendar events.\n\
         When unsure, answer now. It is {now} in {zone}.\n\nRequest: {content}"
    )
}

/// The model's reading, checked: `(when, trigger, task)`. `None` when it isn't JSON of that
/// shape, or the trigger is not one that fits the When.
fn read_draft(text: &str, zone: &str) -> Option<(String, Value, Option<String>)> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    let v: Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let when = v["when"].as_str()?.trim().to_ascii_lowercase();
    let task = v["task"].as_str().map(str::trim).filter(|t| !t.is_empty() && t.len() <= MAX_MESSAGE_BYTES).map(String::from);
    if when == "now" {
        return Some((when, Value::Null, task));
    }
    let mut raw = v.get("trigger")?.clone();
    // The times are the device's: the model isn't asked for a zone.
    if matches!(raw["kind"].as_str(), Some("once" | "time" | "monthly")) {
        raw["timezone"] = json!(zone);
    }
    let trigger = Trigger::parse(raw.clone()).ok()?;
    // Only what the model was offered: not a webhook, a condition or a device trigger.
    let offered = matches!(trigger.kind(), "once" | "time" | "monthly" | "interval" | "email" | "calendar");
    if !offered || !fits(&when, &trigger) {
        return None;
    }
    // A one-off in the past can't be made; the person picks the time.
    if trigger.kind() == "once" && trigger.next_after(Utc::now()).is_none() {
        return None;
    }
    Some((when, serde_json::to_value(&trigger).ok()?, task))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_reading_is_taken_only_when_it_is_a_trigger_that_fits() {
        let (when, trigger, task) = read_draft("```json\n{\"when\": \"every\", \"trigger\": {\"kind\": \"time\", \"frequency\": \"weekdays\", \"time\": \"7:00\"}, \"task\": \"Brief me\"}\n```", "Europe/Zurich").unwrap();
        assert_eq!((when.as_str(), task.as_deref()), ("every", Some("Brief me")));
        assert_eq!(trigger, json!({ "kind": "time", "frequency": "weekdays", "time": "07:00", "timezone": "Europe/Zurich" }));
        assert_eq!(read_draft("{\"when\": \"now\", \"trigger\": null}", "UTC"), Some(("now".into(), Value::Null, None)));
        let mail = read_draft("{\"when\": \"watch\", \"trigger\": {\"kind\": \"email\", \"query\": \"from:bank\"}}", "UTC").unwrap();
        assert_eq!(mail.1["kind"], "email");
        // Not JSON, a trigger that doesn't fit, one not offered, and a one-off in the past.
        assert_eq!(read_draft("Every weekday, I think.", "UTC"), None);
        assert_eq!(read_draft("{\"when\": \"later\", \"trigger\": {\"kind\": \"time\", \"frequency\": \"daily\", \"time\": \"07:00\"}}", "UTC"), None);
        assert_eq!(read_draft("{\"when\": \"watch\", \"trigger\": {\"kind\": \"webhook\"}}", "UTC"), None);
        assert_eq!(read_draft("{\"when\": \"later\", \"trigger\": {\"kind\": \"once\", \"at\": \"2001-01-01T09:00\"}}", "UTC"), None);
    }

    #[test]
    fn a_when_takes_only_the_triggers_that_fit_it() {
        let once = Trigger::parse(json!({ "kind": "once", "at": "2026-12-03T09:00", "timezone": "UTC" })).unwrap();
        let daily = Trigger::parse(json!({ "kind": "time", "frequency": "daily", "time": "07:00", "timezone": "UTC" })).unwrap();
        let mail = Trigger::parse(json!({ "kind": "email", "query": "from:bank" })).unwrap();
        assert!(fits("later", &once) && !fits("later", &daily));
        assert!(fits("every", &daily) && !fits("every", &mail) && !fits("every", &once));
        assert!(fits("watch", &mail) && !fits("watch", &daily));
        assert!(!fits("now", &once));
    }
}
