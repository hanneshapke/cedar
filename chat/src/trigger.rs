//! What starts an agent's run.
//!
//! An agent is a prompt with a trigger. Some triggers are a clock ([`Trigger::Time`],
//! [`Trigger::Once`], [`Trigger::Interval`], [`Trigger::Monthly`]): the run starts when the time
//! comes. Some watch ([`Trigger::Condition`], [`Trigger::Email`], [`Trigger::Calendar`]): every
//! few minutes the scheduler asks cedar for one tool's result, with no model involved (see
//! `poll`), and [`Trigger::evaluate`] decides from that and from what the last check saw whether
//! a run starts. And [`Trigger::Webhook`] waits to be called (see `api::hooks`).
//!
//! Everything here is plain data in and out, so it can be tested without a server: what a
//! trigger is and whether it is valid, when it is next due, what to ask cedar, and what an
//! answer means.

use crate::schedule::Timing;
use crate::store::Schedule;
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// An interval trigger starts a run each time, so it can't be more often than this.
pub const MIN_INTERVAL_MINUTES: u32 = 15;
/// The longest interval, and the longest time between two checks.
pub const MAX_EVERY_MINUTES: u32 = 7 * 24 * 60;
/// How often a watching trigger checks unless it says.
pub const DEFAULT_CHECK_MINUTES: u32 = 5;
/// A personal tool starts containers for every check, so it is checked no more often than this.
pub const MIN_PERSONAL_CHECK_MINUTES: u32 = 5;
/// An event starts a run at most this often unless the agent says ...
pub const DEFAULT_MIN_INTERVAL_SECS: u32 = 300;
/// ... and never more often than this.
pub const MIN_MIN_INTERVAL_SECS: u32 = 60;
/// Runs events may start in any 24 hours, unless the agent says.
pub const DEFAULT_MAX_RUNS_PER_DAY: u32 = 24;
pub const MAX_MAX_RUNS_PER_DAY: u32 = 500;
/// The mail threads a trigger remembers having seen.
const MAX_SEEN_THREADS: usize = 500;
/// The calendar events a trigger remembers.
const MAX_KNOWN_EVENTS: usize = 2000;
/// How far ahead a calendar trigger looks for new events.
const NEW_EVENTS_HORIZON_DAYS: i64 = 60;
/// A watched value is kept as it is up to this length, and as a digest beyond.
const MAX_KEPT_VALUE: usize = 200;
const MAX_QUERY_CHARS: usize = 500;
const MAX_URL_CHARS: usize = 2000;

fn default_check() -> u32 {
    DEFAULT_CHECK_MINUTES
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// Every day, on weekdays, or weekly on `weekday` (0 = Monday), at `time` in `timezone`.
    Time {
        frequency: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weekday: Option<u8>,
        time: String,
        timezone: String,
    },
    /// One run at `at` (`2026-12-03T09:00`, local to `timezone`). The agent is switched off after.
    Once { at: String, timezone: String },
    /// A run every `every_minutes`.
    Interval { every_minutes: u32 },
    /// On `day` of every month (the last day in a shorter month) at `time` in `timezone`.
    Monthly { day: u8, time: String, timezone: String },
    /// A run when what `source` returns passes `predicate`.
    Condition {
        source: Source,
        /// Where in a JSON result the value is: a JSON pointer (`/price/eur`) or a dotted path
        /// (`price.eur`). Without it, the whole result is the value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extract: Option<String>,
        predicate: Predicate,
        #[serde(default = "default_check")]
        every_minutes: u32,
    },
    /// A run when `POST /api/hooks/{hook_id}` is called with the agent's secret.
    Webhook {},
    /// A run when mail matching `query` (Gmail's search syntax) arrives.
    Email {
        query: String,
        #[serde(default = "default_check")]
        every_minutes: u32,
    },
    /// A run `minutes_before` an event starts, and/or when a new event appears (`new_events`),
    /// for events on `calendar_id` (the main calendar without) that match `filter`.
    Calendar {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        calendar_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minutes_before: Option<u32>,
        #[serde(default)]
        new_events: bool,
        #[serde(default = "default_check")]
        every_minutes: u32,
    },
}

/// What a condition watches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// A web address: a JSON API's reply (`as: "json"`) or a page's text (`as: "text"`).
    Url {
        url: String,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// One of cedar's tools that can be called outside a run, with this input.
    Tool {
        name: String,
        #[serde(default)]
        input: Value,
    },
    /// One of the record sets cedar keeps: `{count, updated_at, records}`.
    Records { set: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// The value differs from the last check's.
    Changes,
    Gt,
    Lt,
    Gte,
    Lte,
    Equals,
    Contains,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Changes => "changes",
            Op::Gt => "gt",
            Op::Lt => "lt",
            Op::Gte => "gte",
            Op::Lte => "lte",
            Op::Equals => "equals",
            Op::Contains => "contains",
        }
    }
}

/// What to ask cedar for a check.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// `POST /v1/tools/{name}/call`.
    Tool { name: String, input: Value },
    /// `GET /v1/records/{set}`.
    Records { set: String },
}

/// What a check returned: the text, and the same as JSON when it is JSON.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Observation {
    pub text: String,
    pub json: Option<Value>,
}

impl Observation {
    pub fn of_json(value: Value) -> Self {
        Self { text: value.to_string(), json: Some(value) }
    }

    pub fn of_text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self { json: serde_json::from_str(&text).ok(), text }
    }
}

