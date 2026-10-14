//! Scheduled agents: when a schedule runs next, and the background task that runs it.
//!
//! A schedule fires at a local time of day (in its own IANA time zone) every day, on weekdays, or
//! once a week. The scheduler ticks every `TICK`, takes every enabled schedule whose `next_run_at`
//! has passed, and starts a fresh cedar run with the schedule's prompt in a conversation of its
//! own. Then it moves `next_run_at` past now, so a process that was down for a while fires each
//! schedule once, not once per missed slot.

use crate::api::{AppState, RunSettings};
use crate::error::AppError;
use crate::store::{NewScheduleRun, Schedule, ScheduleRun};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use std::time::Duration;
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
                Ok(n) => tracing::info!(count = n, "started scheduled runs"),
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "scheduler tick failed"),
            }
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(TICK) => {}
            }
        }
    })
}

/// Start every schedule that is due at `now`, one after another. Returns how many were started.
pub async fn tick(state: &AppState, now: DateTime<Utc>) -> anyhow::Result<usize> {
    let mut started = 0;
    for schedule in state.store.due_schedules(now).await? {
        match fire(state, &schedule, schedule.next_run_at, now, false).await {
            Ok(_) => started += 1,
            Err(e) => tracing::warn!(schedule_id = %schedule.id, error = %e, "could not start the scheduled run"),
        }
    }
    Ok(started)
}

/// Whether a failure to start a run is worth another try on a later tick.
fn worth_retrying(e: &AppError) -> bool {
    match e {
        AppError::BadGateway(_) => true,
        AppError::Upstream { status, .. } => *status == reqwest::StatusCode::TOO_MANY_REQUESTS,
        _ => false,
    }
}

/// Start one run of `schedule` for the slot `due_at`: a conversation of its own, a fresh cedar run
/// with the schedule's prompt, and a run record. A scheduled (non-manual) firing also moves
/// `next_run_at` past `now`, unless cedar couldn't take the run and the slot is recent enough to
/// retry.
pub async fn fire(
    state: &AppState,
    schedule: &Schedule,
    due_at: DateTime<Utc>,
    now: DateTime<Utc>,
    manual: bool,
) -> Result<ScheduleRun, AppError> {
    let timing = Timing::of(schedule).map_err(|e| AppError::Internal(anyhow::anyhow!("schedule {} is invalid: {e}", schedule.id)))?;
    let next = (!manual).then(|| timing.next_after(now));
    let prefs = state.store.preferences().await?;
    let settings = RunSettings::resolve(state, &prefs, schedule.model.clone(), schedule.reasoning.clone(), schedule.tools.clone()).await;
    let model = settings.model.as_deref();
    let when = due_at.with_timezone(&timing.tz).format("%a %-d %b %H:%M");
    let title = format!("{} · {when}", schedule.title);

    let conversation = state.store.create_scheduled_conversation(schedule.id, &title).await?;
    let run = match state.cedar.create_run(&schedule.prompt, &settings.options(None)).await {
        Ok(run) => run,
        Err(e) => {
            // Nothing happened; don't leave an empty conversation behind.
            if let Err(del) = state.store.delete_conversation(conversation.id).await {
                tracing::warn!(error = %del, "could not remove the conversation of a failed scheduled run");
            }
            if manual {
                return Err(e);
            }
            if worth_retrying(&e) && now - due_at < RETRY_GRACE {
                tracing::warn!(schedule_id = %schedule.id, error = %e, "cedar could not take the scheduled run; will retry");
                return Err(e);
            }
            state
                .store
                .insert_schedule_run(&NewScheduleRun {
                    schedule_id: schedule.id,
                    conversation_id: None,
                    turn_id: None,
                    due_at,
                    manual,
                    error: Some(e.to_string()),
                })
                .await?;
            state.store.mark_fired(schedule.id, now, next).await?;
            return Err(e);
        }
    };
    // A schedule's prompt is text only; it never carries attachments.
    let turn = state
        .store
        .insert_turn(conversation.id, run.id, &schedule.prompt, &run.status, model, settings.reasoning.as_deref(), &[])
        .await?;
    let record = state
        .store
        .insert_schedule_run(&NewScheduleRun {
            schedule_id: schedule.id,
            conversation_id: Some(conversation.id),
            turn_id: Some(turn.id),
            due_at,
            manual,
            error: None,
        })
        .await?;
    state.store.mark_fired(schedule.id, now, next).await?;
    tracing::info!(schedule_id = %schedule.id, run_id = %run.id, conversation_id = %conversation.id, "started scheduled run");
    Ok(record)
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
