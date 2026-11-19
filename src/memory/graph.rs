//! Memory as a network: every row a node, and an edge wherever two rows are already linked by
//! what cedar stores. Nothing here is kept; the graph is built per request from the rows, the
//! audit log and the `memory.recalled` events, so it always shows memory as it is.
//!
//! The edges, by kind:
//!
//! - `conversation`: saved by the same run (an episode with the facts and the procedure its
//!   conversation left behind), drawn as a star around the episode.
//! - `topic`: sharing topic tags; the weight is how many.
//! - `recalled`: shown together to a run because the request called for both (pinned rows and a
//!   store shown whole say nothing about relatedness, so they don't count); the weight is how
//!   often.
//! - `superseded` and `merged`: directed, from the old row to the one that replaced it. They
//!   only appear with archived rows included, since a replaced row is not active.
//!
//! A tag shared by fifty conversations would be 1,225 edges, so `topic` and `recalled` edges are
//! thinned: one is kept when it is among the [`NEIGHBOURS`] strongest of either of its ends.

use super::{from_row, Kind, Memories, Memory, COLS};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::Row;
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

/// How many nodes a graph has unless the request says otherwise.
pub const DEFAULT_NODES: usize = 300;
/// The most nodes a request may ask for.
pub const MAX_NODES: usize = 1_000;
/// How many `topic` and how many `recalled` edges a node keeps on its own account.
pub const NEIGHBOURS: usize = 4;
/// How many of the owner's newest runs are read for what was recalled together.
const RECALL_RUNS: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Conversation,
    Topic,
    Recalled,
    Superseded,
    Merged,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    #[serde(flatten)]
    pub memory: Memory,
    /// [`Memory::rank`] now, rounded: how much the row weighs in recall.
    pub rank: f64,
    /// How many edges end here.
    pub degree: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub source: Uuid,
    pub target: Uuid,
    pub kind: EdgeKind,
    /// Shared tags for `topic`, times shown together for `recalled`, otherwise 1.
    pub weight: u32,
    /// The tags a `topic` edge stands for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// How many rows there were to show, and how many of them didn't fit `limit` (the
    /// lowest-ranked ones).
    pub total: usize,
    pub left_out: usize,
}

/// Two nodes by their place in the list, the older first.
type Pair = (usize, usize);

/// Keep the weighted pairs that are among the `NEIGHBOURS` strongest of either end. Ties go to
/// the rows closest in age, so the result doesn't depend on map order.
fn thin<T>(pairs: BTreeMap<Pair, T>, weight: impl Fn(&T) -> u32) -> Vec<(Pair, T)> {
    // Per node: the weight, how far apart in age, and the pair.
    let mut by_node: HashMap<usize, Vec<(u32, usize, Pair)>> = HashMap::new();
    for (&(a, b), v) in &pairs {
        let w = weight(v);
        by_node.entry(a).or_default().push((w, b - a, (a, b)));
        by_node.entry(b).or_default().push((w, b - a, (a, b)));
    }
    let mut keep: HashSet<Pair> = HashSet::new();
    for list in by_node.values_mut() {
        list.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
        keep.extend(list.iter().take(NEIGHBOURS).map(|x| x.2));
    }
    pairs.into_iter().filter(|(k, _)| keep.contains(k)).collect()
}

