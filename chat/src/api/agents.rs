//! `/api/agents`: prompts that run on their own, each with a trigger (see `trigger`).
//!
//! These are the schedules of `/api/schedules` and more: that route keeps its shape, and lists
//! only the agents on a daily or weekly clock, for the apps from before there were other triggers.

use super::*;
use crate::store::{never, Polled, NEVER_MS};
use crate::trigger::{self, Source, Trigger};
use chrono::{DateTime, Duration};

/// How many runs the activity feed returns at once, and at most.
const ACTIVITY_PAGE: u32 = 30;
const MAX_ACTIVITY_PAGE: u32 = 100;
/// How much of what a check returned is shown when a trigger is tried out.
const SAMPLE_CHARS: usize = 4000;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentBody {
    title: Option<String>,
    prompt: Option<String>,
    /// A registered model's id. Blank or absent means cedar's default model.
    model: Option<String>,
    reasoning: Option<String>,
    enabled: Option<bool>,
    /// The chat the agent was made from.
    conversation_id: Option<Uuid>,
    /// The tools its runs use; `null` means the defaults. When creating, absent means a careful
    /// choice for an agent that events start (see [`careful_tools`]), else the conversation's.
    #[serde(default, deserialize_with = "nullable")]
    tools: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "nullable")]
    notify: Option<Option<String>>,
    /// What starts its runs (see `trigger::Trigger`).
    trigger: Option<Value>,
    /// The user's time zone, for the triggers that carry none (the times in run titles).
    timezone: Option<String>,
    /// How far apart events may start runs, and how many in 24 hours; `null` for the defaults.
    #[serde(default, deserialize_with = "nullable")]
    min_interval_secs: Option<Option<u32>>,
    #[serde(default, deserialize_with = "nullable")]
    max_runs_per_day: Option<Option<u32>>,
}

