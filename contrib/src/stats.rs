//! The numbers the steady state is run by (`cedar-contrib stats`): what came in, what became of
//! it, how long things wait, how much of it is one person's. Written as `stats/<day>.json` so
//! a month's snapshots make a trend. Counts only; never a pseudonym, never text.

use crate::store::Store;
use anyhow::Result;
use chrono::{Duration, NaiveDate, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

pub async fn compute(store: &dyn Store, days: u32) -> Result<Value> {
    let today = Utc::now().date_naive();
    let since = (today - Duration::days(days as i64)).format("%Y-%m-%d").to_string();
    let receipts = store.list_receipts().await?;
    let withdrawn = store.list_withdrawn().await?;
    let audit = store.list_audit(&since).await?;

    let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_day: BTreeMap<String, usize> = BTreeMap::new();
    let mut flags: BTreeMap<String, usize> = BTreeMap::new();
    let mut pseudonyms: HashSet<&str> = HashSet::new();
    let mut unlinked = 0usize;
    let mut in_window = 0usize;
    let mut held_ages: Vec<i64> = Vec::new();
    let mut weights: BTreeMap<&str, usize> = BTreeMap::new();
    let mut judged = 0usize;
    for r in &receipts {
        *by_status.entry(r.status.clone()).or_default() += 1;
        if r.day >= since {
            in_window += 1;
            *per_day.entry(r.day.clone()).or_default() += 1;
        }
        match &r.pseudonym {
            Some(p) => {
                pseudonyms.insert(p);
            }
            None => unlinked += 1,
        }
        if let Some(v) = &r.verdict {
            for f in &v.flags {
                *flags.entry(f.clone()).or_default() += 1;
            }
            if v.scores.contains_key("judge_harm") {
                judged += 1;
            }
            if r.status == "held" {
                if let Ok(d) = NaiveDate::parse_from_str(&v.decided_day, "%Y-%m-%d") {
                    held_ages.push((today - d).num_days());
                }
            }
        }
        if let Some(w) = r.label_weight {
            let bucket = if w >= 0.9 { "0.9-1.0" } else if w >= 0.5 { "0.5-0.9" } else { "below 0.5" };
            *weights.entry(bucket).or_default() += 1;
        }
    }
    held_ages.sort();
    let median_held_days = if held_ages.is_empty() { Value::Null } else { json!(held_ages[held_ages.len() / 2]) };
    let decided = receipts.iter().filter(|r| matches!(r.status.as_str(), "accepted" | "held" | "rejected" | "released")).count();
    let rate = |n: usize| if decided == 0 { 0.0 } else { n as f64 / decided as f64 };
    let mut reviewer: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_actor: BTreeMap<String, usize> = BTreeMap::new();
    for e in &audit {
        let by = e["by"].as_str().unwrap_or("?").to_string();
        *by_actor.entry(by.clone()).or_default() += 1;
        if by == "reviewer" {
            *reviewer.entry(e["status"].as_str().unwrap_or("?").to_string()).or_default() += 1;
        }
    }
    let released: usize = receipts.iter().filter(|r| r.status == "released").count();
    Ok(json!({
        "day": today.format("%Y-%m-%d").to_string(),
        "window_days": days,
        "totals": { "receipts": receipts.len(), "by_status": by_status, "withdrawn_hashes": withdrawn.len(), "released": released },
        "window": { "submissions": in_window, "per_day": per_day, "audit_lines_by_actor": by_actor, "reviewer_decisions": reviewer },
        "rates": { "held": rate(by_status.get("held").copied().unwrap_or(0)), "rejected": rate(by_status.get("rejected").copied().unwrap_or(0)) },
        "flags": flags,
        "contributors": { "pseudonyms": pseudonyms.len(), "unlinked_bundles": unlinked, "unlinked_share": if receipts.is_empty() { 0.0 } else { unlinked as f64 / receipts.len() as f64 } },
        "judge": { "judged": judged },
        "held": { "count": by_status.get("held").copied().unwrap_or(0), "median_days_waiting": median_held_days },
        "label_weights": weights,
    }))
}

pub async fn run(store: &dyn Store, days: u32, save: bool) -> Result<Value> {
    let v = compute(store, days).await?;
    if save {
        store.put_stats(v["day"].as_str().unwrap_or("undated"), &serde_json::to_vec_pretty(&v)?).await?;
    }
    Ok(v)
}
