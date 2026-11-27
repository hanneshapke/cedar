//! `/api/timeline`: one day of what Cedar did, is doing and will do, as one river.
//!
//! The river is the home of the "Today" page: the day's turns (chats and agents' runs alike)
//! under **earlier**, what is going or waiting on the person under **now**, and the agents due
//! next under **ahead**, with the agents that keep watch under **watching**. What needs the person
//! (a plan to agree to, a call to approve, a question to answer) is counted in `needs_you`; it
//! stays under now whatever day it was asked, so it doesn't drop off the river at midnight.
//! So does what an unattended run (an agent's) held for the person's OK in the outbox: listed
//! under `outbox` with where it came from, and counted in `needs_you`.
//!
//! Nothing here is new data: it reads the turns, the waits `attention` keeps, and the agents.

use super::*;
use crate::store::{TimelineTurn, NEVER_MS};
use crate::trigger::Trigger;
use chrono::{DateTime, NaiveDate, TimeZone};
use chrono_tz::Tz;
use std::collections::HashSet;

/// How many of a day's turns the river shows, and how many under now at most.
const MAX_DAY_TURNS: u32 = 300;
const MAX_NOW_TURNS: u32 = 50;
/// How many upcoming agent runs ahead lists, and how far ahead it looks from today.
const MAX_AHEAD: usize = 20;
const AHEAD_DAYS: i64 = 7;
/// How many held calls the river lists under now.
const MAX_OUTBOX: usize = 50;
/// How long a turn's one-line outcome may be, in characters.
const SUMMARY_CHARS: usize = 240;

#[derive(Deserialize)]
pub(super) struct TimelineQuery {
    /// `YYYY-MM-DD`, in `timezone`. Absent: today.
    day: Option<String>,
    /// The device's IANA time zone. Absent: the one a message last sent, else UTC.
    timezone: Option<String>,
}

pub(super) async fn timeline(Scoped(state): Scoped, ApiQuery(query): ApiQuery<TimelineQuery>) -> Result<Json<Value>, AppError> {
    let zone = match agents::clean_zone(query.timezone.as_deref())? {
        Some(zone) => zone,
        None => state.store.preferences().await?.timezone.unwrap_or_else(|| "UTC".into()),
    };
    let tz: Tz = zone.parse().unwrap_or(chrono_tz::UTC);
    let now = Utc::now();
    let today = now.with_timezone(&tz).date_naive();
    let day = match query.day.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| AppError::BadRequest("day must be a date like 2026-03-13".into()))?,
        None => today,
    };
    let (start, end) = (day_start(day, tz), day_start(day.succ_opt().unwrap_or(day), tz));
    let is_today = day == today;

    // Now is today's: what is going or waits on the person, from whichever day.
    let now_turns = if is_today { state.store.timeline_now(MAX_NOW_TURNS).await? } else { Vec::new() };
    let in_now: HashSet<Uuid> = now_turns.iter().map(|t| t.turn_id).collect();
    let earlier: Vec<TimelineTurn> = state.store.timeline_between(start, end, MAX_DAY_TURNS).await?.into_iter().filter(|t| !in_now.contains(&t.turn_id)).collect();
    // The outbox waits on the person like a question does: today's, whenever it was held.
    let mut outbox = if is_today { handover::held_with_origin(&state, Some("pending"), None).await } else { Vec::new() };
    outbox.sort_by(|a, b| a["created_at"].as_str().cmp(&b["created_at"].as_str()));
    outbox.truncate(MAX_OUTBOX);
    let needs_you = now_turns.iter().filter(|t| !t.waits.is_empty() || t.needs_answer).count() + outbox.len();

    let schedules = state.store.list_schedules().await?;
    let mut ahead: Vec<&Schedule> = Vec::new();
    let mut watching: Vec<&Schedule> = Vec::new();
    if day >= today {
        let from = start.max(now);
        let until = if is_today { end + chrono::Duration::days(AHEAD_DAYS) } else { end };
        for s in schedules.iter().filter(|s| s.enabled) {
            let trigger = Trigger::of(s).ok();
            if trigger.as_ref().is_some_and(Trigger::by_event) {
                if is_today {
                    watching.push(s);
                }
            } else if s.next_run_at.timestamp_millis() < NEVER_MS && s.next_run_at >= from && s.next_run_at < until {
                ahead.push(s);
            }
        }
    }
    ahead.sort_by_key(|s| s.next_run_at);
    ahead.truncate(MAX_AHEAD);

    Ok(Json(json!({
        "day": day.format("%Y-%m-%d").to_string(),
        "timezone": tz.name(),
        "today": is_today,
        "generated_at": now,
        "needs_you": needs_you,
        "earlier": earlier.iter().map(turn_view).collect::<Vec<_>>(),
        "now": now_turns.iter().map(turn_view).collect::<Vec<_>>(),
        "outbox": outbox,
        "ahead": ahead.iter().map(|s| agent_view(s, true)).collect::<Vec<_>>(),
        "watching": watching.iter().map(|s| agent_view(s, false)).collect::<Vec<_>>(),
        "standing": schedules.iter().map(|s| agent_view(s, s.enabled)).collect::<Vec<_>>(),
    })))
}

/// When `day` begins in `tz`. A zone that skips midnight (a DST change at 00:00) begins the day
/// at the first moment that exists.
fn day_start(day: NaiveDate, tz: Tz) -> DateTime<Utc> {
    let midnight = day.and_hms_opt(0, 0, 0).expect("midnight exists");
    (0..=3)
        .find_map(|h| tz.from_local_datetime(&(midnight + chrono::Duration::hours(h))).earliest())
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&midnight))
}

