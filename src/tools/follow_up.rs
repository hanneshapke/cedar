use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{json, Value};

/// The tool's name. Clients draw its result as a suggestion the user can accept.
pub const NAME: &str = "suggest_follow_up";

const MAX_TITLE_CHARS: usize = 80;
const MAX_PROMPT_CHARS: usize = 4000;
/// A follow-up is at least this far off, so the user has time to see the suggestion ...
const MIN_AHEAD: Duration = Duration::minutes(1);
/// ... and at most this far.
const MAX_AHEAD: Duration = Duration::days(366);

/// What the model reads back.
const NOTE: &str = "The follow-up is shown to the user as a suggestion with a button to set it \
    up. Nothing is scheduled unless they accept it. Tell them in a sentence what you suggested \
    and when; don't say it is scheduled.";

/// Suggests running a prompt again later ("check this again on Friday").
///
/// The tool schedules nothing: cedar has no scheduler. It checks the suggestion and works out
/// its time, and the chat front end draws the result as a card with a button; accepting it
/// makes a one-off agent there. A page or an email the run read can therefore make the model
/// suggest something, but never start anything. The result is `{"follow_up": {"title",
/// "prompt", "at"}, "note"}`, `at` in UTC.
pub struct SuggestFollowUp;

#[async_trait]
impl Tool for SuggestFollowUp {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        "Suggest a follow-up run at a later time: when the user asks to be reminded, to have \
         something checked again later, or to be told when something happens that you can only \
         find out by looking again (\"check on Friday whether it shipped\"). Give a short title, \
         a prompt that stands on its own (the follow-up starts in a new conversation and sees \
         nothing of this one: name the things, addresses and numbers it needs, and say what to \
         report), and when: `in_minutes` from now, or `at` as an exact time with its UTC offset \
         if you know the current date and time for certain. The user sees the suggestion and \
         decides; nothing runs unless they accept it. To keep watching something, have the \
         follow-up's prompt say to suggest the next check itself if it isn't settled yet."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "title": { "type": "string",
                               "description": "A few words naming the follow-up, e.g. \"Has the package shipped?\"" },
                    "prompt": { "type": "string",
                                "description": "What the follow-up run should do, complete in itself." },
                    "in_minutes": { "type": "integer", "minimum": 1,
                                    "description": "Run this many minutes from now (1440 is a day, 10080 a week)." },
                    "at": { "type": "string",
                            "description": "Or the exact time, ISO 8601 with a UTC offset, e.g. 2026-12-04T09:00:00+01:00." }
                },
                "required": ["title", "prompt"] })
    }

    async fn call(&self, _ctx: &ToolContext, input: &Value) -> Result<String> {
        let follow_up = parse_input(input, Utc::now())?;
        Ok(json!({ "follow_up": follow_up, "note": NOTE }).to_string())
    }
}

fn text(value: &Value, what: &str, max: usize) -> Result<String> {
    let s = value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("give {what} as text"))?;
    if s.chars().count() > max {
        bail!("{what} is longer than {max} characters");
    }
    Ok(s.to_string())
}

/// The checked suggestion, with its time worked out from `now`.
fn parse_input(input: &Value, now: DateTime<Utc>) -> Result<Value> {
    let title = text(&input["title"], "title", MAX_TITLE_CHARS)?;
    let prompt = text(&input["prompt"], "prompt", MAX_PROMPT_CHARS)?;
    let at = match (input.get("in_minutes").filter(|v| !v.is_null()), input["at"].as_str()) {
        (Some(_), Some(_)) => bail!("give either in_minutes or at, not both"),
        (Some(minutes), None) => {
            let minutes = minutes
                .as_i64()
                .filter(|m| *m >= 1)
                .ok_or_else(|| anyhow!("in_minutes must be a whole number of minutes, at least 1"))?;
            now + Duration::minutes(minutes.min(MAX_AHEAD.num_minutes() + 1))
        }
        (None, Some(at)) => DateTime::parse_from_rfc3339(at.trim())
            .map_err(|_| {
                anyhow!("at must be a time with its UTC offset, like 2026-12-04T09:00:00+01:00; or use in_minutes")
            })?
            .with_timezone(&Utc),
        (None, None) => bail!("say when: in_minutes from now, or at an exact time"),
    };
    if at < now + MIN_AHEAD {
        bail!(
            "that time has passed or is less than a minute away (it is now {}); use in_minutes",
            now.to_rfc3339_opts(SecondsFormat::Secs, true)
        );
    }
    if at > now + MAX_AHEAD {
        bail!("a follow-up can be at most a year away");
    }
    Ok(json!({ "title": title, "prompt": prompt, "at": at.to_rfc3339_opts(SecondsFormat::Secs, true) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-12-01T08:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn a_suggestion_is_checked_and_its_time_worked_out() {
        let ok = parse_input(&json!({ "title": " Shipped? ", "prompt": "Check order 123.", "in_minutes": 90 }), now()).unwrap();
        assert_eq!(ok, json!({ "title": "Shipped?", "prompt": "Check order 123.", "at": "2026-12-01T09:30:00Z" }));
        let at = parse_input(&json!({ "title": "t", "prompt": "p", "at": "2026-12-04T09:00:00+01:00" }), now()).unwrap();
        assert_eq!(at["at"], "2026-12-04T08:00:00Z");

        let err = |input: Value| parse_input(&input, now()).unwrap_err().to_string();
        assert!(err(json!({ "prompt": "p", "in_minutes": 5 })).contains("title"));
        assert!(err(json!({ "title": "t", "prompt": " ", "in_minutes": 5 })).contains("prompt"));
        assert!(err(json!({ "title": "t", "prompt": "p" })).contains("say when"));
        assert!(err(json!({ "title": "t", "prompt": "p", "in_minutes": 5, "at": "2026-12-04T09:00:00Z" })).contains("not both"));
        assert!(err(json!({ "title": "t", "prompt": "p", "in_minutes": 0 })).contains("at least 1"));
        assert!(err(json!({ "title": "t", "prompt": "p", "at": "Friday" })).contains("UTC offset"));
        assert!(err(json!({ "title": "t", "prompt": "p", "at": "2026-11-30T09:00:00Z" })).contains("has passed"));
        assert!(err(json!({ "title": "t", "prompt": "p", "at": "2028-12-04T09:00:00Z" })).contains("a year"));
        assert!(err(json!({ "title": "t", "prompt": "p", "in_minutes": 99_999_999 })).contains("a year"));
        assert!(err(json!({ "title": "x".repeat(81), "prompt": "p", "in_minutes": 5 })).contains("longer than 80"));
    }
}
