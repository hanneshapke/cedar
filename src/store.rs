//! SQLite persistence for runs, conversation messages and events.
//!
//! Uses runtime-checked queries (not the `query!` macros) so building doesn't need a database.

use crate::models::{
    event, Attachment, Event, ModelEntry, NewRun, Notification, PlanMode, Run, RunStatus,
    StoredMessage, Usage,
};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

const RUN_COLS: &str = "id, owner, status, prompt, system, model_id, model, reasoning, max_turns, \
    turns, final_text, stop_reason, error, input_tokens, output_tokens, parent_id, workspace_id, \
    workspace_reused, tools, plan, interactive, locale, callback_url, created_at, started_at, finished_at";

/// What `GET /v1/events` and the webhook worker read for a notification: the row joined with
/// its run's final state.
const NOTIFICATION_COLS: &str = "n.id, n.run_id, n.seq, r.status, r.stop_reason, r.error, \
    r.parent_id, r.finished_at";

const MODEL_COLS: &str =
    "id, label, model, base_url, reasoning_levels, default_reasoning, multimodal, max_tokens, \
    is_default, price_input_usd_per_m, price_output_usd_per_m, position, listed_models, created_at, \
    updated_at";

pub(crate) fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub(crate) fn to_dt(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

fn run_from_row(r: &SqliteRow) -> Result<Run> {
    let status: &str = r.try_get("status")?;
    let id = Uuid::parse_str(r.try_get::<&str, _>("id")?)?;
    Ok(Run {
        id,
        owner: r.try_get("owner")?,
        status: RunStatus::parse(status).ok_or_else(|| anyhow!("unknown run status {status:?}"))?,
        prompt: r.try_get("prompt")?,
        system: r.try_get("system")?,
        model_id: r.try_get("model_id")?,
        model: r.try_get("model")?,
        reasoning: r.try_get("reasoning")?,
        max_turns: r.try_get::<i64, _>("max_turns")? as u32,
        turns: r.try_get::<i64, _>("turns")? as u32,
        final_text: r.try_get("final_text")?,
        stop_reason: r.try_get("stop_reason")?,
        error: r.try_get("error")?,
        usage: Usage {
            input_tokens: r.try_get::<i64, _>("input_tokens")? as u64,
            output_tokens: r.try_get::<i64, _>("output_tokens")? as u64,
        },
        parent_id: r
            .try_get::<Option<&str>, _>("parent_id")?
            .map(Uuid::parse_str)
            .transpose()?,
        workspace_id: r
            .try_get::<Option<&str>, _>("workspace_id")?
            .map(Uuid::parse_str)
            .transpose()?
            .unwrap_or(id),
        workspace_reused: r.try_get("workspace_reused")?,
        tools: r
            .try_get::<Option<&str>, _>("tools")?
            .map(serde_json::from_str)
            .transpose()?,
        callback_url: r.try_get("callback_url")?,
        plan: PlanMode::parse(r.try_get("plan")?).unwrap_or_default(),
        interactive: r.try_get("interactive")?,
        locale: r.try_get("locale")?,
        created_at: to_dt(r.try_get("created_at")?),
        started_at: r.try_get::<Option<i64>, _>("started_at")?.map(to_dt),
        finished_at: r.try_get::<Option<i64>, _>("finished_at")?.map(to_dt),
    })
}

fn model_from_row(r: &SqliteRow) -> Result<ModelEntry> {
    Ok(ModelEntry {
        id: r.try_get("id")?,
        label: r.try_get("label")?,
        model: r.try_get("model")?,
        base_url: r.try_get("base_url")?,
        reasoning_levels: serde_json::from_str(r.try_get("reasoning_levels")?)?,
        default_reasoning: r.try_get("default_reasoning")?,
        multimodal: r.try_get("multimodal")?,
        max_tokens: u32::try_from(r.try_get::<i64, _>("max_tokens")?)
            .unwrap_or(crate::models::DEFAULT_MAX_TOKENS),
        is_default: r.try_get("is_default")?,
        price: match (
            r.try_get::<Option<f64>, _>("price_input_usd_per_m")?,
            r.try_get::<Option<f64>, _>("price_output_usd_per_m")?,
        ) {
            (Some(input_usd_per_m), Some(output_usd_per_m)) => Some(crate::models::Price {
                input_usd_per_m,
                output_usd_per_m,
            }),
            _ => None,
        },
        position: r.try_get("position")?,
        listed_models: r
            .try_get::<Option<&str>, _>("listed_models")?
            .map(serde_json::from_str)
            .transpose()?,
        created_at: to_dt(r.try_get("created_at")?),
        updated_at: to_dt(r.try_get("updated_at")?),
    })
}

/// Why [`Store::insert_run`] did not create a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertRefused {
    /// The owner already has the maximum number of queued/running runs.
    TooManyActive,
    /// The parent doesn't exist or belongs to someone else.
    ParentNotFound,
    /// The parent is still queued or running.
    ParentActive,
    /// The parent was already continued by this run.
    ParentContinued(Uuid),
}

