//! The sleep-time pass: tidying memory off the request path.
//!
//! Every few finished runs of an owner (or once they have been idle for a while with changes
//! pending), the manager runs this pass in the background. It expires temporary rows past
//! their date, rolls old raw episodes up into one summary per month, and makes one model call
//! over the active rows about the user to merge duplicates the admission check let through,
//! close contradictions the extractor could not see, and rewrite the profile: the short summary
//! of the user that every run is shown. It is deliberately conservative: a merge or supersede
//! the model proposes is applied only when the two rows are of one kind and plainly alike, a
//! pinned row is never touched, a profile the user wrote is never rewritten, and every change is
//! an audit row. Nothing is deleted.

use super::recall::similarity;
use super::{Actor, Filter, Kind, Memories, Memory, NewMemory, Source, MAX_PROFILE_CHARS};
use crate::llm::{LlmClient, MessageRequest};
use anyhow::{Context, Result};
use chrono::{Datelike, Duration, Utc};
use serde::Serialize;
use serde_json::{json, Value};

/// Raw episodes older than this are rolled up.
pub const ROLLUP_AFTER_DAYS: i64 = 30;
/// A month needs this many raw episodes to be rolled up.
pub const ROLLUP_MIN: usize = 5;
/// The most merges one pass applies.
const MAX_MERGES: usize = 10;
/// How alike two rows must be for a merge the model proposes to be applied.
const MERGE_SIMILARITY: f64 = 0.2;
/// How alike for a supersede.
const SUPERSEDE_SIMILARITY: f64 = 0.12;

pub const SYSTEM: &str = "You tidy an AI assistant's notes about the person it works for. You \
    merge notes that say the same thing, close notes that a later note has made untrue, and \
    write the short profile of the person that the assistant reads before every conversation. \
    You are careful: when in doubt, leave notes as they are.";

const INSTRUCTION: &str =
    "Reply with JSON only: {\"profile\": \"...\", \"merge\": [{\"keep\": \"m1\", \
    \"drop\": [\"m2\"]}], \"supersede\": [{\"old\": \"m3\", \"new\": \"m4\"}]}.\n\
    profile: the person in at most 150 words, third person, plain prose: name and how to address \
    them, where they are, their work, family and the like, how they want answers, every standing \
    instruction (keep each one; they matter most), and what they are working on now. Only what \
    the notes say; nothing guessed. Keep a tentative note tentative.\n\
    merge: notes that state the same fact or preference in different words; keep the fuller \
    one. Never merge two different facts (two pets are two notes).\n\
    supersede: pairs where a newer note makes an older one untrue (they moved, changed jobs); \
    the old one is kept as history. A stable fact (a birthplace, a name) is not superseded by one \
    passing, tentative mention. Empty lists are a fine answer.";

const ROLLUP_SYSTEM: &str = "You summarise an AI assistant's log of past conversations with one \
    person into a short account of a month, for the assistant's own later reference.";

/// What a pass did.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Report {
    pub expired: usize,
    pub rolled_up: usize,
    pub merged: usize,
    pub superseded: usize,
    pub profile_written: bool,
    /// The model call could not be made or answered nothing usable; the code-only steps ran.
    pub model_skipped: bool,
}

/// The model a pass uses.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub model: String,
    pub max_tokens: u32,
    pub reasoning: Option<String>,
}

fn id_of(v: &Value, rows: &[Memory]) -> Option<Memory> {
    let s = v.as_str()?.trim().trim_matches(['[', ']']);
    rows.iter()
        .find(|m| m.short_id == s || m.id.to_string() == s)
        .cloned()
}