#[derive(Deserialize)]
pub(super) struct ActivityQuery {
    before: Option<DateTime<Utc>>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TestBody {
    trigger: Value,
}

/// An agent as the API shows it. `secret` is a webhook's secret, shown only when it was just made.
fn view(state: &AppState, s: &Schedule, last_run: Option<&RunDetail>, secret: Option<&str>) -> Value {
    let trigger = Trigger::of(s).ok();
    let by_event = trigger.as_ref().is_some_and(Trigger::by_event);
    let watches = trigger.as_ref().is_some_and(Trigger::watches);
    let due = (s.next_run_at.timestamp_millis() < NEVER_MS).then_some(s.next_run_at);
    let hook = s.hook_id.as_ref().map(|id| {
        let last4 = s.hook_secret.as_deref().map(|k| k[k.len().saturating_sub(4)..].to_string());
        let mut hook = json!({ "url": format!("{}/api/hooks/{id}", state.cfg.public_url), "path": format!("/api/hooks/{id}"), "secret_last4": last4 });
        if let Some(secret) = secret {
            hook["secret"] = json!(secret);
        }
        hook
    });
    json!({
        "id": s.id,
        "title": s.title,
        "prompt": s.prompt,
        "model": s.model,
        "reasoning": s.reasoning,
        "tools": s.tools,
        "notify": s.notify,
        "enabled": s.enabled,
        "source_conversation_id": s.source_conversation_id,
        "timezone": s.timezone,
        "trigger": trigger.as_ref().and_then(|t| serde_json::to_value(t).ok()).unwrap_or_else(|| json!({ "kind": s.trigger_kind })),
        // When the next run is due, for a clock; when the next check is, for one that watches.
        "next_run_at": if by_event { None } else { due },
        "next_check_at": if watches { due } else { None },
        "last_run_at": s.last_run_at,
        "last_checked_at": s.last_checked_at,
        "last_error": s.last_error,
        "waiting": s.state.as_ref().is_some_and(|st| st.get("pending").is_some()),
        "min_interval_secs": s.min_interval_secs,
        "max_runs_per_day": s.max_runs_per_day,
        "hook": hook,
        "last_run": last_run,
        "created_at": s.created_at,
        "updated_at": s.updated_at,
    })
}

async fn find(state: &AppState, raw_id: &str) -> Result<Schedule, AppError> {
    let id = parse_id(raw_id, "agent not found")?;
    state.store.get_schedule(id).await?.ok_or(AppError::NotFound("agent not found"))
}

async fn last_run(state: &AppState, id: Uuid) -> Result<Option<RunDetail>, AppError> {
    let last = state.store.list_schedule_runs(id, 1).await?;
    Ok(with_turns(state, last).await?.pop())
}

fn random_hex(bytes: usize) -> Result<String, AppError> {
    let mut buf = vec![0u8; bytes];
    openssl::rand::rand_bytes(&mut buf).map_err(|e| AppError::Internal(e.into()))?;
    Ok(hex::encode(buf))
}

/// What the columns from before triggers hold for this one: `frequency`, `weekday`, `time_of_day`.
fn columns(trigger: &Trigger) -> (String, Option<u8>, String) {
    match trigger {
        Trigger::Time { frequency, weekday, time, .. } => (frequency.clone(), *weekday, time.clone()),
        Trigger::Monthly { time, .. } => ("monthly".into(), None, time.clone()),
        other => (other.kind().into(), None, "00:00".into()),
    }
}

/// When the scheduler should first look at an agent with this trigger.
fn first_due(trigger: &Trigger, now: DateTime<Utc>) -> Result<DateTime<Utc>, AppError> {
    if trigger.watches() {
        // Right away: the first check learns what is there now, so the next one can tell a change.
        return Ok(now);
    }
    match (trigger, trigger.next_after(now)) {
        (_, Some(at)) => Ok(at),
        (Trigger::Once { .. }, None) => Err(AppError::BadRequest("the time for this one-off run has already passed".into())),
        (_, None) => Ok(never()),
    }
}

/// What can't be told from the trigger alone: that cedar offers the tool a condition names and
/// lets it be called outside a run, and that a personal tool isn't checked too often.
async fn check_source(state: &AppState, trigger: &Trigger) -> Result<(), AppError> {
    let Trigger::Condition { source: Source::Tool { name, .. }, every_minutes, .. } = trigger else { return Ok(()) };
    let catalog = state.cedar.tools().await?;
    let found = catalog["groups"].as_array().into_iter().flatten().find_map(|g| {
        let tool = g["tools"].as_array()?.iter().find(|t| t["name"] == name.as_str())?;
        Some((g["id"].as_str().unwrap_or_default().to_string(), tool["pollable"] == true))
    });
    match found {
        None => Err(AppError::BadRequest(format!("cedar offers no tool called {name:?}"))),
        Some((_, false)) => Err(AppError::BadRequest(format!("{name} can't be watched: it needs a run (cedar marks the tools that can as pollable)"))),
        Some((group, true)) if group == "custom" && *every_minutes < trigger::MIN_PERSONAL_CHECK_MINUTES => Err(AppError::BadRequest(format!(
            "a personal tool starts a container for every check, so every_minutes must be at least {}",
            trigger::MIN_PERSONAL_CHECK_MINUTES
        ))),
        Some(_) => Ok(()),
    }
}

/// The tools an agent gets when events start its runs and nobody chose: everything cedar offers
/// except personal tools (other people's code, in the case of community ones) and the tools
/// that write to the Google account. Such a run reads text other people wrote (mail, a
/// webhook's body, a web page) with nobody there to approve a call.
async fn careful_tools(state: &AppState) -> Result<Vec<String>, AppError> {
    let catalog = state.cedar.tools().await?;
    let mut out = Vec::new();
    for group in catalog["groups"].as_array().into_iter().flatten() {
        let Some(id) = group["id"].as_str().filter(|id| *id != "custom") else { continue };
        let tools: Vec<&Value> = group["tools"].as_array().into_iter().flatten().collect();
        if tools.iter().any(|t| t["writes"] == true) {
            out.extend(tools.iter().filter(|t| t["writes"] != true).filter_map(|t| t["name"].as_str().map(String::from)));
        } else if group["available"] == true && group["discovered"] != false {
            out.push(id.to_string());
        }
    }
    out.sort();
    Ok(out)
}

fn clean_zone(zone: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(zone) = zone.map(str::trim).filter(|z| !z.is_empty()) else { return Ok(None) };
    let tz: chrono_tz::Tz = zone.parse().map_err(|_| AppError::BadRequest(format!("unknown time zone {zone:?}")))?;
    Ok(Some(tz.name().to_string()))
}

fn clean_limits(min_interval_secs: Option<u32>, max_runs_per_day: Option<u32>) -> Result<(), AppError> {
    if min_interval_secs.is_some_and(|s| !(trigger::MIN_MIN_INTERVAL_SECS..=7 * 24 * 3600).contains(&s)) {
        return Err(AppError::BadRequest(format!("min_interval_secs must be between {} and a week", trigger::MIN_MIN_INTERVAL_SECS)));
    }
    if max_runs_per_day.is_some_and(|n| !(1..=trigger::MAX_MAX_RUNS_PER_DAY).contains(&n)) {
        return Err(AppError::BadRequest(format!("max_runs_per_day must be between 1 and {}", trigger::MAX_MAX_RUNS_PER_DAY)));
    }
    Ok(())
}

pub(super) async fn list(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let mut agents = Vec::new();
    for schedule in state.store.list_schedules().await? {
        let last = last_run(&state, schedule.id).await?;
        agents.push(view(&state, &schedule, last.as_ref(), None));
    }
    Ok(Json(json!({
        "agents": agents,
        "limits": {
            "max_agents": MAX_SCHEDULES,
            "min_interval_secs": trigger::DEFAULT_MIN_INTERVAL_SECS,
            "max_runs_per_day": trigger::DEFAULT_MAX_RUNS_PER_DAY,
            "min_interval_minutes": trigger::MIN_INTERVAL_MINUTES,
            "check_minutes": trigger::DEFAULT_CHECK_MINUTES,
            "personal_check_minutes": trigger::MIN_PERSONAL_CHECK_MINUTES,
        },
    })))
}

pub(super) async fn create(State(state): State<AppState>, ApiJson(body): ApiJson<AgentBody>) -> Result<(StatusCode, Json<Value>), AppError> {
    let title = clean_title(body.title)?.ok_or_else(|| AppError::BadRequest("title must not be empty".into()))?;
    let prompt = clean_message("prompt", body.prompt.as_deref().unwrap_or_default())?;
    let (model, reasoning) = clean_model_choice(&state, body.model.as_deref(), body.reasoning.as_deref()).await?;
    let trigger = Trigger::parse(body.trigger.ok_or_else(|| AppError::BadRequest("trigger is required".into()))?).map_err(AppError::BadRequest)?;
    check_source(&state, &trigger).await?;
    let now = Utc::now();
    let next_run_at = first_due(&trigger, now)?;
    let (min_interval_secs, max_runs_per_day) = (body.min_interval_secs.flatten(), body.max_runs_per_day.flatten());
    clean_limits(min_interval_secs, max_runs_per_day)?;
    if state.store.count_schedules().await? >= MAX_SCHEDULES {
        return Err(AppError::Conflict(format!("at most {MAX_SCHEDULES} agents; delete one first")));
    }
    let tools = match body.tools {
        Some(tools) => clean_tools(&state, tools).await?,
        None if trigger.by_event() => Some(careful_tools(&state).await?),
        None => match body.conversation_id {
            Some(cid) => state.store.get_conversation(cid).await?.and_then(|c| c.tools),
            None => None,
        },
    };
    let notify = clean_notify(&state, body.notify.flatten()).await?;
    let timezone = match trigger.timezone() {
        Some(zone) => zone.to_string(),
        None => clean_zone(body.timezone.as_deref())?.unwrap_or_else(|| "UTC".into()),
    };
    let hook = match trigger {
        Trigger::Webhook {} => Some((random_hex(16)?, random_hex(32)?)),
        _ => None,
    };
    let (frequency, weekday, time_of_day) = columns(&trigger);
    let stored = serde_json::to_value(&trigger).map_err(|e| AppError::Internal(e.into()))?;
    let schedule = state
        .store
        .create_schedule(&NewSchedule {
            title: &title,
            prompt: &prompt,
            model: model.as_deref(),
            reasoning: reasoning.as_deref(),
            source_conversation_id: body.conversation_id,
            tools: tools.as_deref(),
            notify: notify.as_deref(),
            frequency: &frequency,
            weekday,
            time_of_day: &time_of_day,
            timezone: &timezone,
            next_run_at,
            trigger_kind: trigger.kind(),
            trigger: Some(&stored),
            hook_id: hook.as_ref().map(|(id, _)| id.as_str()),
            hook_secret: hook.as_ref().map(|(_, secret)| secret.as_str()),
            min_interval_secs,
            max_runs_per_day,
        })
        .await?;
    let schedule = match body.enabled {
        Some(false) => {
            let paused = Schedule { enabled: false, ..schedule };
            state.store.update_schedule(&paused, false).await?;
            paused
        }
        _ => schedule,
    };
    Ok((StatusCode::CREATED, Json(view(&state, &schedule, None, hook.as_ref().map(|(_, secret)| secret.as_str())))))
}

pub(super) async fn get(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let schedule = find(&state, &id).await?;
    let runs = with_turns(&state, state.store.list_schedule_runs(schedule.id, MAX_SCHEDULE_RUNS).await?).await?;
    Ok(Json(json!({ "agent": view(&state, &schedule, runs.first(), None), "runs": runs })))
}

pub(super) async fn update(State(state): State<AppState>, Path(id): Path<String>, ApiJson(body): ApiJson<AgentBody>) -> Result<Json<Value>, AppError> {
    let mut schedule = find(&state, &id).await?;
    let now = Utc::now();
    if let Some(title) = body.title {
        schedule.title = clean_title(Some(title))?.ok_or_else(|| AppError::BadRequest("title must not be empty".into()))?;
    }
    if let Some(prompt) = &body.prompt {
        schedule.prompt = clean_message("prompt", prompt)?;
    }
    if body.model.is_some() || body.reasoning.is_some() {
        let model = body.model.as_deref().or(schedule.model.as_deref());
        let reasoning = body.reasoning.as_deref().or(schedule.reasoning.as_deref());
        (schedule.model, schedule.reasoning) = clean_model_choice(&state, model, reasoning).await?;
    }
    if let Some(tools) = body.tools {
        schedule.tools = clean_tools(&state, tools).await?;
    }
    if let Some(notify) = body.notify {
        schedule.notify = clean_notify(&state, notify).await?;
    }
    if let Some(v) = body.min_interval_secs {
        schedule.min_interval_secs = v;
    }
    if let Some(v) = body.max_runs_per_day {
        schedule.max_runs_per_day = v;
    }
    clean_limits(schedule.min_interval_secs, schedule.max_runs_per_day)?;

    let before = Trigger::of(&schedule).map_err(AppError::BadRequest)?;
    let mut secret = None;
    let mut retriggered = false;
    let trigger = match body.trigger {
        Some(value) => {
            let trigger = Trigger::parse(value).map_err(AppError::BadRequest)?;
            check_source(&state, &trigger).await?;
            trigger
        }
        None => before.clone(),
    };
    if trigger != before {
        retriggered = true;
        let (frequency, weekday, time_of_day) = columns(&trigger);
        (schedule.frequency, schedule.weekday, schedule.time_of_day) = (frequency, weekday, time_of_day);
        schedule.trigger_kind = trigger.kind().to_string();
        schedule.trigger = Some(serde_json::to_value(&trigger).map_err(|e| AppError::Internal(e.into()))?);
        schedule.next_run_at = first_due(&trigger, now)?;
        match (&trigger, &schedule.hook_id) {
            (Trigger::Webhook {}, None) => {
                let made = random_hex(32)?;
                (schedule.hook_id, schedule.hook_secret) = (Some(random_hex(16)?), Some(made.clone()));
                secret = Some(made);
            }
            (Trigger::Webhook {}, Some(_)) => {}
            // No longer a webhook: its address stops working.
            _ => (schedule.hook_id, schedule.hook_secret) = (None, None),
        }
    }
    match (trigger.timezone(), clean_zone(body.timezone.as_deref())?) {
        (Some(zone), _) => schedule.timezone = zone.to_string(),
        (None, Some(zone)) => schedule.timezone = zone,
        (None, None) => {}
    }
    if let Some(enabled) = body.enabled {
        if enabled && !schedule.enabled && !retriggered {
            // Resuming: what was missed while paused stays missed. A clock moves to its next
            // time; a watcher learns anew what is there, so only what comes after counts.
            schedule.next_run_at = first_due(&trigger, now)?;
            retriggered = trigger.by_event();
        }
        schedule.enabled = enabled;
    }
    if !state.store.update_schedule(&schedule, retriggered).await? {
        return Err(AppError::NotFound("agent not found"));
    }
    let schedule = state.store.get_schedule(schedule.id).await?.ok_or(AppError::NotFound("agent not found"))?;
    let last = last_run(&state, schedule.id).await?;
    Ok(Json(view(&state, &schedule, last.as_ref(), secret.as_deref())))
}

pub(super) async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    let schedule = find(&state, &id).await?;
    remove_schedule(&state, &schedule).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn run_now(State(state): State<AppState>, Path(id): Path<String>) -> Result<(StatusCode, Json<RunDetail>), AppError> {
    let schedule = find(&state, &id).await?;
    Ok((StatusCode::ACCEPTED, Json(start_now(&state, &schedule).await?)))
}

/// A new secret for a webhook agent; the old one stops working at once. This is the only time
/// the new one is shown.
pub(super) async fn rotate_hook(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let mut schedule = find(&state, &id).await?;
    if schedule.hook_id.is_none() {
        return Err(AppError::Conflict("this agent has no webhook".into()));
    }
    let secret = random_hex(32)?;
    schedule.hook_secret = Some(secret.clone());
    state.store.update_schedule(&schedule, false).await?;
    let last = last_run(&state, schedule.id).await?;
    Ok(Json(view(&state, &schedule, last.as_ref(), Some(&secret))))
}

/// Try a trigger out before saving it: one check, with nothing kept. Says what was seen, so a
/// path can be found in it, and whether a run would start (never at a first look for a trigger
/// that waits for a change or for something new).
pub(super) async fn test(State(state): State<AppState>, ApiJson(body): ApiJson<TestBody>) -> Result<Json<Value>, AppError> {
    let trigger = Trigger::parse(body.trigger).map_err(AppError::BadRequest)?;
    if !trigger.watches() {
        return Err(AppError::BadRequest("only a trigger that watches something can be tried out".into()));
    }
    check_source(&state, &trigger).await?;
    let now = Utc::now();
    let seen = match crate::poll::observe(&state, &trigger, now).await {
        Ok(seen) => seen,
        Err(error) => return Ok(Json(json!({ "ok": false, "error": error }))),
    };
    let sample = match &seen.json {
        Some(json) => serde_json::to_string_pretty(json).unwrap_or_else(|_| seen.text.clone()),
        None => seen.text.clone(),
    };
    let cut = sample.char_indices().nth(SAMPLE_CHARS).map_or(sample.len(), |(i, _)| i);
    let sample = json!({ "text": &sample[..cut], "cut": cut < sample.len(), "json": seen.json.is_some() });
    let locale = state.store.preferences().await?.locale;
    Ok(Json(match trigger.evaluate(None, &seen, now) {
        Ok(outcome) => json!({
            "ok": true,
            "value": outcome.shown,
            "would_fire": outcome.fire.is_some(),
            "summary": outcome.fire.as_ref().map(|e| i18n::event_summary(locale.as_deref(), e)),
            "sample": sample,
        }),
        Err(error) => json!({ "ok": false, "error": error, "sample": sample }),
    }))
}

/// The latest runs of every agent, newest first, each with its agent and what started it.
pub(super) async fn activity(State(state): State<AppState>, ApiQuery(q): ApiQuery<ActivityQuery>) -> Result<Json<Value>, AppError> {
    let limit = q.limit.unwrap_or(ACTIVITY_PAGE).clamp(1, MAX_ACTIVITY_PAGE);
    let rows = state.store.list_activity(q.before, limit + 1).await?;
    let more = rows.len() as u32 > limit;
    let rows: Vec<_> = rows.into_iter().take(limit as usize).collect();
    let next_before = more.then(|| rows.last().map(|r| r.run.started_at)).flatten();
    let agents: Vec<Value> = rows.iter().map(|r| json!({ "id": r.run.schedule_id, "title": r.agent_title, "trigger_kind": r.trigger_kind })).collect();
    let runs = with_turns(&state, rows.into_iter().map(|r| r.run).collect()).await?;
    let items: Vec<Value> = runs
        .into_iter()
        .zip(agents)
        .map(|(run, agent)| {
            let mut item = serde_json::to_value(&run).unwrap_or_else(|_| json!({}));
            item["agent"] = agent;
            item
        })
        .collect();
    Ok(Json(json!({ "items": items, "next_before": next_before })))
}

/// Keep a webhook delivery for later (the agent may not start a run yet), in place of one
/// already waiting: the latest is what the run gets, with a count of those it replaced.
pub(super) async fn hold(state: &AppState, schedule: &Schedule, data: &str, bytes: usize, now: DateTime<Utc>) -> Result<(), AppError> {
    let dropped = schedule.state.as_ref().and_then(|s| s["pending"]["dropped"].as_u64().map(|n| n + 1)).unwrap_or(0);
    let pending = json!({ "pending": { "data": data, "bytes": bytes, "dropped": dropped, "received_at": now.timestamp_millis() } });
    // Due at once: every tick tries to hand it over until the agent may run.
    let p = Polled { rev: schedule.trigger_rev, state: Some(&pending), next_run_at: now, checked_at: now, error: None, failures: 0 };
    state.store.mark_polled(schedule.id, &p).await?;
    Ok(())
}

/// How long until the scheduler tries again to hand over a delivery that is still held.
pub(super) fn retry_after() -> Duration {
    Duration::seconds(schedule::TICK.as_secs() as i64)
}
