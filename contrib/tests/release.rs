//! The release job over an in-memory quarantine.

use cedar_contrib::redteam::clean;
use cedar_contrib::release::{self, verify, Options};
use cedar_contrib::screen::{signature, CHECKS_VERSION};
use cedar_contrib::store::{Ledger, MemoryStore, Receipt, Store, Verdict};
use cedar_contrib_schema::Bundle;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn accepted(id: &str, bundle: &Bundle, pseudonym: Option<&str>, day: &str, judge_outcome: Option<f32>) -> Receipt {
    let mut scores = BTreeMap::new();
    if let Some(o) = judge_outcome {
        scores.insert("judge_outcome".to_string(), o);
    }
    Receipt {
        receipt_id: id.into(),
        sha256: bundle.content_sha256(),
        object: format!("bundles/2026-09/{id}.json"),
        pseudonym: pseudonym.map(String::from),
        day: day.into(),
        status: "accepted".into(),
        released_in: vec![],
        fingerprint: bundle.fingerprint(),
        signature: signature(bundle),
        verdict: Some(Verdict { status: "accepted".into(), flags: vec![], scores, notes: vec![], checks_version: CHECKS_VERSION, by: "screener".into(), decided_day: day.into() }),
        tags: vec!["sandbox_only".into()],
        label_weight: None,
    }
}

/// A distinct conversation per index, far enough apart not to cluster.
fn distinct(i: usize) -> Bundle {
    let topics = ["rivers", "bread", "chess", "trains", "gardens", "lanterns", "comets", "harbours", "violins", "deserts", "glaciers", "orchards"];
    let mut b = clean();
    let t = topics[i % topics.len()];
    b.turns[0].prompt = format!("write {} short paragraphs about {t}, number {i}, each with a different fact and a date, then save them to a file named {t}_{i}.md", 2 + i % 3);
    b.turns[0].messages[0].content[0]["text"] = json!(b.turns[0].prompt.clone());
    b.turns[0].messages[3].content[0]["text"] = json!(format!("Done: {} paragraphs about {t} are in {t}_{i}.md, each with its own fact and date as asked.", 2 + i % 3));
    b
}

async fn put(store: &MemoryStore, r: &Receipt, b: &Bundle) {
    store.put_bundle(r, &serde_json::to_vec(b).unwrap()).await.unwrap();
    store.put_receipt(r).await.unwrap();
}

