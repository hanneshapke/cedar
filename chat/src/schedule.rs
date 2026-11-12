//! Agents: the background task that starts their runs, and when a classic schedule runs next.
//!
//! An agent is a prompt with a trigger (see `trigger`). The scheduler ticks every `TICK` and takes
//! every enabled agent whose `next_run_at` has passed. For one on a clock that is its next run: a
//! fresh cedar run with the agent's prompt in a conversation of its own, after which `next_run_at`
//! moves past now, so a process that was down for a while runs each agent once, not once per
//! missed slot. For one that watches something it is the next check (see `poll`), which starts a
//! run only if what it finds says so. Runs that events start are held to one at a time per agent
//! and to the agent's limits (see [`start_event`]).
//!
//! [`Timing`] is the classic schedule: a local time of day (in its own IANA time zone) every day,
//! on weekdays, or once a week.

use crate::api::{AppState, RunSettings};
use crate::error::AppError;
use crate::poll;
use crate::store::{NewScheduleRun, Polled, Schedule, ScheduleRun};
use crate::trigger::{self, Event, Trigger};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How often the scheduler looks for due schedules.
pub const TICK: Duration = Duration::from_secs(30);
/// How long after its due time a schedule keeps being retried when cedar can't take the run
/// (busy, or unreachable). After that the slot is recorded as failed and skipped.
pub const RETRY_GRACE: ChronoDuration = ChronoDuration::hours(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    Daily,
    Weekdays,
    Weekly,
}

impl Frequency {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "daily" => Frequency::Daily,
            "weekdays" => Frequency::Weekdays,
            "weekly" => Frequency::Weekly,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Frequency::Daily => "daily",
            Frequency::Weekdays => "weekdays",
            Frequency::Weekly => "weekly",
        }
    }
}

/// When a schedule runs, as the API describes it: a frequency, a weekday for weekly ones (0 =
/// Monday .. 6 = Sunday), a time of day, and the time zone that time is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    pub frequency: Frequency,
    pub weekday: Option<Weekday>,
    pub time: NaiveTime,
    pub tz: Tz,
}

impl Timing {
    /// Validate the API's strings. The error is a message for the client.
    pub fn parse(frequency: &str, weekday: Option<u8>, time: &str, timezone: &str) -> Result<Self, String> {
        let frequency = Frequency::parse(frequency)
            .ok_or_else(|| format!("frequency must be daily, weekdays or weekly, not {frequency:?}"))?;
        let weekday = match (frequency, weekday) {
            (Frequency::Weekly, None) => return Err("a weekly schedule needs a weekday (0 = Monday .. 6 = Sunday)".into()),
            (Frequency::Weekly, Some(d)) if d > 6 => return Err("weekday must be between 0 (Monday) and 6 (Sunday)".into()),
            (Frequency::Weekly, Some(d)) => Some(Weekday::try_from(d).expect("0..=6")),
            _ => None,
        };
        let time = NaiveTime::parse_from_str(time.trim(), "%H:%M").map_err(|_| format!("time must look like 09:30, not {time:?}"))?;
        let tz: Tz = timezone.trim().parse().map_err(|_| format!("unknown time zone {timezone:?}"))?;
        Ok(Self { frequency, weekday, time, tz })
    }

    pub fn of(schedule: &Schedule) -> Result<Self, String> {
        Self::parse(&schedule.frequency, schedule.weekday, &schedule.time_of_day, &schedule.timezone)
    }

    fn runs_on(&self, day: Weekday) -> bool {
        match self.frequency {
            Frequency::Daily => true,
            Frequency::Weekdays => !matches!(day, Weekday::Sat | Weekday::Sun),
            Frequency::Weekly => Some(day) == self.weekday,
        }
    }

    /// The first run strictly after `after`.
    pub fn next_after(&self, after: DateTime<Utc>) -> DateTime<Utc> {
        let mut date = after.with_timezone(&self.tz).date_naive();
        // A week covers every frequency; the extra days absorb a run earlier today.
        for _ in 0..9 {
            if self.runs_on(date.weekday()) {
                let local = date.and_time(self.time);
                // On a DST gap the wall-clock time doesn't exist; take the first instant after it.
                let at = self.tz.from_local_datetime(&local).earliest().unwrap_or_else(|| {
                    let later = local + ChronoDuration::hours(1);
                    self.tz.from_local_datetime(&later).earliest().expect("an hour past a gap is valid")
                });
                let at = at.with_timezone(&Utc);
                if at > after {
                    return at;
                }
            }
            date = date.succ_opt().expect("not at the end of time");
        }
        unreachable!("every frequency runs at least once a week")
    }
}

