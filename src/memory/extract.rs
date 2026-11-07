//! The write path: what a run notes once it has answered.
//!
//! The model rarely calls `remember` on its own, so a run ends with one short tool-free call
//! that reads the conversation (what the user and the assistant said, and one line per tool
//! call; never tool results or attached files) next to what is already remembered, and answers
//! with JSON: the durable facts the user stated, each with the user's own words it is taken
//! from; a title and summary for the conversation as an episode; and, when the run did real
//! work that went well, one abstracted procedure.
//!
//! What the model proposes is admitted by checks in code, not trust: a fact is saved only if
//! its quoted support occurs in the user's own words (so nothing from a tool result, an email
//! or a web page can be saved as something the user said), never if it looks like a secret or an
//! identifier, and it is reconciled against what is remembered (see [`Memories::remember`]: the
//! same thing again strengthens, a keyed fact supersedes, a `supersedes` id closes the old one).
//! A procedure is kept only from a run that completed, and as title, one line, rationale and
//! pitfall with commands and values left out, because concrete traces of what went wrong make
//! later runs repeat it.

use super::recall::{rank_by_similarity, similarity};
use super::{tidy, Kind, Memories, Memory, NewMemory, Source};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

/// The system prompt of the note call. The run's own prompt is not used: the model is not acting
/// as the agent here, it is keeping the agent's notes.
pub const SYSTEM: &str =
    "You keep an AI assistant's notes about the person it works for and about \
    the work it does for them. After each conversation you read it and write what is worth \
    remembering for later conversations. You write notes; you do not answer the conversation.";

/// How much of the conversation the call reads, in characters.
pub const TRANSCRIPT_MAX_CHARS: usize = 24_000;
/// The most facts one call may save.
pub const MAX_FACTS: usize = 10;
/// How many remembered rows the call is shown as context, at most.
const SHOWN_MEMORIES: usize = 30;

/// What the note call asks for.
fn instruction(facts: bool, experience: bool, records: bool, outcome: &str) -> String {
    let mut s = String::from("Reply with JSON only, of this shape: {");
    if facts {
        s.push_str(
            "\"remember\": [{\"text\": \"...\", \"kind\": \"fact\", \"support\": \"...\", \
            \"supersedes\": null, \"subject\": null, \"relation\": null, \"valid_until\": null, \
            \"hedged\": false, \"sensitive\": false}], ",
        );
    }
    if records {
        s.push_str(
            "\"records\": [{\"set\": \"lowercase_name\", \"description\": \"...\", \"key\": [\"date\"], \
            \"rows\": [{\"date\": \"2026-11-06\", \"km\": 5.2}]}], ",
        );
    }
    if experience {
        s.push_str(
            "\"episode\": {\"title\": \"...\", \"summary\": \"...\", \"pitfall\": null}, \
            \"procedure\": null, ",
        );
    }
    s.push_str("\"topics\": [\"...\"]}.\n");
    if facts {
        s.push_str(
            "remember: every durable fact about the person that THEY state in their own \
            messages (marked \"User:\") and that is not already remembered: their name and how \
            they like to be addressed, where they live or come from, their work, studies, family, \
            pets, health needs and diet, the languages, tools and services they use, their tastes \
            and preferences, how they want answers (length, units, language, tone), standing \
            instructions, and the projects, plans and goals they are working on. Write each as one \
            short sentence about the person in the third person (\"Lives in Zurich\", \"Has two \
            daughters, 4 and 7\"), in the language of the conversation, keeping the detail they \
            gave, with relative dates made absolute (today is {today}). kind is fact, preference \
            (how they want things), instruction (a standing request to follow from now on), or \
            temporary (true only until a date: then set valid_until, an ISO date). support is the \
            person's own words you took it from, quoted exactly from a User message; a fact with \
            no such quote is not saved. If the fact replaces something remembered that is no \
            longer true, give that memory's id as supersedes. For a fact with one current value \
            (home city, employer, partner, phone, car) set subject (\"user\") and relation \
            (\"home city\"). Set hedged when they said it tentatively, sensitive for health, \
            religion, politics, sexuality or finances. Leave out the task at hand and its details, \
            one-off requests, questions, your guesses, secrets such as passwords and keys, and \
            anything quoted from a document, an email, a web page or a tool result rather than \
            said by the person. An empty list is a fine answer.\n",
        );
    }
    if records {
        s.push_str(
            "records: data the person handed over in their own messages or in the files they \
            attached, that they will want to come back to or compare with later: an export, \
            measurements, a log, a weekly or monthly report, a list of things they track. One JSON \
            object per row with flat lowercase field names, numbers as numbers, dates as ISO 8601 \
            strings; set is the set to save into (one listed under records when its columns fit, \
            else a new lowercase name with a description); key names the fields that identify a \
            row (a date, an id), so the same row saved again updates. Only values the person gave: \
            nothing from tool results, web pages or the assistant's own answers, and never the \
            task's text itself. An empty list is a fine answer; most conversations hold no \
            records.\n",
        );
    }
    if experience {
        s.push_str(&format!("episode: what this conversation was about, for the assistant's own \
            later reference: title (a few words), summary (one or two sentences in the third \
            person: what was asked, what was done, what was produced or decided, names and files \
            included), and pitfall (one sentence on what went wrong or what to avoid next time, \
            or null). The run ended: {outcome}.\n\
            procedure: only if the run completed and did work with tools that went well and that \
            the person may ask for again (a report, an import, a recurring check, a build), one \
            reusable way of doing it for this person, abstracted: {{\"title\": \"...\", \"summary\": \
            \"one sentence on the approach\", \"rationale\": \"why it suited them\", \"pitfall\": \
            \"what to avoid, or null\"}}, with no commands, paths, values or results in it. \
            Otherwise null.\n"));
    }
    s.push_str(
        "topics: up to five short tags for the conversation (a project name, a domain, a place).",
    );
    s.replace("{today}", &Utc::now().format("%Y-%m-%d").to_string())
}