fn notification_from_row(r: &SqliteRow) -> Result<Notification> {
    let status: &str = r.try_get("status")?;
    let status =
        RunStatus::parse(status).ok_or_else(|| anyhow!("unknown run status {status:?}"))?;
    Ok(Notification {
        id: r.try_get("id")?,
        kind: status
            .terminal_event()
            .ok_or_else(|| anyhow!("a notification for a run that is still {status:?}"))?,
        run_id: Uuid::parse_str(r.try_get::<&str, _>("run_id")?)?,
        seq: r.try_get("seq")?,
        status,
        stop_reason: r.try_get("stop_reason")?,
        error: r.try_get("error")?,
        parent_id: r
            .try_get::<Option<&str>, _>("parent_id")?
            .map(Uuid::parse_str)
            .transpose()?,
        finished_at: to_dt(r.try_get::<Option<i64>, _>("finished_at")?.unwrap_or_default()),
    })
}

/// A webhook call the worker has to make: the notification, where to POST it, and with what.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub notification: Notification,
    pub url: String,
    pub secret: Option<String>,
    /// How many times it has been tried so far.
    pub attempts: i64,
}

/// What [`Store::finish_run`] wrote: the terminal event's seq and the notification's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    pub seq: i64,
    pub notification: i64,
    /// Whether the run has a webhook to deliver.
    pub callback: bool,
}