/// Run the scheduler until `stop` is cancelled. The first tick happens right away.
pub fn start(state: AppState, stop: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match tick(&state, Utc::now()).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "started agents' runs"),
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "scheduler tick failed"),
            }
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(TICK) => {}
            }
        }
    })
}

/// How many agents are checked at once.
const CHECKS_AT_ONCE: usize = 4;
/// The longest wait between two checks of something that keeps failing.
const MAX_BACKOFF: ChronoDuration = ChronoDuration::hours(6);

/// Look at every agent that is due at `now`: start the run of one on a clock, and check what a
/// watching one watches (a few at a time), starting its run if that says so. Returns how many
/// runs were started.
pub async fn tick(state: &AppState, now: DateTime<Utc>) -> anyhow::Result<usize> {
    let mut started = 0;
    let mut watching = Vec::new();
    for schedule in state.store.due_schedules(now).await? {
        let trigger = match Trigger::of(&schedule) {
            Ok(trigger) => trigger,
            Err(e) => {
                tracing::warn!(schedule_id = %schedule.id, error = %e, "an agent's trigger can't be read");
                continue;
            }
        };
        if trigger.by_event() {
            watching.push((schedule, trigger));
            continue;
        }
        let next = match trigger.next_after(now) {
            Some(at) => Next::At(at),
            None => Next::Never,
        };
        let firing = Firing { due_at: schedule.next_run_at, now, manual: false, next, event: Some(Event::Clock), data: None };
        match fire(state, &schedule, firing).await {
            Ok(_) => started += 1,
            Err(e) => tracing::warn!(schedule_id = %schedule.id, error = %e, "could not start the scheduled run"),
        }
    }
    let checked: Vec<bool> = futures_util::stream::iter(watching)
        .map(|(schedule, trigger)| async move {
            match check(state, &schedule, &trigger, now).await {
                Ok(started) => started,
                Err(e) => {
                    tracing::warn!(schedule_id = %schedule.id, error = %e, "could not check an agent's trigger");
                    false
                }
            }
        })
        .buffer_unordered(CHECKS_AT_ONCE)
        .collect()
        .await;
    Ok(started + checked.into_iter().filter(|started| *started).count())
}

/// Check what a watching agent watches, and start its run if the check says so. A webhook agent
/// is only ever due to hand over a delivery it had to hold back.
async fn check(state: &AppState, schedule: &Schedule, trigger: &Trigger, now: DateTime<Utc>) -> Result<bool, AppError> {
    let Some(every) = trigger.every() else {
        return crate::api::hooks::deliver_held(state, schedule, now).await;
    };
    let seen = match poll::observe(state, trigger, now).await {
        Ok(seen) => trigger.evaluate(schedule.state.as_ref(), &seen, now),
        Err(e) => Err(e),
    };
    match seen {
        Err(why) => {
            // A check that fails never starts a run, and is tried less and less often.
            let failures = schedule.failures.saturating_add(1);
            let wait = (every * 2i32.pow(failures.min(10))).min(MAX_BACKOFF).max(every);
            tracing::warn!(schedule_id = %schedule.id, error = %why, failures, "an agent's check failed");
            let p = Polled { rev: schedule.trigger_rev, state: schedule.state.as_ref(), next_run_at: now + wait, checked_at: now, error: Some(&why), failures };
            state.store.mark_polled(schedule.id, &p).await?;
            Ok(false)
        }
        Ok(outcome) => {
            let next = now + every;
            let keep = Polled { rev: schedule.trigger_rev, state: Some(&outcome.state), next_run_at: next, checked_at: now, error: None, failures: 0 };
            let Some(event) = outcome.fire.clone() else {
                state.store.mark_polled(schedule.id, &keep).await?;
                return Ok(false);
            };
            match start_event(state, schedule.id, event, outcome.data.clone(), now).await? {
                Started::Run(_) => {
                    state.store.mark_polled(schedule.id, &keep).await?;
                    Ok(true)
                }
                // The run couldn't be recorded as started for good (cedar refused it for a
                // reason that won't pass): move on, or the same thing would be tried forever.
                Started::Failed(_) => {
                    state.store.mark_polled(schedule.id, &keep).await?;
                    Ok(false)
                }
                // Not now: forget what this check saw, so the next one decides again.
                Started::Held(why) => {
                    tracing::info!(schedule_id = %schedule.id, ?why, "an agent's run is held back");
                    let hold = Polled { state: schedule.state.as_ref(), ..keep };
                    state.store.mark_polled(schedule.id, &hold).await?;
                    Ok(false)
                }
            }
        }
    }
}

