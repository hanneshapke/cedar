//! What the agent remembers about the person behind an API key across conversations.
//!
//! Memory is typed: facts, preferences and standing instructions the user stated, temporary
//! facts with an end date, episodes (what a conversation was about and how it ended) and
//! procedures (how a recurring task went well for this user). Every row carries where it came
//! from (the user's own words, quoted in `support`, or the agent's own experience), when it was
//! true (event time) separately from when cedar learned it, and its versions: a contradicted
//! fact is superseded and kept, never overwritten; a row that no longer matters is archived,
//! never silently dropped. Every change is an audit row in `memory_ops`.
//!
//! A run is shown a small core (the owner's profile, written by the background
//! [`consolidate`] pass, and pinned instructions) plus what [`recall`] selects for the request;
//! `note` builds that part of the system prompt per request, so a deletion applies to the very
//! next run. The `remember`, `forget` and `recall` tools (see [`crate::tools::Remember`]) let
//! the model read and change memory in a run, and once a run has answered, one short call reads
//! the conversation and notes what it learned (see [`extract`]).
//!
//! The design and the research behind it are in `docs/design/memory.md`.

pub mod consolidate;
pub mod extract;
pub mod recall;

use crate::store::{now_ms, to_dt, Store};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};
use std::collections::HashMap;
use uuid::Uuid;

/// The most characters one memory may have.
pub const MAX_CHARS: usize = 500;
/// The most characters the profile may have (about 400 tokens).
pub const MAX_PROFILE_CHARS: usize = 1_600;
/// How many facts, preferences, instructions and temporary rows an owner keeps active unless
/// `MEMORY_MAX_ITEMS` says otherwise; beyond it the lowest-ranked rows are archived.
pub const DEFAULT_MAX_ITEMS: usize = 500;
/// How many raw episodes an owner keeps active; older ones are rolled up and archived.
pub const MAX_EPISODES: usize = 2_000;
/// How many procedures an owner keeps active.
pub const MAX_PROCEDURES: usize = 200;
/// Two texts this similar (see [`recall::similarity`]) are the same memory said twice.
pub const DUPLICATE_SIMILARITY: f64 = 0.8;

/// What a memory is, which decides how it is shown and how it ages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// One statement about the user: where they live, their work, their family.
    Fact,
    /// How they want things: units, language, tone, tools.
    Preference,
    /// A standing request. Pinned by default, so it is in every prompt.
    Instruction,
    /// A fact with a known end: an exam next week, a trip until Friday.
    Temporary,
    /// What a conversation was about and how it ended.
    Episode,
    /// How a recurring task went well for this user, abstracted.
    Procedure,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Fact,
        Kind::Preference,
        Kind::Instruction,
        Kind::Temporary,
        Kind::Episode,
        Kind::Procedure,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Fact => "fact",
            Kind::Preference => "preference",
            Kind::Instruction => "instruction",
            Kind::Temporary => "temporary",
            Kind::Episode => "episode",
            Kind::Procedure => "procedure",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL
            .into_iter()
            .find(|k| k.as_str() == s.trim().to_lowercase())
    }

    /// The kinds that hold what the user said about themselves.
    pub fn is_about_user(self) -> bool {
        !matches!(self, Kind::Episode | Kind::Procedure)
    }
}

/// Who wrote a memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The user's own words, in a conversation.
    User,
    /// The agent's own experience: what it did and how it went.
    Agent,
    /// The background pass that merges and rolls up.
    Consolidation,
    /// Added by hand through the API.
    Api,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::User => "user",
            Source::Agent => "agent",
            Source::Consolidation => "consolidation",
            Source::Api => "api",
        }
    }

    fn parse(s: &str) -> Source {
        match s {
            "agent" => Source::Agent,
            "consolidation" => Source::Consolidation,
            "api" => Source::Api,
            _ => Source::User,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Memory {
    pub id: Uuid,
    /// The id the model sees: `m<seq>`.
    pub short_id: String,
    pub kind: Kind,
    pub text: String,
    pub subject: Option<String>,
    pub relation: Option<String>,
    pub topics: Vec<String>,
    pub source: Source,
    /// The user's own words this was taken from, for user-sourced rows.
    pub support: Option<String>,
    pub hedged: bool,
    pub sensitive: bool,
    pub pinned: bool,
    pub mentions: i64,
    pub meta: Value,
    /// The run that saved it; `None` for one added by hand, or when that run was deleted.
    pub run_id: Option<Uuid>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub superseded_by: Option<Uuid>,
    pub archived_at: Option<DateTime<Utc>>,
    pub recall_count: i64,
    pub last_recalled_at: Option<DateTime<Utc>>,
}

impl Memory {
    /// In the active set: not archived, not superseded.
    pub fn is_active(&self) -> bool {
        self.archived_at.is_none() && self.superseded_by.is_none()
    }

    /// True at `now` as far as its validity interval says.
    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        self.valid_from.is_none_or(|from| from <= now)
            && self.valid_until.is_none_or(|until| until > now)
    }

    /// How much this memory should weigh when there is more than fits: its kind's base weight,
    /// strengthened by repetition and recall, decayed by disuse down to a floor of 0.3, and
    /// never decayed when pinned.
    pub fn rank(&self, now: DateTime<Utc>) -> f64 {
        let base = match self.kind {
            Kind::Instruction => 1.0,
            Kind::Preference => 0.9,
            Kind::Fact => 0.8,
            Kind::Temporary => 0.8,
            Kind::Procedure => 0.6,
            Kind::Episode => 0.4,
        };
        let strength = 1.0
            + 0.1 * ((self.mentions - 1).max(0) as f64).min(5.0)
            + 0.05 * (self.recall_count as f64).min(10.0);
        if self.pinned {
            return base * strength;
        }
        let since = self.last_recalled_at.unwrap_or(self.updated_at);
        let days = (now - since).num_seconds().max(0) as f64 / 86_400.0;
        // Half of the way to the floor after 30 days of disuse.
        let decay = 0.3 + 0.7 * 0.5f64.powf(days / 30.0);
        base * strength * decay
    }

    pub fn meta_str(&self, key: &str) -> Option<&str> {
        self.meta[key].as_str()
    }
}

/// What [`Memories::remember`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Saved {
    /// A new memory.
    Added,
    /// A new memory that supersedes an older one, which is kept as history.
    Superseded,
    /// The memory it `replaces` now says the new text.
    Replaced,
    /// The same thing was already remembered; it counts as said once more.
    Unchanged,
}

/// Why [`Memories::remember`] saved nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    Empty,
    TooLong(usize),
    /// The text looks like a secret or an identifier that is never kept.
    Excluded(&'static str),
    /// The memory to supersede isn't the owner's, or doesn't exist.
    NotFound,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Empty => write!(f, "the text is empty"),
            Refused::TooLong(n) => write!(
                f,
                "a memory may have at most {MAX_CHARS} characters, this one has {n}; keep it to one short fact"
            ),
            Refused::Excluded(why) => write!(f, "not kept: {why}"),
            Refused::NotFound => write!(f, "there is no memory with that id"),
        }
    }
}