/// A conversation as the note call reads it: who said what, one line per tool call, no tool
/// results, and the user's own words apart, for the support check.
#[derive(Debug, Default, Clone)]
pub struct Transcript {
    pub text: String,
    pub user_text: String,
    pub tools: Vec<String>,
    pub files: Vec<String>,
}

/// Build the transcript from the run's conversation: the stored messages (a continuation's
/// parent conversation included) with the run's prompt as the newest user message.
pub fn transcript(messages: &[Value], prompt: &str) -> Transcript {
    let mut lines: Vec<String> = Vec::new();
    let mut user_text = String::new();
    let mut tools = Vec::new();
    let mut files = Vec::new();
    let mut prompt_seen = false;
    for m in messages {
        let role = m["role"].as_str().unwrap_or("");
        match &m["content"] {
            Value::String(s) => {
                if role == "user" {
                    user_text.push_str(s);
                    user_text.push('\n');
                    if s.trim() == prompt.trim() {
                        prompt_seen = true;
                    }
                }
                lines.push(format!(
                    "{}: {}",
                    if role == "user" { "User" } else { "Assistant" },
                    s.trim()
                ));
            }
            Value::Array(blocks) => {
                let mut texts = Vec::new();
                for b in blocks {
                    match b["type"].as_str().unwrap_or("") {
                        "text" => {
                            if let Some(t) = b["text"].as_str() {
                                if role == "user" {
                                    user_text.push_str(t);
                                    user_text.push('\n');
                                    if t.trim() == prompt.trim() {
                                        prompt_seen = true;
                                    }
                                }
                                texts.push(t.trim().to_string());
                            }
                        }
                        "tool_use" => {
                            let name = b["name"].as_str().unwrap_or("tool");
                            if !tools.iter().any(|t| t == name) {
                                tools.push(name.to_string());
                            }
                            if let Some(path) =
                                b["input"]["path"].as_str().filter(|_| name == "write_file")
                            {
                                files.push(path.to_string());
                            }
                            let input = b["input"].to_string();
                            let input: String = input.chars().take(160).collect();
                            texts.push(format!("(called {name} {input})"));
                        }
                        // Tool results and attachments are never read.
                        _ => {}
                    }
                }
                if !texts.is_empty() {
                    lines.push(format!(
                        "{}: {}",
                        if role == "user" { "User" } else { "Assistant" },
                        texts.join(" ")
                    ));
                }
            }
            _ => {}
        }
    }
    if !prompt_seen && !prompt.trim().is_empty() {
        lines.push(format!("User: {}", prompt.trim()));
        user_text.push_str(prompt);
        user_text.push('\n');
    }
    // The newest part matters most: cut from the front.
    let mut text = lines.join("\n");
    let len = text.chars().count();
    if len > TRANSCRIPT_MAX_CHARS {
        let skip = len - TRANSCRIPT_MAX_CHARS;
        text = format!("[...]\n{}", text.chars().skip(skip).collect::<String>());
    }
    Transcript {
        text,
        user_text,
        tools,
        files,
    }
}

