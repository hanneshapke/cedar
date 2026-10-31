//! The release job (`docs/design/contrib-ingest-cloud-run.md`, §6): where a coordinated set
//! is seen, and the only place bundles leave quarantine. Run as `cedar-contrib release`,
//! monthly by Cloud Scheduler or by hand.
//!
//! 1. **Aging.** Only accepted bundles at least `aging_days` old are eligible, and any judged by
//!    an older `CHECKS_VERSION` is screened again first.
//! 2. **Caps.** No pseudonym over `cap_share` of the release, unlinked bundles together no more
//!    than `unlinked_share`; the rest wait for a later release.
//! 3. **Clusters.** Bundles whose fingerprints are within `cluster_hamming` bits of each other
//!    form a cluster; one of `min_cluster` or more that is all one pseudonym, or all unlinked,
//!    is held as a set for a reviewer.
//! 4. **Label weights.** Each pseudonym's agreement rate becomes the bundle's `label_weight`;
//!    below `weight_floor`, the self-reported outcome is replaced by the judge's, where the judge
//!    read one, and the bundle tagged `outcome_from_judge`.
//! 5. **Manifest and files.** One `bundles.jsonl` per license and source model, `manifest.json`
//!    with every entry and the exclusion list of withdrawn hashes, and `manifest.sig` when a
//!    release key is set (HMAC-SHA256; KMS can replace it without changing the format).

use crate::judge::Judge;
use crate::screen::{self, CHECKS_VERSION};
use crate::store::{Receipt, Store, Verdict};
use anyhow::{Context, Result};
use cedar_contrib_schema::{Bundle, OutcomeResult, CANARY};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Options {
    /// `v2026.11`, by default this month's.
    pub version: String,
    pub aging_days: u32,
    pub cap_share: f32,
    pub unlinked_share: f32,
    pub cluster_hamming: u32,
    pub min_cluster: usize,
    pub weight_floor: f32,
    /// Also write the release's files here, for mirroring.
    pub out_dir: Option<PathBuf>,
    /// Compute and report, change nothing.
    pub dry_run: bool,
    pub release_key: Option<Vec<u8>>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            version: format!("v{}", Utc::now().format("%Y.%m")),
            aging_days: 14,
            cap_share: 0.01,
            unlinked_share: 0.2,
            cluster_hamming: 6,
            min_cluster: 5,
            weight_floor: 0.5,
            out_dir: None,
            dry_run: false,
            release_key: None,
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub version: String,
    pub eligible: usize,
    pub rescreened: usize,
    pub released: usize,
    pub deferred_over_cap: usize,
    pub held_in_clusters: usize,
    pub excluded_hashes: usize,
    pub outcomes_from_judge: usize,
    pub files: Vec<String>,
    pub dry_run: bool,
}

/// What the manifest says about one bundle.
#[derive(Debug, Clone, Serialize)]
struct Entry {
    receipt_id: String,
    sha256: String,
    license: String,
    model: String,
    language: String,
    day: String,
    label_weight: f32,
    tags: Vec<String>,
    checks_version: u32,
    file: String,
}

fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// A stable pseudo-random order for sampling over a cap: by the hash of the version and the
/// bundle's hash, so a rerun of the same release picks the same bundles.
fn sample_key(version: &str, sha256: &str) -> u64 {
    use sha2::Digest;
    let h = Sha256::digest(format!("{version}|{sha256}").as_bytes());
    u64::from_le_bytes(h[..8].try_into().unwrap())
}

fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c.to_ascii_lowercase() } else { '_' }).collect()
}