/// Run the pass for `owner`. Without a client only the code-only steps run.
pub async fn run(
    memories: &Memories,
    llm: Option<&dyn LlmClient>,
    spec: &ModelSpec,
    owner: &str,
) -> Result<Report> {
    let mut report = Report::default();
    let now = Utc::now();
    let actor = Actor::Consolidation;

    // 1. Temporary rows past their date.
    for m in memories.active(owner).await? {
        if m.valid_until.is_some_and(|u| u <= now) && !m.pinned {
            memories
                .archive_because(owner, m.id, "expire", "past its date", actor)
                .await?;
            report.expired += 1;
        }
    }

    // 2. Old raw episodes, one summary per month.
    let cutoff = now - Duration::days(ROLLUP_AFTER_DAYS);
    let episodes: Vec<Memory> = memories
        .list(
            owner,
            &Filter {
                kinds: vec![Kind::Episode],
                ..Default::default()
            },
        )
        .await?
        .into_iter()
        .filter(|m| {
            m.created_at < cutoff && m.meta["rollup"] != true && m.source != Source::Consolidation
        })
        .collect();
    let mut months: Vec<(String, Vec<Memory>)> = Vec::new();
    for e in episodes {
        let key = format!("{:04}-{:02}", e.created_at.year(), e.created_at.month());
        match months.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(e),
            None => months.push((key, vec![e])),
        }
    }
    for (month, rows) in months
        .into_iter()
        .filter(|(_, r)| r.len() >= ROLLUP_MIN)
        .take(3)
    {
        let Some(llm) = llm else { break };
        let mut body = format!("The conversations of {month}, one per line, oldest first:\n");
        for r in &rows {
            body.push_str(&format!(
                "- {}: {}\n",
                r.created_at.format("%Y-%m-%d"),
                r.text
            ));
        }
        body.push_str("\nWrite one paragraph of at most 120 words, third person, naming the main tasks, \
            what was produced, recurring themes, and anything that went wrong. Reply with the paragraph only.");
        let req = MessageRequest {
            model: &spec.model,
            max_tokens: spec.max_tokens,
            reasoning_effort: spec.reasoning.as_deref(),
            system: ROLLUP_SYSTEM,
            tools: &[],
            messages: &[json!({ "role": "user", "content": body })],
        };
        let summary = match llm.create_message(&req).await {
            Ok(resp) => text_of(&resp.content),
            Err(e) => {
                tracing::warn!(owner, error = %format!("{e:#}"), "the episode roll-up call failed");
                report.model_skipped = true;
                break;
            }
        };
        let summary = super::tidy(&summary);
        if summary.is_empty() {
            report.model_skipped = true;
            break;
        }
        let text: String = format!("{month}: {summary}")
            .chars()
            .take(super::MAX_CHARS)
            .collect();
        let mut topics: Vec<String> = Vec::new();
        for r in &rows {
            for t in &r.topics {
                if !topics.contains(t) && topics.len() < 8 {
                    topics.push(t.clone());
                }
            }
        }
        let new = NewMemory {
            kind: Some(Kind::Episode),
            text,
            topics,
            source: Some(Source::Consolidation),
            meta: Some(json!({ "rollup": true, "month": month, "count": rows.len() })),
            valid_from: Some(rows[0].created_at),
            ..Default::default()
        };
        if let Ok((rolled, _)) = memories.remember(owner, new, actor).await? {
            for r in &rows {
                memories
                    .archive_because(owner, r.id, "rollup", &rolled.short_id, actor)
                    .await?;
            }
            report.rolled_up += rows.len();
        }
    }

    // 3. The model's pass over what is known about the user.
    let rows: Vec<Memory> = memories
        .active(owner)
        .await?
        .into_iter()
        .filter(|m| m.kind.is_about_user())
        .collect();
    let profile = memories.profile(owner).await?;
    if rows.is_empty() {
        memories.consolidated(owner).await?;
        return Ok(report);
    }
    let Some(llm) = llm else {
        report.model_skipped = true;
        memories.consolidated(owner).await?;
        return Ok(report);
    };
    let recent: Vec<Memory> = memories
        .list(
            owner,
            &Filter {
                kinds: vec![Kind::Episode],
                ..Default::default()
            },
        )
        .await?
        .into_iter()
        .rev()
        .take(20)
        .collect();
    let (system, messages) = call(
        &rows,
        profile.as_ref().map(|p| (p.text.as_str(), p.user_edited)),
        &recent,
    );
    let req = MessageRequest {
        model: &spec.model,
        max_tokens: spec.max_tokens,
        reasoning_effort: spec.reasoning.as_deref(),
        system: &system,
        tools: &[],
        messages: &messages,
    };
    let reply = match llm.create_message(&req).await {
        Ok(resp) => text_of(&resp.content),
        Err(e) => {
            tracing::warn!(owner, error = %format!("{e:#}"), "the memory consolidation call failed");
            report.model_skipped = true;
            memories.consolidated(owner).await?;
            return Ok(report);
        }
    };
    let Some(plan) = parse(&reply) else {
        report.model_skipped = true;
        memories.consolidated(owner).await?;
        return Ok(report);
    };
    for merge in plan["merge"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_MERGES)
    {
        let Some(keep) = id_of(&merge["keep"], &rows) else {
            continue;
        };
        let drop: Vec<_> = merge["drop"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| id_of(d, &rows))
            .filter(|d| {
                d.id != keep.id
                    && d.kind == keep.kind
                    && !d.pinned
                    && similarity(&d.text, &keep.text) >= MERGE_SIMILARITY
            })
            .map(|d| d.id)
            .collect();
        if !drop.is_empty() {
            report.merged += memories.merge(owner, keep.id, &drop, actor).await?;
        }
    }
    for pair in plan["supersede"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_MERGES)
    {
        let (Some(old), Some(new)) = (id_of(&pair["old"], &rows), id_of(&pair["new"], &rows))
        else {
            continue;
        };
        if old.id == new.id || old.pinned || old.kind != new.kind || old.created_at > new.created_at
        {
            continue;
        }
        if similarity(&old.text, &new.text) < SUPERSEDE_SIMILARITY {
            continue;
        }
        if memories.supersede(owner, old.id, new.id, actor).await? {
            report.superseded += 1;
        }
    }
    if let Some(text) = plan["profile"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        let text: String = text.chars().take(MAX_PROFILE_CHARS).collect();
        if let Ok(Some(p)) = memories
            .set_profile(owner, &text, actor)
            .await
            .context("writing the profile")?
        {
            report.profile_written = !p.user_edited && p.text == text;
        }
    }
    memories.consolidated(owner).await?;
    Ok(report)
}

