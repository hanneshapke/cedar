//! Google Calendar through the REST API (`GOOGLE_BACKEND=api`), with tools named like those of
//! Google's Calendar MCP server. Like `web_search` they run in the cedar process: the OAuth tokens
//! stay here, and the sandbox has no network.
//!
//! Only reading is implemented; nothing here creates, changes, or answers events.
//!
//! Times without a UTC offset are read in the user's calendar time zone (or the `timeZone` the
//! model passes), and results are given in it, together with the current time: the model has no
//! clock of its own, and needs one to resolve "this week".

use super::fetch::html_to_text;
use super::google::{page_size, required_str, select, str_arg, str_list, without_empty, GoogleApi};
use super::{truncate_output, Tool, ToolContext};
use crate::config::GoogleApiConfig;
use crate::mcp::GoogleOAuth;
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

/// The tools implemented here, in the order they are offered.
const TOOLS: &[&str] = &[
    "list_calendars",
    "list_events",
    "get_event",
    "search_events",
    "suggest_time",
];
/// Event descriptions in a list are cut to this many bytes; get_event gives them in full.
const LIST_DESCRIPTION_MAX: usize = 300;
/// search_events looks back this far unless given a start.
const SEARCH_LOOKBACK_DAYS: i64 = 180;
/// The longest range suggest_time searches.
const SUGGEST_MAX_DAYS: i64 = 62;

/// The Calendar tools `cfg` allows, sharing one API client.
pub fn tools(cfg: &GoogleApiConfig, auth: Arc<GoogleOAuth>) -> Result<Vec<Box<dyn Tool>>> {
    let calendar = Arc::new(Calendar {
        api: GoogleApi::new(&cfg.url, auth, "Calendar")?,
        zone: OnceCell::new(),
    });
    Ok(select(&cfg.prefix, &cfg.tools, TOOLS)
        .into_iter()
        .map(|op| {
            Box::new(CalendarTool {
                name: format!("{}_{op}", cfg.prefix),
                op,
                calendar: calendar.clone(),
            }) as Box<dyn Tool>
        })
        .collect())
}

struct Calendar {
    api: GoogleApi,
    /// The user's calendar time zone, looked up once.
    zone: OnceCell<Tz>,
}

impl Calendar {
    /// The `timeZone` argument if given, else the user's calendar time zone.
    async fn zone(&self, input: &Value, t: Duration) -> Result<Tz> {
        if let Some(name) = str_arg(input, "timeZone") {
            return parse_zone(name);
        }
        self.zone
            .get_or_try_init(|| async {
                let setting = self
                    .api
                    .get(&["users", "me", "settings", "timezone"], &[], t)
                    .await?;
                parse_zone(setting["value"].as_str().unwrap_or("UTC"))
            })
            .await
            .copied()
    }
}

/// One Calendar operation; `op` is one of [`TOOLS`].
pub struct CalendarTool {
    name: String,
    op: &'static str,
    calendar: Arc<Calendar>,
}

