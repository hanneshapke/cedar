//! SQLite persistence for conversations and their turns.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn to_dt(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    pub id: Uuid,
    pub title: Option<String>,
    /// The model (a cedar registry id) and reasoning level used most recently, so the pickers
    /// can restore them.
    pub model: Option<String>,
    pub reasoning: Option<String>,
    /// Set on conversations a schedule created; they are listed under it, not in the main list.
    pub schedule_id: Option<Uuid>,
    /// The tools this conversation uses instead of the default ones.
    pub tools: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A prompt that runs on its own at a local time of day. `frequency`, `weekday`, `time_of_day`
/// and `timezone` are kept as the API received them; `schedule::Timing` interprets them.
#[derive(Debug, Clone, Serialize)]
pub struct Schedule {
    pub id: Uuid,
    pub title: String,
    pub prompt: String,
    pub model: Option<String>,
    /// The reasoning level its runs ask for; `None` means the model's default.
    pub reasoning: Option<String>,
    pub source_conversation_id: Option<Uuid>,
    /// The tools the schedule's runs use; `None` means the defaults at the time of the run.
    pub tools: Option<Vec<String>>,
    pub frequency: String,
    pub weekday: Option<u8>,
    pub time_of_day: String,
    pub timezone: String,
    pub enabled: bool,
    pub next_run_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct NewSchedule<'a> {
    pub title: &'a str,
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    pub reasoning: Option<&'a str>,
    pub source_conversation_id: Option<Uuid>,
    pub tools: Option<&'a [String]>,
    pub frequency: &'a str,
    pub weekday: Option<u8>,
    pub time_of_day: &'a str,
    pub timezone: &'a str,
    pub next_run_at: DateTime<Utc>,
}

/// One firing of a schedule. `conversation_id` and `turn_id` are `None` when the cedar run could
/// not be started; `error` then says why.
#[derive(Debug, Clone, Serialize)]
pub struct ScheduleRun {
    pub id: Uuid,
    pub schedule_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub due_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub manual: bool,
    pub error: Option<String>,
}

pub struct NewScheduleRun {
    pub schedule_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub due_at: DateTime<Utc>,
    pub manual: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Turn {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub seq: i64,
    pub run_id: Uuid,
    pub prompt: String,
    pub status: String,
    pub final_text: Option<String>,
    pub error: Option<String>,
    /// The files sent with the message (name, type and size only; the bytes are in cedar's run).
    pub attachments: Vec<AttachmentInfo>,
    pub created_at: DateTime<Utc>,
}

/// What is remembered about a file sent with a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub name: String,
    pub media_type: String,
    /// Decoded size in bytes.
    pub size: u64,
}

fn opt_uuid(r: &SqliteRow, col: &str) -> Result<Option<Uuid>> {
    r.try_get::<Option<&str>, _>(col)?.map(Uuid::parse_str).transpose().map_err(Into::into)
}

fn opt_json<T: serde::de::DeserializeOwned>(r: &SqliteRow, col: &str) -> Result<Option<T>> {
    r.try_get::<Option<&str>, _>(col)?.map(serde_json::from_str).transpose().map_err(Into::into)
}

fn to_json<T: Serialize + ?Sized>(v: Option<&T>) -> Result<Option<String>> {
    v.map(serde_json::to_string).transpose().map_err(Into::into)
}

/// Defaults for new runs, set in the Settings dialog. Each `None` leaves cedar's own default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Preferences {
    /// Tool groups and tool names; `None` means every tool cedar offers.
    pub tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub system: Option<String>,
    pub max_turns: Option<u32>,
}

fn conversation_from_row(r: &SqliteRow) -> Result<Conversation> {
    Ok(Conversation {
        id: Uuid::parse_str(r.try_get("id")?)?,
        title: r.try_get("title")?,
        model: r.try_get("model")?,
        reasoning: r.try_get("reasoning")?,
        schedule_id: opt_uuid(r, "schedule_id")?,
        tools: opt_json(r, "tools")?,
        created_at: to_dt(r.try_get("created_at")?),
        updated_at: to_dt(r.try_get("updated_at")?),
    })
}

