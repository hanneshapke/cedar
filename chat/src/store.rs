//! SQLite persistence for conversations and their turns.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
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
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    pub created_at: DateTime<Utc>,
}

fn conversation_from_row(r: &SqliteRow) -> Result<Conversation> {
    Ok(Conversation {
        id: Uuid::parse_str(r.try_get("id")?)?,
        title: r.try_get("title")?,
        model: r.try_get("model")?,
        created_at: to_dt(r.try_get("created_at")?),
        updated_at: to_dt(r.try_get("updated_at")?),
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
        created_at: to_dt(r.try_get("created_at")?),
    })
}

const TURN_COLS: &str = "id, conversation_id, seq, run_id, prompt, status, final_text, error, created_at";

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

    /// Most recently active first.
    pub async fn list_conversations(&self, limit: u32) -> Result<Vec<Conversation>> {
        let rows = sqlx::query("SELECT * FROM conversations ORDER BY updated_at DESC, id DESC LIMIT ?")
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(conversation_from_row).collect()
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

    /// Append a turn, bump the conversation's activity time, remember the model, and name the
    /// conversation after its first message if it has no title yet.
    pub async fn insert_turn(
        &self,
        conversation_id: Uuid,
        run_id: Uuid,
        prompt: &str,
        status: &str,
        model: Option<&str>,
    ) -> Result<Turn> {
        let id = Uuid::now_v7();
        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO turns (id, conversation_id, seq, run_id, prompt, status, created_at) \
             SELECT ?1, ?2, COALESCE(MAX(seq), 0) + 1, ?3, ?4, ?5, ?6 FROM turns WHERE conversation_id = ?2",
        )
        .bind(id.to_string())
        .bind(conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(prompt)
        .bind(status)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE conversations SET updated_at = ?, model = ?, title = COALESCE(title, ?) WHERE id = ?",
        )
        .bind(now)
        .bind(model)
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
        let t1 = store.insert_turn(c.id, Uuid::new_v4(), "\n  first question\nmore", "queued", Some("m")).await.unwrap();
        let t2 = store.insert_turn(c.id, Uuid::new_v4(), "second", "queued", None).await.unwrap();
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
        store.insert_turn(c.id, Uuid::new_v4(), "hello", "queued", None).await.unwrap();
        assert_eq!(store.get_conversation(c.id).await.unwrap().unwrap().title.as_deref(), Some("Mine"));
    }

    #[test]
    fn titles_are_shortened_on_a_word_boundary() {
        assert_eq!(title_from("   "), "New chat");
        let t = title_from(&"word ".repeat(30));
        assert!(t.ends_with('…') && t.chars().count() <= 61);
        assert!(!t.contains("wor…"));
    }
}