/// The note call's system prompt and messages: what is remembered (the rows most related to the
/// conversation), the transcript, and the instruction, in one user message.
pub fn call(
    remembered: &[Memory],
    sets: Option<&[(String, Vec<String>)]>,
    transcript: &Transcript,
    facts: bool,
    experience: bool,
    outcome: &str,
) -> (String, Vec<Value>) {
    let mut body = String::new();
    let about_user: Vec<Memory> = remembered
        .iter()
        .filter(|m| m.kind.is_about_user())
        .cloned()
        .collect();
    let shown: Vec<Memory> = if about_user.len() <= SHOWN_MEMORIES {
        about_user
    } else {
        let mut top: Vec<Memory> = rank_by_similarity(&transcript.user_text, &about_user)
            .into_iter()
            .take(SHOWN_MEMORIES)
            .map(|(m, _)| m)
            .collect();
        top.sort_by_key(|m| m.created_at);
        top
    };
    if shown.is_empty() {
        body.push_str("Nothing is remembered about this person yet.");
    } else {
        body.push_str("What is already remembered about this person, [id] (kind) text:");
        for m in &shown {
            body.push_str(&format!(
                "\n[{}] ({}) {}",
                m.short_id,
                m.kind.as_str(),
                m.text
            ));
        }
    }
    if let Some(sets) = sets {
        if sets.is_empty() {
            body.push_str("\n\nNo record sets are kept for this person yet.");
        } else {
            body.push_str("\n\nRecord sets kept for this person, name (columns):");
            for (name, columns) in sets {
                body.push_str(&format!("\n- {} ({})", name, columns.join(", ")));
            }
        }
    }
    body.push_str("\n\nThe conversation:\n\"\"\"\n");
    body.push_str(&transcript.text);
    body.push_str("\n\"\"\"\n\n---\n");
    body.push_str(&instruction(facts, experience, sets.is_some(), outcome));
    (
        SYSTEM.to_string(),
        vec![json!({ "role": "user", "content": body })],
    )
}

/// One fact the call proposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub kind: Option<Kind>,
    pub support: Option<String>,
    pub supersedes: Option<String>,
    pub subject: Option<String>,
    pub relation: Option<String>,
    pub valid_until: Option<DateTime<Utc>>,
    pub hedged: bool,
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EpisodeNote {
    pub title: String,
    pub summary: String,
    pub pitfall: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureNote {
    pub title: String,
    pub summary: String,
    pub rationale: Option<String>,
    pub pitfall: Option<String>,
}

/// Rows the call proposed to save as records.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordsNote {
    pub set: String,
    pub description: Option<String>,
    pub key: Vec<String>,
    pub rows: Vec<Value>,
}

/// What the call answered.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Noted {
    pub facts: Vec<Candidate>,
    pub records: Vec<RecordsNote>,
    pub episode: Option<EpisodeNote>,
    pub procedure: Option<ProcedureNote>,
    pub topics: Vec<String>,
}

/// The most sets one call may save into, and rows per set.
pub const MAX_RECORD_SETS: usize = 5;
pub const MAX_RECORD_ROWS: usize = 500;

fn opt_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "null")
        .map(String::from)
}

/// An ISO date or datetime as the end of a day.
pub fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Some(dt);
    }
    let date = NaiveDate::parse_from_str(&s[..s.len().min(10)], "%Y-%m-%d").ok()?;
    date.and_hms_opt(23, 59, 59).map(|d| d.and_utc())
}