/// What happened, for the run's record and the note in its prompt. `i18n::event_summary` puts it
/// into words.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The clock: the time came.
    Clock,
    /// A watched value is no longer what it was.
    Changed { from: String, to: String },
    /// A watched value passes its test.
    Holds { value: String, op: Op, target: String },
    Webhook { bytes: usize, dropped: u32 },
    Mail { count: usize, from: String, subject: String },
    /// An event starts in `minutes`.
    Soon { title: String, minutes: i64, count: usize },
    NewEvent { title: String, count: usize },
}

/// What a check came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// `Some` if a run should start.
    pub fire: Option<Event>,
    /// What the run is told, as data: the new mail, the event, the value.
    pub data: Option<String>,
    /// What to remember for the next check. Only kept if the run starts or none was due, so a
    /// run that has to wait is decided again from the same starting point.
    pub state: Value,
    /// What was seen, for a person: the value, or how much matched.
    pub shown: String,
}

impl Trigger {
    /// The trigger of a stored agent: its `trigger` JSON, or the schedule columns from before
    /// agents had other triggers.
    pub fn of(schedule: &Schedule) -> Result<Self, String> {
        match &schedule.trigger {
            Some(value) => serde_json::from_value(value.clone()).map_err(|e| format!("unreadable trigger: {e}")),
            None => Ok(Trigger::Time {
                frequency: schedule.frequency.clone(),
                weekday: schedule.weekday,
                time: schedule.time_of_day.clone(),
                timezone: schedule.timezone.clone(),
            }),
        }
    }