#[async_trait]
impl Tool for CalendarTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        match self.op {
            "list_calendars" => {
                "List the user's Google calendars (their own and those they subscribe to) with \
                 IDs and time zones. Also returns the current date and time in the user's time \
                 zone: call it first if you need today's date."
            }
            "list_events" => {
                "List events on one of the user's Google calendars in a time range, in start \
                 order, with recurring events expanded. Times without a UTC offset (e.g. \
                 2026-04-30T09:00:00, or just 2026-04-30 for midnight) are in the user's time \
                 zone. startTime defaults to now. Also returns the current date and time."
            }
            "get_event" => {
                "Get one calendar event in full: description, attendees and their responses, \
                 meeting link, and recurrence. Get event IDs from list_events or search_events."
            }
            "search_events" => {
                "Search the user's Google Calendar for events whose title, description, location, \
                 or attendees contain the query words. Searches from 180 days ago onward unless \
                 startTime is given."
            }
            "suggest_time" => {
                "Find free time for a meeting: the windows between startTime and endTime when the \
                 user and every attendee are free for at least durationMinutes, within working \
                 hours (09:00-17:00 on weekdays in the user's time zone unless preferences say \
                 otherwise). Free/busy is only visible for people whose calendars share it; the \
                 result lists anyone whose availability is unknown."
            }
            _ => unreachable!("not a Calendar tool: {}", self.op),
        }
    }

    fn schema(&self) -> Value {
        let calendar_id = json!({ "type": "string",
            "description": "The calendar's ID, from list_calendars (default: the user's primary calendar)." });
        let time = |what: &str| {
            json!({ "type": "string", "description": format!(
                "{what}, ISO 8601 (e.g. 2026-04-30T09:00:00). Without a UTC offset it is in the user's time zone.") })
        };
        let zone = json!({ "type": "string",
            "description": "An IANA time zone (e.g. Europe/Zurich) for times without an offset and for the results; only if the user names one. Default: the user's calendar time zone." });
        let page = |default: u64, max: u64| {
            json!({ "type": "integer", "minimum": 1, "maximum": max,
                    "description": format!("How many to return (default {default}, at most {max}).") })
        };
        let page_token = json!({ "type": "string",
            "description": "The nextPageToken of a previous call, to get the next page." });
        match self.op {
            "list_calendars" => json!({ "type": "object", "properties": {
                "pageSize": page(100, 250),
                "pageToken": page_token,
            } }),
            "list_events" => json!({ "type": "object", "properties": {
                "calendarId": calendar_id,
                "startTime": time("The start of the range (default: now)"),
                "endTime": time("The end of the range"),
                "fullText": { "type": "string", "description": "Only events containing these words." },
                "pageSize": page(50, 250),
                "pageToken": page_token,
                "timeZone": zone,
            } }),
            "get_event" => json!({ "type": "object", "properties": {
                "calendarId": calendar_id,
                "eventId": { "type": "string", "description": "The event's ID." },
                "timeZone": zone,
            }, "required": ["eventId"] }),
            "search_events" => json!({ "type": "object", "properties": {
                "query": { "type": "string", "description": "Words to search for." },
                "calendarId": calendar_id,
                "startTime": time("Only events after this time (default: 180 days ago)"),
                "endTime": time("Only events before this time"),
                "pageSize": page(25, 250),
                "pageToken": page_token,
                "timeZone": zone,
            }, "required": ["query"] }),
            "suggest_time" => json!({ "type": "object", "properties": {
                "attendeeEmails": { "type": "array", "items": { "type": "string" },
                    "description": "The other attendees' email addresses; the user is always included." },
                "startTime": time("The start of the range to search"),
                "endTime": time("The end of the range to search"),
                "durationMinutes": { "type": "integer", "minimum": 5, "maximum": 1440,
                    "description": "The meeting length in minutes (default 30)." },
                "timeZone": zone,
                "preferences": { "type": "object", "properties": {
                    "startHour": { "type": "string", "description": "The start of the working day, \"HH:mm\" (default 09:00)." },
                    "endHour": { "type": "string", "description": "The end of the working day, \"HH:mm\" (default 17:00)." },
                    "excludeWeekends": { "type": "boolean", "description": "Skip Saturdays and Sundays (default true)." },
                    "pageSize": { "type": "integer", "minimum": 1, "maximum": 20, "description": "How many windows to return (default 5)." },
                } },
            }, "required": ["startTime", "endTime"] }),
            _ => unreachable!("not a Calendar tool: {}", self.op),
        }
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let t = ctx.timeout;
        let zone = self.calendar.zone(input, t).await?;
        let now = Utc::now();
        let mut out = match self.op {
            "list_calendars" => self.list_calendars(input, t).await?,
            "list_events" => self.events(input, None, zone, now, t).await?,
            "search_events" => {
                let query = required_str(input, "query")?;
                self.events(input, Some(query), zone, now, t).await?
            }
            "get_event" => {
                let id = required_str(input, "eventId")?;
                let cal = str_arg(input, "calendarId").unwrap_or("primary");
                let event = self
                    .calendar
                    .api
                    .get(
                        &["calendars", cal, "events", id],
                        &[("timeZone", zone.name().to_string())],
                        t,
                    )
                    .await?;
                json!({ "event": render_event(&event, true) })
            }
            "suggest_time" => self.suggest_time(input, zone, t).await?,
            _ => unreachable!("not a Calendar tool: {}", self.op),
        };
        out["now"] = json!(now
            .with_timezone(&zone)
            .format("%A, %Y-%m-%dT%H:%M:%S%:z")
            .to_string());
        out["timeZone"] = json!(zone.name());
        Ok(truncate_output(
            &serde_json::to_string(&without_empty(out))?,
            ctx.max_output,
        ))
    }
}