/// The consolidation call's prompt and messages.
pub fn call(
    rows: &[Memory],
    profile: Option<(&str, bool)>,
    recent: &[Memory],
) -> (String, Vec<Value>) {
    let mut body = String::new();
    match profile {
        Some((text, true)) => {
            body.push_str("The current profile, written by the person themselves (it will be kept as is; still write yours, it is only used if they clear theirs):\n");
            body.push_str(text);
        }
        Some((text, false)) => {
            body.push_str("The current profile:\n");
            body.push_str(text);
        }
        None => body.push_str("There is no profile yet."),
    }
    body.push_str("\n\nThe notes about the person, [id] (kind, date) text:");
    for m in rows {
        body.push_str(&format!(
            "\n[{}] ({}{}{}, {}) {}",
            m.short_id,
            m.kind.as_str(),
            if m.pinned { ", pinned" } else { "" },
            if m.hedged { ", tentative" } else { "" },
            m.valid_from.unwrap_or(m.created_at).format("%Y-%m-%d"),
            m.text
        ));
    }
    if !recent.is_empty() {
        body.push_str("\n\nRecent conversations, newest first:");
        for e in recent {
            body.push_str(&format!(
                "\n- {}: {}",
                e.created_at.format("%Y-%m-%d"),
                e.text
            ));
        }
    }
    body.push_str("\n\n---\n");
    body.push_str(INSTRUCTION);
    (
        SYSTEM.to_string(),
        vec![json!({ "role": "user", "content": body })],
    )
}

/// The JSON object in the reply, if any.
pub fn parse(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start)
        .then(|| serde_json::from_str::<Value>(&text[start..=end]).ok())
        .flatten()
        .filter(Value::is_object)
}

fn text_of(content: &[Value]) -> String {
    content
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect()
}