    /// Read a client's trigger and check it. The error is a message for the client.
    pub fn parse(value: Value) -> Result<Self, String> {
        let trigger: Trigger = serde_json::from_value(value).map_err(|e| format!("trigger: {e}"))?;
        trigger.checked()
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Trigger::Time { .. } => "time",
            Trigger::Once { .. } => "once",
            Trigger::Interval { .. } => "interval",
            Trigger::Monthly { .. } => "monthly",
            Trigger::Condition { .. } => "condition",
            Trigger::Webhook {} => "webhook",
            Trigger::Email { .. } => "email",
            Trigger::Calendar { .. } => "calendar",
        }
    }

    /// Whether the scheduler checks something for it between runs.
    pub fn watches(&self) -> bool {
        matches!(self, Trigger::Condition { .. } | Trigger::Email { .. } | Trigger::Calendar { .. })
    }

    /// Whether something outside starts its runs (a value, mail, an event, a call) rather than
    /// the clock: these get the limits on how often, and the careful choice of tools.
    pub fn by_event(&self) -> bool {
        self.watches() || matches!(self, Trigger::Webhook {})
    }

    /// The time zone its times are in, for the triggers that have one.
    pub fn timezone(&self) -> Option<&str> {
        match self {
            Trigger::Time { timezone, .. } | Trigger::Once { timezone, .. } | Trigger::Monthly { timezone, .. } => Some(timezone),
            _ => None,
        }
    }

    /// How often it checks, for the triggers that watch.
    pub fn every(&self) -> Option<Duration> {
        match self {
            Trigger::Condition { every_minutes, .. } | Trigger::Email { every_minutes, .. } | Trigger::Calendar { every_minutes, .. } => {
                Some(Duration::minutes(i64::from(*every_minutes)))
            }
            _ => None,
        }
    }

    /// The trigger with its strings trimmed, or why it isn't valid.
    pub fn checked(self) -> Result<Self, String> {
        let every = |n: u32, least: u32, what: &str| match n {
            n if n < least => Err(format!("{what} must be at least {least} minutes")),
            n if n > MAX_EVERY_MINUTES => Err(format!("{what} must be at most {MAX_EVERY_MINUTES} minutes (a week)")),
            n => Ok(n),
        };
        Ok(match self {
            Trigger::Time { frequency, weekday, time, timezone } => {
                let t = Timing::parse(&frequency, weekday, &time, &timezone)?;
                Trigger::Time {
                    frequency: t.frequency.as_str().to_string(),
                    weekday: t.weekday.map(|d| d.num_days_from_monday() as u8),
                    time: t.time.format("%H:%M").to_string(),
                    timezone: t.tz.name().to_string(),
                }
            }
            Trigger::Once { at, timezone } => {
                let tz = zone(&timezone)?;
                let at = local_time(&at)?;
                Trigger::Once { at: at.format("%Y-%m-%dT%H:%M").to_string(), timezone: tz.name().to_string() }
            }
            Trigger::Interval { every_minutes } => Trigger::Interval { every_minutes: every(every_minutes, MIN_INTERVAL_MINUTES, "every_minutes")? },
            Trigger::Monthly { day, time, timezone } => {
                if !(1..=31).contains(&day) {
                    return Err("day must be between 1 and 31".into());
                }
                let tz = zone(&timezone)?;
                Trigger::Monthly { day, time: clock(&time)?.format("%H:%M").to_string(), timezone: tz.name().to_string() }
            }
            Trigger::Condition { source, extract, predicate, every_minutes } => {
                let source = match source {
                    Source::Url { url, format } => {
                        let url = url.trim().to_string();
                        if !(url.starts_with("https://") || url.starts_with("http://")) {
                            return Err("url must start with https:// or http://".into());
                        }
                        if url.chars().count() > MAX_URL_CHARS {
                            return Err(format!("url is longer than {MAX_URL_CHARS} characters"));
                        }
                        if let Some(f) = format.as_deref() {
                            if f != "json" && f != "text" {
                                return Err(format!("as must be \"json\" or \"text\", not {f:?}"));
                            }
                        }
                        Source::Url { url, format }
                    }
                    Source::Tool { name, input } => {
                        let name = name.trim().to_string();
                        if name.is_empty() || name.len() > 100 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                            return Err("source.name must be a tool's name".into());
                        }
                        let input = if input.is_null() { json!({}) } else { input };
                        if !input.is_object() {
                            return Err("source.input must be an object".into());
                        }
                        if input.to_string().len() > 4000 {
                            return Err("source.input is longer than 4000 bytes".into());
                        }
                        Source::Tool { name, input }
                    }
                    Source::Records { set } => {
                        let set = set.trim().to_string();
                        if set.is_empty() || set.len() > 100 {
                            return Err("source.set must name a record set".into());
                        }
                        Source::Records { set }
                    }
                };
                let extract = extract.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
                if extract.as_deref().is_some_and(|p| p.len() > 500) {
                    return Err("extract is longer than 500 characters".into());
                }
                let predicate = match predicate.op {
                    Op::Changes => Predicate { op: Op::Changes, value: None },
                    op @ (Op::Gt | Op::Lt | Op::Gte | Op::Lte) => {
                        let n = predicate.value.as_ref().and_then(number).ok_or("predicate.value must be a number for this comparison")?;
                        Predicate { op, value: Some(json!(n)) }
                    }
                    op @ (Op::Equals | Op::Contains) => {
                        let v = predicate.value.filter(|v| !v.is_null() && !v.is_object() && !v.is_array()).ok_or("predicate.value is required: a text, a number, or true or false")?;
                        if plain(&v).chars().count() > 500 {
                            return Err("predicate.value is longer than 500 characters".into());
                        }
                        Predicate { op, value: Some(v) }
                    }
                };
                Trigger::Condition { source, extract, predicate, every_minutes: every(every_minutes, 1, "every_minutes")? }
            }
            Trigger::Webhook {} => Trigger::Webhook {},
            Trigger::Email { query, every_minutes } => {
                let query = query.trim().to_string();
                if query.chars().count() > MAX_QUERY_CHARS {
                    return Err(format!("query is longer than {MAX_QUERY_CHARS} characters"));
                }
                Trigger::Email { query, every_minutes: every(every_minutes, 1, "every_minutes")? }
            }
            Trigger::Calendar { calendar_id, filter, minutes_before, new_events, every_minutes } => {
                let trimmed = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                let (calendar_id, filter) = (trimmed(calendar_id), trimmed(filter));
                if filter.as_deref().is_some_and(|f| f.chars().count() > MAX_QUERY_CHARS) || calendar_id.as_deref().is_some_and(|c| c.len() > MAX_QUERY_CHARS) {
                    return Err(format!("filter and calendar_id may be at most {MAX_QUERY_CHARS} characters"));
                }
                if minutes_before.is_none() && !new_events {
                    return Err("a calendar trigger needs minutes_before, new_events, or both".into());
                }
                if minutes_before.is_some_and(|m| m > MAX_EVERY_MINUTES) {
                    return Err(format!("minutes_before must be at most {MAX_EVERY_MINUTES} (a week)"));
                }
                Trigger::Calendar { calendar_id, filter, minutes_before, new_events, every_minutes: every(every_minutes, 1, "every_minutes")? }
            }
        })
    }

    /// When the scheduler should look at the agent next, after `after`: its next run for a
    /// clock, its next check for one that watches. `None` when nothing is due any more (a one-off
    /// in the past) or ever by itself (a webhook).
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Trigger::Time { frequency, weekday, time, timezone } => Timing::parse(frequency, *weekday, time, timezone).ok().map(|t| t.next_after(after)),
            Trigger::Once { at, timezone } => {
                let at = resolve(zone(timezone).ok()?, local_time(at).ok()?);
                (at > after).then_some(at)
            }
            Trigger::Interval { every_minutes } => Some(after + Duration::minutes(i64::from(*every_minutes))),
            Trigger::Monthly { day, time, timezone } => {
                let (tz, time) = (zone(timezone).ok()?, clock(time).ok()?);
                let today = after.with_timezone(&tz).date_naive();
                let (mut year, mut month) = (today.year(), today.month());
                // This month's day may have passed; next month's hasn't.
                for _ in 0..3 {
                    let date = NaiveDate::from_ymd_opt(year, month, u32::from(*day).min(days_in(year, month)))?;
                    let at = resolve(tz, date.and_time(time));
                    if at > after {
                        return Some(at);
                    }
                    (year, month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
                }
                None
            }
            Trigger::Condition { .. } | Trigger::Email { .. } | Trigger::Calendar { .. } => Some(after + self.every()?),
            Trigger::Webhook {} => None,
        }
    }

    /// What to ask cedar for a check at `now`. `None` for the triggers that don't watch.
    pub fn request(&self, now: DateTime<Utc>) -> Option<Request> {
        Some(match self {
            Trigger::Condition { source, .. } => match source {
                Source::Url { url, format } => {
                    let mut input = json!({ "url": url });
                    if let Some(f) = format {
                        input["as"] = json!(f);
                    }
                    Request::Tool { name: "url".into(), input }
                }
                Source::Tool { name, input } => Request::Tool { name: name.clone(), input: input.clone() },
                Source::Records { set } => Request::Records { set: set.clone() },
            },
            Trigger::Email { query, .. } => Request::Tool { name: "gmail_search_threads".into(), input: json!({ "query": query, "pageSize": 25 }) },
            Trigger::Calendar { calendar_id, filter, minutes_before, new_events, every_minutes } => {
                // Far enough to see an event before its lead time begins, even if a check is late.
                let lead = i64::from(minutes_before.unwrap_or(0)) + 2 * i64::from(*every_minutes) + 5;
                let ahead = if *new_events { Duration::days(NEW_EVENTS_HORIZON_DAYS).max(Duration::minutes(lead)) } else { Duration::minutes(lead) };
                let mut input = json!({ "startTime": now.to_rfc3339(), "endTime": (now + ahead).to_rfc3339(), "pageSize": 250 });
                if let Some(id) = calendar_id {
                    input["calendarId"] = json!(id);
                }
                if let Some(f) = filter {
                    input["fullText"] = json!(f);
                }
                Request::Tool { name: "calendar_list_events".into(), input }
            }
            _ => return None,
        })
    }

    /// Decide from what a check returned, and what the last one left in `state`, whether a run
    /// starts. The error is why the answer couldn't be read (nothing at the path, not a number).
    pub fn evaluate(&self, state: Option<&Value>, seen: &Observation, now: DateTime<Utc>) -> Result<Outcome, String> {
        match self {
            Trigger::Condition { extract, predicate, .. } => condition(extract.as_deref(), predicate, state, seen),
            Trigger::Email { .. } => mail(state, seen),
            Trigger::Calendar { minutes_before, new_events, .. } => calendar(*minutes_before, *new_events, state, seen, now),
            _ => Err("this trigger doesn't watch anything".into()),
        }
    }
}

