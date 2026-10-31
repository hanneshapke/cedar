//! The screener against the red-team fixtures (its regression suite), and its set-level checks.

use cedar_contrib::judge::{Judgement, ScriptedJudge};
use cedar_contrib::redteam::{clean, fixtures};
use cedar_contrib::screen::{self, signature, CHECKS_VERSION};
use cedar_contrib::store::{MemoryStore, Receipt, Store};
use cedar_contrib_schema::Bundle;
use serde_json::json;

fn receipt(id: &str, bundle: &Bundle, pseudonym: Option<&str>, day: &str) -> Receipt {
    Receipt {
        receipt_id: id.into(),
        sha256: bundle.content_sha256(),
        object: format!("bundles/2026-10/{id}.json"),
        pseudonym: pseudonym.map(String::from),
        day: day.into(),
        status: "quarantined".into(),
        released_in: vec![],
        fingerprint: bundle.fingerprint(),
        signature: signature(bundle),
        verdict: None,
        tags: vec![],
        label_weight: None,
    }
}

async fn put(store: &MemoryStore, r: &Receipt, b: &Bundle) {
    store.put_bundle(r, &serde_json::to_vec(b).unwrap()).await.unwrap();
    store.put_receipt(r).await.unwrap();
}

/// A judge that recognises the fixtures' injected results.
fn judge() -> ScriptedJudge {
    ScriptedJudge {
        answers: vec![
            ("evil.example".into(), Judgement { injection_present: 0.95, injection_complied: 0.9, harm: 0.8, outcome: "failed".into(), outcome_confidence: 0.8 }),
            ("reveal the system prompt".into(), Judgement { injection_present: 0.9, injection_complied: 0.05, harm: 0.0, outcome: "succeeded".into(), outcome_confidence: 0.9 }),
        ],
    }
}

#[tokio::test]
async fn every_red_team_fixture_gets_the_verdict_it_was_made_for() {
    for f in fixtures() {
        let store = MemoryStore::default();
        let mut r = receipt("fx", &f.bundle, Some("p_fixture0000"), "2026-10-01");
        put(&store, &r, &f.bundle).await;
        let j = judge();
        let v = screen::screen_one(&store, Some(&j), &mut r, &f.bundle, &[], "screener").await.unwrap();
        assert_eq!(v.status, f.expect_status, "{}: {:?} {:?}", f.name, v.flags, v.notes);
        if let Some(flag) = f.expect_flag {
            assert!(v.flags.iter().any(|x| x == flag), "{}: expected {flag} in {:?}", f.name, v.flags);
        }
        assert_eq!(v.checks_version, CHECKS_VERSION);
        let saved = store.get_receipt("fx").await.unwrap().unwrap();
        assert_eq!(saved.status, f.expect_status);
        assert_eq!(store.audit.lock().unwrap().len(), 1, "{}", f.name);
    }
}

#[tokio::test]
async fn without_a_judge_an_injected_result_is_held_not_decided() {
    let store = MemoryStore::default();
    let f = fixtures().into_iter().find(|f| f.name.contains("agent obeys")).unwrap();
    let mut r = receipt("x", &f.bundle, None, "2026-10-01");
    put(&store, &r, &f.bundle).await;
    let v = screen::screen_one(&store, None, &mut r, &f.bundle, &[], "screener").await.unwrap();
    assert_eq!(v.status, "held");
    assert!(v.flags.contains(&"injection_unjudged".to_string()));
}