impl Memories {
    /// Close `old` as superseded by `new`, both active rows of the owner. Returns whether it was.
    pub async fn supersede(
        &self,
        owner: &str,
        old: uuid::Uuid,
        new: uuid::Uuid,
        actor: Actor<'_>,
    ) -> Result<bool> {
        let (Some(o), Some(n)) = (self.get(owner, old).await?, self.get(owner, new).await?) else {
            return Ok(false);
        };
        if !o.is_active() || !n.is_active() || o.id == n.id {
            return Ok(false);
        }
        let now = crate::store::now_ms();
        let mut tx = self.store().pool().begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE memories SET superseded_by = ?, valid_until = COALESCE(valid_until, ?), updated_at = ? WHERE id = ?")
            .bind(new.to_string())
            .bind(now)
            .bind(now)
            .bind(old.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO memory_ops (owner, memory_id, op, actor, at, before, after) VALUES (?, ?, 'supersede', ?, ?, ?, ?)")
            .bind(owner)
            .bind(old.to_string())
            .bind(actor.to_string())
            .bind(now)
            .bind(&o.text)
            .bind(new.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmResponse;
    use crate::memory::tests::{fact, memories};
    use crate::models::Usage;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct Fixed(Mutex<Vec<String>>);

    #[async_trait]
    impl LlmClient for Fixed {
        async fn create_message(&self, _req: &MessageRequest<'_>) -> Result<LlmResponse> {
            let text = self.0.lock().unwrap().remove(0);
            Ok(LlmResponse {
                content: vec![json!({ "type": "text", "text": text })],
                stop_reason: None,
                usage: Usage::default(),
                reasoning: None,
            })
        }
    }

    fn spec() -> ModelSpec {
        ModelSpec {
            model: "m".into(),
            max_tokens: 1000,
            reasoning: None,
        }
    }

    #[tokio::test]
    async fn the_pass_expires_merges_supersedes_and_writes_the_profile() {
        let m = memories(50).await;
        let (tea, _) = m
            .remember("o", fact("Likes tea"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let (tea2, _) = m
            .remember("o", fact("Drinks tea every morning"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let (bern, _) = m
            .remember("o", fact("Lives in Bern"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let (zurich, _) = m
            .remember("o", fact("Lives in Zurich since March"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let (cat, _) = m
            .remember("o", fact("Has a cat called Mio"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let (pinned, _) = m
            .remember(
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
        let mut exam = fact("Has an exam");
        exam.kind = Some(Kind::Temporary);
        exam.valid_until = Some(Utc::now() - Duration::days(1));
        let (exam, _) = m.remember("o", exam, Actor::User).await.unwrap().unwrap();

        let reply = json!({
            "profile": "Ana lives in Zurich, drinks tea, has a cat called Mio. Always cite sources.",
            "merge": [
                { "keep": tea2.short_id, "drop": [tea.short_id] },
                // Different facts: refused by the similarity guard.
                { "keep": cat.short_id, "drop": [pinned.short_id, zurich.short_id] },
            ],
            "supersede": [
                { "old": bern.short_id, "new": zurich.short_id },
                // Pinned: never.
                { "old": pinned.short_id, "new": cat.short_id },
            ],
        });
        let llm = Fixed(Mutex::new(vec![reply.to_string()]));
        let report = run(&m, Some(&llm), &spec(), "o").await.unwrap();
        assert_eq!(
            report,
            Report {
                expired: 1,
                rolled_up: 0,
                merged: 1,
                superseded: 1,
                profile_written: true,
                model_skipped: false
            }
        );
        let active = m.active("o").await.unwrap();
        let ids: Vec<_> = active.iter().map(|x| x.id).collect();
        assert_eq!(ids, [tea2.id, zurich.id, cat.id, pinned.id]);
        assert_eq!(
            m.get("o", tea.id).await.unwrap().unwrap().superseded_by,
            Some(tea2.id)
        );
        assert_eq!(m.get("o", tea2.id).await.unwrap().unwrap().mentions, 2);
        assert_eq!(
            m.get("o", bern.id).await.unwrap().unwrap().superseded_by,
            Some(zurich.id)
        );
        assert!(m
            .get("o", exam.id)
            .await
            .unwrap()
            .unwrap()
            .archived_at
            .is_some());
        assert!(m
            .profile("o")
            .await
            .unwrap()
            .unwrap()
            .text
            .starts_with("Ana lives in Zurich"));
        assert!(m.consolidated_at("o").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn old_episodes_roll_up_into_a_month() {
        let m = memories(50).await;
        let old = Utc::now() - Duration::days(60);
        for i in 0..ROLLUP_MIN {
            let (e, _) = m
                .remember(
                    "o",
                    NewMemory {
                        kind: Some(Kind::Episode),
                        source: Some(Source::Agent),
                        ..fact(&format!("Episode {i}: did a thing"))
                    },
                    Actor::Run(uuid::Uuid::now_v7()),
                )
                .await
                .unwrap()
                .unwrap();
            sqlx::query("UPDATE memories SET created_at = ? WHERE id = ?")
                .bind(old.timestamp_millis() + i as i64)
                .bind(e.id.to_string())
                .execute(m.store().pool())
                .await
                .unwrap();
        }
        m.remember(
            "o",
            NewMemory {
                kind: Some(Kind::Episode),
                source: Some(Source::Agent),
                ..fact("Yesterday's thing")
            },
            Actor::User,
        )
        .await
        .unwrap()
        .unwrap();
        let llm = Fixed(Mutex::new(vec![
            "A month of doing things.".into(),
            json!({"profile": "", "merge": [], "supersede": []}).to_string(),
        ]));
        let report = run(&m, Some(&llm), &spec(), "o").await.unwrap();
        assert_eq!(report.rolled_up, ROLLUP_MIN);
        let episodes = m
            .list(
                "o",
                &Filter {
                    kinds: vec![Kind::Episode],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(episodes.len(), 2);
        let rolled = episodes.iter().find(|e| e.meta["rollup"] == true).unwrap();
        assert!(
            rolled.text.ends_with(": A month of doing things."),
            "{}",
            rolled.text
        );
        assert_eq!(rolled.meta["count"], ROLLUP_MIN);
        // A second pass has nothing left to roll up and no profile to write.
        let llm = Fixed(Mutex::new(vec![
            json!({"profile": "", "merge": [], "supersede": []}).to_string(),
        ]));
        let report = run(&m, Some(&llm), &spec(), "o").await.unwrap();
        assert_eq!(report.rolled_up, 0);
    }

    #[tokio::test]
    async fn without_a_model_only_the_code_steps_run() {
        let m = memories(50).await;
        m.remember("o", fact("Likes tea"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let report = run(&m, None, &spec(), "o").await.unwrap();
        assert!(report.model_skipped);
        assert!(m.profile("o").await.unwrap().is_none());
    }
}