/// A memory to save.
#[derive(Debug, Clone, Default)]
pub struct NewMemory {
    pub kind: Option<Kind>,
    pub text: String,
    pub subject: Option<String>,
    pub relation: Option<String>,
    pub topics: Vec<String>,
    pub source: Option<Source>,
    pub support: Option<String>,
    pub hedged: bool,
    pub sensitive: bool,
    pub pinned: Option<bool>,
    pub meta: Option<Value>,
    pub run_id: Option<Uuid>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    /// The memory this one supersedes (it stays as history), or `replaces` in place.
    pub supersedes: Option<Uuid>,
    pub replaces: Option<Uuid>,
}

impl NewMemory {
    pub fn text(text: &str) -> Self {
        Self {
            text: text.to_string(),
            ..Default::default()
        }
    }
}

/// The memory text as it is kept: one line, its whitespace collapsed, so every memory is one line
/// of the note and no text can pass for another entry.
pub fn tidy(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The form two texts are compared in to find an exact duplicate.
pub fn same(a: &str, b: &str) -> bool {
    a.to_lowercase().trim_end_matches('.') == b.to_lowercase().trim_end_matches('.')
}

/// Why a text is never kept: it looks like a secret or an identifier. These are checks in code,
/// not a request to the model.
pub fn excluded(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    let secret_words = [
        "password",
        "passwd",
        "passphrase",
        "api key",
        "api-key",
        "apikey",
        "secret key",
        "private key",
        "access token",
        "bearer ",
    ];
    if secret_words.iter().any(|w| lower.contains(w)) {
        let has_value = lower.chars().any(|c| c == ':' || c == '=')
            || lower
                .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                .any(|w| {
                    w.len() >= 6
                        && w.chars().any(|c| c.is_ascii_digit())
                        && w.chars().any(|c| c.is_ascii_alphabetic())
                });
        if has_value {
            return Some("it looks like a password or a key");
        }
    }
    for word in lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_') {
        if word.len() >= 20
            && word.chars().any(|c| c.is_ascii_digit())
            && word.chars().any(|c| c.is_ascii_alphabetic())
            && ["sk-", "ghp_", "xoxb-", "akia", "ya29.", "eyj"]
                .iter()
                .any(|p| word.starts_with(p))
        {
            return Some("it looks like an access token");
        }
    }
    // Long digit runs, with spaces or dashes: card, account and government identifiers.
    let mut digits = 0;
    let mut run = 0;
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits += 1;
            run += 1;
        } else if c == ' ' || c == '-' {
            // keep the run going
        } else {
            run = 0;
        }
        if run >= 13 {
            return Some("it looks like a card, account or identity number");
        }
    }
    let _ = digits;
    let id_words = [
        "iban",
        "social security",
        "ssn",
        "passport number",
        "credit card",
        "card number",
        "routing number",
        "account number",
        "ahv-nummer",
        "steuer-id",
        "numéro de sécurité sociale",
    ];
    if id_words.iter().any(|w| lower.contains(w))
        && text.chars().filter(|c| c.is_ascii_digit()).count() >= 6
    {
        return Some("it looks like a financial or government identifier");
    }
    None
}

fn parse_topics(s: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
}

fn from_row(r: &SqliteRow) -> Result<Memory> {
    let run_id: Option<&str> = r.try_get("run_id")?;
    let superseded_by: Option<&str> = r.try_get("superseded_by")?;
    let seq: i64 = r.try_get("seq")?;
    let kind: &str = r.try_get("kind")?;
    let source: &str = r.try_get("source")?;
    let topics: &str = r.try_get("topics")?;
    let meta: &str = r.try_get("meta")?;
    Ok(Memory {
        id: Uuid::parse_str(r.try_get::<&str, _>("id")?)?,
        short_id: format!("m{seq}"),
        kind: Kind::parse(kind).ok_or_else(|| anyhow!("unknown memory kind {kind}"))?,
        text: r.try_get("text")?,
        subject: r.try_get("subject")?,
        relation: r.try_get("relation")?,
        topics: parse_topics(topics),
        source: Source::parse(source),
        support: r.try_get("support")?,
        hedged: r.try_get::<i64, _>("hedged")? != 0,
        sensitive: r.try_get::<i64, _>("sensitive")? != 0,
        pinned: r.try_get::<i64, _>("pinned")? != 0,
        mentions: r.try_get("mentions")?,
        meta: serde_json::from_str(meta).unwrap_or_else(|_| json!({})),
        run_id: run_id.map(Uuid::parse_str).transpose()?,
        valid_from: r.try_get::<Option<i64>, _>("valid_from")?.map(to_dt),
        valid_until: r.try_get::<Option<i64>, _>("valid_until")?.map(to_dt),
        created_at: to_dt(r.try_get("created_at")?),
        updated_at: to_dt(r.try_get("updated_at")?),
        superseded_by: superseded_by.map(Uuid::parse_str).transpose()?,
        archived_at: r.try_get::<Option<i64>, _>("archived_at")?.map(to_dt),
        recall_count: r.try_get("recall_count")?,
        last_recalled_at: r.try_get::<Option<i64>, _>("last_recalled_at")?.map(to_dt),
    })
}

const COLS: &str = "id, seq, kind, text, subject, relation, topics, source, support, hedged, sensitive, \
    pinned, mentions, meta, run_id, valid_from, valid_until, created_at, updated_at, superseded_by, \
    archived_at, recall_count, last_recalled_at";

/// One audit row.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Op {
    pub id: i64,
    pub memory_id: String,
    pub op: String,
    pub actor: String,
    pub at: DateTime<Utc>,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// The owner's profile.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Profile {
    pub text: String,
    /// The user wrote it; consolidation leaves it alone.
    pub user_edited: bool,
    pub updated_at: DateTime<Utc>,
}

/// Which rows [`Memories::list`] returns.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub kinds: Vec<Kind>,
    /// Archived and superseded rows instead of active ones.
    pub archived: bool,
    pub query: Option<String>,
}

/// What a run's memory section may hold, in tokens, beyond the profile and what is pinned.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_items: usize,
    pub recall_tokens: usize,
    pub consolidate_after: u32,
}

/// The memories of every owner, in the run store's database.
#[derive(Clone)]
pub struct Memories {
    store: Store,
    pub limits: Limits,
}

/// Who changed a memory, for the audit log.
#[derive(Debug, Clone, Copy)]
pub enum Actor<'a> {
    Run(Uuid),
    User,
    Consolidation,
    Other(&'a str),
}

impl std::fmt::Display for Actor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Actor::Run(id) => write!(f, "{id}"),
            Actor::User => write!(f, "user"),
            Actor::Consolidation => write!(f, "consolidation"),
            Actor::Other(s) => write!(f, "{s}"),
        }
    }
}