/// Why an event didn't start a run right away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// The agent's previous run is still going.
    Busy,
    /// Its last run started less than `min_interval_secs` ago.
    TooSoon,
    /// It has started `max_runs_per_day` runs in the last 24 hours.
    DailyLimit,
    /// cedar can't take a run right now.
    Cedar,
}

pub enum Started {
    Run(ScheduleRun),
    Held(Held),
    /// cedar refused the run for good; it is recorded as one that didn't start.
    Failed(AppError),
}

/// Start an agent's run because something happened (a value, mail, an event, a call), unless
/// the agent has to wait: one run at a time, not more often than its limits say.
pub async fn start_event(state: &AppState, id: Uuid, event: Event, data: Option<String>, now: DateTime<Utc>) -> Result<Started, AppError> {
    // One at a time, so a call and a check can't both find the agent free.
    let _one = state.firing.lock().await;
    let Some(schedule) = state.store.get_schedule(id).await?.filter(|s| s.enabled) else {
        return Ok(Started::Held(Held::Busy));
    };
    if let Some(why) = held(state, &schedule, now).await? {
        return Ok(Started::Held(why));
    }
    let firing = Firing { due_at: now, now, manual: false, next: Next::Keep, event: Some(event), data };
    Ok(match fire(state, &schedule, firing).await {
        Ok(run) => Started::Run(run),
        Err(e) if worth_retrying(&e) => Started::Held(Held::Cedar),
        Err(e) => Started::Failed(e),
    })
}

async fn held(state: &AppState, schedule: &Schedule, now: DateTime<Utc>) -> Result<Option<Held>, AppError> {
    if crate::api::run_going(state, schedule.id).await? {
        return Ok(Some(Held::Busy));
    }
    let apart = schedule.min_interval_secs.unwrap_or(trigger::DEFAULT_MIN_INTERVAL_SECS);
    if state.store.count_schedule_runs_since(schedule.id, now - ChronoDuration::seconds(i64::from(apart))).await? > 0 {
        return Ok(Some(Held::TooSoon));
    }
    let a_day = schedule.max_runs_per_day.unwrap_or(trigger::DEFAULT_MAX_RUNS_PER_DAY);
    if state.store.count_schedule_runs_since(schedule.id, now - ChronoDuration::hours(24)).await? >= i64::from(a_day) {
        return Ok(Some(Held::DailyLimit));
    }
    Ok(None)
}

/// What cedar is told besides the agent's prompt: that nobody is there to answer a clarifying
/// question (`ask_user`), what started the run, and, fenced off as data, what came with it. The
/// conversation keeps the prompt as it was written.
fn unattended(prompt: &str, event: Option<&Event>, data: Option<&str>) -> String {
    let what = match event {
        None | Some(Event::Clock) => "This is a scheduled run".to_string(),
        Some(event) => format!("This run was started automatically: {}. It is unattended", crate::i18n::event_summary(None, event)),
    };
    let mut out = format!(
        "{prompt}\n\n({what} and nobody is there to answer questions: don't ask any, \
         make reasonable assumptions and say what you assumed.)"
    );
    if let Some(data) = data.filter(|d| !d.trim().is_empty()) {
        out.push_str("\n\n");
        out.push_str(&crate::prompt::fence(data));
    }
    out
}

/// Whether a failure to start a run is worth another try on a later tick.
fn worth_retrying(e: &AppError) -> bool {
    match e {
        AppError::BadGateway(_) => true,
        AppError::Upstream { status, .. } => *status == reqwest::StatusCode::TOO_MANY_REQUESTS,
        _ => false,
    }
}

/// What becomes of `next_run_at` when a run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Leave it: "run now", and runs an event starts (the check sets it).
    Keep,
    At(DateTime<Utc>),
    /// Nothing is due any more: a one-off has run. The agent is switched off.
    Never,
}