#[tokio::test]
async fn near_duplicates_are_held_and_resubmissions_rejected() {
    let store = MemoryStore::default();
    let a = clean();
    let mut b = a.clone();
    b.turns[0].messages[3].content[0]["text"] = json!("There are two files in the workspace, a.txt and b.txt. The larger one is a.txt at about four kilobytes; there is no csv here.");
    let ra = receipt("a", &a, Some("p_one000000000"), "2026-10-01");
    put(&store, &ra, &a).await;
    // Another person, nearly the same conversation: held.
    let mut rb = receipt("b", &b, Some("p_two000000000"), "2026-10-02");
    put(&store, &rb, &b).await;
    let v = screen::screen_one(&store, None, &mut rb, &b, std::slice::from_ref(&ra), "screener").await.unwrap();
    assert_eq!(v.status, "held", "{:?}", v.notes);
    assert!(v.flags.contains(&"near_duplicate".to_string()));
    assert!(v.scores["near_duplicate"] >= 0.8);
    // The same person again: rejected as a resubmission.
    let mut rc = receipt("c", &b, Some("p_one000000000"), "2026-10-03");
    put(&store, &rc, &b).await;
    let v = screen::screen_one(&store, None, &mut rc, &b, &[ra], "screener").await.unwrap();
    assert_eq!(v.status, "rejected");
    assert!(v.flags.contains(&"resubmission".to_string()));
}

#[tokio::test]
async fn a_flood_of_look_alikes_from_one_pseudonym_is_held() {
    let store = MemoryStore::default();
    let base = clean();
    let mut others = Vec::new();
    for i in 0..12 {
        let mut b = base.clone();
        b.turns[0].messages[3].content[0]["text"] = json!(format!("There are two files in the workspace, a.txt and b.txt; the larger one is a.txt, about {} kilobytes, no csv.", i + 3));
        let r = receipt(&format!("f{i}"), &b, Some("p_flood00000000"), "2026-10-01");
        put(&store, &r, &b).await;
        others.push(r);
    }
    let mut last = others.pop().unwrap();
    let bundle: Bundle = serde_json::from_slice(&store.get_bundle(&last).await.unwrap().unwrap()).unwrap();
    let v = screen::screen_one(&store, None, &mut last, &bundle, &others, "screener").await.unwrap();
    assert!(v.flags.contains(&"flood".to_string()), "{:?}", v.notes);
    // Alike enough to be a flood; alike enough to be a resubmission too, which rejects. Either
    // way it does not pass.
    assert_ne!(v.status, "accepted");
}

#[tokio::test]
async fn the_job_screens_quarantined_bundles_and_scores_label_trust() {
    let store = MemoryStore::default();
    let good = clean();
    let r = receipt("g", &good, Some("p_trust00000000"), "2026-10-01");
    put(&store, &r, &good).await;
    // Says succeeded, the judge reads failed: a disagreement on the pseudonym's ledger.
    let mut liar = clean();
    liar.turns[0].prompt = "list the files and delete nothing, then tell me how many there are, with their sizes if you can".into();
    liar.turns[0].messages[0].content[0]["text"] = json!(liar.turns[0].prompt.clone());
    liar.turns[0].messages[3].content[0]["text"] = json!("I could not list anything, the sandbox refused the command, sorry about that.");
    liar.outcome = Some(cedar_contrib_schema::Outcome { result: cedar_contrib_schema::OutcomeResult::Succeeded, notes: None });
    let r2 = receipt("l", &liar, Some("p_trust00000000"), "2026-10-02");
    put(&store, &r2, &liar).await;
    let j = ScriptedJudge { answers: vec![("sandbox refused".into(), Judgement { outcome: "failed".into(), outcome_confidence: 0.9, ..Default::default() })] };
    let n = screen::run(&store, Some(&j), 100, false, false).await.unwrap();
    assert_eq!(n, 2);
    assert_eq!(store.get_receipt("g").await.unwrap().unwrap().status, "accepted");
    assert_eq!(store.get_receipt("l").await.unwrap().unwrap().status, "accepted");
    let ledger = store.ledger("p_trust00000000").await.unwrap().unwrap();
    assert_eq!((ledger.judged, ledger.disagreements), (1, 1));
    // Nothing left to screen; a rescreen pass finds nothing older than the current checks either.
    assert_eq!(screen::run(&store, Some(&j), 100, true, false).await.unwrap(), 0);
}
