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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