impl CalendarTool {
    async fn list_calendars(&self, input: &Value, t: Duration) -> Result<Value> {
        let mut query = vec![("maxResults", page_size(input, 100, 250).to_string())];
        if let Some(token) = str_arg(input, "pageToken") {
            query.push(("pageToken", token.to_string()));
        }
        let list = self
            .calendar
            .api
            .get(&["users", "me", "calendarList"], &query, t)
            .await?;
        let calendars: Vec<Value> = items(&list, "items")
            .map(|c| {
                json!({
                    "id": c["id"],
                    "name": c.get("summaryOverride").unwrap_or(&c["summary"]),
                    "primary": c["primary"],
                    "accessRole": c["accessRole"],
                    "timeZone": c["timeZone"],
                    "description": c["description"],
                })
            })
            .collect();
        Ok(json!({ "calendars": calendars, "nextPageToken": list["nextPageToken"] }))
    }

    /// list_events, or search_events when `search` is given.
    async fn events(
        &self,
        input: &Value,
        search: Option<&str>,
        zone: Tz,
        now: DateTime<Utc>,
        t: Duration,
    ) -> Result<Value> {
        let end = str_arg(input, "endTime")
            .map(|s| parse_time(s, zone))
            .transpose()?;
        let start = match str_arg(input, "startTime") {
            Some(s) => parse_time(s, zone)?,
            None if search.is_some() => now - chrono::Duration::days(SEARCH_LOOKBACK_DAYS),
            // A range that ended in the past wants the events before it, not none.
            None => match end {
                Some(end) if end <= now => end - chrono::Duration::days(30),
                _ => now,
            },
        };
        if end.is_some_and(|end| end <= start) {
            bail!("endTime must be after startTime");
        }
        let cal = str_arg(input, "calendarId").unwrap_or("primary");
        let default_page = if search.is_some() { 25 } else { 50 };
        let mut query = vec![
            ("singleEvents", "true".to_string()),
            ("orderBy", "startTime".into()),
            ("timeMin", start.to_rfc3339()),
            (
                "maxResults",
                page_size(input, default_page, 250).to_string(),
            ),
            ("timeZone", zone.name().to_string()),
        ];
        if let Some(end) = end {
            query.push(("timeMax", end.to_rfc3339()));
        }
        if let Some(q) = search.or_else(|| str_arg(input, "fullText")) {
            query.push(("q", q.to_string()));
        }
        if let Some(token) = str_arg(input, "pageToken") {
            query.push(("pageToken", token.to_string()));
        }
        let list = self
            .calendar
            .api
            .get(&["calendars", cal, "events"], &query, t)
            .await?;
        let events: Vec<Value> = items(&list, "items")
            .map(|e| render_event(e, false))
            .collect();
        Ok(json!({
            "calendarId": cal,
            "from": local(start, zone),
            "to": end.map(|e| local(e, zone)),
            "events": events,
            "nextPageToken": list["nextPageToken"],
        }))
    }