/// Read the call's reply leniently: the JSON object, possibly in a code fence; `remember` may be
/// a bare array of objects or strings. Unusable entries are dropped; at most [`MAX_FACTS`] facts.
pub fn parse(text: &str) -> Noted {
    let object = |open: char, close: char| {
        let start = text.find(open)?;
        let end = text.rfind(close)?;
        (end > start)
            .then(|| serde_json::from_str::<Value>(&text[start..=end]).ok())
            .flatten()
    };
    let mut noted = Noted::default();
    let obj = match object('{', '}') {
        Some(v) if v.is_object() => v,
        _ => {
            if let Some(Value::Array(items)) = object('[', ']') {
                noted.facts = candidates(&items);
            }
            return noted;
        }
    };
    if let Some(items) = obj["remember"].as_array() {
        noted.facts = candidates(items);
    }
    for item in obj["records"].as_array().into_iter().flatten().take(MAX_RECORD_SETS) {
        let Some(set) = opt_str(&item["set"]) else { continue };
        let rows: Vec<Value> = item["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r.is_object())
            .take(MAX_RECORD_ROWS)
            .cloned()
            .collect();
        if rows.is_empty() {
            continue;
        }
        noted.records.push(RecordsNote {
            set: set.trim().to_lowercase(),
            description: opt_str(&item["description"]).map(|s| tidy(&s)),
            key: item["key"].as_array().map(|k| k.iter().filter_map(opt_str).collect()).unwrap_or_default(),
            rows,
        });
    }
    if let Some(title) = opt_str(&obj["episode"]["title"]) {
        noted.episode = Some(EpisodeNote {
            title: tidy(&title),
            summary: opt_str(&obj["episode"]["summary"])
                .map(|s| tidy(&s))
                .unwrap_or_default(),
            pitfall: opt_str(&obj["episode"]["pitfall"]).map(|s| tidy(&s)),
        });
    }
    if let (Some(title), Some(summary)) = (
        opt_str(&obj["procedure"]["title"]),
        opt_str(&obj["procedure"]["summary"]),
    ) {
        noted.procedure = Some(ProcedureNote {
            title: tidy(&title),
            summary: tidy(&summary),
            rationale: opt_str(&obj["procedure"]["rationale"]).map(|s| tidy(&s)),
            pitfall: opt_str(&obj["procedure"]["pitfall"]).map(|s| tidy(&s)),
        });
    }
    noted.topics = obj["topics"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(opt_str)
                .map(|s| tidy(&s).to_lowercase())
                .take(5)
                .collect()
        })
        .unwrap_or_default();
    noted
}

fn candidates(items: &[Value]) -> Vec<Candidate> {
    items
        .iter()
        .filter_map(|item| {
            let c = match item {
                Value::String(t) => Candidate {
                    text: tidy(t),
                    kind: None,
                    support: None,
                    supersedes: None,
                    subject: None,
                    relation: None,
                    valid_until: None,
                    hedged: false,
                    sensitive: false,
                },
                other => Candidate {
                    text: tidy(other["text"].as_str()?),
                    kind: other["kind"]
                        .as_str()
                        .and_then(Kind::parse)
                        .filter(|k| k.is_about_user()),
                    support: opt_str(&other["support"]),
                    supersedes: opt_str(&other["supersedes"])
                        .or_else(|| opt_str(&other["replaces"])),
                    subject: opt_str(&other["subject"]),
                    relation: opt_str(&other["relation"]),
                    valid_until: other["valid_until"].as_str().and_then(parse_date),
                    hedged: other["hedged"] == true,
                    sensitive: other["sensitive"] == true,
                },
            };
            (!c.text.is_empty()).then_some(c)
        })
        .take(MAX_FACTS)
        .collect()
}

/// Why a candidate was not saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub text: String,
    pub reason: String,
}

/// Whether `support` is what the user wrote: the quote, whitespace and case folded, occurs in the
/// user's own text, or every content word of it does in order of a short span.
pub fn supported(support: &str, user_text: &str) -> bool {
    let quote = tidy(support).to_lowercase();
    let quote = quote
        .trim_matches(|c: char| {
            c == '"' || c == '“' || c == '”' || c == '\'' || c == '.' || c == ','
        })
        .to_string();
    if quote.is_empty() {
        return false;
    }
    let text = tidy(user_text).to_lowercase();
    if text.contains(&quote) {
        return true;
    }
    // Lenient: the quote's words all occur in the user's text and the quote is close to it.
    let words = super::recall::tokens(&quote);
    if words.is_empty() {
        return false;
    }
    let user_words = super::recall::tokens(&text);
    words.iter().all(|w| user_words.contains(w)) && similarity(&quote, &text) > 0.0
}

/// Whether a field holds a date, whose value the model may have normalised to ISO form.
fn is_date_field(name: &str) -> bool {
    ["date", "day", "time", "week", "month", "year", "when", "at"]
        .iter()
        .any(|d| name == *d || name.ends_with(&format!("_{d}")) || name.starts_with(&format!("{d}_")))
}

/// Whether a value is what the user handed over: a string of three or more characters occurs
/// in `source` (the user's own words and attached files, case and whitespace folded), a number
/// occurs with a dot or a comma, and a date field is taken as given, since the model writes
/// dates in ISO form. Short strings, booleans and nulls carry nothing and pass.
fn value_supported(field: &str, value: &Value, source: &str) -> bool {
    match value {
        Value::String(s) => {
            let s = tidy(s).to_lowercase();
            if s.chars().count() < 3 {
                return true;
            }
            if is_date_field(field) {
                return s.chars().count() <= 25;
            }
            source.contains(&s)
        }
        Value::Number(n) => {
            let text = n.to_string();
            let plain = text.trim_end_matches(".0").to_string();
            source.contains(&plain) || source.contains(&plain.replace('.', ","))
        }
        Value::Array(items) => items.iter().all(|v| value_supported(field, v, source)),
        Value::Object(map) => map.iter().all(|(k, v)| value_supported(k, v, source)),
        _ => true,
    }
}

