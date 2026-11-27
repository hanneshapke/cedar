//! `GET /api/search?q=`: everything of the person's that has the words, for the Today page's
//! search box: conversations (as `/api/conversations/search` finds them, each with its latest
//! turn so a hit opens as an outcome), agents by title or prompt, what Cedar remembers, and
//! record sets by name or description.
//!
//! Workspace files are not searched: they live in each run's sandbox, and listing every one
//! of them for a search would start containers. A part cedar can't give is left empty.

use super::*;
use crate::store::search_terms;

/// How many hits of each kind one search returns.
const MAX_PER_KIND: u32 = 20;
/// The memory kinds about the user, as `/api/memory` lists them by default.
const USER_KINDS: &str = "fact,preference,instruction,temporary";

#[derive(Deserialize)]
pub(super) struct Query {
    #[serde(default)]
    q: String,
}

pub(super) async fn search(Scoped(state): Scoped, ApiQuery(query): ApiQuery<Query>) -> Result<Json<Value>, AppError> {
    let q = query.q.trim();
    if q.chars().count() > MAX_SEARCH_CHARS {
        return Err(AppError::BadRequest(format!("q is longer than {MAX_SEARCH_CHARS} characters")));
    }
    let terms = search_terms(q);
    if terms.is_empty() {
        return Ok(Json(json!({ "q": q, "conversations": [], "agents": [], "memories": [], "records": [] })));
    }

    let hits = state.store.search_conversations(q, MAX_PER_KIND).await?;
    let mut conversations = Vec::with_capacity(hits.len());
    for hit in &hits {
        let turn = state.store.list_turns(hit.conversation.id).await?.last().map(|t| t.id);
        let mut row = serde_json::to_value(hit).map_err(|e| AppError::Internal(e.into()))?;
        row["turn_id"] = json!(turn);
        conversations.push(row);
    }

    let agents: Vec<Value> = state.store.search_schedules(q, MAX_PER_KIND).await?.iter().map(|s| timeline::agent_view(s, None)).collect();

    let memories: Vec<Value> = match state.cedar.memory(&[("kind", USER_KINDS), ("q", q)]).await {
        Ok(Some(body)) => body["items"]
            .as_array()
            .into_iter()
            .flatten()
            // cedar's own search may be looser than "every word"; keep it to that, as elsewhere.
            .filter(|m| has_all(&terms, &[m["text"].as_str().unwrap_or_default()]))
            .take(MAX_PER_KIND as usize)
            .map(|m| json!({ "id": m["id"], "text": m["text"], "kind": m["kind"] }))
            .collect(),
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::debug!(error = %e, "could not search what Cedar remembers");
            Vec::new()
        }
    };

    let records: Vec<Value> = match state.cedar.records().await {
        Ok(Some(body)) => body["sets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| has_all(&terms, &[s["name"].as_str().unwrap_or_default(), s["description"].as_str().unwrap_or_default()]))
            .take(MAX_PER_KIND as usize)
            .map(|s| json!({ "name": s["name"], "description": s["description"], "count": s["count"] }))
            .collect(),
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::debug!(error = %e, "could not search the record sets");
            Vec::new()
        }
    };

    Ok(Json(json!({ "q": q, "conversations": conversations, "agents": agents, "memories": memories, "records": records })))
}

/// Whether every term is in one of `texts`, whatever the case.
fn has_all(terms: &[String], texts: &[&str]) -> bool {
    let texts: Vec<String> = texts.iter().map(|t| t.to_lowercase()).collect();
    terms.iter().all(|term| texts.iter().any(|t| t.contains(term.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_must_be_somewhere() {
        let terms = search_terms("Lisbon TRIP");
        assert!(has_all(&terms, &["Trips", "A weekend in Lisbon"]));
        assert!(!has_all(&terms, &["Trips", "A weekend in Porto"]));
    }
}