fn schedule_from_row(r: &SqliteRow) -> Result<Schedule> {
    Ok(Schedule {
        id: Uuid::parse_str(r.try_get("id")?)?,
        title: r.try_get("title")?,
        prompt: r.try_get("prompt")?,
        model: r.try_get("model")?,
        reasoning: r.try_get("reasoning")?,
        source_conversation_id: opt_uuid(r, "source_conversation_id")?,
        tools: opt_json(r, "tools")?,
        frequency: r.try_get("frequency")?,
        weekday: r.try_get::<Option<i64>, _>("weekday")?.map(|d| d as u8),
        time_of_day: r.try_get("time_of_day")?,
        timezone: r.try_get("timezone")?,
        enabled: r.try_get::<i64, _>("enabled")? != 0,
        next_run_at: to_dt(r.try_get("next_run_at")?),
        last_run_at: r.try_get::<Option<i64>, _>("last_run_at")?.map(to_dt),
        created_at: to_dt(r.try_get("created_at")?),
        updated_at: to_dt(r.try_get("updated_at")?),
    })
}

fn schedule_run_from_row(r: &SqliteRow) -> Result<ScheduleRun> {
    Ok(ScheduleRun {
        id: Uuid::parse_str(r.try_get("id")?)?,
        schedule_id: Uuid::parse_str(r.try_get("schedule_id")?)?,
        conversation_id: opt_uuid(r, "conversation_id")?,
        turn_id: opt_uuid(r, "turn_id")?,
        due_at: to_dt(r.try_get("due_at")?),
        started_at: to_dt(r.try_get("started_at")?),
        manual: r.try_get::<i64, _>("manual")? != 0,
        error: r.try_get("error")?,
    })
}

fn turn_from_row(r: &SqliteRow) -> Result<Turn> {
    Ok(Turn {
        id: Uuid::parse_str(r.try_get("id")?)?,
        conversation_id: Uuid::parse_str(r.try_get("conversation_id")?)?,
        seq: r.try_get("seq")?,
        run_id: Uuid::parse_str(r.try_get("run_id")?)?,
        prompt: r.try_get("prompt")?,
        status: r.try_get("status")?,
        final_text: r.try_get("final_text")?,
        error: r.try_get("error")?,
        attachments: opt_json(r, "attachments")?.unwrap_or_default(),
        created_at: to_dt(r.try_get("created_at")?),
    })
}