    async fn suggest_time(&self, input: &Value, zone: Tz, t: Duration) -> Result<Value> {
        let start = parse_time(required_str(input, "startTime")?, zone)?;
        let end = parse_time(required_str(input, "endTime")?, zone)?;
        if end <= start {
            bail!("endTime must be after startTime");
        }
        if end - start > chrono::Duration::days(SUGGEST_MAX_DAYS) {
            bail!("the range may be at most {SUGGEST_MAX_DAYS} days");
        }
        let minutes = input["durationMinutes"]
            .as_i64()
            .unwrap_or(30)
            .clamp(5, 1440);
        let prefs = &input["preferences"];
        let day_start = parse_hour(str_arg(prefs, "startHour").unwrap_or("09:00"))?;
        let day_end = parse_hour(str_arg(prefs, "endHour").unwrap_or("17:00"))?;
        if day_end <= day_start {
            bail!("preferences.endHour must be after startHour");
        }
        let weekends = prefs["excludeWeekends"].as_bool() == Some(false);
        let max_slots = prefs["pageSize"].as_u64().unwrap_or(5).clamp(1, 20) as usize;

        let mut ids = vec!["primary".to_string()];
        for email in str_list(input, "attendeeEmails") {
            if !ids.contains(&email) {
                ids.push(email);
            }
        }
        let body = json!({
            "timeMin": start.to_rfc3339(),
            "timeMax": end.to_rfc3339(),
            "items": ids.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
        });
        let reply = self.calendar.api.post(&["freeBusy"], &body, t).await?;

        let mut busy = Vec::new();
        let mut unknown = Vec::new();
        for (id, cal) in reply["calendars"].as_object().into_iter().flatten() {
            if let Some(err) = cal["errors"].as_array().and_then(|e| e.first()) {
                unknown.push(json!({ "id": id, "reason": err["reason"] }));
                continue;
            }
            for period in items(cal, "busy") {
                let parse = |k: &str| {
                    period[k]
                        .as_str()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc))
                };
                if let (Some(s), Some(e)) = (parse("start"), parse("end")) {
                    busy.push((s, e));
                }
            }
        }
        let windows = free_windows(
            start,
            end,
            &busy,
            zone,
            (day_start, day_end),
            !weekends,
            chrono::Duration::minutes(minutes),
        );
        let found = windows.len();
        let slots: Vec<Value> = windows
            .into_iter()
            .take(max_slots)
            .map(|(s, e)| json!({ "start": local(s, zone), "end": local(e, zone) }))
            .collect();
        Ok(json!({
            "durationMinutes": minutes,
            "freeWindows": slots,
            "moreWindows": (found > max_slots).then(|| found - max_slots),
            "unknownAvailability": unknown,
        }))
    }
}

fn items<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    v[key].as_array().into_iter().flatten()
}

fn parse_zone(name: &str) -> Result<Tz> {
    name.parse()
        .map_err(|_| anyhow!("unknown time zone {name:?}: use an IANA name like Europe/Zurich"))
}

/// An ISO 8601 time: with an offset as given, without one in `zone`, a bare date at midnight.
fn parse_time(s: &str, zone: Tz) -> Result<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&Utc));
    }
    let naive = [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
    .or_else(|| {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .ok()
            .map(|d| d.and_time(NaiveTime::MIN))
    })
    .ok_or_else(|| anyhow!("invalid time {s:?}: use ISO 8601, e.g. 2026-04-30T09:00:00"))?;
    in_zone(naive, zone)
}

/// A wall-clock time in `zone`. A time skipped by a daylight-saving change moves an hour later.
fn in_zone(naive: NaiveDateTime, zone: Tz) -> Result<DateTime<Utc>> {
    zone.from_local_datetime(&naive)
        .earliest()
        .or_else(|| {
            zone.from_local_datetime(&(naive + chrono::Duration::hours(1)))
                .earliest()
        })
        .map(|t| t.with_timezone(&Utc))
        .ok_or_else(|| anyhow!("{naive} does not exist in {}", zone.name()))
}