/// Group receipts whose fingerprints are within `max` bits: a greedy single-link clustering,
/// fine for the few thousand bundles a release sees.
pub fn clusters(items: &[(String, u64)], max: u32) -> Vec<Vec<String>> {
    let mut parent: Vec<usize> = (0..items.len()).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        if p[i] != i {
            let r = find(p, p[i]);
            p[i] = r;
        }
        p[i]
    }
    for (i, (_, fi)) in items.iter().enumerate() {
        for (j, (_, fj)) in items.iter().enumerate().skip(i + 1) {
            if hamming(*fi, *fj) <= max {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, (id, _)) in items.iter().enumerate() {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(id.clone());
    }
    groups.into_values().collect()
}

pub async fn run(store: &dyn Store, judge: Option<&dyn Judge>, opts: &Options) -> Result<Summary> {
    let today = Utc::now();
    let cutoff = (today - Duration::days(opts.aging_days as i64)).format("%Y-%m-%d").to_string();
    let mut summary = Summary { version: opts.version.clone(), dry_run: opts.dry_run, ..Default::default() };
    let all = store.list_receipts().await?;
    let withdrawn: HashSet<String> = store.list_withdrawn().await?.into_iter().collect();
    summary.excluded_hashes = withdrawn.len();

    // 1. Eligible, re-screened where the checks moved on.
    let mut eligible: Vec<(Receipt, Bundle)> = Vec::new();
    for r in all.iter().filter(|r| r.status == "accepted" && r.day <= cutoff && !withdrawn.contains(&r.sha256)) {
        let Some(bytes) = store.get_bundle(r).await? else { continue };
        let Ok(bundle) = serde_json::from_slice::<Bundle>(&bytes) else { continue };
        let mut r = r.clone();
        if r.verdict.as_ref().is_none_or(|v| v.checks_version < CHECKS_VERSION) {
            if opts.dry_run {
                continue;
            }
            summary.rescreened += 1;
            let v = screen::screen_one(store, judge, &mut r, &bundle, &all, "release").await?;
            if v.status != "accepted" {
                continue;
            }
        }
        eligible.push((r, bundle));
    }
    summary.eligible = eligible.len();
    let n = eligible.len();
    if n == 0 {
        return Ok(summary);
    }

    // 3. Clusters, before caps: a cluster is a set however it is spread over pseudonyms.
    let items: Vec<(String, u64)> = eligible.iter().map(|(r, _)| (r.receipt_id.clone(), u64::from_str_radix(&r.fingerprint, 16).unwrap_or(0))).collect();
    let by_id: HashMap<&str, &Receipt> = eligible.iter().map(|(r, _)| (r.receipt_id.as_str(), r)).collect();
    let mut held: HashSet<String> = HashSet::new();
    for group in clusters(&items, opts.cluster_hamming) {
        if group.len() < opts.min_cluster {
            continue;
        }
        let pseudonyms: HashSet<Option<&str>> = group.iter().map(|id| by_id[id.as_str()].pseudonym.as_deref()).collect();
        let suspicious = pseudonyms.len() == 1; // all one pseudonym, or all unlinked
        if !suspicious {
            continue;
        }
        let why = if pseudonyms.contains(&None) { "all unlinked" } else { "all one pseudonym" };
        for id in &group {
            held.insert(id.clone());
            if opts.dry_run {
                continue;
            }
            let mut r = by_id[id.as_str()].clone();
            r.status = "held".into();
            let mut v = r.verdict.clone().unwrap_or(Verdict { status: "held".into(), flags: vec![], scores: BTreeMap::new(), notes: vec![], checks_version: CHECKS_VERSION, by: "release".into(), decided_day: String::new() });
            v.status = "held".into();
            v.flags.push("cluster".into());
            v.notes.push(format!("cluster of {} within {} bits, {why} ({})", group.len(), opts.cluster_hamming, opts.version));
            v.by = "release".into();
            v.decided_day = today.format("%Y-%m-%d").to_string();
            r.verdict = Some(v);
            store.update_receipt(&r).await?;
            store
                .put_audit(&json!({ "day": today.format("%Y-%m-%d").to_string(), "receipt_id": r.receipt_id, "sha256": r.sha256, "by": "release", "status": "held", "flags": ["cluster"], "notes": [format!("cluster of {} ({why})", group.len())], "release": opts.version }))
                .await?;
        }
    }
    summary.held_in_clusters = held.len();
    let mut eligible: Vec<(Receipt, Bundle)> = eligible.into_iter().filter(|(r, _)| !held.contains(&r.receipt_id)).collect();

    // 2. Caps: per pseudonym, and for the unlinked together.
    let n = eligible.len();
    let cap = ((n as f32 * opts.cap_share).floor() as usize).max(1);
    let unlinked_cap = ((n as f32 * opts.unlinked_share).floor() as usize).max(1);
    eligible.sort_by_key(|(r, _)| sample_key(&opts.version, &r.sha256));
    let mut per: HashMap<Option<String>, usize> = HashMap::new();
    let mut chosen: Vec<(Receipt, Bundle)> = Vec::new();
    for (r, b) in eligible {
        let key = r.pseudonym.clone();
        let limit = if key.is_none() { unlinked_cap } else { cap };
        let count = per.entry(key).or_default();
        if *count >= limit {
            summary.deferred_over_cap += 1;
            continue;
        }
        *count += 1;
        chosen.push((r, b));
    }

    // 4. Label weights, and the judge's outcome where the contributor's can't be trusted.
    let mut ledgers: HashMap<String, f32> = HashMap::new();
    for (r, _) in &chosen {
        if let Some(p) = &r.pseudonym {
            if !ledgers.contains_key(p) {
                let w = store.ledger(p).await?.map(|l| l.label_weight()).unwrap_or(1.0);
                ledgers.insert(p.clone(), w);
            }
        }
    }
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut entries: Vec<Entry> = Vec::new();
    for (r, b) in chosen.iter_mut() {
        let weight = r.pseudonym.as_ref().and_then(|p| ledgers.get(p)).copied().unwrap_or(1.0);
        r.label_weight = Some(weight);
        if weight < opts.weight_floor {
            if let Some(code) = r.verdict.as_ref().and_then(|v| v.scores.get("judge_outcome")).copied() {
                if let Some(o) = &mut b.outcome {
                    o.result = if code >= 0.75 { OutcomeResult::Succeeded } else if code >= 0.25 { OutcomeResult::Partly } else { OutcomeResult::Failed };
                    o.notes = None;
                    if !r.tags.iter().any(|t| t == "outcome_from_judge") {
                        r.tags.push("outcome_from_judge".into());
                    }
                    summary.outcomes_from_judge += 1;
                }
            }
        }
        let file = format!("{}/{}/{}/bundles.jsonl", opts.version, b.license.id(), slug(&b.model.name));
        let line = json!({ "receipt_id": r.receipt_id, "sha256": r.sha256, "tags": r.tags, "label_weight": weight, "bundle": b });
        let buf = files.entry(file.clone()).or_default();
        buf.extend_from_slice(line.to_string().as_bytes());
        buf.push(b'\n');
        entries.push(Entry {
            receipt_id: r.receipt_id.clone(),
            sha256: r.sha256.clone(),
            license: b.license.id().to_string(),
            model: b.model.name.clone(),
            language: b.language.clone(),
            day: r.day.clone(),
            label_weight: weight,
            tags: r.tags.clone(),
            checks_version: r.verdict.as_ref().map_or(0, |v| v.checks_version),
            file,
        });
    }
    summary.released = entries.len();

    // 5. The manifest, its signature, and the exclusion list.
    let mut excluded: Vec<String> = withdrawn.into_iter().collect();
    excluded.sort();
    let manifest = json!({
        "version": opts.version,
        "created_at": today.to_rfc3339(),
        "canary": CANARY,
        "checks_version": CHECKS_VERSION,
        "caps": { "per_pseudonym_share": opts.cap_share, "unlinked_share": opts.unlinked_share, "aging_days": opts.aging_days },
        "counts": { "released": entries.len(), "deferred_over_cap": summary.deferred_over_cap, "held_in_clusters": summary.held_in_clusters },
        "entries": entries,
        "excluded_sha256": excluded,
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    files.insert(format!("{}/manifest.json", opts.version), manifest_bytes.clone());
    if let Some(key) = &opts.release_key {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
        mac.update(&manifest_bytes);
        let sig = json!({ "alg": "hmac-sha256", "file": "manifest.json", "value": hex::encode(mac.finalize().into_bytes()) });
        files.insert(format!("{}/manifest.sig", opts.version), serde_json::to_vec_pretty(&sig)?);
    }
    summary.files = files.keys().cloned().collect();
    if opts.dry_run {
        return Ok(summary);
    }
    for (path, bytes) in &files {
        store.put_release_file(path, bytes).await?;
        if let Some(dir) = &opts.out_dir {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).with_context(|| format!("creating {}", full.display()))?;
            std::fs::write(&full, bytes).with_context(|| format!("writing {}", full.display()))?;
        }
    }
    for (r, _) in chosen.iter_mut() {
        r.status = "released".into();
        r.released_in.push(opts.version.clone());
        store.update_receipt(r).await?;
        store
            .put_audit(&json!({ "day": today.format("%Y-%m-%d").to_string(), "receipt_id": r.receipt_id, "sha256": r.sha256, "by": "release", "status": "released", "release": opts.version, "label_weight": r.label_weight, "tags": r.tags }))
            .await?;
    }
    tracing::info!(version = %opts.version, released = summary.released, deferred = summary.deferred_over_cap, held = summary.held_in_clusters, "release written");
    Ok(summary)
}

/// Check a manifest's signature with the release key.
pub fn verify(manifest_bytes: &[u8], sig: &Value, key: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    let Some(value) = sig["value"].as_str() else { return false };
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
    mac.update(manifest_bytes);
    let expected = hex::encode(mac.finalize().into_bytes());
    expected.as_bytes().ct_eq(value.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clusters_join_nearby_fingerprints() {
        let items = vec![("a".to_string(), 0b0000u64), ("b".to_string(), 0b0011), ("c".to_string(), 0b1111_0000_0000), ("d".to_string(), 0b0001)];
        let mut got = clusters(&items, 2);
        got.iter_mut().for_each(|g| g.sort());
        got.sort();
        assert_eq!(got, vec![vec!["a".to_string(), "b".into(), "d".into()], vec!["c".to_string()]]);
    }

    #[test]
    fn sampling_is_stable_per_version() {
        assert_eq!(sample_key("v1", "abc"), sample_key("v1", "abc"));
        assert_ne!(sample_key("v1", "abc"), sample_key("v2", "abc"));
    }
}