/// One start of an agent's run.
pub struct Firing {
    /// The slot it is for; now, for anything but a clock.
    pub due_at: DateTime<Utc>,
    pub now: DateTime<Utc>,
    /// "Run now".
    pub manual: bool,
    pub next: Next,
    /// What started it; `None` for "run now".
    pub event: Option<Event>,
    /// What came with the event, handed to the run as data.
    pub data: Option<String>,
}

impl Firing {
    pub fn manual(now: DateTime<Utc>) -> Self {
        Self { due_at: now, now, manual: true, next: Next::Keep, event: None, data: None }
    }
}

/// Start one run of `schedule`: a conversation of its own, a fresh cedar run with the agent's
/// prompt, and a run record. A run on the clock also moves `next_run_at` past `now`, unless cedar
/// couldn't take the run and the slot is recent enough to retry.
pub async fn fire(state: &AppState, schedule: &Schedule, firing: Firing) -> Result<ScheduleRun, AppError> {
    let Firing { due_at, now, manual, next, event, data } = firing;
    let prefs = state.store.preferences().await?;
    let settings = RunSettings::resolve(state, &prefs, schedule.model.clone(), schedule.reasoning.clone(), schedule.tools.clone()).await;
    let model = settings.model.as_deref();
    let tz: Tz = schedule.timezone.parse().unwrap_or(chrono_tz::UTC);
    let when = due_at.with_timezone(&tz).format("%a %-d %b %H:%M");
    let title = format!("{} · {when}", schedule.title);
    let cause = event.as_ref().map(|e| cause_of(e, prefs.locale.as_deref(), data.as_deref()));
    let moved = match next {
        Next::Keep => None,
        Next::At(at) => Some(at),
        Next::Never => Some(crate::store::never()),
    };
    let settle = |ran: bool| async move {
        state.store.mark_fired(schedule.id, now, moved).await?;
        if next == Next::Never && (ran || !manual) {
            state.store.disable_schedule(schedule.id).await?;
        }
        Ok::<_, AppError>(())
    };

    let conversation = state.store.create_scheduled_conversation(schedule.id, &title).await?;
    let run = match state.cedar.create_run(&unattended(&schedule.prompt, event.as_ref(), data.as_deref()), &settings.options(None)).await {
        Ok(run) => run,
        Err(e) => {
            // Nothing happened; don't leave an empty conversation behind.
            if let Err(del) = state.store.delete_conversation(conversation.id).await {
                tracing::warn!(error = %del, "could not remove the conversation of a run that didn't start");
            }
            if manual {
                return Err(e);
            }
            if worth_retrying(&e) && now - due_at < RETRY_GRACE {
                tracing::warn!(schedule_id = %schedule.id, error = %e, "cedar could not take the run; will retry");
                return Err(e);
            }
            state
                .store
                .insert_schedule_run(&NewScheduleRun { schedule_id: schedule.id, conversation_id: None, turn_id: None, due_at, manual, error: Some(e.to_string()), cause })
                .await?;
            settle(false).await?;
            return Err(e);
        }
    };
    // An agent's prompt is text only; it never carries attachments.
    let turn = state
        .store
        .insert_turn(conversation.id, run.id, &schedule.prompt, &run.status, model, settings.reasoning.as_deref(), &[])
        .await?;
    let record = state
        .store
        .insert_schedule_run(&NewScheduleRun { schedule_id: schedule.id, conversation_id: Some(conversation.id), turn_id: Some(turn.id), due_at, manual, error: None, cause })
        .await?;
    settle(true).await?;
    tracing::info!(schedule_id = %schedule.id, run_id = %run.id, conversation_id = %conversation.id, "started an agent's run");
    Ok(record)
}

/// How many characters of what came with an event are kept with the run's record, to show.
const EXCERPT_CHARS: usize = 400;

