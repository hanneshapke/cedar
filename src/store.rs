//! SQLite persistence for runs, conversation messages and events.
//!
//! Uses runtime-checked queries (not the `query!` macros) so building doesn't need a database.

use crate::models::{event, Event, NewRun, Run, RunStatus, StoredMessage, Usage};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

const RUN_COLS: &str = "id, owner, status, prompt, system, model, max_turns, turns, final_text, \
    stop_reason, error, input_tokens, output_tokens, created_at, started_at, finished_at";

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn to_dt(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

fn run_from_row(r: &SqliteRow) -> Result<Run> {
    let status: &str = r.try_get("status")?;
    Ok(Run {
        id: Uuid::parse_str(r.try_get::<&str, _>("id")?)?,
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
        created_at: to_dt(r.try_get("created_at")?),
        started_at: r.try_get::<Option<i64>, _>("started_at")?.map(to_dt),
        finished_at: r.try_get::<Option<i64>, _>("finished_at")?.map(to_dt),
    })
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
        sqlx::migrate!("./migrations").run(&pool).await.context("running migrations")?;
        Ok(Self { pool })
    }

    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    // -- runs -------------------------------------------------------------------------------

    /// Insert a `queued` run unless `owner` already has `max_active` queued/running runs.
    /// The check and the insert are one statement, so concurrent requests can't both slip past it.
    pub async fn insert_run_limited(&self, new: &NewRun, max_active: i64) -> Result<Option<Run>> {
        let id = Uuid::now_v7();
        let res = sqlx::query(
            "INSERT INTO runs (id, owner, status, prompt, system, model, max_turns, created_at) \
             SELECT ?1, ?2, 'queued', ?3, ?4, ?5, ?6, ?7 \
             WHERE (SELECT COUNT(*) FROM runs WHERE owner = ?2 AND status IN ('queued', 'running')) < ?8",
        )
        .bind(id.to_string())
        .bind(&new.owner)
        .bind(&new.prompt)
        .bind(&new.system)
        .bind(&new.model)
        .bind(new.max_turns as i64)
        .bind(now_ms())
        .bind(max_active)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_run(id).await
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
        sqlx::query("UPDATE runs SET status = 'running', started_at = ? WHERE id = ? AND status = 'queued'")
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
        let row = sqlx::query("SELECT COUNT(*) AS n FROM runs WHERE owner = ? AND status IN ('queued', 'running')")
            .bind(owner)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n")?)
    }

    /// Delete a run (and, by cascade, its messages and events) if it is terminal.
    pub async fn delete_finished_run(&self, id: Uuid) -> Result<bool> {
        let res = sqlx::query("DELETE FROM runs WHERE id = ? AND status NOT IN ('queued', 'running')")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() == 1)
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
        let rows = sqlx::query("SELECT seq, role, content FROM messages WHERE run_id = ? ORDER BY seq")
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
async fn insert_event(conn: &mut sqlx::SqliteConnection, run_id: Uuid, kind: &str, data: &Value) -> Result<i64> {
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
    async fn active_run_limit_is_enforced_per_owner() {
        let store = Store::connect_memory().await.unwrap();
        let a = store.insert_run_limited(&new_run("a"), 2).await.unwrap().unwrap();
        store.insert_run_limited(&new_run("a"), 2).await.unwrap().unwrap();
        assert!(store.insert_run_limited(&new_run("a"), 2).await.unwrap().is_none());
        // Another owner is unaffected, and finishing a run frees a slot.
        assert!(store.insert_run_limited(&new_run("b"), 2).await.unwrap().is_some());
        store.finish_run(a.id, completed("done")).await.unwrap().unwrap();
        assert!(store.insert_run_limited(&new_run("a"), 2).await.unwrap().is_some());
        assert_eq!(store.count_active("a").await.unwrap(), 2);
    }

    #[tokio::test]
    async fn finish_is_idempotent_and_records_terminal_event() {
        let store = Store::connect_memory().await.unwrap();
        let run = store.insert_run_limited(&new_run("a"), 5).await.unwrap().unwrap();
        store.mark_running(run.id).await.unwrap();
        store.append_event(run.id, event::RUN_STARTED, &json!({})).await.unwrap();

        assert_eq!(store.finish_run(run.id, completed("ok")).await.unwrap(), Some(2));
        assert_eq!(store.finish_run(run.id, completed("again")).await.unwrap(), None);

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
        let queued = store.insert_run_limited(&new_run("a"), 5).await.unwrap().unwrap();
        let running = store.insert_run_limited(&new_run("a"), 5).await.unwrap().unwrap();
        store.mark_running(running.id).await.unwrap();
        store.append_message(running.id, "user", &json!("hi")).await.unwrap();

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
        assert!(store.events_after(running.id, 0, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn active_runs_cannot_be_deleted_and_listing_paginates_newest_first() {
        let store = Store::connect_memory().await.unwrap();
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(store.insert_run_limited(&new_run("a"), 10).await.unwrap().unwrap().id);
        }
        assert!(!store.delete_finished_run(ids[0]).await.unwrap());

        let page1 = store.list_runs("a", None, None, 2).await.unwrap();
        assert_eq!(page1.iter().map(|r| r.id).collect::<Vec<_>>(), [ids[2], ids[1]]);
        let page2 = store.list_runs("a", None, Some(page1[1].id), 2).await.unwrap();
        assert_eq!(page2.iter().map(|r| r.id).collect::<Vec<_>>(), [ids[0]]);
        assert!(store.list_runs("b", None, None, 10).await.unwrap().is_empty());
        assert_eq!(store.list_runs("a", Some(RunStatus::Failed), None, 10).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn messages_keep_order_and_content_verbatim() {
        let store = Store::connect_memory().await.unwrap();
        let run = store.insert_run_limited(&new_run("a"), 5).await.unwrap().unwrap();
        let blocks = json!([{ "type": "web_search_tool_result", "content": [{ "encrypted_content": "abc==" }] }]);
        store.append_message(run.id, "user", &json!("hi")).await.unwrap();
        store.append_message(run.id, "assistant", &blocks).await.unwrap();
        let msgs = store.list_messages(run.id).await.unwrap();
        assert_eq!(msgs.iter().map(|m| m.seq).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(msgs[1].content, blocks);
    }
}