fn zone(name: &str) -> Result<Tz, String> {
    name.trim().parse().map_err(|_| format!("unknown time zone {name:?}"))
}

fn clock(time: &str) -> Result<NaiveTime, String> {
    NaiveTime::parse_from_str(time.trim(), "%H:%M").map_err(|_| format!("time must look like 09:30, not {time:?}"))
}

fn local_time(at: &str) -> Result<NaiveDateTime, String> {
    let at = at.trim();
    NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|_| format!("at must look like 2026-12-03T09:00, not {at:?}"))
}

/// The instant a local time means. A time a daylight-saving change skips becomes the first one
/// after the gap; one that happens twice, the first.
fn resolve(tz: Tz, local: NaiveDateTime) -> DateTime<Utc> {
    tz.from_local_datetime(&local)
        .earliest()
        .unwrap_or_else(|| tz.from_local_datetime(&(local + Duration::hours(1))).earliest().expect("an hour past a gap is valid"))
        .with_timezone(&Utc)
}

fn days_in(year: i32, month: u32) -> u32 {
    let (y, m) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).and_then(|d| d.pred_opt()).map_or(28, |d| d.day())
}

/// A value as a number: a JSON number, or text that is one (`"41.5"`, `" 17 "`).
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok().filter(|n: &f64| n.is_finite()),
        _ => None,
    }
}

/// A value as a person would write it: text without its quotes, anything else as JSON.
fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        // 40, not 40.0: a whole number reads as one however it was stored.
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.0}"),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