/// Where a turn stands, in the river's words: `waiting` (on the person's go-ahead or OK),
/// `asking` (ended with a question), `working`, `done`, `failed` or `stopped`.
fn turn_state(t: &TimelineTurn) -> &'static str {
    if !t.waits.is_empty() {
        "waiting"
    } else if t.needs_answer {
        "asking"
    } else {
        match t.status.as_str() {
            "queued" | "running" => "working",
            "completed" => "done",
            "failed" => "failed",
            _ => "stopped",
        }
    }
}

fn turn_view(t: &TimelineTurn) -> Value {
    json!({
        "turn_id": t.turn_id,
        "conversation_id": t.conversation_id,
        "title": t.title,
        "prompt": t.prompt,
        "state": turn_state(t),
        "status": t.status,
        "stop_reason": t.stop_reason,
        "summary": t.final_text.as_deref().and_then(summary),
        "error": t.error,
        "created_at": t.created_at,
        "finished_at": t.finished_at,
        "seen": t.seen,
        "needs_answer": t.needs_answer,
        "agent": t.agent,
        "waits": t.waits,
    })
}

/// An agent as the river lists it. `due` adds when its next run is.
fn agent_view(s: &Schedule, due: bool) -> Value {
    let trigger = Trigger::of(s).ok();
    let next = (due && s.next_run_at.timestamp_millis() < NEVER_MS).then_some(s.next_run_at);
    json!({
        "id": s.id,
        "title": s.title,
        "enabled": s.enabled,
        "trigger_kind": s.trigger_kind,
        "trigger": trigger,
        "watches": trigger.as_ref().is_some_and(Trigger::by_event),
        "next_run_at": next,
        "last_run_at": s.last_run_at,
        "last_checked_at": s.last_checked_at,
    })
}

/// A response's outcome in one line: its first paragraph of prose, with the markdown taken out,
/// cut at a word to [`SUMMARY_CHARS`]. `None` for a response with no prose.
pub(crate) fn summary(text: &str) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    // A heading names the response; it is the summary only when nothing follows it.
    let mut heading: Option<String> = None;
    let mut in_code = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || line.starts_with('|') || line.starts_with("---") {
            continue;
        }
        if line.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        if line.starts_with('#') {
            if !lines.is_empty() {
                break;
            }
            heading.get_or_insert_with(|| line.trim_start_matches('#').trim().to_string());
            continue;
        }
        let line = line.trim_start_matches(['-', '*', '>', ' ']).trim();
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    if lines.is_empty() {
        lines.extend(heading);
    }
    let plain = strip_inline(&lines.join(" "));
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.is_empty() {
        return None;
    }
    Some(shorten(&plain, SUMMARY_CHARS))
}

/// Markdown links become their text; emphasis and code marks go.
fn strip_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' | '_' | '`' => i += 1,
            '[' => {
                // [text](url) -> text; [n] citations go.
                let close = chars[i..].iter().position(|&c| c == ']').map(|p| i + p);
                match close {
                    Some(c) if chars.get(c + 1) == Some(&'(') => {
                        out.extend(&chars[i + 1..c]);
                        i = chars[c..].iter().position(|&ch| ch == ')').map(|p| c + p + 1).unwrap_or(c + 1);
                    }
                    Some(c) if chars[i + 1..c].iter().all(|ch| ch.is_ascii_digit()) && c > i + 1 => {
                        // "rent [1]." reads "rent."; the space after a citation stays.
                        while out.ends_with(' ') {
                            out.pop();
                        }
                        i = c + 1;
                    }
                    _ => {
                        out.push('[');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// `s` cut at a word to at most `max` characters, with an ellipsis when cut.
pub(crate) fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max - 1).collect();
    let cut = match cut.rfind(' ') {
        Some(at) if at > max / 2 => &cut[..at],
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.', ' ']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_is_the_first_paragraph_in_plain_words() {
        let text = "## Lisbon\n\nFriday 13 to Sunday 15 March is **free**, and [it works](https://x.org)[1].\nFlights are `cheap`.\n\n- Day one\n- Day two";
        assert_eq!(summary(text).unwrap(), "Friday 13 to Sunday 15 March is free, and it works. Flights are cheap.");
        assert_eq!(summary("# Only a title"), Some("Only a title".into()));
        assert_eq!(summary("Rent is due [2]. Keys [3] go back."), Some("Rent is due. Keys go back.".into()));
        assert_eq!(summary("- first point\n- second"), Some("first point second".into()));
        assert_eq!(summary("```\ncode\n```\n\nAfter the code."), Some("After the code.".into()));
        assert_eq!(summary("   \n\n"), None);
        assert_eq!(summary("| a | b |\n|---|---|"), None);
    }

    #[test]
    fn a_long_summary_is_cut_at_a_word() {
        let long = "word ".repeat(100);
        let s = summary(&long).unwrap();
        assert!(s.chars().count() <= SUMMARY_CHARS, "{}", s.len());
        assert!(s.ends_with("word…"), "{s}");
        assert_eq!(shorten("short", 10), "short");
    }

    #[test]
    fn a_day_begins_at_local_midnight() {
        let zurich: Tz = "Europe/Zurich".parse().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 11, 27).unwrap();
        assert_eq!(day_start(day, zurich).to_rfc3339(), "2026-11-26T23:00:00+00:00");
        // Santiago skips from 00:00 to 01:00 when its summer time starts.
        let santiago: Tz = "America/Santiago".parse().unwrap();
        let skipped = NaiveDate::from_ymd_opt(2026, 9, 6).unwrap();
        assert_eq!(day_start(skipped, santiago).with_timezone(&santiago).format("%H:%M").to_string(), "01:00");
    }
}