#[tokio::test]
async fn a_release_ages_caps_clusters_weights_signs_and_excludes() {
    let store = MemoryStore::default();
    // 40 distinct bundles from 40 pseudonyms, old enough.
    for i in 0..40 {
        let b = distinct(i);
        put(&store, &accepted(&format!("d{i}"), &b, Some(&format!("p_person{i:08}")), "2026-09-01", None), &b).await;
    }
    // One pseudonym with 5 more: over the 1% cap (which is 1 of ~50), so 4 wait.
    for i in 0..5 {
        let b = distinct(100 + i);
        put(&store, &accepted(&format!("h{i}"), &b, Some("p_heavy00000000"), "2026-09-02", None), &b).await;
    }
    // A cluster of 6 near-identical bundles, all unlinked: held as a set.
    for i in 0..6 {
        let mut b = clean();
        b.turns[0].messages[3].content[0]["text"] = json!(format!("Two files, a.txt is larger ({} KB).", i));
        put(&store, &accepted(&format!("c{i}"), &b, None, "2026-09-03", None), &b).await;
    }
    // Too young.
    let young = distinct(200);
    put(&store, &accepted("y", &young, Some("p_young00000000"), "2099-01-01", None), &young).await;
    // Withdrawn: excluded by hash.
    let gone = distinct(201);
    let gone_r = accepted("w", &gone, Some("p_gone000000000"), "2026-09-01", None);
    put(&store, &gone_r, &gone).await;
    store.note_withdrawn(&gone_r.sha256).await.unwrap();
    // An untrusted labeller whose bundle the judge read as failed: outcome replaced.
    let mut lb = distinct(202);
    lb.outcome = Some(cedar_contrib_schema::Outcome { result: cedar_contrib_schema::OutcomeResult::Succeeded, notes: Some("great".into()) });
    put(&store, &accepted("u", &lb, Some("p_untrusted0000"), "2026-09-01", Some(0.0)), &lb).await;
    store.put_ledger("p_untrusted0000", &Ledger { created_day: "2026-08-01".into(), days: BTreeMap::new(), judged: 10, disagreements: 8 }).await.unwrap();

    let opts = Options { version: "v2026.10".into(), release_key: Some(b"release-key-for-tests".to_vec()), ..Default::default() };
    // Dry run changes nothing.
    let dry = release::run(&store, None, &Options { dry_run: true, ..opts.clone() }).await.unwrap();
    assert!(dry.dry_run);
    assert!(store.releases.lock().unwrap().is_empty());
    assert_eq!(store.get_receipt("d0").await.unwrap().unwrap().status, "accepted");

    let s = release::run(&store, None, &opts).await.unwrap();
    assert_eq!(s.held_in_clusters, 6, "{s:?}");
    assert_eq!(s.deferred_over_cap, 4, "{s:?}");
    assert_eq!(s.excluded_hashes, 1);
    assert_eq!(s.outcomes_from_judge, 1);
    assert_eq!(s.released, 40 + 1 + 1, "{s:?}");
    // Statuses.
    assert_eq!(store.get_receipt("c0").await.unwrap().unwrap().status, "held");
    assert!(store.get_receipt("c0").await.unwrap().unwrap().verdict.unwrap().flags.contains(&"cluster".to_string()));
    assert_eq!(store.get_receipt("y").await.unwrap().unwrap().status, "accepted");
    assert_eq!(store.get_receipt("w").await.unwrap().unwrap().status, "accepted");
    let d0 = store.get_receipt("d0").await.unwrap().unwrap();
    assert_eq!(d0.status, "released");
    assert_eq!(d0.released_in, vec!["v2026.10"]);
    let heavy: Vec<String> = (0..5).map(|i| format!("h{i}")).collect();
    let released_heavy = futures_count(&store, &heavy).await;
    assert_eq!(released_heavy, 1);
    let u = store.get_receipt("u").await.unwrap().unwrap();
    assert!((u.label_weight.unwrap() - 0.2).abs() < 1e-5, "{:?}", u.label_weight);
    assert!(u.tags.contains(&"outcome_from_judge".to_string()));

    // Files: the manifest, its signature, the exclusion list, one jsonl per license and model.
    let files = store.releases.lock().unwrap().clone();
    let manifest_bytes = files.get("v2026.10/manifest.json").unwrap();
    let manifest: Value = serde_json::from_slice(manifest_bytes).unwrap();
    assert_eq!(manifest["version"], "v2026.10");
    assert_eq!(manifest["entries"].as_array().unwrap().len(), 42);
    assert_eq!(manifest["excluded_sha256"], json!([gone_r.sha256]));
    assert_eq!(manifest["canary"], cedar_contrib_schema::CANARY);
    let sig: Value = serde_json::from_slice(files.get("v2026.10/manifest.sig").unwrap()).unwrap();
    assert!(verify(manifest_bytes, &sig, b"release-key-for-tests"));
    assert!(!verify(manifest_bytes, &sig, b"other-key"));
    let jsonl = files.get("v2026.10/cc0/deepseek-ai_deepseek-v4-flash/bundles.jsonl").unwrap();
    let lines: Vec<Value> = String::from_utf8(jsonl.clone()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 42);
    let uline = lines.iter().find(|l| l["receipt_id"] == "u").unwrap();
    assert_eq!(uline["bundle"]["outcome"]["result"], "failed");
    assert!((uline["label_weight"].as_f64().unwrap() - 0.2).abs() < 1e-5);
    assert!(lines.iter().all(|l| l["bundle"]["canary"] == cedar_contrib_schema::CANARY));
    // Every release decision is in the audit log.
    assert_eq!(store.audit.lock().unwrap().iter().filter(|e| e["status"] == "released").count(), 42);

    // A second release of the same version finds nothing new; the deferred ones go next time.
    let again = release::run(&store, None, &Options { version: "v2026.11".into(), ..opts }).await.unwrap();
    assert_eq!(again.released, 1, "{again:?}");
}

async fn futures_count(store: &MemoryStore, ids: &[String]) -> usize {
    let mut n = 0;
    for id in ids {
        if store.get_receipt(id).await.unwrap().unwrap().status == "released" {
            n += 1;
        }
    }
    n
}
