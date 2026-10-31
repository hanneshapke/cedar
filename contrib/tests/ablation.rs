//! The ablation job over an in-memory quarantine, with a hook that scores by set name.

use cedar_contrib::ablation::{self, AblationOptions};
use cedar_contrib::redteam::clean;
use cedar_contrib::release::Options;
use cedar_contrib::screen::{signature, CHECKS_VERSION};
use cedar_contrib::stats;
use cedar_contrib::store::{MemoryStore, Receipt, Store, Verdict};
use cedar_contrib_schema::Bundle;
use serde_json::json;
use std::collections::BTreeMap;

fn accepted(id: &str, bundle: &Bundle, pseudonym: Option<&str>) -> Receipt {
    Receipt {
        receipt_id: id.into(),
        sha256: bundle.content_sha256(),
        object: format!("bundles/2026-09/{id}.json"),
        pseudonym: pseudonym.map(String::from),
        day: "2026-09-01".into(),
        status: "accepted".into(),
        released_in: vec![],
        fingerprint: bundle.fingerprint(),
        signature: signature(bundle),
        verdict: Some(Verdict { status: "accepted".into(), flags: vec![], scores: BTreeMap::new(), notes: vec![], checks_version: CHECKS_VERSION, by: "screener".into(), decided_day: "2026-09-01".into() }),
        tags: vec![],
        label_weight: None,
    }
}

fn distinct(i: usize) -> Bundle {
    let topics = ["rivers", "bread", "chess", "trains", "gardens", "lanterns", "comets", "harbours"];
    let mut b = clean();
    let t = topics[i % topics.len()];
    b.turns[0].prompt = format!("write {} short paragraphs about {t}, number {i}, each with a different fact and a date, then save them to {t}_{i}.md", 2 + i % 3);
    b.turns[0].messages[0].content[0]["text"] = json!(b.turns[0].prompt.clone());
    b.turns[0].messages[3].content[0]["text"] = json!(format!("Done: {} paragraphs about {t} are in {t}_{i}.md.", 2 + i % 3));
    b
}

async fn seed() -> MemoryStore {
    let store = MemoryStore::default();
    for i in 0..8 {
        let b = distinct(i);
        let r = accepted(&format!("d{i}"), &b, Some(&format!("p_person{i:08}")));
        store.put_bundle(&r, &serde_json::to_vec(&b).unwrap()).await.unwrap();
        store.put_receipt(&r).await.unwrap();
    }
    // A cluster of five from five people: not suspicious to the release, but an ablation set.
    for i in 0..5 {
        let mut b = clean();
        b.turns[0].messages[3].content[0]["text"] = json!(format!("Two files, a.txt is larger ({} KB).", i));
        let r = accepted(&format!("c{i}"), &b, Some(&format!("p_cluster{i:07}")));
        store.put_bundle(&r, &serde_json::to_vec(&b).unwrap()).await.unwrap();
        store.put_receipt(&r).await.unwrap();
    }
    store
}

fn opts(dir: &std::path::Path, hook: Option<&str>) -> AblationOptions {
    AblationOptions {
        release: Options { version: "v2026.10".into(), dry_run: true, ..Default::default() },
        out_dir: dir.to_path_buf(),
        hook: hook.map(String::from),
        delta: 0.02,
        apply: true,
    }
}

/// A hook that writes a safety score by set name.
fn hook(scores: &str) -> String {
    format!("case \"$ABLATION_NAME\" in {scores} esac; echo \"{{\\\"safety\\\": $s}}\" > \"$ABLATION_RESULT\"")
}

#[tokio::test]
async fn without_a_hook_the_sets_are_written_and_nothing_decided() {
    let store = seed().await;
    let dir = std::env::temp_dir().join(format!("cedar-ablation-{}", uuid()));
    let report = ablation::run(&store, None, &opts(&dir, None)).await.unwrap();
    assert_eq!(report.state, "pending");
    assert_eq!(report.sets.len(), 2, "{:?}", report.sets);
    assert_eq!(report.sets[0].bundles, 13);
    assert_eq!(report.sets[1].bundles, 8);
    assert!(dir.join("sets/full.jsonl").exists());
    assert!(dir.join("sets/minus-1.jsonl").exists());
    assert!(store.releases.lock().unwrap().contains_key("v2026.10/ablation.json"));
    assert_eq!(store.get_receipt("c0").await.unwrap().unwrap().status, "accepted");
}

#[tokio::test]
async fn a_batch_that_lowers_safety_is_held_whole() {
    let store = seed().await;
    let dir = std::env::temp_dir().join(format!("cedar-ablation-{}", uuid()));
    let h = hook("baseline) s=0.90;; full) s=0.80;; *) s=0.90;;");
    let report = ablation::run(&store, None, &opts(&dir, Some(&h))).await.unwrap();
    assert_eq!(report.state, "applied");
    assert_eq!(report.decisions.len(), 1);
    assert_eq!(report.decisions[0].action, "hold_batch");
    assert_eq!(store.get_receipt("d0").await.unwrap().unwrap().status, "held");
    assert!(store.get_receipt("d0").await.unwrap().unwrap().verdict.unwrap().flags.contains(&"ablation_batch".to_string()));
}

#[tokio::test]
async fn a_cluster_whose_removal_raises_safety_is_held_alone() {
    let store = seed().await;
    let dir = std::env::temp_dir().join(format!("cedar-ablation-{}", uuid()));
    let h = hook("baseline) s=0.90;; full) s=0.90;; minus-1) s=0.95;; *) s=0.90;;");
    let report = ablation::run(&store, None, &opts(&dir, Some(&h))).await.unwrap();
    assert_eq!(report.state, "applied");
    assert_eq!(report.decisions.len(), 1, "{:?}", report.decisions);
    assert_eq!(report.decisions[0].action, "hold_cluster");
    assert_eq!(report.decisions[0].receipts, 5);
    assert_eq!(store.get_receipt("c0").await.unwrap().unwrap().status, "held");
    assert_eq!(store.get_receipt("d0").await.unwrap().unwrap().status, "accepted");
    assert_eq!(store.audit.lock().unwrap().iter().filter(|e| e["by"] == "ablation").count(), 5);

    // The stats see the held ones and the audit lines.
    let v = stats::run(&store, 30, true).await.unwrap();
    assert_eq!(v["totals"]["receipts"], 13);
    assert_eq!(v["totals"]["by_status"]["held"], 5);
    assert_eq!(v["flags"]["ablation_cluster"], 5);
    assert_eq!(v["window"]["audit_lines_by_actor"]["ablation"], 5);
    assert_eq!(v["contributors"]["pseudonyms"], 13);
    assert_eq!(v["held"]["median_days_waiting"], 0);
    assert_eq!(store.stats.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_hook_without_a_safety_number_is_an_error() {
    let store = seed().await;
    let dir = std::env::temp_dir().join(format!("cedar-ablation-{}", uuid()));
    let h = "echo '{\"oops\": 1}' > \"$ABLATION_RESULT\"";
    assert!(ablation::run(&store, None, &opts(&dir, Some(h))).await.is_err());
}

fn uuid() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}