fn shorten(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

/// A JSON pointer from what people type: `/a/0/b` as it is, `a.0.b` and `a[0].b` converted.
fn pointer(path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    path.replace('[', ".")
        .replace(']', "")
        .split('.')
        .filter(|part| !part.is_empty())
        .map(|part| format!("/{}", part.replace('~', "~0").replace('/', "~1")))
        .collect()
}

fn condition(extract: Option<&str>, predicate: &Predicate, state: Option<&Value>, seen: &Observation) -> Result<Outcome, String> {
    let value = match extract {
        Some(path) => {
            let json = seen.json.as_ref().ok_or("the result is not JSON, so there is no path to follow in it")?;
            json.pointer(&pointer(path)).cloned().ok_or_else(|| format!("nothing at {path:?} in the result"))?
        }
        None => seen.json.clone().unwrap_or_else(|| Value::String(seen.text.clone())),
    };
    let text = plain(&value);
    let shown = shorten(&text, MAX_KEPT_VALUE);
    // What is compared with next time: the value itself while it is short enough to read.
    let kept = if text.chars().count() > MAX_KEPT_VALUE { hex::encode(Sha256::digest(text.as_bytes())) } else { text.clone() };
    let first = state.is_none();
    let (fire, armed) = match predicate.op {
        Op::Changes => {
            let before = state.and_then(|s| s["last"].as_str());
            let fire = match before {
                // The first check only learns what the value is.
                None => None,
                Some(b) if b == kept => None,
                Some(_) => Some(Event::Changed { from: state.and_then(|s| s["shown"].as_str()).unwrap_or_default().to_string(), to: shown.clone() }),
            };
            (fire, true)
        }
        op => {
            let target = predicate.value.clone().unwrap_or(Value::Null);
            let holds = match op {
                Op::Equals => match (number(&value), number(&target)) {
                    (Some(a), Some(b)) => a == b,
                    _ => text == plain(&target),
                },
                Op::Contains => text.to_lowercase().contains(&plain(&target).to_lowercase()),
                _ => {
                    let n = number(&value).ok_or_else(|| format!("{shown:?} is not a number, so it can't be compared"))?;
                    let t = number(&target).ok_or("the trigger has no number to compare with")?;
                    match op {
                        Op::Gt => n > t,
                        Op::Lt => n < t,
                        Op::Gte => n >= t,
                        _ => n <= t,
                    }
                }
            };
            // Once it has started a run, it has to stop holding before it can start another.
            let armed = first || state.is_some_and(|s| s["armed"].as_bool().unwrap_or(true));
            let fire = (holds && armed).then(|| Event::Holds { value: shown.clone(), op, target: plain(&target) });
            (fire, !holds)
        }
    };
    let data = fire.as_ref().map(|event| match event {
        Event::Changed { from, .. } => format!("Value now:\n{text}\n\nValue at the last check:\n{from}"),
        _ => format!("Value now:\n{text}"),
    });
    Ok(Outcome { fire, data, state: json!({ "last": kept, "shown": shown, "armed": armed }), shown })
}

fn mail(state: Option<&Value>, seen: &Observation) -> Result<Outcome, String> {
    let threads = seen.json.as_ref().and_then(|j| j["threads"].as_array()).ok_or("the mail search returned something unexpected")?;
    // [thread id, message count], most recently seen first.
    let mut known: Vec<(String, u64)> = state
        .and_then(|s| s["seen"].as_array())
        .map(|rows| rows.iter().filter_map(|r| Some((r[0].as_str()?.to_string(), r[1].as_u64()?))).collect())
        .unwrap_or_default();
    let mut fresh = Vec::new();
    for thread in threads.iter().rev() {
        let Some(id) = thread["id"].as_str() else { continue };
        let count = thread["messageCount"].as_u64().unwrap_or(1);
        let before = known.iter().position(|(k, _)| k == id);
        // New mail is a thread not seen before, or one with more messages than it had.
        if before.is_none_or(|i| known[i].1 < count) {
            fresh.push(thread);
        }
        if let Some(i) = before {
            known.remove(i);
        }
        known.insert(0, (id.to_string(), count));
    }
    known.truncate(MAX_SEEN_THREADS);
    fresh.reverse();
    let next = json!({ "seen": known.iter().map(|(id, n)| json!([id, n])).collect::<Vec<_>>() });
    let shown = format!("{}", threads.len());
    // The first check only learns what is already there.
    if state.is_none() || fresh.is_empty() {
        return Ok(Outcome { fire: None, data: None, state: next, shown });
    }
    let first = fresh[0];
    let from = first["lastFrom"].as_str().or(first["from"].as_str()).unwrap_or_default();
    let event = Event::Mail { count: fresh.len(), from: shorten(from, 80), subject: shorten(first["subject"].as_str().unwrap_or_default(), 120) };
    let rows: Vec<Value> = fresh
        .iter()
        .map(|t| json!({ "id": t["id"], "subject": t["subject"], "from": t["from"], "lastFrom": t["lastFrom"], "date": t["date"], "messageCount": t["messageCount"], "snippet": t["snippet"] }))
        .collect();
    let data = format!("New mail (read a thread with gmail_get_thread and its id):\n{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
    Ok(Outcome { fire: Some(event), data: Some(data), state: next, shown })
}

fn calendar(minutes_before: Option<u32>, new_events: bool, state: Option<&Value>, seen: &Observation, now: DateTime<Utc>) -> Result<Outcome, String> {
    let events = seen.json.as_ref().and_then(|j| j["events"].as_array()).ok_or("the calendar returned something unexpected")?;
    let when = |e: &Value, key: &str| e[key].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&Utc));
    let live: Vec<&Value> = events.iter().filter(|e| e["status"] != "cancelled" && e["id"].is_string()).collect();

    // Reminders already given: "id@start" -> start, kept until a day after the event.
    let mut fired: Map<String, Value> = state.and_then(|s| s["fired"].as_object()).cloned().unwrap_or_default();
    fired.retain(|_, start| start.as_i64().is_some_and(|ms| ms > (now - Duration::days(1)).timestamp_millis()));
    let mut soon = Vec::new();
    if let Some(lead) = minutes_before {
        for event in &live {
            // An all-day event has no time to be minutes before.
            let Some(start) = when(event, "start").filter(|_| event["allDay"] != true) else { continue };
            let key = format!("{}@{}", event["id"].as_str().unwrap_or_default(), start.timestamp());
            if start - Duration::minutes(i64::from(lead)) <= now && now < start && !fired.contains_key(&key) {
                fired.insert(key, json!(start.timestamp_millis()));
                soon.push((*event, (start - now).num_minutes()));
            }
        }
    }

    let mut known: Vec<String> = state.and_then(|s| s["known"].as_array()).map(|ids| ids.iter().filter_map(|i| i.as_str().map(String::from)).collect()).unwrap_or_default();
    let since = state.and_then(|s| s["since"].as_i64());
    let mut added = Vec::new();
    if new_events {
        for event in &live {
            let id = event["id"].as_str().unwrap_or_default();
            if known.iter().any(|k| k == id) {
                continue;
            }
            known.insert(0, id.to_string());
            // An event that only now comes into the window being looked at isn't new: it has to
            // have been made since the last check, too. (A minute of slack for clocks.)
            let made = when(event, "created").map(|t| t.timestamp_millis());
            if since.is_some_and(|s| made.is_none_or(|m| m >= s - 60_000)) {
                added.push(*event);
            }
        }
        known.truncate(MAX_KNOWN_EVENTS);
    }
    let next = json!({ "fired": fired, "known": known, "since": now.timestamp_millis() });
    let shown = format!("{}", live.len());
    let title = |e: &Value| shorten(e["summary"].as_str().unwrap_or_default(), 120);
    let fire = match (soon.first(), added.first()) {
        (Some((event, minutes)), _) => Some(Event::Soon { title: title(event), minutes: *minutes, count: soon.len() }),
        (None, Some(event)) => Some(Event::NewEvent { title: title(event), count: added.len() }),
        (None, None) => None,
    };
    let data = fire.as_ref().map(|_| {
        let brief = |e: &Value| json!({ "id": e["id"], "summary": e["summary"], "start": e["start"], "end": e["end"], "location": e["location"], "organizer": e["organizer"], "meetingLink": e["meetingLink"], "description": e["description"] });
        let mut parts = Vec::new();
        if !soon.is_empty() {
            let rows: Vec<Value> = soon.iter().map(|(e, _)| brief(e)).collect();
            parts.push(format!("Starting soon:\n{}", serde_json::to_string_pretty(&rows).unwrap_or_default()));
        }
        if !added.is_empty() {
            let rows: Vec<Value> = added.iter().map(|e| brief(e)).collect();
            parts.push(format!("New on the calendar:\n{}", serde_json::to_string_pretty(&rows).unwrap_or_default()));
        }
        parts.join("\n\n")
    });
    Ok(Outcome { fire, data, state: next, shown })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn cond(extract: Option<&str>, op: Op, value: Option<Value>) -> Trigger {
        Trigger::Condition { source: Source::Url { url: "https://example.com/x".into(), format: None }, extract: extract.map(String::from), predicate: Predicate { op, value }, every_minutes: 5 }
    }

    #[test]
    fn triggers_are_checked_and_tidied() {
        let t = Trigger::parse(json!({ "kind": "time", "frequency": "weekly", "weekday": 2, "time": " 9:05", "timezone": "Europe/Berlin" })).unwrap();
        assert_eq!(t, Trigger::Time { frequency: "weekly".into(), weekday: Some(2), time: "09:05".into(), timezone: "Europe/Berlin".into() });
        assert_eq!(t.kind(), "time");
        assert!(Trigger::parse(json!({ "kind": "time", "frequency": "hourly", "time": "09:00", "timezone": "UTC" })).is_err());
        assert!(Trigger::parse(json!({ "kind": "nope" })).is_err());
        assert!(Trigger::parse(json!({ "kind": "webhook", "extra": 1 })).is_err(), "unknown fields are refused");

        assert!(Trigger::parse(json!({ "kind": "interval", "every_minutes": 5 })).unwrap_err().contains("at least 15"));
        assert!(Trigger::parse(json!({ "kind": "interval", "every_minutes": 60 })).is_ok());
        assert!(Trigger::parse(json!({ "kind": "monthly", "day": 0, "time": "08:00", "timezone": "UTC" })).is_err());
        assert!(Trigger::parse(json!({ "kind": "once", "at": "tomorrow", "timezone": "UTC" })).is_err());
        assert!(Trigger::parse(json!({ "kind": "once", "at": "2026-12-03T09:00", "timezone": "Mars/Olympus" })).is_err());

        let c = Trigger::parse(json!({ "kind": "condition", "source": { "type": "url", "url": " https://example.com/p " }, "predicate": { "op": "lt", "value": "40" } })).unwrap();
        let Trigger::Condition { source, predicate, every_minutes, .. } = &c else { panic!() };
        assert_eq!(*source, Source::Url { url: "https://example.com/p".into(), format: None });
        assert_eq!(predicate.value, Some(json!(40.0)));
        assert_eq!(*every_minutes, DEFAULT_CHECK_MINUTES);
        assert!(c.watches() && c.by_event());
        assert!(Trigger::parse(json!({ "kind": "condition", "source": { "type": "url", "url": "file:///etc/passwd" }, "predicate": { "op": "changes" } })).is_err());
        assert!(Trigger::parse(json!({ "kind": "condition", "source": { "type": "tool", "name": "weather_forecast", "input": [1] }, "predicate": { "op": "changes" } })).is_err());
        assert!(Trigger::parse(json!({ "kind": "condition", "source": { "type": "records", "set": "expenses" }, "predicate": { "op": "gt" } })).is_err());
        assert!(Trigger::parse(json!({ "kind": "condition", "source": { "type": "records", "set": "expenses" }, "predicate": { "op": "contains" } })).is_err());

        assert!(Trigger::parse(json!({ "kind": "calendar" })).unwrap_err().contains("minutes_before"));
        assert!(Trigger::parse(json!({ "kind": "calendar", "minutes_before": 30 })).is_ok());
        assert!(Trigger::parse(json!({ "kind": "email", "query": "from:boss", "every_minutes": 0 })).is_err());
        assert!(Trigger::Webhook {}.by_event() && !Trigger::Webhook {}.watches());
    }

    #[test]
    fn a_stored_schedule_from_before_is_a_time_trigger() {
        let t: Trigger = serde_json::from_value(json!({ "kind": "time", "frequency": "daily", "time": "07:00", "timezone": "UTC" })).unwrap();
        assert_eq!(t.next_after(at("2026-03-01T08:00:00Z")), Some(at("2026-03-02T07:00:00Z")));
    }

    #[test]
    fn a_one_off_runs_once_and_then_never() {
        let t = Trigger::parse(json!({ "kind": "once", "at": "2026-12-03T09:00", "timezone": "Europe/Berlin" })).unwrap();
        assert_eq!(t.next_after(at("2026-12-01T00:00:00Z")), Some(at("2026-12-03T08:00:00Z")));
        assert_eq!(t.next_after(at("2026-12-03T08:00:00Z")), None);
        assert_eq!(Trigger::Webhook {}.next_after(at("2026-12-01T00:00:00Z")), None);
    }

    #[test]
    fn an_interval_counts_from_the_last_run() {
        let t = Trigger::Interval { every_minutes: 90 };
        assert_eq!(t.next_after(at("2026-03-01T08:00:00Z")), Some(at("2026-03-01T09:30:00Z")));
    }

    #[test]
    fn a_monthly_day_past_the_end_of_a_month_is_its_last_day() {
        let t = Trigger::parse(json!({ "kind": "monthly", "day": 31, "time": "08:00", "timezone": "UTC" })).unwrap();
        assert_eq!(t.next_after(at("2026-01-31T07:00:00Z")), Some(at("2026-01-31T08:00:00Z")));
        assert_eq!(t.next_after(at("2026-01-31T08:00:00Z")), Some(at("2026-02-28T08:00:00Z")));
        assert_eq!(t.next_after(at("2028-02-01T00:00:00Z")), Some(at("2028-02-29T08:00:00Z")), "a leap year");
        assert_eq!(t.next_after(at("2026-12-31T09:00:00Z")), Some(at("2027-01-31T08:00:00Z")), "over the year's end");
        // Local time: the 1st at 00:30 in Auckland is still the last day of the month before in UTC.
        let nz = Trigger::parse(json!({ "kind": "monthly", "day": 1, "time": "00:30", "timezone": "Pacific/Auckland" })).unwrap();
        assert_eq!(nz.next_after(at("2026-06-15T00:00:00Z")), Some(at("2026-06-30T12:30:00Z")));
    }

    #[test]
    fn a_change_is_only_one_after_the_first_look() {
        let t = cond(Some("price.eur"), Op::Changes, None);
        let look = |state: Option<&Value>, price: f64| t.evaluate(state, &Observation::of_json(json!({ "price": { "eur": price } })), Utc::now()).unwrap();
        let first = look(None, 41.5);
        assert_eq!(first.fire, None, "the first check only learns the value");
        assert_eq!(first.shown, "41.5");
        let same = look(Some(&first.state), 41.5);
        assert_eq!(same.fire, None);
        let changed = look(Some(&same.state), 39.0);
        assert_eq!(changed.fire, Some(Event::Changed { from: "41.5".into(), to: "39".into() }));
        assert!(changed.data.unwrap().contains("Value now:\n39\n"));
        assert_eq!(look(Some(&changed.state), 39.0).fire, None);
    }

    #[test]
    fn a_long_value_is_compared_by_its_digest() {
        let t = cond(None, Op::Changes, None);
        let page = |tail: &str| Observation::of_text(format!("{}{tail}", "lorem ipsum ".repeat(100)));
        let first = t.evaluate(None, &page("a"), Utc::now()).unwrap();
        assert_eq!(first.state["last"].as_str().unwrap().len(), 64);
        assert_eq!(t.evaluate(Some(&first.state), &page("a"), Utc::now()).unwrap().fire, None);
        assert!(t.evaluate(Some(&first.state), &page("b"), Utc::now()).unwrap().fire.is_some());
    }

    #[test]
    fn a_threshold_starts_a_run_when_crossed_and_again_only_after_going_back() {
        let t = cond(Some("/price"), Op::Lt, Some(json!(40)));
        let look = |state: Option<&Value>, price: Value| t.evaluate(state, &Observation::of_json(json!({ "price": price })), Utc::now()).unwrap();
        let high = look(None, json!(45));
        assert_eq!(high.fire, None);
        let low = look(Some(&high.state), json!("39.5"));
        assert_eq!(low.fire, Some(Event::Holds { value: "39.5".into(), op: Op::Lt, target: "40".into() }));
        assert_eq!(look(Some(&low.state), json!(38)).fire, None, "still below: no second run");
        let back = look(Some(&low.state), json!(41));
        assert_eq!(back.fire, None);
        assert!(look(Some(&back.state), json!(39)).fire.is_some(), "crossed again");
        // Already past the line at the first look: that is what was asked for.
        assert!(look(None, json!(12)).fire.is_some());
    }

    #[test]
    fn text_is_matched_and_bad_answers_are_errors() {
        let has = cond(None, Op::Contains, Some(json!("in stock")));
        assert!(has.evaluate(None, &Observation::of_text("Now In Stock!"), Utc::now()).unwrap().fire.is_some());
        assert!(has.evaluate(None, &Observation::of_text("Sold out"), Utc::now()).unwrap().fire.is_none());
        let is = cond(Some("status"), Op::Equals, Some(json!("shipped")));
        assert!(is.evaluate(None, &Observation::of_json(json!({ "status": "shipped" })), Utc::now()).unwrap().fire.is_some());
        let n = cond(Some("items[1].n"), Op::Equals, Some(json!("7")));
        assert!(n.evaluate(None, &Observation::of_json(json!({ "items": [{ "n": 1 }, { "n": 7.0 }] })), Utc::now()).unwrap().fire.is_some());

        let gt = cond(Some("a"), Op::Gt, Some(json!(1)));
        assert!(gt.evaluate(None, &Observation::of_json(json!({ "b": 1 })), Utc::now()).unwrap_err().contains("nothing at"));
        assert!(gt.evaluate(None, &Observation::of_json(json!({ "a": "soon" })), Utc::now()).unwrap_err().contains("not a number"));
        assert!(gt.evaluate(None, &Observation::of_text("<html>"), Utc::now()).unwrap_err().contains("not JSON"));
    }

    fn threads(rows: &[(&str, u64)]) -> Observation {
        let rows: Vec<Value> = rows.iter().map(|(id, n)| json!({ "id": id, "messageCount": n, "subject": format!("Subject {id}"), "from": "Alice <a@example.com>", "lastFrom": "Bob <b@example.com>", "snippet": "hi" })).collect();
        Observation::of_json(json!({ "threads": rows }))
    }

    #[test]
    fn mail_that_was_already_there_starts_nothing() {
        let t = Trigger::Email { query: "is:unread".into(), every_minutes: 5 };
        let first = t.evaluate(None, &threads(&[("t1", 1), ("t2", 3)]), Utc::now()).unwrap();
        assert_eq!(first.fire, None);
        assert_eq!(first.shown, "2");
        assert_eq!(t.evaluate(Some(&first.state), &threads(&[("t1", 1), ("t2", 3)]), Utc::now()).unwrap().fire, None);

        // A new thread, and a reply in a known one, each count once.
        let second = t.evaluate(Some(&first.state), &threads(&[("t3", 1), ("t2", 4), ("t1", 1)]), Utc::now()).unwrap();
        assert_eq!(second.fire, Some(Event::Mail { count: 2, from: "Bob <b@example.com>".into(), subject: "Subject t3".into() }));
        let data = second.data.unwrap();
        assert!(data.contains("Subject t3") && data.contains("Subject t2") && !data.contains("Subject t1"), "{data}");
        assert_eq!(t.evaluate(Some(&second.state), &threads(&[("t3", 1), ("t2", 4), ("t1", 1)]), Utc::now()).unwrap().fire, None);
        // A thread that drops out of the search and comes back unchanged is not new.
        let gone = t.evaluate(Some(&second.state), &threads(&[("t3", 1)]), Utc::now()).unwrap();
        assert_eq!(t.evaluate(Some(&gone.state), &threads(&[("t1", 1), ("t3", 1)]), Utc::now()).unwrap().fire, None);
        assert!(t.evaluate(None, &Observation::of_text("oops"), Utc::now()).is_err());
    }

    fn events(rows: &[(&str, &str, &str)]) -> Observation {
        let rows: Vec<Value> = rows.iter().map(|(id, start, created)| json!({ "id": id, "summary": format!("Event {id}"), "start": start, "created": created })).collect();
        Observation::of_json(json!({ "events": rows }))
    }

    #[test]
    fn an_event_is_announced_once_shortly_before_it_starts() {
        let t = Trigger::Calendar { calendar_id: None, filter: None, minutes_before: Some(30), new_events: false, every_minutes: 5 };
        let cal = events(&[("e1", "2026-03-02T10:00:00+01:00", "2026-01-01T00:00:00Z"), ("e2", "2026-03-02T15:00:00Z", "2026-01-01T00:00:00Z")]);
        let early = t.evaluate(None, &cal, at("2026-03-02T08:00:00Z")).unwrap();
        assert_eq!(early.fire, None, "an hour before is too early");
        let due = t.evaluate(Some(&early.state), &cal, at("2026-03-02T08:35:00Z")).unwrap();
        assert_eq!(due.fire, Some(Event::Soon { title: "Event e1".into(), minutes: 25, count: 1 }));
        assert!(due.data.unwrap().contains("Starting soon"));
        assert_eq!(t.evaluate(Some(&due.state), &cal, at("2026-03-02T08:40:00Z")).unwrap().fire, None, "only once");
        assert_eq!(t.evaluate(Some(&early.state), &cal, at("2026-03-02T09:00:00Z")).unwrap().fire, None, "not once it has started");
        // The very first check may announce: the time is what matters, not what was seen before.
        assert!(t.evaluate(None, &cal, at("2026-03-02T08:45:00Z")).unwrap().fire.is_some());
        // All-day and cancelled events are left out.
        let odd = Observation::of_json(json!({ "events": [
            { "id": "d", "summary": "Day", "start": "2026-03-02", "allDay": true },
            { "id": "c", "summary": "Off", "start": "2026-03-02T09:00:00Z", "status": "cancelled" } ] }));
        assert_eq!(t.evaluate(None, &odd, at("2026-03-02T08:45:00Z")).unwrap().fire, None);
    }

    #[test]
    fn a_new_event_is_one_made_since_the_last_check() {
        let t = Trigger::Calendar { calendar_id: None, filter: None, minutes_before: None, new_events: true, every_minutes: 5 };
        let first = t.evaluate(None, &events(&[("e1", "2026-03-05T10:00:00Z", "2026-01-01T00:00:00Z")]), at("2026-03-02T08:00:00Z")).unwrap();
        assert_eq!(first.fire, None, "what is there at the first look is not new");
        let cal = events(&[
            ("e1", "2026-03-05T10:00:00Z", "2026-01-01T00:00:00Z"),
            ("e2", "2026-03-06T10:00:00Z", "2026-03-02T08:03:00Z"),
            // Made long ago; it only now comes into the days being looked at.
            ("e3", "2026-05-01T10:00:00Z", "2025-11-01T00:00:00Z"),
        ]);
        let second = t.evaluate(Some(&first.state), &cal, at("2026-03-02T08:05:00Z")).unwrap();
        assert_eq!(second.fire, Some(Event::NewEvent { title: "Event e2".into(), count: 1 }));
        assert_eq!(t.evaluate(Some(&second.state), &cal, at("2026-03-02T08:10:00Z")).unwrap().fire, None);
    }

    #[test]
    fn requests_ask_cedar_for_the_right_thing() {
        let now = at("2026-03-02T08:00:00Z");
        assert_eq!(cond(None, Op::Changes, None).request(now), Some(Request::Tool { name: "url".into(), input: json!({ "url": "https://example.com/x" }) }));
        let mail = Trigger::Email { query: "from:boss".into(), every_minutes: 5 };
        assert_eq!(mail.request(now), Some(Request::Tool { name: "gmail_search_threads".into(), input: json!({ "query": "from:boss", "pageSize": 25 }) }));
        let cal = Trigger::Calendar { calendar_id: Some("work".into()), filter: Some("standup".into()), minutes_before: Some(30), new_events: false, every_minutes: 5 };
        let Some(Request::Tool { name, input }) = cal.request(now) else { panic!() };
        assert_eq!(name, "calendar_list_events");
        assert_eq!(input["calendarId"], "work");
        assert_eq!(input["fullText"], "standup");
        assert_eq!(input["endTime"], "2026-03-02T08:45:00+00:00");
        assert_eq!(Trigger::Webhook {}.request(now), None);
        let records = Trigger::Condition { source: Source::Records { set: "expenses".into() }, extract: Some("count".into()), predicate: Predicate { op: Op::Changes, value: None }, every_minutes: 5 };
        assert_eq!(records.request(now), Some(Request::Records { set: "expenses".into() }));
    }
}