async fn log(
    tx: &mut SqliteConnection,
    owner: &str,
    memory_id: &str,
    op: &str,
    actor: Actor<'_>,
    before: Option<&str>,
    after: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO memory_ops (owner, memory_id, op, actor, at, before, after) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(owner)
    .bind(memory_id)
    .bind(op)
    .bind(actor.to_string())
    .bind(now_ms())
    .bind(before)
    .bind(after)
    .execute(tx)
    .await?;
    Ok(())
}

async fn fetch(tx: &mut SqliteConnection, id: &str) -> Result<Memory> {
    let row = sqlx::query(&format!("SELECT {COLS} FROM memories WHERE id = ?"))
        .bind(id)
        .fetch_one(tx)
        .await?;
    from_row(&row)
}

impl Memories {
    pub fn new(store: Store, limits: Limits) -> Self {
        Self { store, limits }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The limits, for clients that show how much fits. `max_items` is kept for clients from
    /// before kinds; it is the facts budget.
    pub fn limits_json(&self) -> Value {
        json!({
            "max_items": self.limits.max_items,
            "max_chars": MAX_CHARS,
            "max_profile_chars": MAX_PROFILE_CHARS,
            "budgets": {
                "facts": self.limits.max_items,
                "episodes": MAX_EPISODES,
                "procedures": MAX_PROCEDURES,
            },
            "recall_tokens": self.limits.recall_tokens,
        })
    }

    /// The owner's memories, oldest first, as the filter says: active rows unless it asks for
    /// archived ones, of the kinds it names (all when none), matching `query` when given.
    pub async fn list(&self, owner: &str, filter: &Filter) -> Result<Vec<Memory>> {
        let rows = sqlx::query(&format!(
            "SELECT {COLS} FROM memories WHERE owner = ? ORDER BY seq"
        ))
        .bind(owner)
        .fetch_all(self.store.pool())
        .await?;
        let mut items: Vec<Memory> = rows.iter().map(from_row).collect::<Result<_>>()?;
        items.retain(|m| m.is_active() != filter.archived);
        if !filter.kinds.is_empty() {
            items.retain(|m| filter.kinds.contains(&m.kind));
        }
        if let Some(q) = filter
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
        {
            let scored = recall::rank_by_similarity(q, &items);
            let lower = q.to_lowercase();
            let mut keep: Vec<Memory> = Vec::new();
            for (m, score) in scored {
                if score > 0.0 || m.text.to_lowercase().contains(&lower) {
                    keep.push(m);
                }
            }
            items = keep;
        }
        Ok(items)
    }

    /// The owner's active rows, oldest first.
    pub async fn active(&self, owner: &str) -> Result<Vec<Memory>> {
        self.list(owner, &Filter::default()).await
    }

    pub async fn get(&self, owner: &str, id: Uuid) -> Result<Option<Memory>> {
        let row = sqlx::query(&format!(
            "SELECT {COLS} FROM memories WHERE owner = ? AND id = ?"
        ))
        .bind(owner)
        .bind(id.to_string())
        .fetch_optional(self.store.pool())
        .await?;
        row.as_ref().map(from_row).transpose()
    }

    /// The memory an id names: a UUID, or the short `m<seq>` the model sees.
    pub async fn resolve(&self, owner: &str, id: &str) -> Result<Option<Memory>> {
        let id = id.trim().trim_matches(['[', ']']);
        if let Ok(uuid) = Uuid::parse_str(id) {
            return self.get(owner, uuid).await;
        }
        let Some(seq) = id.strip_prefix('m').and_then(|s| s.parse::<i64>().ok()) else {
            return Ok(None);
        };
        let row = sqlx::query(&format!(
            "SELECT {COLS} FROM memories WHERE owner = ? AND seq = ?"
        ))
        .bind(owner)
        .bind(seq)
        .fetch_optional(self.store.pool())
        .await?;
        row.as_ref().map(from_row).transpose()
    }

    /// Save a memory for `owner`. The same thing already remembered (the same text, or one that
    /// similar) is not saved twice: that memory counts as said once more. A keyed fact (same
    /// subject and relation as an active one, different text) supersedes the old one without
    /// being asked, as does `supersedes`; `replaces` changes a memory in place. When the kind's
    /// budget is full, the lowest-ranked unpinned row is archived to make room.
    pub async fn remember(
        &self,
        owner: &str,
        new: NewMemory,
        actor: Actor<'_>,
    ) -> Result<Result<(Memory, Saved), Refused>> {
        let text = tidy(&new.text);
        if text.is_empty() {
            return Ok(Err(Refused::Empty));
        }
        let chars = text.chars().count();
        if chars > MAX_CHARS {
            return Ok(Err(Refused::TooLong(chars)));
        }
        if let Some(why) = excluded(&text) {
            return Ok(Err(Refused::Excluded(why)));
        }
        let kind = new.kind.unwrap_or(Kind::Fact);
        let source = new.source.unwrap_or(Source::User);
        let now = now_ms();
        let now_dt = to_dt(now);
        let mut tx = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        let rows = sqlx::query(&format!(
            "SELECT {COLS} FROM memories WHERE owner = ? ORDER BY seq"
        ))
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        let all: Vec<Memory> = rows.iter().map(from_row).collect::<Result<_>>()?;
        let active: Vec<&Memory> = all.iter().filter(|m| m.is_active()).collect();
        let find = |id: Uuid| active.iter().find(|m| m.id == id).copied();
        let replaces = match new.replaces {
            Some(id) => match find(id) {
                Some(m) => Some(m),
                None => return Ok(Err(Refused::NotFound)),
            },
            None => None,
        };
        let supersedes = match new.supersedes {
            Some(id) => match find(id) {
                Some(m) => Some(m),
                None => return Ok(Err(Refused::NotFound)),
            },
            None => None,
        };

        // The same thing said again. Every episode is its own, so they are never deduplicated.
        let duplicate = active
            .iter()
            .filter(|_| kind != Kind::Episode)
            .filter(|m| m.kind == kind || (m.kind.is_about_user() && kind.is_about_user()))
            .find(|m| recall::same_thing(&m.text, &text))
            .copied();
        if let Some(dup) = duplicate.filter(|_| replaces.is_none()) {
            // Said with more detail: the fuller text wins.
            let fuller = recall::tokens(&text).len() > recall::tokens(&dup.text).len();
            sqlx::query("UPDATE memories SET updated_at = ?, mentions = mentions + 1, text = ?, support = COALESCE(?, support) WHERE id = ?")
                .bind(now)
                .bind(if fuller { &text } else { &dup.text })
                .bind(if fuller { new.support.as_deref().map(tidy) } else { None })
                .bind(dup.id.to_string())
                .execute(&mut *tx)
                .await?;
            if fuller {
                log(
                    &mut tx,
                    owner,
                    &dup.id.to_string(),
                    "update",
                    actor,
                    Some(&dup.text),
                    Some(&text),
                )
                .await?;
            }
            // Superseding with a duplicate of another memory: the old one is still gone.
            let mut saved = Saved::Unchanged;
            if let Some(old) = supersedes.filter(|o| o.id != dup.id) {
                self.close(&mut tx, owner, old, &dup.id.to_string(), now, actor)
                    .await?;
                saved = Saved::Superseded;
            }
            let m = fetch(&mut tx, &dup.id.to_string()).await?;
            tx.commit().await?;
            return Ok(Ok((m, saved)));
        }

        if let Some(old) = replaces {
            let before = old.text.clone();
            sqlx::query("UPDATE memories SET text = ?, run_id = COALESCE(?, run_id), updated_at = ?, support = COALESCE(?, support) WHERE id = ?")
                .bind(&text)
                .bind(new.run_id.map(|r| r.to_string()))
                .bind(now)
                .bind(new.support.as_deref().map(tidy))
                .bind(old.id.to_string())
                .execute(&mut *tx)
                .await?;
            log(
                &mut tx,
                owner,
                &old.id.to_string(),
                "update",
                actor,
                Some(&before),
                Some(&text),
            )
            .await?;
            let m = fetch(&mut tx, &old.id.to_string()).await?;
            tx.commit().await?;
            return Ok(Ok((m, Saved::Replaced)));
        }

        // A keyed fact supersedes the active one with the same key.
        let key = |m: &Memory| -> Option<(String, String)> {
            Some((
                m.subject.as_deref()?.trim().to_lowercase(),
                m.relation.as_deref()?.trim().to_lowercase(),
            ))
        };
        let new_key = match (&new.subject, &new.relation) {
            (Some(s), Some(r)) => Some((s.trim().to_lowercase(), r.trim().to_lowercase())),
            _ => None,
        };
        let keyed_old = new_key.as_ref().and_then(|k| {
            active
                .iter()
                .find(|m| m.kind.is_about_user() && key(m).as_ref() == Some(k))
                .copied()
        });
        let old = supersedes
            .or(keyed_old)
            .filter(|o| !o.pinned || supersedes.is_some());

        let seq: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) + 1 FROM memories WHERE owner = ?")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await?;
        let id = Uuid::now_v7().to_string();
        let pinned = new.pinned.unwrap_or(kind == Kind::Instruction);
        let valid_from = new.valid_from.or(old.map(|_| now_dt)).or(Some(now_dt));
        sqlx::query(
            "INSERT INTO memories (id, owner, seq, kind, text, subject, relation, topics, source, support, \
             hedged, sensitive, pinned, mentions, meta, run_id, valid_from, valid_until, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(owner)
        .bind(seq)
        .bind(kind.as_str())
        .bind(&text)
        .bind(new.subject.as_deref().map(str::trim))
        .bind(new.relation.as_deref().map(str::trim))
        .bind(serde_json::to_string(&new.topics)?)
        .bind(source.as_str())
        .bind(new.support.as_deref().map(tidy))
        .bind(new.hedged as i64)
        .bind(new.sensitive as i64)
        .bind(pinned as i64)
        .bind(new.meta.unwrap_or_else(|| json!({})).to_string())
        .bind(new.run_id.map(|r| r.to_string()))
        .bind(valid_from.map(|d| d.timestamp_millis()))
        .bind(new.valid_until.map(|d| d.timestamp_millis()))
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        log(&mut tx, owner, &id, "add", actor, None, Some(&text)).await?;
        let mut saved = Saved::Added;
        if let Some(old) = old {
            self.close(&mut tx, owner, old, &id, now, actor).await?;
            saved = Saved::Superseded;
        }
        // Make room: archive the lowest-ranked unpinned row of the kind's budget.
        let budget = match kind {
            Kind::Episode => MAX_EPISODES,
            Kind::Procedure => MAX_PROCEDURES,
            _ => self.limits.max_items,
        };
        let mut same_budget: Vec<&Memory> = active
            .iter()
            .filter(|m| match kind {
                Kind::Episode | Kind::Procedure => m.kind == kind,
                _ => m.kind.is_about_user(),
            })
            .filter(|m| !m.pinned && old.is_none_or(|o| o.id != m.id))
            .copied()
            .collect();
        // Pinned rows are never archived, so they don't count against the budget.
        let count = same_budget.len() + usize::from(!pinned);
        if count > budget {
            same_budget.sort_by(|a, b| {
                a.rank(now_dt)
                    .partial_cmp(&b.rank(now_dt))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for m in same_budget.iter().take(count - budget) {
                sqlx::query("UPDATE memories SET archived_at = ?, updated_at = ? WHERE id = ?")
                    .bind(now)
                    .bind(now)
                    .bind(m.id.to_string())
                    .execute(&mut *tx)
                    .await?;
                log(
                    &mut tx,
                    owner,
                    &m.id.to_string(),
                    "archive",
                    actor,
                    Some(&m.text),
                    Some("over budget"),
                )
                .await?;
            }
        }
        let m = fetch(&mut tx, &id).await?;
        tx.commit().await?;
        Ok(Ok((m, saved)))
    }

    /// Close `old`: superseded by `by`, its validity ended now unless it already had an end.
    async fn close(
        &self,
        tx: &mut SqliteConnection,
        owner: &str,
        old: &Memory,
        by: &str,
        now: i64,
        actor: Actor<'_>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE memories SET superseded_by = ?, valid_until = COALESCE(valid_until, ?), updated_at = ? WHERE id = ?",
        )
        .bind(by)
        .bind(now)
        .bind(now)
        .bind(old.id.to_string())
        .execute(&mut *tx)
        .await?;
        log(
            tx,
            owner,
            &old.id.to_string(),
            "supersede",
            actor,
            Some(&old.text),
            Some(by),
        )
        .await
    }

    /// Change a memory's text, pin, topics, validity or meta by hand. Returns `None` when it isn't
    /// the owner's.
    pub async fn edit(
        &self,
        owner: &str,
        id: Uuid,
        change: &Edit,
        actor: Actor<'_>,
    ) -> Result<Result<Option<Memory>, Refused>> {
        let Some(m) = self.get(owner, id).await? else {
            return Ok(Ok(None));
        };
        let now = now_ms();
        let mut tx = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        if let Some(text) = &change.text {
            let text = tidy(text);
            if text.is_empty() {
                return Ok(Err(Refused::Empty));
            }
            if text.chars().count() > MAX_CHARS {
                return Ok(Err(Refused::TooLong(text.chars().count())));
            }
            if let Some(why) = excluded(&text) {
                return Ok(Err(Refused::Excluded(why)));
            }
            if text != m.text {
                sqlx::query("UPDATE memories SET text = ?, updated_at = ? WHERE id = ?")
                    .bind(&text)
                    .bind(now)
                    .bind(id.to_string())
                    .execute(&mut *tx)
                    .await?;
                log(
                    &mut tx,
                    owner,
                    &id.to_string(),
                    "update",
                    actor,
                    Some(&m.text),
                    Some(&text),
                )
                .await?;
            }
        }
        if let Some(pinned) = change.pinned.filter(|p| *p != m.pinned) {
            sqlx::query("UPDATE memories SET pinned = ?, updated_at = ? WHERE id = ?")
                .bind(pinned as i64)
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            log(
                &mut tx,
                owner,
                &id.to_string(),
                if pinned { "pin" } else { "unpin" },
                actor,
                None,
                None,
            )
            .await?;
        }
        if let Some(archived) = change.archived.filter(|a| *a != m.archived_at.is_some()) {
            sqlx::query("UPDATE memories SET archived_at = ?, updated_at = ? WHERE id = ?")
                .bind(archived.then_some(now))
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            log(
                &mut tx,
                owner,
                &id.to_string(),
                if archived { "archive" } else { "restore" },
                actor,
                None,
                None,
            )
            .await?;
        }
        if let Some(kind) = change.kind.filter(|k| *k != m.kind) {
            sqlx::query("UPDATE memories SET kind = ?, updated_at = ? WHERE id = ?")
                .bind(kind.as_str())
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            log(
                &mut tx,
                owner,
                &id.to_string(),
                "update",
                actor,
                Some(m.kind.as_str()),
                Some(kind.as_str()),
            )
            .await?;
        }
        if let Some(valid_until) = change.valid_until {
            sqlx::query("UPDATE memories SET valid_until = ?, updated_at = ? WHERE id = ?")
                .bind(valid_until.map(|d| d.timestamp_millis()))
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        if let Some(topics) = &change.topics {
            sqlx::query("UPDATE memories SET topics = ?, updated_at = ? WHERE id = ?")
                .bind(serde_json::to_string(topics)?)
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        if let Some(meta) = &change.meta {
            sqlx::query("UPDATE memories SET meta = ? WHERE id = ?")
                .bind(meta.to_string())
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        let m = fetch(&mut tx, &id.to_string()).await?;
        tx.commit().await?;
        Ok(Ok(Some(m)))
    }

    /// Archive one memory: out of the active set, kept and restorable. Returns whether it was
    /// the owner's.
    pub async fn archive(&self, owner: &str, id: Uuid, actor: Actor<'_>) -> Result<bool> {
        Ok(self
            .edit(
                owner,
                id,
                &Edit {
                    archived: Some(true),
                    ..Default::default()
                },
                actor,
            )
            .await?
            .ok()
            .flatten()
            .is_some())
    }

    /// Delete one of the owner's memories for good, with its history. Returns whether there was one.
    pub async fn forget(&self, owner: &str, id: Uuid) -> Result<bool> {
        let mut tx = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        let res = sqlx::query("DELETE FROM memories WHERE owner = ? AND id = ?")
            .bind(owner)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        if res.rows_affected() == 1 {
            sqlx::query("DELETE FROM memory_ops WHERE owner = ? AND memory_id = ?")
                .bind(owner)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(res.rows_affected() == 1)
    }

    /// Delete all the owner's memories, their history and the profile. Returns how many rows there were.
    pub async fn forget_all(&self, owner: &str) -> Result<u64> {
        let mut tx = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        let res = sqlx::query("DELETE FROM memories WHERE owner = ?")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        for table in ["memory_ops", "memory_profiles", "memory_owners"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE owner = ?"))
                .bind(owner)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// A memory's versions (the chain it superseded, oldest first, ending in itself) and its audit rows.
    pub async fn history(&self, owner: &str, id: Uuid) -> Result<Option<(Vec<Memory>, Vec<Op>)>> {
        let Some(m) = self.get(owner, id).await? else {
            return Ok(None);
        };
        let all = sqlx::query(&format!("SELECT {COLS} FROM memories WHERE owner = ?"))
            .bind(owner)
            .fetch_all(self.store.pool())
            .await?;
        let all: Vec<Memory> = all.iter().map(from_row).collect::<Result<_>>()?;
        let mut chain = vec![m.clone()];
        let mut cur = m.id;
        while let Some(prev) = all.iter().find(|x| x.superseded_by == Some(cur)) {
            chain.insert(0, prev.clone());
            cur = prev.id;
            if chain.len() > 100 {
                break;
            }
        }
        let ids: Vec<String> = chain.iter().map(|m| m.id.to_string()).collect();
        let mut ops = Vec::new();
        for id in ids {
            let rows = sqlx::query("SELECT id, memory_id, op, actor, at, before, after FROM memory_ops WHERE owner = ? AND memory_id = ? ORDER BY id")
                .bind(owner)
                .bind(&id)
                .fetch_all(self.store.pool())
                .await?;
            for r in rows {
                ops.push(Op {
                    id: r.try_get("id")?,
                    memory_id: r.try_get("memory_id")?,
                    op: r.try_get("op")?,
                    actor: r.try_get("actor")?,
                    at: to_dt(r.try_get("at")?),
                    before: r.try_get("before")?,
                    after: r.try_get("after")?,
                });
            }
        }
        ops.sort_by_key(|o| o.id);
        Ok(Some((chain, ops)))
    }

    /// Note that these memories were shown to a run.
    pub async fn recalled(&self, owner: &str, ids: &[Uuid]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let mut tx = self.store.pool().begin().await?;
        for id in ids {
            sqlx::query("UPDATE memories SET recall_count = recall_count + 1, last_recalled_at = ? WHERE owner = ? AND id = ?")
                .bind(now)
                .bind(owner)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // -- profile ---------------------------------------------------------------------------

    pub async fn profile(&self, owner: &str) -> Result<Option<Profile>> {
        let row = sqlx::query(
            "SELECT text, user_edited, updated_at FROM memory_profiles WHERE owner = ?",
        )
        .bind(owner)
        .fetch_optional(self.store.pool())
        .await?;
        row.map(|r| -> Result<Profile> {
            Ok(Profile {
                text: r.try_get("text")?,
                user_edited: r.try_get::<i64, _>("user_edited")? != 0,
                updated_at: to_dt(r.try_get("updated_at")?),
            })
        })
        .transpose()
    }

    /// Write the profile. One the user wrote stays until they change it; consolidation's version
    /// is dropped while `user_edited` is set. An empty text removes it.
    pub async fn set_profile(
        &self,
        owner: &str,
        text: &str,
        actor: Actor<'_>,
    ) -> Result<Result<Option<Profile>, Refused>> {
        let text = text.trim().to_string();
        if text.chars().count() > MAX_PROFILE_CHARS {
            return Ok(Err(Refused::TooLong(text.chars().count())));
        }
        if let Some(why) = excluded(&text) {
            return Ok(Err(Refused::Excluded(why)));
        }
        let by_user = matches!(actor, Actor::User);
        let mut tx = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        let before: Option<(String, i64)> =
            sqlx::query("SELECT text, user_edited FROM memory_profiles WHERE owner = ?")
                .bind(owner)
                .fetch_optional(&mut *tx)
                .await?
                .map(|r| Ok::<_, anyhow::Error>((r.try_get("text")?, r.try_get("user_edited")?)))
                .transpose()?;
        if !by_user && before.as_ref().is_some_and(|(_, edited)| *edited != 0) {
            tx.commit().await?;
            return Ok(Ok(self.profile(owner).await?));
        }
        let now = now_ms();
        if text.is_empty() {
            sqlx::query("DELETE FROM memory_profiles WHERE owner = ?")
                .bind(owner)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO memory_profiles (owner, text, user_edited, updated_at) VALUES (?, ?, ?, ?) \
                 ON CONFLICT (owner) DO UPDATE SET text = excluded.text, user_edited = excluded.user_edited, updated_at = excluded.updated_at",
            )
            .bind(owner)
            .bind(&text)
            .bind(by_user as i64)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        if before.as_ref().map(|(t, _)| t.as_str()) != Some(text.as_str()) {
            log(
                &mut tx,
                owner,
                "profile",
                "profile",
                actor,
                before.as_ref().map(|(t, _)| t.as_str()),
                Some(&text),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(Ok(self.profile(owner).await?))
    }

    // -- consolidation bookkeeping ---------------------------------------------------------

    /// A run of `owner` finished. Returns how many have since the last consolidation pass.
    pub async fn run_finished(&self, owner: &str) -> Result<u32> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO memory_owners (owner, pending_runs, last_run_at) VALUES (?, 1, ?) \
             ON CONFLICT (owner) DO UPDATE SET pending_runs = pending_runs + 1, last_run_at = excluded.last_run_at",
        )
        .bind(owner)
        .bind(now)
        .execute(self.store.pool())
        .await?;
        let pending: i64 =
            sqlx::query_scalar("SELECT pending_runs FROM memory_owners WHERE owner = ?")
                .bind(owner)
                .fetch_one(self.store.pool())
                .await?;
        Ok(pending as u32)
    }

    /// Owners with runs since their last pass whose last run finished before `idle_before`.
    pub async fn owners_idle_since(&self, idle_before: DateTime<Utc>) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT owner FROM memory_owners WHERE pending_runs > 0 AND last_run_at < ?",
        )
        .bind(idle_before.timestamp_millis())
        .fetch_all(self.store.pool())
        .await?;
        Ok(rows)
    }

    pub async fn consolidated(&self, owner: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO memory_owners (owner, pending_runs, consolidated_at) VALUES (?, 0, ?) \
             ON CONFLICT (owner) DO UPDATE SET pending_runs = 0, consolidated_at = excluded.consolidated_at",
        )
        .bind(owner)
        .bind(now_ms())
        .execute(self.store.pool())
        .await?;
        Ok(())
    }

    pub async fn consolidated_at(&self, owner: &str) -> Result<Option<DateTime<Utc>>> {
        let at: Option<Option<i64>> =
            sqlx::query_scalar("SELECT consolidated_at FROM memory_owners WHERE owner = ?")
                .bind(owner)
                .fetch_optional(self.store.pool())
                .await?;
        Ok(at.flatten().map(to_dt))
    }

    /// Merge `drop` into `keep`: the dropped rows are superseded by the kept one and the kept
    /// one counts their mentions. Both must be the owner's and active.
    pub async fn merge(
        &self,
        owner: &str,
        keep: Uuid,
        drop: &[Uuid],
        actor: Actor<'_>,
    ) -> Result<usize> {
        let now = now_ms();
        let mut tx = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        let kept = fetch(&mut tx, &keep.to_string()).await?;
        let mut merged = 0;
        for id in drop {
            if *id == keep {
                continue;
            }
            let Ok(old) = fetch(&mut tx, &id.to_string()).await else {
                continue;
            };
            if !old.is_active() || old.pinned {
                continue;
            }
            sqlx::query(
                "UPDATE memories SET superseded_by = ?, updated_at = ? WHERE id = ? AND owner = ?",
            )
            .bind(keep.to_string())
            .bind(now)
            .bind(id.to_string())
            .bind(owner)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE memories SET mentions = mentions + ?, updated_at = ? WHERE id = ? AND owner = ?")
                .bind(old.mentions)
                .bind(now)
                .bind(keep.to_string())
                .bind(owner)
                .execute(&mut *tx)
                .await?;
            log(
                &mut tx,
                owner,
                &id.to_string(),
                "merge",
                actor,
                Some(&old.text),
                Some(&kept.short_id),
            )
            .await?;
            merged += 1;
        }
        tx.commit().await?;
        Ok(merged)
    }

    /// Archive a row with a reason in the audit log (expiry, roll-up, a failed procedure).
    pub async fn archive_because(
        &self,
        owner: &str,
        id: Uuid,
        op: &str,
        why: &str,
        actor: Actor<'_>,
    ) -> Result<()> {
        let now = now_ms();
        let mut tx = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        let res = sqlx::query("UPDATE memories SET archived_at = ?, updated_at = ? WHERE id = ? AND owner = ? AND archived_at IS NULL")
            .bind(now)
            .bind(now)
            .bind(id.to_string())
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        if res.rows_affected() == 1 {
            log(&mut tx, owner, &id.to_string(), op, actor, None, Some(why)).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Everything the owner has, for export: active and archived rows, the profile.
    pub async fn export(&self, owner: &str) -> Result<Value> {
        let active = self.list(owner, &Filter::default()).await?;
        let archived = self
            .list(
                owner,
                &Filter {
                    archived: true,
                    ..Default::default()
                },
            )
            .await?;
        Ok(json!({
            "version": 2,
            "profile": self.profile(owner).await?,
            "items": active.into_iter().chain(archived).collect::<Vec<_>>(),
        }))
    }

    /// Add what an export holds (its active rows; superseded and archived ones are left out), as
    /// if by hand. Returns how many were added.
    pub async fn import(&self, owner: &str, data: &Value) -> Result<usize> {
        let mut added = 0;
        if let Some(text) = data["profile"]["text"]
            .as_str()
            .filter(|t| !t.trim().is_empty())
        {
            if self.profile(owner).await?.is_none() {
                let _ = self.set_profile(owner, text, Actor::User).await?;
            }
        }
        for item in data["items"].as_array().into_iter().flatten() {
            if item["archived_at"].is_string() || item["superseded_by"].is_string() {
                continue;
            }
            let Some(text) = item["text"].as_str() else {
                continue;
            };
            let new = NewMemory {
                kind: item["kind"].as_str().and_then(Kind::parse),
                text: text.to_string(),
                subject: item["subject"].as_str().map(String::from),
                relation: item["relation"].as_str().map(String::from),
                topics: item["topics"]
                    .as_array()
                    .map(|t| {
                        t.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                source: Some(Source::Api),
                support: item["support"].as_str().map(String::from),
                hedged: item["hedged"] == true,
                sensitive: item["sensitive"] == true,
                pinned: item["pinned"].as_bool(),
                meta: item.get("meta").filter(|m| m.is_object()).cloned(),
                run_id: None,
                valid_from: item["valid_from"].as_str().and_then(|s| s.parse().ok()),
                valid_until: item["valid_until"].as_str().and_then(|s| s.parse().ok()),
                supersedes: None,
                replaces: None,
            };
            if let Ok((_, Saved::Added | Saved::Superseded)) =
                self.remember(owner, new, Actor::User).await?
            {
                added += 1;
            }
        }
        Ok(added)
    }

    /// How many active rows there are per kind.
    pub async fn counts(&self, owner: &str) -> Result<HashMap<Kind, usize>> {
        let mut counts = HashMap::new();
        for m in self.active(owner).await? {
            *counts.entry(m.kind).or_insert(0) += 1;
        }
        Ok(counts)
    }
}

/// A change to one memory by hand.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Edit {
    pub text: Option<String>,
    pub kind: Option<Kind>,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    #[serde(default, with = "double_option")]
    pub valid_until: Option<Option<DateTime<Utc>>>,
    pub topics: Option<Vec<String>>,
    pub meta: Option<Value>,
}

/// `null` clears, absent keeps.
mod double_option {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

/// What the memory tools need from the run they are called in.
#[derive(Clone)]
pub struct MemoryContext {
    pub memories: Memories,
    pub owner: String,
    pub run_id: Uuid,
    /// Only interactive (attended) runs may change what is remembered about the user; unattended
    /// ones only read it.
    pub interactive: bool,
}

pub use recall::{note, note_span, Recall, Shown};

/// What a run's request says, for recall: its prompt and the newest user turns of its
/// conversation, newest last, at most a couple of thousand characters.
pub fn recall_query(messages: &[Value], prompt: &str) -> String {
    let mut turns: Vec<String> = messages
        .iter()
        .rev()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| match &m["content"] {
            Value::String(s) => Some(s.clone()),
            Value::Array(blocks) => {
                let text: Vec<&str> = blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect();
                (!text.is_empty()).then(|| text.join(" "))
            }
            _ => None,
        })
        .take(2)
        .collect();
    turns.reverse();
    turns.push(prompt.to_string());
    let q = turns.join("\n");
    let len = q.chars().count();
    if len > 2_000 {
        q.chars().skip(len - 2_000).collect()
    } else {
        q
    }
}

/// Apply the user's verdict on a run to the procedures it learned and followed: a thumbs down
/// archives the one it learned and counts against the ones it was shown; a thumbs up counts for
/// them. Returns what was archived.
pub async fn feedback(
    memories: &Memories,
    store: &Store,
    run: &crate::models::Run,
    rating: crate::models::Rating,
) -> Result<Vec<Memory>> {
    use crate::models::Rating;
    let good = rating == Rating::Up;
    let mut archived = Vec::new();
    let actor = Actor::User;
    // The procedures the run was shown, from its own event log.
    let mut shown: Vec<Uuid> = Vec::new();
    for e in store.events_after(run.id, 0, 10_000).await? {
        if e.kind != crate::models::event::MEMORY_RECALLED {
            continue;
        }
        for item in e.data["items"].as_array().into_iter().flatten() {
            if item["kind"] == "procedure" {
                if let Some(id) = item["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                    shown.push(id);
                }
            }
        }
    }
    archived.extend(
        memories
            .procedures_went(&run.owner, &shown, good, actor)
            .await?,
    );
    if !good {
        for m in memories
            .list(
                &run.owner,
                &Filter {
                    kinds: vec![Kind::Procedure],
                    ..Default::default()
                },
            )
            .await?
        {
            if m.run_id == Some(run.id)
                && m.meta["learned_in"].as_str() == Some(&run.id.to_string())
                && !m.pinned
            {
                memories
                    .archive_because(
                        &run.owner,
                        m.id,
                        "archive",
                        "the user gave the run that learned it a thumbs down",
                        actor,
                    )
                    .await?;
                archived.push(m);
            }
        }
    }
    Ok(archived)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) async fn memories(max: usize) -> Memories {
        Memories::new(
            Store::connect_memory().await.unwrap(),
            Limits {
                max_items: max,
                recall_tokens: 600,
                consolidate_after: 5,
            },
        )
    }

    pub(crate) fn fact(text: &str) -> NewMemory {
        NewMemory::text(text)
    }

    #[tokio::test]
    async fn remembers_lists_and_forgets_per_owner() {
        let m = memories(10).await;
        let (a, saved) = m
            .remember("o1", fact("  Prefers\nmetric   units "), Actor::User)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved, Saved::Added);
        assert_eq!(a.text, "Prefers metric units");
        assert_eq!(a.short_id, "m1");
        assert_eq!(a.kind, Kind::Fact);
        let (b, _) = m
            .remember("o1", fact("Lives in Zurich"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(b.short_id, "m2");
        m.remember("o2", fact("Someone else"), Actor::User)
            .await
            .unwrap()
            .unwrap();

        let list = m.active("o1").await.unwrap();
        assert_eq!(list.iter().map(|x| x.id).collect::<Vec<_>>(), [a.id, b.id]);
        assert_eq!(m.resolve("o1", "m2").await.unwrap().unwrap().id, b.id);
        assert_eq!(m.resolve("o1", "[m2]").await.unwrap().unwrap().id, b.id);
        assert!(m.resolve("o2", "m2").await.unwrap().is_none());

        // Another owner can't forget it.
        assert!(!m.forget("o2", a.id).await.unwrap());
        assert!(m.forget("o1", a.id).await.unwrap());
        assert!(!m.forget("o1", a.id).await.unwrap());
        assert_eq!(m.active("o1").await.unwrap(), std::slice::from_ref(&b));

        assert_eq!(m.forget_all("o1").await.unwrap(), 1);
        assert!(m.active("o1").await.unwrap().is_empty());
        assert_eq!(m.active("o2").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_same_thing_said_again_is_kept_once() {
        let m = memories(10).await;
        let (a, _) = m
            .remember("o", fact("Likes tea"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let (again, saved) = m
            .remember("o", fact(" likes TEA. "), Actor::User)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved, Saved::Unchanged);
        assert_eq!(again.id, a.id);
        assert_eq!(again.mentions, 2);
        // Said with more detail, it is the same memory with the fuller text.
        let (near, saved) = m
            .remember("o", fact("Likes tea a lot"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((near.id, saved), (a.id, Saved::Unchanged));
        assert_eq!(near.text, "Likes tea a lot");
        assert_eq!(m.active("o").await.unwrap().len(), 1);
        // One word of difference is another fact.
        let (other, saved) = m
            .remember("o", fact("Likes coffee a lot"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved, Saved::Added);
        assert_ne!(other.id, a.id);
    }

    #[tokio::test]
    async fn a_keyed_fact_supersedes_without_being_asked() {
        let m = memories(10).await;
        let keyed = |text: &str| NewMemory {
            subject: Some("user".into()),
            relation: Some("home city".into()),
            ..fact(text)
        };
        let (bern, _) = m
            .remember("o", keyed("Lives in Bern"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let (zurich, saved) = m
            .remember("o", keyed("Lives in Zurich"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved, Saved::Superseded);
        let active = m.active("o").await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, zurich.id);
        let old = m.get("o", bern.id).await.unwrap().unwrap();
        assert_eq!(old.superseded_by, Some(zurich.id));
        assert!(old.valid_until.is_some());
        let (chain, ops) = m.history("o", zurich.id).await.unwrap().unwrap();
        assert_eq!(
            chain.iter().map(|x| x.id).collect::<Vec<_>>(),
            [bern.id, zurich.id]
        );
        assert_eq!(
            ops.iter().map(|o| o.op.as_str()).collect::<Vec<_>>(),
            ["add", "add", "supersede"]
        );
        // Listing the archive shows the old one.
        let gone = m
            .list(
                "o",
                &Filter {
                    archived: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(gone.len(), 1);
    }

    #[tokio::test]
    async fn supersedes_and_replaces_by_id() {
        let m = memories(1).await;
        let (a, _) = m
            .remember("o", fact("Works at Acme"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let (b, saved) = m
            .remember(
                "o",
                NewMemory {
                    supersedes: Some(a.id),
                    ..fact("Works at Globex")
                },
                Actor::User,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved, Saved::Superseded);
        assert_ne!(a.id, b.id);
        assert_eq!(m.active("o").await.unwrap().len(), 1);
        let (c, saved) = m
            .remember(
                "o",
                NewMemory {
                    replaces: Some(b.id),
                    ..fact("Works at Globex as a designer")
                },
                Actor::User,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!((c.id, saved), (b.id, Saved::Replaced));
        assert_eq!(c.text, "Works at Globex as a designer");
        // Someone else's id, or a missing one, can't be superseded.
        assert_eq!(
            m.remember(
                "p",
                NewMemory {
                    supersedes: Some(a.id),
                    ..fact("x")
                },
                Actor::User
            )
            .await
            .unwrap(),
            Err(Refused::NotFound)
        );
    }

    #[tokio::test]
    async fn a_full_budget_archives_the_lowest_rank_instead_of_refusing() {
        let m = memories(2).await;
        let (one, _) = m
            .remember("o", fact("one thing"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        m.remember("o", fact("second thing"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        m.recalled("o", &[one.id]).await.unwrap();
        let (three, saved) = m
            .remember("o", fact("third thing"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved, Saved::Added);
        let active = m.active("o").await.unwrap();
        assert_eq!(
            active.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(),
            ["one thing", "third thing"]
        );
        let archived = m
            .list(
                "o",
                &Filter {
                    archived: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(archived[0].text, "second thing");
        assert!(archived[0].archived_at.is_some());
        // Restoring brings it back.
        let back = m
            .edit(
                "o",
                archived[0].id,
                &Edit {
                    archived: Some(false),
                    ..Default::default()
                },
                Actor::User,
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(back.archived_at.is_none());
        assert_eq!(m.active("o").await.unwrap().len(), 3);
        let _ = three;
    }

    #[tokio::test]
    async fn bad_text_is_refused() {
        let m = memories(2).await;
        assert_eq!(
            m.remember("o", fact(" \n "), Actor::User).await.unwrap(),
            Err(Refused::Empty)
        );
        let long = "x".repeat(MAX_CHARS + 1);
        assert_eq!(
            m.remember("o", fact(&long), Actor::User).await.unwrap(),
            Err(Refused::TooLong(MAX_CHARS + 1))
        );
        assert!(matches!(
            m.remember("o", fact("My password: hunter2"), Actor::User)
                .await
                .unwrap(),
            Err(Refused::Excluded(_))
        ));
        assert!(matches!(
            m.remember("o", fact("Card number 4111 1111 1111 1111"), Actor::User)
                .await
                .unwrap(),
            Err(Refused::Excluded(_))
        ));
        assert!(m
            .remember(
                "o",
                fact("Has two daughters, 4 and 7, born 2019 and 2022"),
                Actor::User
            )
            .await
            .unwrap()
            .is_ok());
    }

    #[tokio::test]
    async fn the_profile_is_kept_once_the_user_edits_it() {
        let m = memories(10).await;
        assert!(m.profile("o").await.unwrap().is_none());
        let p = m
            .set_profile("o", "Ana, in Zurich.", Actor::Consolidation)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!p.user_edited);
        let p = m
            .set_profile("o", "Ana, in Zurich, prefers tea.", Actor::User)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(p.user_edited);
        let p = m
            .set_profile("o", "Something else", Actor::Consolidation)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(p.text, "Ana, in Zurich, prefers tea.");
        let p = m.set_profile("o", "", Actor::User).await.unwrap().unwrap();
        assert!(p.is_none());
    }

    #[tokio::test]
    async fn export_and_import_round_trip() {
        let m = memories(10).await;
        m.remember(
            "o",
            NewMemory {
                kind: Some(Kind::Instruction),
                ..fact("Always cite sources")
            },
            Actor::User,
        )
        .await
        .unwrap()
        .unwrap();
        m.remember("o", fact("Lives in Bern"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        m.set_profile("o", "Ana.", Actor::Consolidation)
            .await
            .unwrap()
            .unwrap();
        let data = m.export("o").await.unwrap();
        assert_eq!(data["items"].as_array().unwrap().len(), 2);
        assert_eq!(m.import("p", &data).await.unwrap(), 2);
        let items = m.active("p").await.unwrap();
        assert!(items
            .iter()
            .any(|x| x.kind == Kind::Instruction && x.pinned));
        assert_eq!(m.profile("p").await.unwrap().unwrap().text, "Ana.");
        assert_eq!(m.import("p", &data).await.unwrap(), 0);
    }

    #[test]
    fn rank_decays_with_disuse_but_not_below_the_floor() {
        let now = Utc::now();
        let mut m = Memory {
            id: Uuid::nil(),
            short_id: "m1".into(),
            kind: Kind::Fact,
            text: "x".into(),
            subject: None,
            relation: None,
            topics: vec![],
            source: Source::User,
            support: None,
            hedged: false,
            sensitive: false,
            pinned: false,
            mentions: 1,
            meta: json!({}),
            run_id: None,
            valid_from: None,
            valid_until: None,
            created_at: now,
            updated_at: now,
            superseded_by: None,
            archived_at: None,
            recall_count: 0,
            last_recalled_at: None,
        };
        let fresh = m.rank(now);
        m.updated_at = now - chrono::Duration::days(365);
        let stale = m.rank(now);
        assert!(stale < fresh);
        assert!(stale >= 0.8 * 0.3 - 1e-9);
        m.pinned = true;
        assert_eq!(m.rank(now), fresh);
        m.pinned = false;
        m.kind = Kind::Episode;
        assert!(m.rank(now) < stale);
    }
}