/// The rows of a proposed set that the user actually handed over (see [`value_supported`]), and
/// why the others were left out.
pub fn admit_rows(note: &RecordsNote, source_text: &str) -> (Vec<Value>, Vec<Skipped>) {
    let source = tidy(source_text).to_lowercase();
    let mut rows = Vec::new();
    let mut skipped = Vec::new();
    for row in &note.rows {
        let Some(map) = row.as_object() else { continue };
        match map.iter().find(|(k, v)| !value_supported(k, v, &source)) {
            None => rows.push(row.clone()),
            Some((k, v)) => skipped.push(Skipped {
                text: format!("{}: {}", note.set, row),
                reason: format!("{k} = {v} is not in the user's own words or files"),
            }),
        }
    }
    (rows, skipped)
}

/// Admit a candidate to [`NewMemory`], or say why not. `remembered` is what the call was shown,
/// to resolve `supersedes`.
pub fn admit(
    c: &Candidate,
    user_text: &str,
    remembered: &[Memory],
    run_id: Uuid,
    topics: &[String],
) -> Result<NewMemory, Skipped> {
    let skip = |reason: &str| Skipped {
        text: c.text.clone(),
        reason: reason.into(),
    };
    let Some(support) = c.support.as_deref().filter(|s| !s.trim().is_empty()) else {
        return Err(skip("no quote from the user's own words"));
    };
    if !supported(support, user_text) {
        return Err(skip("the quote is not in the user's own words"));
    }
    if let Some(why) = super::excluded(&c.text) {
        return Err(skip(why));
    }
    let supersedes = match &c.supersedes {
        Some(id) => {
            let id = id.trim().trim_matches(['[', ']']);
            let found = remembered
                .iter()
                .find(|m| m.short_id == id || m.id.to_string() == id)
                .map(|m| m.id);
            // An id that isn't the owner's is ignored, and the fact saved as new.
            found
        }
        None => None,
    };
    let kind = c.kind.unwrap_or(Kind::Fact);
    let kind = if c.valid_until.is_some() && kind == Kind::Fact {
        Kind::Temporary
    } else {
        kind
    };
    Ok(NewMemory {
        kind: Some(kind),
        text: c.text.clone(),
        subject: c.subject.clone(),
        relation: c.relation.clone(),
        topics: topics.to_vec(),
        source: Some(Source::User),
        support: Some(support.to_string()),
        hedged: c.hedged,
        sensitive: c.sensitive,
        pinned: None,
        meta: None,
        run_id: Some(run_id),
        valid_from: None,
        valid_until: c.valid_until,
        supersedes,
        replaces: None,
    })
}

/// The episode row for a run: what the model summarised, or a bare line from the prompt.
pub fn episode(
    run_id: Uuid,
    prompt: &str,
    noted: Option<&EpisodeNote>,
    outcome: &str,
    transcript: &Transcript,
    topics: &[String],
) -> NewMemory {
    let text = match noted {
        Some(e) if !e.summary.is_empty() => format!("{}: {}", e.title, e.summary),
        Some(e) => e.title.clone(),
        None => {
            let p: String = tidy(prompt).chars().take(200).collect();
            format!("Asked: {p}")
        }
    };
    let text: String = text.chars().take(super::MAX_CHARS).collect();
    let mut meta = json!({
        "outcome": outcome,
        "tools": transcript.tools,
    });
    if let Some(title) = noted.map(|e| e.title.as_str()) {
        meta["title"] = json!(title);
    }
    if let Some(p) = noted.and_then(|e| e.pitfall.as_deref()) {
        meta["pitfall"] = json!(p);
    }
    if !transcript.files.is_empty() {
        meta["files"] = json!(transcript.files);
    }
    NewMemory {
        kind: Some(Kind::Episode),
        text,
        topics: topics.to_vec(),
        source: Some(Source::Agent),
        meta: Some(meta),
        run_id: Some(run_id),
        ..Default::default()
    }
}