/// What started a run, as its record keeps it: the event, a line about it in the user's
/// language, and the start of what came with it.
fn cause_of(event: &Event, locale: Option<&str>, data: Option<&str>) -> Value {
    let mut cause = serde_json::to_value(event).unwrap_or_else(|_| json!({}));
    cause["kind"] = cause["event"].take();
    if let Some(map) = cause.as_object_mut() {
        map.remove("event");
    }
    cause["summary"] = json!(crate::i18n::event_summary(locale, event));
    if let Some(data) = data.map(str::trim).filter(|d| !d.is_empty()) {
        let cut = data.char_indices().nth(EXCERPT_CHARS).map_or(data.len(), |(i, _)| i);
        cause["excerpt"] = json!(&data[..cut]);
    }
    cause
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn timing(frequency: &str, weekday: Option<u8>, time: &str, tz: &str) -> Timing {
        Timing::parse(frequency, weekday, time, tz).unwrap()
    }

    #[test]
    fn parsing_checks_every_field() {
        assert!(Timing::parse("hourly", None, "09:00", "UTC").is_err());
        assert!(Timing::parse("weekly", None, "09:00", "UTC").unwrap_err().contains("weekday"));
        assert!(Timing::parse("weekly", Some(7), "09:00", "UTC").is_err());
        assert!(Timing::parse("daily", None, "9am", "UTC").unwrap_err().contains("09:30"));
        assert!(Timing::parse("daily", None, "09:00", "Mars/Olympus").unwrap_err().contains("time zone"));
        // A weekday on a non-weekly schedule is ignored rather than refused.
        assert_eq!(timing("daily", Some(3), "09:00", "UTC").weekday, None);
        assert_eq!(timing("weekly", Some(6), " 23:59 ", " Europe/Berlin ").weekday, Some(Weekday::Sun));
    }

    #[test]
    fn daily_runs_later_today_or_tomorrow() {
        let t = timing("daily", None, "09:00", "UTC");
        assert_eq!(t.next_after(utc("2026-10-12T08:59:00Z")), utc("2026-10-12T09:00:00Z"));
        assert_eq!(t.next_after(utc("2026-10-12T09:00:00Z")), utc("2026-10-13T09:00:00Z"), "strictly after");
        assert_eq!(t.next_after(utc("2026-10-12T15:00:00Z")), utc("2026-10-13T09:00:00Z"));
    }

    #[test]
    fn local_time_zones_are_honored() {
        let t = timing("daily", None, "09:00", "America/Los_Angeles");
        // 09:00 PDT is 16:00 UTC; 2026-10-12 is a Monday.
        assert_eq!(t.next_after(utc("2026-10-12T15:00:00Z")), utc("2026-10-12T16:00:00Z"));
        // After the November switch to PST it's 17:00 UTC.
        assert_eq!(t.next_after(utc("2026-11-02T00:00:00Z")), utc("2026-11-02T17:00:00Z"));
        let t = timing("daily", None, "00:30", "Asia/Tokyo");
        assert_eq!(t.next_after(utc("2026-10-12T15:00:00Z")), utc("2026-10-12T15:30:00Z"));
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        let t = timing("weekdays", None, "18:00", "UTC");
        // Friday 2026-10-16 after 18:00 -> Monday 2026-10-19.
        assert_eq!(t.next_after(utc("2026-10-16T18:00:00Z")), utc("2026-10-19T18:00:00Z"));
        assert_eq!(t.next_after(utc("2026-10-17T10:00:00Z")), utc("2026-10-19T18:00:00Z"));
        assert_eq!(t.next_after(utc("2026-10-19T10:00:00Z")), utc("2026-10-19T18:00:00Z"));
    }

    #[test]
    fn weekly_runs_on_its_weekday() {
        let t = timing("weekly", Some(2), "07:00", "UTC"); // Wednesday
        assert_eq!(t.next_after(utc("2026-10-12T00:00:00Z")), utc("2026-10-14T07:00:00Z"));
        assert_eq!(t.next_after(utc("2026-10-14T07:00:00Z")), utc("2026-10-21T07:00:00Z"));
        assert_eq!(t.next_after(utc("2026-10-14T06:59:59Z")), utc("2026-10-14T07:00:00Z"));
    }

    #[test]
    fn dst_gaps_and_overlaps_resolve() {
        // Berlin skips 02:00-03:00 on 2026-03-29; 02:30 becomes 03:30 CEST (01:30 UTC).
        let t = timing("daily", None, "02:30", "Europe/Berlin");
        assert_eq!(t.next_after(utc("2026-03-28T12:00:00Z")), utc("2026-03-29T01:30:00Z"));
        // Berlin repeats 02:00-03:00 on 2026-10-25; the first (CEST, 00:30 UTC) is used.
        assert_eq!(t.next_after(utc("2026-10-24T12:00:00Z")), utc("2026-10-25T00:30:00Z"));
        // And the day after is back to a plain CET 02:30 (01:30 UTC).
        assert_eq!(t.next_after(utc("2026-10-25T00:30:00Z")), utc("2026-10-26T01:30:00Z"));
    }
}