/// How a run ended, as written by [`Store::finish_run`].
pub struct Finish<'a> {
    pub status: RunStatus,
    pub final_text: Option<&'a str>,
    pub stop_reason: Option<&'a str>,
    pub error: Option<&'a str>,
    /// The terminal event (`run.completed` / `run.failed` / `run.cancelled`) to append atomically.
    pub event_kind: &'a str,
    pub event_data: Value,
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(url)
            .with_context(|| format!("invalid DATABASE_URL {url:?}"))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await
            .context("opening database")?;
        Self::migrated(pool).await
    }

    /// A private in-memory database (one connection, since each connection would get its own).
    pub async fn connect_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await?;
        Self::migrated(pool).await
    }

    async fn migrated(pool: SqlitePool) -> Result<Self> {
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("running migrations")?;
        Ok(Self { pool })
    }

    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    // -- runs -------------------------------------------------------------------------------

    /// Insert a `queued` run unless `owner` already has `max_active` queued/running runs.
    /// Only for runs without a parent; see [`Store::insert_run`].
    pub async fn insert_run_limited(&self, new: &NewRun, max_active: i64) -> Result<Option<Run>> {
        Ok(self.insert_run(new, max_active).await?.ok())
    }

    /// Insert a `queued` run, checking the owner's active-run cap and, for a continuation, that the
    /// parent is theirs, finished, and not continued yet. A continuation gets a copy of the parent's
    /// conversation and reuses its workspace unless that has expired.
    ///
    /// Everything happens in one write transaction, so concurrent requests can't both slip past a
    /// check, and a workspace can't expire between being picked and being used.
    pub async fn insert_run(
        &self,
        new: &NewRun,
        max_active: i64,
    ) -> Result<Result<Run, InsertRefused>> {
        let id = Uuid::now_v7();
        let now = now_ms();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        let active: i64 = sqlx::query(
            "SELECT COUNT(*) AS n FROM runs WHERE owner = ? AND status IN ('queued', 'running')",
        )
        .bind(&new.owner)
        .fetch_one(&mut *tx)
        .await?
        .try_get("n")?;
        if active >= max_active {
            return Ok(Err(InsertRefused::TooManyActive));
        }

        let mut workspace = None;
        // A continuation keeps its parent's webhook unless the request names one.
        let mut callback = (new.callback_url.clone(), new.callback_secret.clone());
        if let Some(parent_id) = new.parent_id {
            let parent = sqlx::query(
                "SELECT owner, status, workspace_id, callback_url, callback_secret FROM runs WHERE id = ?",
            )
                .bind(parent_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
            let Some(parent) =
                parent.filter(|p| p.try_get::<&str, _>("owner").ok() == Some(new.owner.as_str()))
            else {
                return Ok(Err(InsertRefused::ParentNotFound));
            };
            if matches!(parent.try_get::<&str, _>("status")?, "queued" | "running") {
                return Ok(Err(InsertRefused::ParentActive));
            }
            let child = sqlx::query("SELECT id FROM runs WHERE parent_id = ?")
                .bind(parent_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
            if let Some(child) = child {
                return Ok(Err(InsertRefused::ParentContinued(Uuid::parse_str(
                    child.try_get("id")?,
                )?)));
            }
            if callback.0.is_none() {
                callback = (
                    parent.try_get("callback_url")?,
                    parent.try_get("callback_secret")?,
                );
            }
            let parent_ws: Option<String> = parent.try_get("workspace_id")?;
            if let Some(ws) = parent_ws {
                let reserved = sqlx::query(
                    "UPDATE workspaces SET last_used_at = ? WHERE id = ? AND expired_at IS NULL",
                )
                .bind(now)
                .bind(&ws)
                .execute(&mut *tx)
                .await?;
                if reserved.rows_affected() == 1 {
                    workspace = Some(Uuid::parse_str(&ws)?);
                }
            }
        }
        let reused = workspace.is_some();
        let workspace_id = match workspace {
            Some(ws) => ws,
            None => {
                sqlx::query("INSERT INTO workspaces (id, owner, created_at, last_used_at) VALUES (?, ?, ?, ?)")
                    .bind(id.to_string())
                    .bind(&new.owner)
                    .bind(now)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
                id
            }
        };

        sqlx::query(
            "INSERT INTO runs (id, owner, status, prompt, system, model_id, model, reasoning, max_turns, \
             parent_id, workspace_id, workspace_reused, tools, callback_url, callback_secret, plan, interactive, locale, created_at) \
             VALUES (?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(&new.owner)
        .bind(&new.prompt)
        .bind(&new.system)
        .bind(&new.model_id)
        .bind(&new.model)
        .bind(&new.reasoning)
        .bind(new.max_turns as i64)
        .bind(new.parent_id.map(|p| p.to_string()))
        .bind(workspace_id.to_string())
        .bind(reused)
        .bind(new.tools.as_ref().map(serde_json::to_string).transpose()?)
        .bind(&callback.0)
        .bind(&callback.1)
        .bind(new.plan.as_str())
        .bind(new.interactive)
        .bind(&new.locale)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        for (i, attachment) in new.attachments.iter().enumerate() {
            sqlx::query(
                "INSERT INTO run_attachments (run_id, seq, name, media_type, data) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(i as i64 + 1)
            .bind(&attachment.name)
            .bind(&attachment.media_type)
            .bind(&attachment.data)
            .execute(&mut *tx)
            .await?;
        }
        if let Some(parent_id) = new.parent_id {
            sqlx::query("INSERT INTO messages (run_id, seq, role, content) SELECT ?, seq, role, content FROM messages WHERE run_id = ?")
                .bind(id.to_string())
                .bind(parent_id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(Ok(self.get_run(id).await?.ok_or_else(|| {
            anyhow!("run {id} vanished after insert")
        })?))
    }

    /// For modules that keep their own tables (see `secrets`).
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn get_run(&self, id: Uuid) -> Result<Option<Run>> {
        let row = sqlx::query(&format!("SELECT {RUN_COLS} FROM runs WHERE id = ?"))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(run_from_row).transpose()
    }

    /// Newest first (ids are UUIDv7, so id order is creation order). `before` is an exclusive cursor.
    pub async fn list_runs(
        &self,
        owner: &str,
        status: Option<RunStatus>,
        before: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<Run>> {
        let rows = sqlx::query(&format!(
            "SELECT {RUN_COLS} FROM runs WHERE owner = ?1 \
             AND (?2 IS NULL OR status = ?2) AND (?3 IS NULL OR id < ?3) \
             ORDER BY id DESC LIMIT ?4"
        ))
        .bind(owner)
        .bind(status.map(|s| s.as_str()))
        .bind(before.map(|b| b.to_string()))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(run_from_row).collect()
    }

    pub async fn mark_running(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE runs SET status = 'running', started_at = ? WHERE id = ? AND status = 'queued'",
        )
        .bind(now_ms())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record that a model turn finished: the turn counter and token usage.
    /// Whether `owner` has said to always allow every one of `scopes` for `rule`.
    pub async fn granted(&self, owner: &str, rule: &str, scopes: &[String]) -> Result<bool> {
        if scopes.is_empty() {
            return Ok(false);
        }
        for scope in scopes {
            let row = sqlx::query(
                "SELECT 1 FROM approval_grants WHERE owner = ? AND rule = ? AND scope = ?",
            )
            .bind(owner)
            .bind(rule)
            .bind(scope)
            .fetch_optional(&self.pool)
            .await?;
            if row.is_none() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Everything `owner` always allows for `rule`, oldest first.
    pub async fn grants(&self, owner: &str, rule: &str) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT scope FROM approval_grants WHERE owner = ? AND rule = ? ORDER BY created_at, scope",
        )
        .bind(owner)
        .bind(rule)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(scope,)| scope).collect())
    }

    /// Remember that `owner` always allows `scopes` for `rule`.
    pub async fn grant(&self, owner: &str, rule: &str, scopes: &[String]) -> Result<()> {
        for scope in scopes {
            sqlx::query(
                "INSERT OR IGNORE INTO approval_grants (owner, rule, scope, created_at) VALUES (?, ?, ?, ?)",
            )
            .bind(owner)
            .bind(rule)
            .bind(scope)
            .bind(now_ms())
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    pub async fn record_turn(&self, id: Uuid, turns: u32, usage: Usage) -> Result<()> {
        sqlx::query(
            "UPDATE runs SET turns = ?, input_tokens = input_tokens + ?, output_tokens = output_tokens + ? WHERE id = ?",
        )
        .bind(turns as i64)
        .bind(usage.input_tokens as i64)
        .bind(usage.output_tokens as i64)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Move a queued/running run to a terminal state, append its terminal event and queue its
    /// notification, atomically. Returns `None` if the run was already terminal (so finishing
    /// twice is a no-op).
    pub async fn finish_run(&self, id: Uuid, f: Finish<'_>) -> Result<Option<Finished>> {
        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "UPDATE runs SET status = ?, final_text = ?, stop_reason = ?, error = ?, finished_at = ? \
             WHERE id = ? AND status IN ('queued', 'running') RETURNING owner, callback_url",
        )
        .bind(f.status.as_str())
        .bind(f.final_text)
        .bind(f.stop_reason)
        .bind(f.error)
        .bind(now)
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let owner: String = row.try_get("owner")?;
        let callback_url: Option<String> = row.try_get("callback_url")?;
        // The idle clock for the workspace starts when its run ends.
        sqlx::query("UPDATE workspaces SET last_used_at = ? WHERE id = (SELECT workspace_id FROM runs WHERE id = ?)")
            .bind(now)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        let seq = insert_event(&mut tx, id, f.event_kind, &f.event_data).await?;
        // The notification feeds the owner's event stream, and the webhook worker if there is a
        // callback: due right away.
        let notification: i64 = sqlx::query_scalar(
            "INSERT INTO run_notifications (run_id, owner, seq, callback_url, next_attempt_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(id.to_string())
        .bind(&owner)
        .bind(seq)
        .bind(&callback_url)
        .bind(callback_url.as_ref().map(|_| now))
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(Finished {
            seq,
            notification,
            callback: callback_url.is_some(),
        }))
    }

    // -- notifications ----------------------------------------------------------------------

    /// An owner's notifications with `id > after`, oldest first. Deleted runs' are gone.
    pub async fn notifications_after(
        &self,
        owner: &str,
        after: i64,
        limit: u32,
    ) -> Result<Vec<Notification>> {
        let rows = sqlx::query(&format!(
            "SELECT {NOTIFICATION_COLS} FROM run_notifications n JOIN runs r ON r.id = n.run_id \
             WHERE n.owner = ? AND n.id > ? ORDER BY n.id LIMIT ?"
        ))
        .bind(owner)
        .bind(after)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(notification_from_row).collect()
    }

    /// Webhook calls whose time has come, oldest first.
    pub async fn due_deliveries(&self, limit: u32) -> Result<Vec<Delivery>> {
        let rows = sqlx::query(&format!(
            "SELECT {NOTIFICATION_COLS}, n.callback_url, n.attempts, r.callback_secret \
             FROM run_notifications n JOIN runs r ON r.id = n.run_id \
             WHERE n.next_attempt_at IS NOT NULL AND n.next_attempt_at <= ? ORDER BY n.next_attempt_at, n.id LIMIT ?"
        ))
        .bind(now_ms())
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Delivery {
                    notification: notification_from_row(r)?,
                    url: r.try_get("callback_url")?,
                    secret: r.try_get("callback_secret")?,
                    attempts: r.try_get("attempts")?,
                })
            })
            .collect()
    }

    /// When the next webhook call is due, if any is pending.
    pub async fn next_delivery_at(&self) -> Result<Option<DateTime<Utc>>> {
        let at: Option<i64> =
            sqlx::query_scalar("SELECT MIN(next_attempt_at) FROM run_notifications")
                .fetch_one(&self.pool)
                .await?;
        Ok(at.map(to_dt))
    }

    /// Record a webhook call: delivered, or failed with `error` and tried again at `retry_at`
    /// (`None` gives up).
    pub async fn record_delivery(
        &self,
        id: i64,
        outcome: Result<(), (String, Option<DateTime<Utc>>)>,
    ) -> Result<()> {
        match outcome {
            Ok(()) => {
                sqlx::query(
                    "UPDATE run_notifications SET attempts = attempts + 1, delivered_at = ?, \
                     next_attempt_at = NULL, last_error = NULL WHERE id = ?",
                )
                .bind(now_ms())
                .bind(id)
                .execute(&self.pool)
                .await?;
            }
            Err((error, retry_at)) => {
                sqlx::query(
                    "UPDATE run_notifications SET attempts = attempts + 1, next_attempt_at = ?, \
                     last_error = ? WHERE id = ?",
                )
                .bind(retry_at.map(|t| t.timestamp_millis()))
                .bind(error)
                .bind(id)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    /// How a run's webhook delivery stands, for tests and diagnostics:
    /// `(attempts, delivered_at, last_error)`; `None` for a run without a notification.
    pub async fn delivery_status(
        &self,
        run_id: Uuid,
    ) -> Result<Option<(i64, Option<DateTime<Utc>>, Option<String>)>> {
        let row = sqlx::query(
            "SELECT attempts, delivered_at, last_error FROM run_notifications WHERE run_id = ?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok((
                r.try_get("attempts")?,
                r.try_get::<Option<i64>, _>("delivered_at")?.map(to_dt),
                r.try_get("last_error")?,
            ))
        })
        .transpose()
    }

    /// Runs left `queued`/`running` by a previous process can never finish; mark them failed.
    pub async fn fail_orphans(&self) -> Result<Vec<Uuid>> {
        let rows = sqlx::query("SELECT id FROM runs WHERE status IN ('queued', 'running')")
            .fetch_all(&self.pool)
            .await?;
        let mut failed = Vec::new();
        for row in rows {
            let id = Uuid::parse_str(row.try_get::<&str, _>("id")?)?;
            let msg = "server restarted before the run finished";
            let done = self
                .finish_run(
                    id,
                    Finish {
                        status: RunStatus::Failed,
                        final_text: None,
                        stop_reason: None,
                        error: Some(msg),
                        event_kind: event::RUN_FAILED,
                        event_data: json!({ "error": msg }),
                    },
                )
                .await?;
            if done.is_some() {
                failed.push(id);
            }
        }
        Ok(failed)
    }

    pub async fn count_active(&self, owner: &str) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM runs WHERE owner = ? AND status IN ('queued', 'running')",
        )
        .bind(owner)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("n")?)
    }

    /// Delete a run (and, by cascade, its messages and events) if it is terminal.
    pub async fn delete_finished_run(&self, id: Uuid) -> Result<bool> {
        let res =
            sqlx::query("DELETE FROM runs WHERE id = ? AND status NOT IN ('queued', 'running')")
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected() == 1)
    }

    // -- workspaces -------------------------------------------------------------------------

    /// Forget a workspace no run refers to any more. Returns true if the caller should remove its
    /// volume. Once a workspace has no runs, nothing can continue into it, so this can't race.
    pub async fn release_workspace_if_unused(&self, workspace_id: Uuid) -> Result<bool> {
        let res = sqlx::query("DELETE FROM workspaces WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM runs WHERE workspace_id = ?1)")
            .bind(workspace_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Mark workspaces idle since before `cutoff` (and with no active run) as expired and return
    /// them, so their volumes can be removed. Marking first means a continuation can no longer pick
    /// them up while the volume is being deleted.
    pub async fn expire_idle_workspaces(&self, cutoff: DateTime<Utc>) -> Result<Vec<Uuid>> {
        let rows = sqlx::query(
            "UPDATE workspaces SET expired_at = ?1 WHERE expired_at IS NULL AND last_used_at < ?2 \
             AND NOT EXISTS (SELECT 1 FROM runs WHERE runs.workspace_id = workspaces.id AND status IN ('queued', 'running')) \
             RETURNING id",
        )
        .bind(now_ms())
        .bind(cutoff.timestamp_millis())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok(Uuid::parse_str(r.try_get("id")?)?))
            .collect()
    }

    /// Does this workspace still have its files (exists and hasn't expired)?
    pub async fn workspace_is_live(&self, workspace_id: Uuid) -> Result<bool> {
        let row = sqlx::query("SELECT 1 FROM workspaces WHERE id = ? AND expired_at IS NULL")
            .bind(workspace_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    /// Workspaces whose volumes should exist.
    pub async fn live_workspaces(&self) -> Result<Vec<Uuid>> {
        let rows = sqlx::query("SELECT id FROM workspaces WHERE expired_at IS NULL")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| Ok(Uuid::parse_str(r.try_get("id")?)?))
            .collect()
    }

    // -- events -----------------------------------------------------------------------------

    /// Append an event and return its per-run sequence number.
    pub async fn append_event(&self, run_id: Uuid, kind: &str, data: &Value) -> Result<i64> {
        let mut conn = self.pool.acquire().await?;
        insert_event(&mut conn, run_id, kind, data).await
    }

    pub async fn events_after(&self, run_id: Uuid, after: i64, limit: u32) -> Result<Vec<Event>> {
        let rows = sqlx::query(
            "SELECT seq, type, data, created_at FROM events WHERE run_id = ? AND seq > ? ORDER BY seq LIMIT ?",
        )
        .bind(run_id.to_string())
        .bind(after)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Event {
                    seq: r.try_get("seq")?,
                    kind: r.try_get("type")?,
                    data: serde_json::from_str(r.try_get::<&str, _>("data")?)?,
                    created_at: to_dt(r.try_get("created_at")?),
                })
            })
            .collect()
    }

    // -- messages ---------------------------------------------------------------------------

    pub async fn append_message(&self, run_id: Uuid, role: &str, content: &Value) -> Result<i64> {
        let row = sqlx::query(
            "INSERT INTO messages (run_id, seq, role, content) \
             SELECT ?1, COALESCE(MAX(seq), 0) + 1, ?2, ?3 FROM messages WHERE run_id = ?1 RETURNING seq",
        )
        .bind(run_id.to_string())
        .bind(role)
        .bind(serde_json::to_string(content)?)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("seq")?)
    }

    /// The files sent with the run's prompt, in the order they were given.
    pub async fn list_attachments(&self, run_id: Uuid) -> Result<Vec<Attachment>> {
        let rows = sqlx::query(
            "SELECT name, media_type, data FROM run_attachments WHERE run_id = ? ORDER BY seq",
        )
        .bind(run_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Attachment {
                    name: r.try_get("name")?,
                    media_type: r.try_get("media_type")?,
                    data: r.try_get("data")?,
                })
            })
            .collect()
    }

    pub async fn list_messages(&self, run_id: Uuid) -> Result<Vec<StoredMessage>> {
        let rows =
            sqlx::query("SELECT seq, role, content FROM messages WHERE run_id = ? ORDER BY seq")
                .bind(run_id.to_string())
                .fetch_all(&self.pool)
                .await?;
        rows.iter()
            .map(|r| {
                Ok(StoredMessage {
                    seq: r.try_get("seq")?,
                    role: r.try_get("role")?,
                    content: serde_json::from_str(r.try_get::<&str, _>("content")?)?,
                })
            })
            .collect()
    }

    // -- the model registry ------------------------------------------------------------------

    /// Every registered model, in picker order.
    pub async fn list_models(&self) -> Result<Vec<ModelEntry>> {
        let rows = sqlx::query(&format!(
            "SELECT {MODEL_COLS} FROM models ORDER BY position, id"
        ))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(model_from_row).collect()
    }

    pub async fn get_model(&self, id: &str) -> Result<Option<ModelEntry>> {
        let row = sqlx::query(&format!("SELECT {MODEL_COLS} FROM models WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(model_from_row).transpose()
    }

    /// Add or replace a model. A new entry goes last; an existing one keeps its place and its
    /// creation time. Exactly one entry is the default afterwards: this one if `is_default` is
    /// set or it is the only entry, else the one that was.
    pub async fn upsert_model(&self, entry: &ModelEntry) -> Result<ModelEntry> {
        let mut tx = self.pool.begin().await?;
        let now = now_ms();
        let existing = sqlx::query("SELECT position, created_at FROM models WHERE id = ?")
            .bind(&entry.id)
            .fetch_optional(&mut *tx)
            .await?;
        let (position, created_at) = match existing {
            Some(r) => (
                r.try_get::<i64, _>("position")?,
                r.try_get::<i64, _>("created_at")?,
            ),
            None => {
                let next: i64 =
                    sqlx::query_scalar("SELECT COALESCE(MAX(position), 0) + 1 FROM models")
                        .fetch_one(&mut *tx)
                        .await?;
                (next, now)
            }
        };
        let others: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM models WHERE id != ?")
            .bind(&entry.id)
            .fetch_one(&mut *tx)
            .await?;
        let is_default = entry.is_default || others == 0;
        if is_default {
            sqlx::query("UPDATE models SET is_default = 0")
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO models (id, label, model, base_url, reasoning_levels, default_reasoning, \
             multimodal, max_tokens, is_default, price_input_usd_per_m, price_output_usd_per_m, \
             position, listed_models, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET label = excluded.label, model = excluded.model, \
             base_url = excluded.base_url, reasoning_levels = excluded.reasoning_levels, \
             default_reasoning = excluded.default_reasoning, multimodal = excluded.multimodal, \
             max_tokens = excluded.max_tokens, is_default = excluded.is_default, price_input_usd_per_m = excluded.price_input_usd_per_m, \
             price_output_usd_per_m = excluded.price_output_usd_per_m, listed_models = excluded.listed_models, \
             updated_at = excluded.updated_at",
        )
        .bind(&entry.id)
        .bind(&entry.label)
        .bind(&entry.model)
        .bind(&entry.base_url)
        .bind(serde_json::to_string(&entry.reasoning_levels)?)
        .bind(&entry.default_reasoning)
        .bind(entry.multimodal)
        .bind(i64::from(entry.max_tokens))
        .bind(is_default)
        .bind(entry.price.map(|p| p.input_usd_per_m))
        .bind(entry.price.map(|p| p.output_usd_per_m))
        .bind(position)
        .bind(
            entry
                .listed_models
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(created_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get_model(&entry.id)
            .await?
            .ok_or_else(|| anyhow!("model {} vanished after saving", entry.id))
    }

    /// Remove a model. Returns whether it existed. If it was the default, the first remaining
    /// entry becomes the default.
    pub async fn delete_model(&self, id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let removed = sqlx::query("DELETE FROM models WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            > 0;
        let has_default: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM models WHERE is_default")
            .fetch_one(&mut *tx)
            .await?;
        if has_default == 0 {
            sqlx::query(
                "UPDATE models SET is_default = 1 WHERE id = \
                 (SELECT id FROM models ORDER BY position, id LIMIT 1)",
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(removed)
    }

    // -- server-wide settings ----------------------------------------------------------------

    /// The setting saved under `key`, if any.
    pub async fn setting(&self, key: &str) -> Result<Option<Value>> {
        let value: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        value
            .map(|v| serde_json::from_str(&v))
            .transpose()
            .map_err(Into::into)
    }

    /// Save `value` under `key`, or remove the setting (back to the default) with `None`.
    pub async fn set_setting(&self, key: &str, value: Option<&Value>) -> Result<()> {
        match value {
            Some(v) => {
                sqlx::query(
                    "INSERT INTO settings (key, value) VALUES (?, ?) \
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                )
                .bind(key)
                .bind(serde_json::to_string(v)?)
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM settings WHERE key = ?")
                    .bind(key)
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// Whether anything is saved under `name` in the secrets table (without decrypting it).
    pub async fn has_secret(&self, name: &str) -> Result<bool> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM secrets WHERE name = ?")
            .bind(name)
            .fetch_one(&self.pool)
            .await?;
        Ok(n > 0)
    }

    /// Remove a saved secret, whether or not it can be decrypted. Returns whether there was one.
    pub async fn delete_secret(&self, name: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM secrets WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

/// `seq` is computed inside the INSERT, so it is atomic with respect to other writers.
async fn insert_event(
    conn: &mut sqlx::SqliteConnection,
    run_id: Uuid,
    kind: &str,
    data: &Value,
) -> Result<i64> {
    let row = sqlx::query(
        "INSERT INTO events (run_id, seq, type, data, created_at) \
         SELECT ?1, COALESCE(MAX(seq), 0) + 1, ?2, ?3, ?4 FROM events WHERE run_id = ?1 RETURNING seq",
    )
    .bind(run_id.to_string())
    .bind(kind)
    .bind(serde_json::to_string(data)?)
    .bind(now_ms())
    .fetch_one(&mut *conn)
    .await?;
    Ok(row.try_get("seq")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_run(owner: &str) -> NewRun {
        NewRun {
            owner: owner.into(),
            prompt: "hi".into(),
            system: "sys".into(),
            model_id: "m".into(),
            model: "m".into(),
            reasoning: None,
            max_turns: 5,
            parent_id: None,
            tools: None,
            attachments: vec![],
            callback_url: None,
            callback_secret: None,
            plan: PlanMode::Off,
            interactive: false,
            locale: "en".into(),
        }
    }

    fn completed(text: &str) -> Finish<'_> {
        Finish {
            status: RunStatus::Completed,
            final_text: Some(text),
            stop_reason: Some("end_turn"),
            error: None,
            event_kind: event::RUN_COMPLETED,
            event_data: json!({ "final_text": text }),
        }
    }

    #[tokio::test]
    async fn runs_from_before_continuations_migrate_and_can_be_continued() {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        let mut first_only = sqlx::migrate!("./migrations");
        first_only.migrations = first_only.migrations[..1].to_vec().into();
        first_only.run(&pool).await.unwrap();
        let old = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO runs (id, owner, status, prompt, system, model, max_turns, created_at) \
             VALUES (?, 'a', 'completed', 'p', 's', 'm', 5, 1)",
        )
        .bind(old.to_string())
        .execute(&pool)
        .await
        .unwrap();

        let store = Store::migrated(pool).await.unwrap();
        let run = store.get_run(old).await.unwrap().unwrap();
        assert_eq!(
            (run.workspace_id, run.parent_id, run.workspace_reused),
            (old, None, false)
        );

        // Its workspace is long gone, so a continuation gets a new one.
        let child = store
            .insert_run(
                &NewRun {
                    parent_id: Some(old),
                    ..new_run("a")
                },
                5,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(child.parent_id, Some(old));
        assert!(!child.workspace_reused);
        assert_eq!(child.workspace_id, child.id);
    }

    #[tokio::test]
    async fn active_run_limit_is_enforced_per_owner() {
        let store = Store::connect_memory().await.unwrap();
        let a = store
            .insert_run_limited(&new_run("a"), 2)
            .await
            .unwrap()
            .unwrap();
        store
            .insert_run_limited(&new_run("a"), 2)
            .await
            .unwrap()
            .unwrap();
        assert!(store
            .insert_run_limited(&new_run("a"), 2)
            .await
            .unwrap()
            .is_none());
        // Another owner is unaffected, and finishing a run frees a slot.
        assert!(store
            .insert_run_limited(&new_run("b"), 2)
            .await
            .unwrap()
            .is_some());
        store
            .finish_run(a.id, completed("done"))
            .await
            .unwrap()
            .unwrap();
        assert!(store
            .insert_run_limited(&new_run("a"), 2)
            .await
            .unwrap()
            .is_some());
        assert_eq!(store.count_active("a").await.unwrap(), 2);
    }

    #[tokio::test]
    async fn finish_is_idempotent_and_records_terminal_event() {
        let store = Store::connect_memory().await.unwrap();
        let run = store
            .insert_run_limited(&new_run("a"), 5)
            .await
            .unwrap()
            .unwrap();
        store.mark_running(run.id).await.unwrap();
        store
            .append_event(run.id, event::RUN_STARTED, &json!({}))
            .await
            .unwrap();

        let finished = store.finish_run(run.id, completed("ok")).await.unwrap().unwrap();
        assert_eq!((finished.seq, finished.callback), (2, false));
        assert_eq!(
            store.finish_run(run.id, completed("again")).await.unwrap(),
            None
        );
        // The notification is in the owner's stream, and nothing is queued for delivery.
        let notes = store.notifications_after("a", 0, 10).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].id, notes[0].run_id, notes[0].seq), (finished.notification, run.id, 2));
        assert_eq!((notes[0].kind, notes[0].stop_reason.as_deref()), ("run.completed", Some("end_turn")));
        assert!(store.notifications_after("a", finished.notification, 10).await.unwrap().is_empty());
        assert!(store.notifications_after("b", 0, 10).await.unwrap().is_empty());
        assert!(store.due_deliveries(10).await.unwrap().is_empty());
        assert_eq!(store.next_delivery_at().await.unwrap(), None);

        let got = store.get_run(run.id).await.unwrap().unwrap();
        assert_eq!(got.status, RunStatus::Completed);
        assert_eq!(got.final_text.as_deref(), Some("ok"));
        let events = store.events_after(run.id, 0, 100).await.unwrap();
        assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2]);
        assert!(events[1].is_terminal());
        assert_eq!(store.events_after(run.id, 1, 100).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_run_with_a_callback_queues_a_delivery_that_a_continuation_inherits() {
        let store = Store::connect_memory().await.unwrap();
        let new = NewRun {
            callback_url: Some("https://hooks.example/cedar".into()),
            callback_secret: Some("s3cret".into()),
            ..new_run("a")
        };
        let run = store.insert_run_limited(&new, 5).await.unwrap().unwrap();
        assert_eq!(run.callback_url.as_deref(), Some("https://hooks.example/cedar"));
        assert_eq!(store.delivery_status(run.id).await.unwrap(), None);

        let finished = store.finish_run(run.id, completed("ok")).await.unwrap().unwrap();
        assert!(finished.callback);
        let due = store.due_deliveries(10).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].url, "https://hooks.example/cedar");
        assert_eq!(due[0].secret.as_deref(), Some("s3cret"));
        assert_eq!((due[0].attempts, due[0].notification.id), (0, finished.notification));
        assert!(store.next_delivery_at().await.unwrap().is_some());

        // A failed attempt is retried later; a delivered one is done.
        let later = Utc::now() + chrono::Duration::hours(1);
        store
            .record_delivery(finished.notification, Err(("503".into(), Some(later))))
            .await
            .unwrap();
        assert!(store.due_deliveries(10).await.unwrap().is_empty());
        assert_eq!(store.next_delivery_at().await.unwrap().unwrap().timestamp(), later.timestamp());
        let (attempts, delivered, error) = store.delivery_status(run.id).await.unwrap().unwrap();
        assert_eq!((attempts, delivered, error.as_deref()), (1, None, Some("503")));
        store.record_delivery(finished.notification, Ok(())).await.unwrap();
        assert_eq!(store.next_delivery_at().await.unwrap(), None);
        let (attempts, delivered, error) = store.delivery_status(run.id).await.unwrap().unwrap();
        assert!(attempts == 2 && delivered.is_some() && error.is_none());

        // The continuation keeps the webhook unless it names its own.
        let child = NewRun { parent_id: Some(run.id), ..new_run("a") };
        let child = store.insert_run(&child, 5).await.unwrap().unwrap();
        assert_eq!(child.callback_url.as_deref(), Some("https://hooks.example/cedar"));
        store.finish_run(child.id, completed("ok")).await.unwrap();
        let due = store.due_deliveries(10).await.unwrap();
        assert_eq!((due.len(), due[0].secret.as_deref()), (1, Some("s3cret")));
        assert_eq!(due[0].notification.parent_id, Some(run.id));
        let own = NewRun {
            parent_id: Some(child.id),
            callback_url: Some("https://other.example/".into()),
            ..new_run("a")
        };
        let own = store.insert_run(&own, 5).await.unwrap().unwrap();
        assert_eq!(own.callback_url.as_deref(), Some("https://other.example/"));

        // Deleting a run takes its notification with it.
        store.finish_run(own.id, completed("ok")).await.unwrap();
        assert!(store.delete_finished_run(own.id).await.unwrap());
        assert_eq!(store.delivery_status(own.id).await.unwrap(), None);
        assert_eq!(store.notifications_after("a", 0, 10).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn orphans_are_failed_and_deleting_cascades() {
        let store = Store::connect_memory().await.unwrap();
        let queued = store
            .insert_run_limited(&new_run("a"), 5)
            .await
            .unwrap()
            .unwrap();
        let running = store
            .insert_run_limited(&new_run("a"), 5)
            .await
            .unwrap()
            .unwrap();
        store.mark_running(running.id).await.unwrap();
        store
            .append_message(running.id, "user", &json!("hi"))
            .await
            .unwrap();

        let mut failed = store.fail_orphans().await.unwrap();
        failed.sort();
        assert_eq!(failed.len(), 2);
        for id in [queued.id, running.id] {
            let run = store.get_run(id).await.unwrap().unwrap();
            assert_eq!(run.status, RunStatus::Failed);
            assert!(store.events_after(id, 0, 10).await.unwrap()[0].is_terminal());
        }

        assert!(store.delete_finished_run(running.id).await.unwrap());
        assert!(store.get_run(running.id).await.unwrap().is_none());
        assert!(store.list_messages(running.id).await.unwrap().is_empty());
        assert!(store
            .events_after(running.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn active_runs_cannot_be_deleted_and_listing_paginates_newest_first() {
        let store = Store::connect_memory().await.unwrap();
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(
                store
                    .insert_run_limited(&new_run("a"), 10)
                    .await
                    .unwrap()
                    .unwrap()
                    .id,
            );
        }
        assert!(!store.delete_finished_run(ids[0]).await.unwrap());

        let page1 = store.list_runs("a", None, None, 2).await.unwrap();
        assert_eq!(
            page1.iter().map(|r| r.id).collect::<Vec<_>>(),
            [ids[2], ids[1]]
        );
        let page2 = store
            .list_runs("a", None, Some(page1[1].id), 2)
            .await
            .unwrap();
        assert_eq!(page2.iter().map(|r| r.id).collect::<Vec<_>>(), [ids[0]]);
        assert!(store
            .list_runs("b", None, None, 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .list_runs("a", Some(RunStatus::Failed), None, 10)
                .await
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn messages_keep_order_and_content_verbatim() {
        let store = Store::connect_memory().await.unwrap();
        let run = store
            .insert_run_limited(&new_run("a"), 5)
            .await
            .unwrap()
            .unwrap();
        let blocks = json!([{ "type": "opaque", "content": [{ "payload": "abc==" }] }]);
        store
            .append_message(run.id, "user", &json!("hi"))
            .await
            .unwrap();
        store
            .append_message(run.id, "assistant", &blocks)
            .await
            .unwrap();
        let msgs = store.list_messages(run.id).await.unwrap();
        assert_eq!(msgs.iter().map(|m| m.seq).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(msgs[1].content, blocks);
    }

    #[tokio::test]
    async fn attachments_are_stored_with_the_run_and_deleted_with_it() {
        let store = Store::connect_memory().await.unwrap();
        let files = vec![
            Attachment {
                name: "a.png".into(),
                media_type: "image/png".into(),
                data: "aGk=".into(),
            },
            Attachment {
                name: "b.pdf".into(),
                media_type: "application/pdf".into(),
                data: "cGRm".into(),
            },
        ];
        let new = NewRun {
            attachments: files.clone(),
            plan: PlanMode::Off,
            interactive: false,
            locale: "en".into(),
            ..new_run("a")
        };
        let run = store.insert_run_limited(&new, 5).await.unwrap().unwrap();
        assert_eq!(store.list_attachments(run.id).await.unwrap(), files);

        // A continuation copies the parent's messages, not its attachment rows.
        store.finish_run(run.id, completed("done")).await.unwrap();
        let child = NewRun {
            parent_id: Some(run.id),
            ..new_run("a")
        };
        let child = store.insert_run(&child, 5).await.unwrap().unwrap();
        assert!(store.list_attachments(child.id).await.unwrap().is_empty());

        store.finish_run(child.id, completed("done")).await.unwrap();
        assert!(store.delete_finished_run(child.id).await.unwrap());
        assert!(store.delete_finished_run(run.id).await.unwrap());
        assert!(store.list_attachments(run.id).await.unwrap().is_empty());
    }
}