/// The procedure row a completed run learned.
pub fn procedure(run_id: Uuid, p: &ProcedureNote, topics: &[String]) -> NewMemory {
    let text: String = format!("{}: {}", p.title, p.summary)
        .chars()
        .take(super::MAX_CHARS)
        .collect();
    NewMemory {
        kind: Some(Kind::Procedure),
        text,
        topics: topics.to_vec(),
        source: Some(Source::Agent),
        meta: Some(json!({
            "title": p.title,
            "rationale": p.rationale,
            "pitfall": p.pitfall,
            "successes": 0,
            "failures": 0,
            "learned_in": run_id,
        })),
        run_id: Some(run_id),
        ..Default::default()
    }
}

impl Memories {
    /// Record how a run that was shown procedures went, or the user's verdict on it:
    /// `good` adds to their successes, otherwise to their failures, and a procedure that has
    /// failed more than it succeeded is archived, so a bad one does not compound.
    pub async fn procedures_went(
        &self,
        owner: &str,
        ids: &[Uuid],
        good: bool,
        actor: super::Actor<'_>,
    ) -> anyhow::Result<Vec<Memory>> {
        let mut archived = Vec::new();
        for id in ids {
            let Some(m) = self.get(owner, *id).await? else {
                continue;
            };
            if m.kind != Kind::Procedure || !m.is_active() {
                continue;
            }
            let mut meta = m.meta.clone();
            let key = if good { "successes" } else { "failures" };
            meta[key] = json!(meta[key].as_i64().unwrap_or(0) + 1);
            let successes = meta["successes"].as_i64().unwrap_or(0);
            let failures = meta["failures"].as_i64().unwrap_or(0);
            let _ = self
                .edit(
                    owner,
                    *id,
                    &super::Edit {
                        meta: Some(meta),
                        ..Default::default()
                    },
                    actor,
                )
                .await?;
            if !good && failures > successes && !m.pinned {
                self.archive_because(
                    owner,
                    *id,
                    "archive",
                    "the task failed when this was followed",
                    actor,
                )
                .await?;
                archived.push(m);
            }
        }
        Ok(archived)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::tests::{fact, memories};
    use crate::memory::Actor;

    #[test]
    fn the_transcript_reads_text_and_tool_calls_but_no_results() {
        let messages = vec![
            json!({ "role": "user", "content": "Hi, I'm Ana. Run primes.py" }),
            json!({ "role": "assistant", "content": [
                { "type": "text", "text": "Sure." },
                { "type": "tool_use", "id": "t1", "name": "bash", "input": { "command": "python primes.py" } },
            ]}),
            json!({ "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": "SECRET OUTPUT: lives in Oslo" },
            ]}),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "Done: 2 3 5 7." }] }),
        ];
        let t = transcript(&messages, "Hi, I'm Ana. Run primes.py");
        assert!(
            t.text
                .starts_with("User: Hi, I'm Ana. Run primes.py\nAssistant: Sure. (called bash"),
            "{}",
            t.text
        );
        assert!(!t.text.contains("SECRET"), "{}", t.text);
        assert!(!t.user_text.contains("SECRET"));
        assert_eq!(t.tools, ["bash"]);
        // The prompt of a run whose messages were not stored yet is appended once.
        let t = transcript(&[], "Hello there");
        assert_eq!(t.text, "User: Hello there");
        assert_eq!(t.user_text.trim(), "Hello there");
        let t = transcript(&[], &"x".repeat(TRANSCRIPT_MAX_CHARS * 2));
        assert!(t.text.starts_with("[...]"));
        assert!(t.text.len() < TRANSCRIPT_MAX_CHARS + 100);
    }

    #[test]
    fn the_call_shows_memories_and_asks_for_the_right_sections() {
        let t = transcript(&[], "I moved to Zurich");
        let (system, messages) = call(&[], None, &t, true, false, "completed");
        assert_eq!(system, SYSTEM);
        let body = messages[0]["content"].as_str().unwrap();
        assert!(body.starts_with("Nothing is remembered"), "{body}");
        assert!(body.contains("I moved to Zurich"));
        assert!(body.contains("\"remember\""));
        assert!(!body.contains("\"episode\""));
        let (_, messages) = call(&[], None, &t, false, true, "failed: timed out");
        let body = messages[0]["content"].as_str().unwrap();
        assert!(!body.contains("\"remember\""));
        assert!(body.contains("\"episode\"") && body.contains("The run ended: failed: timed out"));
        assert!(!body.contains("\"records\""));
        let sets = vec![("running_log".to_string(), vec!["date".to_string(), "km".to_string()])];
        let (_, messages) = call(&[], Some(&sets), &t, true, false, "completed");
        let body = messages[0]["content"].as_str().unwrap();
        assert!(body.contains("Record sets kept for this person, name (columns):\n- running_log (date, km)"), "{body}");
        assert!(body.contains("\"records\": [{"), "{body}");
    }

    #[test]
    fn proposed_rows_are_admitted_only_from_the_users_own_words_or_files() {
        let noted = parse(
            "{\"records\": [{\"set\": \"Running_Log\", \"description\": \"Weekly runs\", \"key\": [\"date\"], \"rows\": [\
             {\"date\": \"2026-11-01\", \"km\": 5.2, \"note\": \"easy run\"}, \
             {\"date\": \"2026-11-02\", \"km\": 10, \"note\": \"hard run\"}, \
             {\"date\": \"2026-11-03\", \"km\": 42.2, \"note\": \"marathon\"}]}, \
             {\"set\": \"empty\", \"rows\": []}]}",
        );
        assert_eq!(noted.records.len(), 1);
        assert_eq!(noted.records[0].set, "running_log");
        assert_eq!(noted.records[0].key, ["date"]);
        let source = "Sunday 1 Nov: 5,2 km, easy run. Monday: 10 km, hard run.\nAttached: runs.csv";
        let (rows, skipped) = admit_rows(&noted.records[0], source);
        assert_eq!(rows.len(), 2, "{skipped:?}");
        assert_eq!(rows[1]["km"], 10);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].reason.contains("km = 42.2"), "{}", skipped[0].reason);
    }

    #[test]
    fn the_reply_is_read_leniently() {
        let noted = parse(
            "```json\n{\"remember\": [{\"text\": \"Lives in  Zurich\", \"kind\": \"fact\", \"support\": \"I live in Zurich\", \
             \"supersedes\": \"[m1]\", \"subject\": \"user\", \"relation\": \"home city\"}, \
             {\"text\": \"Has an exam\", \"valid_until\": \"2026-11-12\"}, \"Prefers tea\", {\"text\": \"  \"}, 7], \
             \"episode\": {\"title\": \"Primes script\", \"summary\": \"Wrote primes.py.\", \"pitfall\": null}, \
             \"procedure\": {\"title\": \"Prime lists\", \"summary\": \"Sieve, then print.\", \"rationale\": \"fast\"}, \
             \"topics\": [\"Primes\", \"python\"]}\n```",
        );
        assert_eq!(noted.facts.len(), 3);
        assert_eq!(noted.facts[0].text, "Lives in Zurich");
        assert_eq!(noted.facts[0].supersedes.as_deref(), Some("[m1]"));
        assert_eq!(noted.facts[0].relation.as_deref(), Some("home city"));
        assert_eq!(
            noted.facts[1]
                .valid_until
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2026-11-12"
        );
        assert_eq!(noted.facts[2].text, "Prefers tea");
        assert_eq!(noted.episode.as_ref().unwrap().title, "Primes script");
        assert_eq!(
            noted.procedure.as_ref().unwrap().rationale.as_deref(),
            Some("fast")
        );
        assert_eq!(noted.topics, ["primes", "python"]);
        assert_eq!(parse("[\"a\", \"b\"]").facts.len(), 2);
        assert_eq!(parse("Nothing new here."), Noted::default());
        assert!(
            parse("{\"remember\": \"Lives in Zurich\", \"procedure\": null}")
                .facts
                .is_empty()
        );
        let many: Vec<String> = (0..20).map(|i| format!("\"fact {i}\"")).collect();
        assert_eq!(
            parse(&format!("[{}]", many.join(","))).facts.len(),
            MAX_FACTS
        );
    }

    #[test]
    fn support_must_be_the_users_own_words() {
        let user = "Hi! I'm Ana, I live in Zürich with my cat Mio.\nCan you check the weather?";
        assert!(supported("I live in Zürich", user));
        assert!(supported("\"i live in zurich with my cat mio.\"", user));
        assert!(supported("live in Zurich, cat Mio", user));
        assert!(!supported("works at Acme", user));
        assert!(!supported("", user));
        assert!(!supported("The page says she lives in Oslo", user));
    }

    #[tokio::test]
    async fn admission_checks_the_quote_the_exclusions_and_resolves_ids() {
        let m = memories(10).await;
        let (bern, _) = m
            .remember("o", fact("Lives in Bern"), Actor::User)
            .await
            .unwrap()
            .unwrap();
        let remembered = m.active("o").await.unwrap();
        let user = "I moved to Zurich! My card is 4111 1111 1111 1111.";
        let run = Uuid::now_v7();
        let c = |text: &str, support: Option<&str>| Candidate {
            text: text.into(),
            kind: None,
            support: support.map(String::from),
            supersedes: Some("m1".into()),
            subject: None,
            relation: None,
            valid_until: None,
            hedged: false,
            sensitive: false,
        };
        let ok = admit(
            &c("Lives in Zurich", Some("I moved to Zurich")),
            user,
            &remembered,
            run,
            &[],
        )
        .unwrap();
        assert_eq!(ok.supersedes, Some(bern.id));
        assert_eq!(ok.kind, Some(Kind::Fact));
        assert_eq!(ok.support.as_deref(), Some("I moved to Zurich"));
        let no_quote = admit(&c("Lives in Zurich", None), user, &remembered, run, &[]).unwrap_err();
        assert!(no_quote.reason.contains("no quote"));
        let wrong_quote = admit(
            &c("Lives in Oslo", Some("she lives in Oslo")),
            user,
            &remembered,
            run,
            &[],
        )
        .unwrap_err();
        assert!(wrong_quote.reason.contains("not in the user's own words"));
        let card = admit(
            &c(
                "Card is 4111 1111 1111 1111",
                Some("My card is 4111 1111 1111 1111"),
            ),
            user,
            &remembered,
            run,
            &[],
        )
        .unwrap_err();
        assert!(card.reason.contains("card"));
        // An unknown supersedes id is ignored, not refused.
        let mut other = c("Lives in Zurich", Some("I moved to Zurich"));
        other.supersedes = Some("m99".into());
        assert_eq!(
            admit(&other, user, &remembered, run, &[])
                .unwrap()
                .supersedes,
            None
        );
        // A date makes a fact temporary.
        let mut exam = c("Has an exam", Some("I moved to Zurich"));
        exam.valid_until = parse_date("2026-11-12");
        assert_eq!(
            admit(&exam, user, &remembered, run, &[]).unwrap().kind,
            Some(Kind::Temporary)
        );
    }

    #[test]
    fn episode_and_procedure_rows() {
        let run = Uuid::now_v7();
        let t = Transcript {
            text: String::new(),
            user_text: String::new(),
            tools: vec!["bash".into()],
            files: vec!["primes.py".into()],
        };
        let e = episode(
            run,
            "Write primes.py and run it",
            None,
            "completed",
            &t,
            &["python".into()],
        );
        assert_eq!(e.kind, Some(Kind::Episode));
        assert_eq!(e.text, "Asked: Write primes.py and run it");
        assert_eq!(e.meta.as_ref().unwrap()["files"][0], "primes.py");
        let e = episode(
            run,
            "x",
            Some(&EpisodeNote {
                title: "Primes".into(),
                summary: "Wrote primes.py.".into(),
                pitfall: Some("no numpy".into()),
            }),
            "failed: x",
            &t,
            &[],
        );
        assert_eq!(e.text, "Primes: Wrote primes.py.");
        assert_eq!(e.meta.as_ref().unwrap()["pitfall"], "no numpy");
        let p = procedure(
            run,
            &ProcedureNote {
                title: "Prime lists".into(),
                summary: "Sieve.".into(),
                rationale: None,
                pitfall: None,
            },
            &[],
        );
        assert_eq!(p.text, "Prime lists: Sieve.");
        assert_eq!(p.meta.as_ref().unwrap()["successes"], 0);
    }

    #[tokio::test]
    async fn a_procedure_that_keeps_failing_is_archived() {
        let m = memories(10).await;
        let run = Uuid::now_v7();
        let mut new = procedure(
            run,
            &ProcedureNote {
                title: "T".into(),
                summary: "S".into(),
                rationale: None,
                pitfall: None,
            },
            &[],
        );
        new.run_id = None; // no such run in this database
        let (p, _) = m
            .remember("o", new, Actor::Run(run))
            .await
            .unwrap()
            .unwrap();
        assert!(m
            .procedures_went("o", &[p.id], true, Actor::User)
            .await
            .unwrap()
            .is_empty());
        assert!(m
            .procedures_went("o", &[p.id], false, Actor::User)
            .await
            .unwrap()
            .is_empty());
        let archived = m
            .procedures_went("o", &[p.id], false, Actor::User)
            .await
            .unwrap();
        assert_eq!(archived.len(), 1);
        assert!(m
            .get("o", p.id)
            .await
            .unwrap()
            .unwrap()
            .archived_at
            .is_some());
    }
}