fn local(t: DateTime<Utc>, zone: Tz) -> String {
    t.with_timezone(&zone)
        .format("%a %Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

fn parse_hour(s: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M")
        .map_err(|_| anyhow!("invalid hour {s:?}: use HH:mm, e.g. 09:00"))
}

/// The stretches of at least `min` within working hours on each day of `start..end` (in `zone`)
/// that overlap none of `busy`.
fn free_windows(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    busy: &[(DateTime<Utc>, DateTime<Utc>)],
    zone: Tz,
    (day_start, day_end): (NaiveTime, NaiveTime),
    skip_weekends: bool,
    min: chrono::Duration,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut busy = busy.to_vec();
    busy.sort();
    let mut windows = Vec::new();
    let mut day = start.with_timezone(&zone).date_naive();
    let last = end.with_timezone(&zone).date_naive();
    while day <= last {
        let weekend = matches!(day.weekday(), Weekday::Sat | Weekday::Sun);
        if !(skip_weekends && weekend) {
            if let (Ok(open), Ok(close)) = (
                in_zone(day.and_time(day_start), zone),
                in_zone(day.and_time(day_end), zone),
            ) {
                let mut cursor = open.max(start);
                let close = close.min(end);
                for &(b_start, b_end) in &busy {
                    if b_end <= cursor || b_start >= close {
                        continue;
                    }
                    if b_start - cursor >= min {
                        windows.push((cursor, b_start));
                    }
                    cursor = cursor.max(b_end);
                }
                if close - cursor >= min {
                    windows.push((cursor, close));
                }
            }
        }
        day = day.succ_opt().expect("dates this far out exist");
    }
    windows
}

/// An event as the model sees it. `full` adds the attendee list, recurrence, the link, and the
/// whole description.
fn render_event(e: &Value, full: bool) -> Value {
    let when = |k: &str| e[k].get("dateTime").or_else(|| e[k].get("date")).cloned();
    let attendees: Vec<&Value> = items(e, "attendees").collect();
    let me = attendees.iter().find(|a| a["self"] == true);
    let meeting = e.get("hangoutLink").cloned().or_else(|| {
        items(&e["conferenceData"], "entryPoints")
            .find(|p| p["entryPointType"] == "video")
            .map(|p| p["uri"].clone())
    });
    let mut description = e["description"].as_str().unwrap_or_default().to_string();
    if description.contains('<') {
        description = html_to_text(description.as_bytes())
            .unwrap_or(description)
            .trim()
            .to_string();
    }
    if !full {
        description = truncate_output(&description, LIST_DESCRIPTION_MAX);
    }
    let mut out = json!({
        "id": e["id"],
        "summary": e["summary"],
        "start": when("start"),
        "end": when("end"),
        "allDay": e["start"].get("date").map(|_| true),
        "location": e["location"],
        "status": e["status"].as_str().filter(|s| *s != "confirmed"),
        "eventType": e["eventType"].as_str().filter(|s| *s != "default"),
        "organizer": e["organizer"]["email"],
        "myResponse": me.map(|a| a["responseStatus"].clone()),
        "meetingLink": meeting,
        "description": description,
    });
    if full {
        out["attendees"] = json!(attendees
            .iter()
            .map(|a| json!({
                "email": a["email"],
                "name": a["displayName"],
                "response": a["responseStatus"],
                "optional": a["optional"],
                "organizer": a["organizer"],
            }))
            .collect::<Vec<_>>());
        out["recurrence"] = e["recurrence"].clone();
        out["recurringEventId"] = e["recurringEventId"].clone();
        out["link"] = e["htmlLink"].clone();
    } else if !attendees.is_empty() {
        out["attendeeCount"] = json!(attendees.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LA: Tz = chrono_tz::America::Los_Angeles;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn times_without_an_offset_are_in_the_zone() {
        assert_eq!(
            parse_time("2026-10-12T09:00:00", LA).unwrap(),
            utc("2026-10-12T16:00:00Z")
        );
        assert_eq!(
            parse_time("2026-10-12T09:00", LA).unwrap(),
            utc("2026-10-12T16:00:00Z")
        );
        assert_eq!(
            parse_time("2026-12-01", LA).unwrap(),
            utc("2026-12-01T08:00:00Z")
        );
        assert_eq!(
            parse_time("2026-10-12T09:00:00+02:00", LA).unwrap(),
            utc("2026-10-12T07:00:00Z")
        );
        assert_eq!(
            parse_time("2026-10-12T09:00:00Z", LA).unwrap(),
            utc("2026-10-12T09:00:00Z")
        );
        // 02:30 doesn't exist on the morning clocks spring forward.
        assert_eq!(
            parse_time("2026-03-08T02:30:00", LA).unwrap(),
            utc("2026-03-08T10:30:00Z")
        );
        assert!(parse_time("next tuesday", LA).is_err());
        assert!(parse_zone("Mars/Olympus").is_err());
    }

    #[test]
    fn free_windows_skip_busy_times_nights_and_weekends() {
        let nine_to_five = (parse_hour("09:00").unwrap(), parse_hour("17:00").unwrap());
        // Friday 2026-10-16 through Monday 2026-10-19, Los Angeles time (UTC-7).
        let busy = [
            (utc("2026-10-16T17:00:00Z"), utc("2026-10-16T18:00:00Z")), // Fri 10-11
            (utc("2026-10-16T17:30:00Z"), utc("2026-10-16T19:00:00Z")), // Fri 10:30-12, overlaps
            (utc("2026-10-16T22:00:00Z"), utc("2026-10-16T22:20:00Z")), // Fri 15-15:20
        ];
        let windows = free_windows(
            utc("2026-10-16T16:00:00Z"),
            utc("2026-10-19T19:00:00Z"),
            &busy,
            LA,
            nine_to_five,
            true,
            chrono::Duration::minutes(30),
        );
        let shown: Vec<(String, String)> = windows
            .iter()
            .map(|(s, e)| (local(*s, LA), local(*e, LA)))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "Fri 2026-10-16T09:00:00-07:00".into(),
                    "Fri 2026-10-16T10:00:00-07:00".into()
                ),
                (
                    "Fri 2026-10-16T12:00:00-07:00".into(),
                    "Fri 2026-10-16T15:00:00-07:00".into()
                ),
                (
                    "Fri 2026-10-16T15:20:00-07:00".into(),
                    "Fri 2026-10-16T17:00:00-07:00".into()
                ),
                (
                    "Mon 2026-10-19T09:00:00-07:00".into(),
                    "Mon 2026-10-19T12:00:00-07:00".into()
                ),
            ]
        );
    }

    #[test]
    fn events_are_summarized_for_lists() {
        let event = json!({
            "id": "e1", "summary": "Standup", "status": "confirmed", "eventType": "default",
            "start": { "dateTime": "2026-10-13T09:00:00-07:00" },
            "end": { "dateTime": "2026-10-13T09:15:00-07:00" },
            "description": format!("<p>{}</p>", "agenda ".repeat(100)),
            "hangoutLink": "https://meet.google.com/abc",
            "attendees": [{ "email": "me@x.com", "self": true, "responseStatus": "accepted" },
                          { "email": "bo@x.com", "responseStatus": "needsAction" }],
        });
        let short = without_empty(render_event(&event, false));
        assert_eq!(short["start"], "2026-10-13T09:00:00-07:00");
        assert_eq!(short["myResponse"], "accepted");
        assert_eq!(short["attendeeCount"], 2);
        assert_eq!(short["meetingLink"], "https://meet.google.com/abc");
        assert!(short.get("status").is_none() && short.get("eventType").is_none());
        assert!(short["description"]
            .as_str()
            .unwrap()
            .starts_with("agenda agenda"));
        assert!(short["description"].as_str().unwrap().len() < 400);

        let full = without_empty(render_event(&event, true));
        assert_eq!(full["attendees"][1]["email"], "bo@x.com");
        assert!(full.get("attendeeCount").is_none());

        let all_day = render_event(
            &json!({ "start": { "date": "2026-10-13" }, "end": { "date": "2026-10-14" } }),
            false,
        );
        assert_eq!(all_day["allDay"], true);
        assert_eq!(all_day["start"], "2026-10-13");
    }
}
