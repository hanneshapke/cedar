//! SQLite persistence for runs, conversation messages and events.
//!
//! Uses runtime-checked queries (not the `query!` macros) so building doesn't need a database.

use crate::models::{event, Event, NewRun, Run, RunStatus, StoredMessage, Usage};
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

const RUN_COLS: &str = "id, owner, status, prompt, system, model, max_turns, turns, final_text, \
    stop_reason, error, input_tokens, output_tokens, parent_id, workspace_id, workspace_reused, \
    created_at, started_at, finished_at";

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn to_dt(ms: i64) -> DateTime<Utc> {
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
        model: r.try_get("model")?,
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
        created_at: to_dt(r.try_get("created_at")?),
        started_at: r.try_get::<Option<i64>, _>("started_at")?.map(to_dt),
        finished_at: r.try_get::<Option<i64>, _>("finished_at")?.map(to_dt),
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
        if let Some(parent_id) = new.parent_id {
            let parent = sqlx::query("SELECT owner, status, workspace_id FROM runs WHERE id = ?")
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
            "INSERT INTO runs (id, owner, status, prompt, system, model, max_turns, parent_id, workspace_id, \
             workspace_reused, created_at) VALUES (?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(&new.owner)
        .bind(&new.prompt)
        .bind(&new.system)
        .bind(&new.model)
        .bind(new.max_turns as i64)
        .bind(new.parent_id.map(|p| p.to_string()))
        .bind(workspace_id.to_string())
        .bind(reused)
        .bind(now)
        .execute(&mut *tx)
        .await?;
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

    /// Move a queued/running run to a terminal state and append its terminal event, atomically.
    /// Returns the event's seq, or `None` if the run was already terminal (so finishing twice is a no-op).
    pub async fn finish_run(&self, id: Uuid, f: Finish<'_>) -> Result<Option<i64>> {
        let mut tx = self.pool.begin().await?;
        let res = sqlx::query(
            "UPDATE runs SET status = ?, final_text = ?, stop_reason = ?, error = ?, finished_at = ? \
             WHERE id = ? AND status IN ('queued', 'running')",
        )
        .bind(f.status.as_str())
        .bind(f.final_text)
        .bind(f.stop_reason)
        .bind(f.error)
        .bind(now_ms())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        // The idle clock for the workspace starts when its run ends.
        sqlx::query("UPDATE workspaces SET last_used_at = ? WHERE id = (SELECT workspace_id FROM runs WHERE id = ?)")
            .bind(now_ms())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        let seq = insert_event(&mut tx, id, f.event_kind, &f.event_data).await?;
        tx.commit().await?;
        Ok(Some(seq))
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
            model: "m".into(),
            max_turns: 5,
            parent_id: None,
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

        assert_eq!(
            store.finish_run(run.id, completed("ok")).await.unwrap(),
            Some(2)
        );
        assert_eq!(
            store.finish_run(run.id, completed("again")).await.unwrap(),
            None
        );

        let got = store.get_run(run.id).await.unwrap().unwrap();
        assert_eq!(got.status, RunStatus::Completed);
        assert_eq!(got.final_text.as_deref(), Some("ok"));
        let events = store.events_after(run.id, 0, 100).await.unwrap();
        assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2]);
        assert!(events[1].is_terminal());
        assert_eq!(store.events_after(run.id, 1, 100).await.unwrap().len(), 1);
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
}