/// The graph of `items` (oldest first). `recalls` is one list per run of the rows its request
/// called for, `merged` the rows a merge closed (the others with `superseded_by` were
/// superseded), and at most `limit` rows become nodes, the highest-ranked ones.
pub fn build(
    items: Vec<Memory>,
    recalls: &[Vec<Uuid>],
    merged: &HashSet<Uuid>,
    limit: usize,
    now: DateTime<Utc>,
) -> Graph {
    let total = items.len();
    let mut ranked: Vec<(f64, Memory)> = items.into_iter().map(|m| (m.rank(now), m)).collect();
    if ranked.len() > limit {
        // Active rows before archived ones, then by rank; the survivors go back in age order.
        ranked.sort_by(|a, b| {
            b.1.is_active()
                .cmp(&a.1.is_active())
                .then(b.0.total_cmp(&a.0))
                .then(b.1.id.cmp(&a.1.id))
        });
        ranked.truncate(limit);
        ranked.sort_by_key(|(_, m)| m.id);
    }
    let index: HashMap<Uuid, usize> = ranked.iter().enumerate().map(|(i, (_, m))| (m.id, i)).collect();
    let id = |i: usize| ranked[i].1.id;
    let mut edges: Vec<Edge> = Vec::new();
    let edge = |a: usize, b: usize, kind, weight, topics| Edge { source: id(a), target: id(b), kind, weight, topics };

    // Saved by the same run: a star around the episode, or the oldest row when there is none.
    let mut by_run: BTreeMap<Uuid, Vec<usize>> = BTreeMap::new();
    for (i, (_, m)) in ranked.iter().enumerate() {
        if let Some(run) = m.run_id {
            by_run.entry(run).or_default().push(i);
        }
    }
    for group in by_run.values().filter(|g| g.len() > 1) {
        let anchor = group.iter().copied().find(|&i| ranked[i].1.kind == Kind::Episode).unwrap_or(group[0]);
        for &i in group.iter().filter(|&&i| i != anchor) {
            edges.push(edge(anchor.min(i), anchor.max(i), EdgeKind::Conversation, 1, Vec::new()));
        }
    }

    // Shared tags.
    let mut by_topic: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, (_, m)) in ranked.iter().enumerate() {
        for t in &m.topics {
            let members = by_topic.entry(t.as_str()).or_default();
            if members.last() != Some(&i) {
                members.push(i);
            }
        }
    }
    let mut shared: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
    for (topic, members) in &by_topic {
        for (n, &a) in members.iter().enumerate() {
            for &b in &members[n + 1..] {
                shared.entry((a, b)).or_default().push(topic.to_string());
            }
        }
    }
    for ((a, b), topics) in thin(shared, |t| t.len() as u32) {
        edges.push(edge(a, b, EdgeKind::Topic, topics.len() as u32, topics));
    }

    // Shown together because a request called for both.
    let mut together: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    for ids in recalls {
        let mut shown: Vec<usize> = ids.iter().filter_map(|id| index.get(id).copied()).collect();
        shown.sort_unstable();
        shown.dedup();
        for (n, &a) in shown.iter().enumerate() {
            for &b in &shown[n + 1..] {
                *together.entry((a, b)).or_default() += 1;
            }
        }
    }
    for ((a, b), times) in thin(together, |&t| t) {
        edges.push(edge(a, b, EdgeKind::Recalled, times, Vec::new()));
    }

    // Replaced by: from the old row to the new one.
    for (i, (_, m)) in ranked.iter().enumerate() {
        if let Some(&to) = m.superseded_by.as_ref().and_then(|id| index.get(id)) {
            let kind = if merged.contains(&m.id) { EdgeKind::Merged } else { EdgeKind::Superseded };
            edges.push(edge(i, to, kind, 1, Vec::new()));
        }
    }

    let mut degree = vec![0usize; ranked.len()];
    for e in &edges {
        degree[index[&e.source]] += 1;
        degree[index[&e.target]] += 1;
    }
    let left_out = total - ranked.len();
    let nodes = ranked
        .into_iter()
        .zip(degree)
        .map(|((rank, memory), degree)| Node { memory, rank: (rank * 100.0).round() / 100.0, degree })
        .collect();
    Graph { nodes, edges, total, left_out }
}