const TURN_COLS: &str = "id, conversation_id, seq, run_id, prompt, status, final_text, error, attachments, created_at";

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(url)
            .with_context(|| format!("invalid CHAT_DATABASE_URL {url:?}"))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await.context("opening database")?;
        Self::migrated(pool).await
    }

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

    // -- conversations ----------------------------------------------------------------------

    pub async fn create_conversation(&self, title: Option<&str>) -> Result<Conversation> {
        let id = Uuid::now_v7();
        let now = now_ms();
        sqlx::query("INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(id.to_string())
            .bind(title)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(self.get_conversation(id).await?.expect("just inserted"))
    }

    pub async fn get_conversation(&self, id: Uuid) -> Result<Option<Conversation>> {
        let row = sqlx::query("SELECT * FROM conversations WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(conversation_from_row).transpose()
    }

    /// Most recently active first. Conversations a schedule created are left out; they are
    /// listed under their schedule.
    pub async fn list_conversations(&self, limit: u32) -> Result<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT * FROM conversations WHERE schedule_id IS NULL ORDER BY updated_at DESC, id DESC LIMIT ?",
        )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(conversation_from_row).collect()
    }

    /// Set (or, with `None`, clear) the tools a conversation uses instead of the defaults.
    pub async fn set_conversation_tools(&self, id: Uuid, tools: Option<&[String]>) -> Result<bool> {
        let res = sqlx::query("UPDATE conversations SET tools = ? WHERE id = ?")
            .bind(to_json(tools)?)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn rename_conversation(&self, id: Uuid, title: &str) -> Result<bool> {
        let res = sqlx::query("UPDATE conversations SET title = ? WHERE id = ?")
            .bind(title)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn delete_conversation(&self, id: Uuid) -> Result<bool> {
        let res = sqlx::query("DELETE FROM conversations WHERE id = ?").bind(id.to_string()).execute(&self.pool).await?;
        Ok(res.rows_affected() == 1)
    }

    // -- turns ------------------------------------------------------------------------------

    /// Append a turn, bump the conversation's activity time, remember the model and reasoning
    /// level, and name the conversation after its first message if it has no title yet.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_turn(
        &self,
        conversation_id: Uuid,
        run_id: Uuid,
        prompt: &str,
        status: &str,
        model: Option<&str>,
        reasoning: Option<&str>,
        attachments: &[AttachmentInfo],
    ) -> Result<Turn> {
        let id = Uuid::now_v7();
        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO turns (id, conversation_id, seq, run_id, prompt, status, attachments, created_at) \
             SELECT ?1, ?2, COALESCE(MAX(seq), 0) + 1, ?3, ?4, ?5, ?6, ?7 FROM turns WHERE conversation_id = ?2",
        )
        .bind(id.to_string())
        .bind(conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(prompt)
        .bind(status)
        .bind(to_json(if attachments.is_empty() { None } else { Some(attachments) })?)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE conversations SET updated_at = ?, model = ?, reasoning = ?, title = COALESCE(title, ?) WHERE id = ?",
        )
        .bind(now)
        .bind(model)
        .bind(reasoning)
        .bind(title_from(prompt))
        .bind(conversation_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(self.get_turn(id).await?.expect("just inserted"))
    }

    pub async fn get_turn(&self, id: Uuid) -> Result<Option<Turn>> {
        let row = sqlx::query(&format!("SELECT {TURN_COLS} FROM turns WHERE id = ?"))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(turn_from_row).transpose()
    }

    pub async fn list_turns(&self, conversation_id: Uuid) -> Result<Vec<Turn>> {
        let rows = sqlx::query(&format!("SELECT {TURN_COLS} FROM turns WHERE conversation_id = ? ORDER BY seq"))
            .bind(conversation_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(turn_from_row).collect()
    }

    pub async fn update_turn(&self, id: Uuid, status: &str, final_text: Option<&str>, error: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE turns SET status = ?, final_text = ?, error = ? WHERE id = ?")
            .bind(status)
            .bind(final_text)
            .bind(error)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // -- settings ---------------------------------------------------------------------------

    pub async fn preferences(&self) -> Result<Preferences> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = 'preferences'").fetch_optional(&self.pool).await?;
        match row {
            Some(r) => Ok(serde_json::from_str(r.try_get("value")?)?),
            None => Ok(Preferences::default()),
        }
    }

    pub async fn set_preferences(&self, prefs: &Preferences) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('preferences', ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(serde_json::to_string(prefs)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // -- schedules --------------------------------------------------------------------------

    pub async fn create_schedule(&self, new: &NewSchedule<'_>) -> Result<Schedule> {
        let id = Uuid::now_v7();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO schedules (id, title, prompt, model, reasoning, source_conversation_id, tools, frequency, \
             weekday, time_of_day, timezone, enabled, next_run_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(new.title)
        .bind(new.prompt)
        .bind(new.model)
        .bind(new.reasoning)
        .bind(new.source_conversation_id.map(|c| c.to_string()))
        .bind(to_json(new.tools)?)
        .bind(new.frequency)
        .bind(new.weekday.map(i64::from))
        .bind(new.time_of_day)
        .bind(new.timezone)
        .bind(new.next_run_at.timestamp_millis())
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(self.get_schedule(id).await?.expect("just inserted"))
    }

    pub async fn get_schedule(&self, id: Uuid) -> Result<Option<Schedule>> {
        let row = sqlx::query("SELECT * FROM schedules WHERE id = ?").bind(id.to_string()).fetch_optional(&self.pool).await?;
        row.as_ref().map(schedule_from_row).transpose()
    }

    /// Enabled schedules first, soonest first.
    pub async fn list_schedules(&self) -> Result<Vec<Schedule>> {
        let rows = sqlx::query("SELECT * FROM schedules ORDER BY enabled DESC, next_run_at, id").fetch_all(&self.pool).await?;
        rows.iter().map(schedule_from_row).collect()
    }

    pub async fn count_schedules(&self) -> Result<i64> {
        Ok(sqlx::query("SELECT COUNT(*) AS n FROM schedules").fetch_one(&self.pool).await?.try_get("n")?)
    }

    /// Write back a schedule's editable fields (everything but its id, origin and timestamps).
    pub async fn update_schedule(&self, s: &Schedule) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE schedules SET title = ?, prompt = ?, model = ?, reasoning = ?, tools = ?, frequency = ?, \
             weekday = ?, time_of_day = ?, timezone = ?, enabled = ?, next_run_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&s.title)
        .bind(&s.prompt)
        .bind(&s.model)
        .bind(&s.reasoning)
        .bind(to_json(s.tools.as_ref())?)
        .bind(&s.frequency)
        .bind(s.weekday.map(i64::from))
        .bind(&s.time_of_day)
        .bind(&s.timezone)
        .bind(s.enabled as i64)
        .bind(s.next_run_at.timestamp_millis())
        .bind(now_ms())
        .bind(s.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Deleting a schedule takes its runs and the conversations they created with it.
    pub async fn delete_schedule(&self, id: Uuid) -> Result<bool> {
        let res = sqlx::query("DELETE FROM schedules WHERE id = ?").bind(id.to_string()).execute(&self.pool).await?;
        Ok(res.rows_affected() == 1)
    }

    /// Enabled schedules whose next run is at or before `now`, earliest first.
    pub async fn due_schedules(&self, now: DateTime<Utc>) -> Result<Vec<Schedule>> {
        let rows = sqlx::query("SELECT * FROM schedules WHERE enabled = 1 AND next_run_at <= ? ORDER BY next_run_at, id")
            .bind(now.timestamp_millis())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(schedule_from_row).collect()
    }

    /// Record a firing: when it ran and, unless `next_run_at` is `None` (a manual run), when the
    /// next one is due.
    pub async fn mark_fired(&self, id: Uuid, last_run_at: DateTime<Utc>, next_run_at: Option<DateTime<Utc>>) -> Result<()> {
        sqlx::query("UPDATE schedules SET last_run_at = ?, next_run_at = COALESCE(?, next_run_at) WHERE id = ?")
            .bind(last_run_at.timestamp_millis())
            .bind(next_run_at.map(|t| t.timestamp_millis()))
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// A conversation that belongs to a schedule: hidden from the main list, deleted with it.
    pub async fn create_scheduled_conversation(&self, schedule_id: Uuid, title: &str) -> Result<Conversation> {
        let id = Uuid::now_v7();
        let now = now_ms();
        sqlx::query("INSERT INTO conversations (id, title, schedule_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?)")
            .bind(id.to_string())
            .bind(title)
            .bind(schedule_id.to_string())
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(self.get_conversation(id).await?.expect("just inserted"))
    }

    pub async fn insert_schedule_run(&self, new: &NewScheduleRun) -> Result<ScheduleRun> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO schedule_runs (id, schedule_id, conversation_id, turn_id, due_at, started_at, manual, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(new.schedule_id.to_string())
        .bind(new.conversation_id.map(|c| c.to_string()))
        .bind(new.turn_id.map(|t| t.to_string()))
        .bind(new.due_at.timestamp_millis())
        .bind(now_ms())
        .bind(new.manual as i64)
        .bind(&new.error)
        .execute(&self.pool)
        .await?;
        let row = sqlx::query("SELECT * FROM schedule_runs WHERE id = ?").bind(id.to_string()).fetch_one(&self.pool).await?;
        schedule_run_from_row(&row)
    }

    /// Newest first.
    pub async fn list_schedule_runs(&self, schedule_id: Uuid, limit: u32) -> Result<Vec<ScheduleRun>> {
        let rows = sqlx::query("SELECT * FROM schedule_runs WHERE schedule_id = ? ORDER BY started_at DESC, id DESC LIMIT ?")
            .bind(schedule_id.to_string())
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(schedule_run_from_row).collect()
    }

    /// Every turn of every conversation the schedule created, so their cedar runs can be deleted.
    pub async fn list_schedule_turns(&self, schedule_id: Uuid) -> Result<Vec<Turn>> {
        let rows = sqlx::query(&format!(
            "SELECT {} FROM turns t JOIN conversations c ON c.id = t.conversation_id WHERE c.schedule_id = ? ORDER BY t.created_at",
            TURN_COLS.split(", ").map(|c| format!("t.{c}")).collect::<Vec<_>>().join(", ")
        ))
        .bind(schedule_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(turn_from_row).collect()
    }
}

/// A short title from the first line of a message.
pub fn title_from(prompt: &str) -> String {
    const MAX_CHARS: usize = 60;
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("New chat");
    if line.chars().count() <= MAX_CHARS {
        return line.to_string();
    }
    let cut: String = line.chars().take(MAX_CHARS).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head).filter(|h| h.len() > 20).unwrap_or(&cut);
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn turns_are_numbered_and_name_the_conversation() {
        let store = Store::connect_memory().await.unwrap();
        let c = store.create_conversation(None).await.unwrap();
        assert!(c.title.is_none());
        let t1 = store.insert_turn(c.id, Uuid::new_v4(), "\n  first question\nmore", "queued", Some("m"), Some("high"), &[]).await.unwrap();
        let t2 = store.insert_turn(c.id, Uuid::new_v4(), "second", "queued", None, None, &[]).await.unwrap();
        assert_eq!((t1.seq, t2.seq), (1, 2));
        let c = store.get_conversation(c.id).await.unwrap().unwrap();
        assert_eq!(c.title.as_deref(), Some("first question"));

        store.update_turn(t1.id, "completed", Some("answer"), None).await.unwrap();
        let turns = store.list_turns(c.id).await.unwrap();
        assert_eq!(turns[0].final_text.as_deref(), Some("answer"));

        assert!(store.delete_conversation(c.id).await.unwrap());
        assert!(store.get_turn(t2.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn renamed_conversations_keep_their_title() {
        let store = Store::connect_memory().await.unwrap();
        let c = store.create_conversation(Some("Mine")).await.unwrap();
        store.insert_turn(c.id, Uuid::new_v4(), "hello", "queued", None, None, &[]).await.unwrap();
        assert_eq!(store.get_conversation(c.id).await.unwrap().unwrap().title.as_deref(), Some("Mine"));
    }

    #[tokio::test]
    async fn scheduled_conversations_are_hidden_and_go_with_their_schedule() {
        let store = Store::connect_memory().await.unwrap();
        let s = store
            .create_schedule(&NewSchedule {
                title: "Daily digest",
                prompt: "summarize",
                model: None,
                reasoning: None,
                source_conversation_id: None,
                tools: Some(&["weather".to_string()]),
                frequency: "daily",
                weekday: None,
                time_of_day: "09:00",
                timezone: "UTC",
                next_run_at: to_dt(1_000),
            })
            .await
            .unwrap();
        assert!(s.enabled && s.last_run_at.is_none());
        assert_eq!(s.tools, Some(vec!["weather".to_string()]));
        let plain = store.create_conversation(Some("Mine")).await.unwrap();
        let c = store.create_scheduled_conversation(s.id, "Daily digest · Mon").await.unwrap();
        assert_eq!(c.schedule_id, Some(s.id));
        let t = store.insert_turn(c.id, Uuid::new_v4(), "summarize", "queued", None, None, &[]).await.unwrap();
        assert_eq!(store.get_conversation(c.id).await.unwrap().unwrap().title.as_deref(), Some("Daily digest · Mon"));
        let run = store
            .insert_schedule_run(&NewScheduleRun {
                schedule_id: s.id,
                conversation_id: Some(c.id),
                turn_id: Some(t.id),
                due_at: to_dt(1_000),
                manual: false,
                error: None,
            })
            .await
            .unwrap();
        assert_eq!(run.turn_id, Some(t.id));

        let listed: Vec<Uuid> = store.list_conversations(10).await.unwrap().iter().map(|c| c.id).collect();
        assert_eq!(listed, [plain.id], "scheduled conversations stay out of the main list");
        assert_eq!(store.list_schedule_turns(s.id).await.unwrap().len(), 1);

        store.mark_fired(s.id, to_dt(2_000), Some(to_dt(90_000))).await.unwrap();
        assert!(store.due_schedules(to_dt(80_000)).await.unwrap().is_empty());
        assert_eq!(store.due_schedules(to_dt(90_000)).await.unwrap().len(), 1);
        store.mark_fired(s.id, to_dt(3_000), None).await.unwrap();
        let s = store.get_schedule(s.id).await.unwrap().unwrap();
        assert_eq!((s.last_run_at, s.next_run_at), (Some(to_dt(3_000)), to_dt(90_000)));

        assert!(store.delete_schedule(s.id).await.unwrap());
        assert!(store.get_conversation(c.id).await.unwrap().is_none(), "its conversations are deleted too");
        assert!(store.get_turn(t.id).await.unwrap().is_none());
        assert!(store.list_schedule_runs(s.id, 10).await.unwrap().is_empty());
        assert!(store.get_conversation(plain.id).await.unwrap().is_some());
    }

    #[test]
    fn titles_are_shortened_on_a_word_boundary() {
        assert_eq!(title_from("   "), "New chat");
        let t = title_from(&"word ".repeat(30));
        assert!(t.ends_with('…') && t.chars().count() <= 61);
        assert!(!t.contains("wor…"));
    }

    #[tokio::test]
    async fn preferences_and_conversation_tools_round_trip() {
        let store = Store::connect_memory().await.unwrap();
        assert_eq!(store.preferences().await.unwrap(), Preferences::default());
        let prefs = Preferences {
            tools: Some(vec!["maps".into()]),
            model: Some("m".into()),
            system: Some("be brief".into()),
            max_turns: Some(5),
        };
        store.set_preferences(&prefs).await.unwrap();
        store.set_preferences(&prefs).await.unwrap();
        assert_eq!(store.preferences().await.unwrap(), prefs);

        let c = store.create_conversation(None).await.unwrap();
        assert_eq!(c.tools, None);
        assert!(store.set_conversation_tools(c.id, Some(&["web_search".to_string()])).await.unwrap());
        assert_eq!(store.get_conversation(c.id).await.unwrap().unwrap().tools, Some(vec!["web_search".to_string()]));
        store.set_conversation_tools(c.id, None).await.unwrap();
        assert_eq!(store.get_conversation(c.id).await.unwrap().unwrap().tools, None);
    }

    #[tokio::test]
    async fn turns_keep_attachment_metadata() {
        let store = Store::connect_memory().await.unwrap();
        let c = store.create_conversation(None).await.unwrap();
        let files = vec![AttachmentInfo { name: "a.png".into(), media_type: "image/png".into(), size: 3 }];
        let with = store.insert_turn(c.id, Uuid::new_v4(), "look", "queued", None, None, &files).await.unwrap();
        let without = store.insert_turn(c.id, Uuid::new_v4(), "next", "queued", None, None, &[]).await.unwrap();
        assert_eq!(store.get_turn(with.id).await.unwrap().unwrap().attachments, files);
        assert!(store.get_turn(without.id).await.unwrap().unwrap().attachments.is_empty());
        let turns = store.list_turns(c.id).await.unwrap();
        assert_eq!(turns.iter().map(|t| t.attachments.len()).collect::<Vec<_>>(), [1, 0]);
    }
}