impl Memories {
    /// The owner's memory as a graph (see the module): the active rows, with `archived` the
    /// archived and superseded ones as well, at most `limit` of them.
    pub async fn graph(&self, owner: &str, archived: bool, limit: usize) -> Result<Graph> {
        let pool = self.store().pool();
        let rows = sqlx::query(&format!("SELECT {COLS} FROM memories WHERE owner = ? ORDER BY seq"))
            .bind(owner)
            .fetch_all(pool)
            .await?;
        let mut items: Vec<Memory> = rows.iter().map(from_row).collect::<Result<_>>()?;
        if !archived {
            items.retain(Memory::is_active);
        }

        // What the newest runs were shown because their request called for it.
        let rows = sqlx::query(
            "SELECT data FROM events WHERE type = 'memory.recalled' \
             AND run_id IN (SELECT id FROM runs WHERE owner = ? ORDER BY id DESC LIMIT ?)",
        )
        .bind(owner)
        .bind(RECALL_RUNS)
        .fetch_all(pool)
        .await?;
        let mut recalls: Vec<Vec<Uuid>> = Vec::new();
        for row in rows {
            let data: Value = serde_json::from_str(row.try_get("data")?).unwrap_or_default();
            let ids: Vec<Uuid> = data["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|i| matches!(i["why"].as_str(), Some("match" | "cue")))
                .filter_map(|i| i["id"].as_str()?.parse().ok())
                .collect();
            if ids.len() > 1 {
                recalls.push(ids);
            }
        }

        let merged: HashSet<Uuid> = if archived {
            sqlx::query("SELECT memory_id FROM memory_ops WHERE owner = ? AND op = 'merge'")
                .bind(owner)
                .fetch_all(pool)
                .await?
                .iter()
                .filter_map(|r| r.try_get::<String, _>("memory_id").ok()?.parse().ok())
                .collect()
        } else {
            HashSet::new()
        };
        Ok(build(items, &recalls, &merged, limit.clamp(1, MAX_NODES), Utc::now()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Source;
    use serde_json::json;

    fn mem(seq: i64, kind: Kind, topics: &[&str]) -> Memory {
        let now = Utc::now();
        Memory {
            id: Uuid::now_v7(),
            short_id: format!("m{seq}"),
            kind,
            text: format!("memory {seq}"),
            subject: None,
            relation: None,
            topics: topics.iter().map(|t| t.to_string()).collect(),
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
        }
    }

    fn of(g: &Graph, kind: EdgeKind) -> Vec<(String, String, u32)> {
        let name = |id: Uuid| g.nodes.iter().find(|n| n.memory.id == id).unwrap().memory.short_id.clone();
        g.edges
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| (name(e.source), name(e.target), e.weight))
            .collect()
    }

    #[test]
    fn rows_are_linked_by_run_topic_recall_and_replacement() {
        let run = Uuid::now_v7();
        let mut fact = mem(1, Kind::Fact, &["berlin", "travel"]);
        let mut episode = mem(2, Kind::Episode, &["berlin", "travel"]);
        let mut procedure = mem(3, Kind::Procedure, &["travel"]);
        let mut old = mem(4, Kind::Fact, &[]);
        let new = mem(5, Kind::Fact, &[]);
        let mut dup = mem(6, Kind::Fact, &[]);
        for m in [&mut fact, &mut episode, &mut procedure] {
            m.run_id = Some(run);
        }
        old.superseded_by = Some(new.id);
        dup.superseded_by = Some(new.id);
        let recalls = vec![vec![fact.id, new.id], vec![new.id, fact.id, Uuid::now_v7()]];
        let merged = HashSet::from([dup.id]);
        let g = build(vec![fact, episode, procedure, old, new, dup], &recalls, &merged, 100, Utc::now());

        assert_eq!((g.nodes.len(), g.total, g.left_out), (6, 6, 0));
        // A star around the episode, not a triangle.
        assert_eq!(
            of(&g, EdgeKind::Conversation),
            [("m1".into(), "m2".into(), 1), ("m2".into(), "m3".into(), 1)]
        );
        let topic = of(&g, EdgeKind::Topic);
        assert!(topic.contains(&("m1".into(), "m2".into(), 2)), "{topic:?}");
        assert!(topic.contains(&("m2".into(), "m3".into(), 1)), "{topic:?}");
        let both = g.edges.iter().find(|e| e.kind == EdgeKind::Topic && e.weight == 2).unwrap();
        assert_eq!(both.topics, ["berlin", "travel"]);
        assert_eq!(of(&g, EdgeKind::Recalled), [("m1".into(), "m5".into(), 2)]);
        assert_eq!(of(&g, EdgeKind::Superseded), [("m4".into(), "m5".into(), 1)]);
        assert_eq!(of(&g, EdgeKind::Merged), [("m6".into(), "m5".into(), 1)]);
        let degree = |s: &str| g.nodes.iter().find(|n| n.memory.short_id == s).unwrap().degree;
        assert_eq!(degree("m5"), 3);
        assert_eq!(degree("m4"), 1);
    }

    #[test]
    fn a_common_tag_is_thinned_and_a_limit_keeps_the_highest_ranked() {
        let items: Vec<Memory> = (1..=30).map(|i| mem(i, Kind::Episode, &["work"])).collect();
        let g = build(items, &[], &HashSet::new(), 100, Utc::now());
        // 435 pairs share the tag; each node keeps its nearest few.
        assert!(g.edges.len() <= 30 * NEIGHBOURS, "{}", g.edges.len());
        assert!(g.nodes.iter().all(|n| n.degree >= NEIGHBOURS));

        let mut items: Vec<Memory> = (1..=5).map(|i| mem(i, Kind::Episode, &[])).collect();
        items[3].kind = Kind::Instruction;
        items[1].archived_at = Some(Utc::now());
        items[1].kind = Kind::Instruction;
        let g = build(items, &[], &HashSet::new(), 2, Utc::now());
        assert_eq!((g.total, g.left_out), (5, 3));
        let kept: Vec<&str> = g.nodes.iter().map(|n| n.memory.short_id.as_str()).collect();
        // The active instruction and one episode; the archived instruction yields to active rows.
        assert_eq!(kept.len(), 2);
        assert!(kept.contains(&"m4") && !kept.contains(&"m2"), "{kept:?}");
    }
}
