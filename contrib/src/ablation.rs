//! The ablation evaluation (`docs/design/contrib-ingest-cloud-run.md`, §6.5): the one check
//! that measures the thing the corpus is for. Before a release, the candidate batch is written
//! out as training sets, with and without each cluster, and a hook trains a model on each set
//! and runs cedar-bench's safety suite on it. A batch that moves safety below the baseline is
//! held whole; a cluster whose removal raises safety is held on its own.
//!
//! Training and evaluation are the hook's business (`CONTRIB_ABLATION_HOOK`, a shell command run
//! once per set with `ABLATION_NAME`, `ABLATION_SET` and `ABLATION_RESULT` in its environment),
//! so the GPU, the trainer and the bench live wherever they live; this job writes the sets,
//! reads the results (`{"safety": 0..1, ...}`) and decides. Without a hook it writes the sets and
//! reports `pending`, which is the first thing to do while the bench's safety suite settles.

use crate::judge::Judge;
use crate::release::{self, hold_group, Options, Plan};
use crate::store::{Receipt, Store};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AblationOptions {
    pub release: Options,
    /// Where the sets and results go.
    pub out_dir: PathBuf,
    /// The command to run per set; none writes the sets only.
    pub hook: Option<String>,
    /// How much safety may move before it counts.
    pub delta: f32,
    /// Hold what the results say to hold; otherwise only report.
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetResult {
    pub name: String,
    pub bundles: usize,
    pub file: String,
    /// What the hook reported, or `null` without a hook.
    pub result: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub set: String,
    /// `hold_batch`, `hold_cluster` or `keep`.
    pub action: String,
    pub reason: String,
    pub receipts: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub version: String,
    /// `pending` (no hook), `evaluated`, or `applied`.
    pub state: String,
    pub baseline: Option<Value>,
    pub sets: Vec<SetResult>,
    pub decisions: Vec<Decision>,
}

fn safety(v: &Value) -> Option<f32> {
    v["safety"].as_f64().map(|f| f as f32)
}

fn write_set(dir: &Path, name: &str, items: &[&(Receipt, cedar_contrib_schema::Bundle)]) -> Result<PathBuf> {
    let path = dir.join(format!("{name}.jsonl"));
    let mut out = Vec::new();
    for (r, b) in items {
        out.extend_from_slice(json!({ "receipt_id": r.receipt_id, "sha256": r.sha256, "tags": r.tags, "bundle": b }).to_string().as_bytes());
        out.push(b'\n');
    }
    std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Run the hook for one set; `set` is empty for the baseline (the untrained model).
fn run_hook(hook: &str, out_dir: &Path, name: &str, set: Option<&Path>) -> Result<Value> {
    let result = out_dir.join(format!("{name}.result.json"));
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(hook)
        .env("ABLATION_NAME", name)
        .env("ABLATION_SET", set.map(|p| p.to_string_lossy().to_string()).unwrap_or_default())
        .env("ABLATION_RESULT", &result)
        .status()
        .with_context(|| format!("running the ablation hook for {name}"))?;
    if !status.success() {
        bail!("the ablation hook failed for {name}: {status}");
    }
    let bytes = std::fs::read(&result).with_context(|| format!("the hook wrote no {}", result.display()))?;
    let v: Value = serde_json::from_slice(&bytes)?;
    if safety(&v).is_none() {
        bail!("the hook's result for {name} has no numeric \"safety\"");
    }
    Ok(v)
}

pub async fn run(store: &dyn Store, judge: Option<&dyn Judge>, opts: &AblationOptions) -> Result<Report> {
    let plan: Plan = release::plan(store, judge, &opts.release).await?;
    let sets_dir = opts.out_dir.join("sets");
    std::fs::create_dir_all(&sets_dir)?;
    let mut report = Report { version: opts.release.version.clone(), state: "pending".into(), ..Default::default() };
    let all: Vec<&(Receipt, cedar_contrib_schema::Bundle)> = plan.eligible.iter().collect();
    let full = write_set(&sets_dir, "full", &all)?;
    report.sets.push(SetResult { name: "full".into(), bundles: all.len(), file: full.to_string_lossy().into(), result: None });
    let mut minus: Vec<(String, Vec<String>, PathBuf)> = Vec::new();
    for (i, group) in plan.clusters.iter().enumerate() {
        let name = format!("minus-{}", i + 1);
        let items: Vec<&(Receipt, cedar_contrib_schema::Bundle)> = all.iter().copied().filter(|(r, _)| !group.contains(&r.receipt_id)).collect();
        let path = write_set(&sets_dir, &name, &items)?;
        report.sets.push(SetResult { name: name.clone(), bundles: items.len(), file: path.to_string_lossy().into(), result: None });
        minus.push((name, group.clone(), path));
    }
    std::fs::write(sets_dir.join("clusters.json"), serde_json::to_vec_pretty(&plan.clusters)?)?;

    let Some(hook) = &opts.hook else {
        tracing::info!(sets = report.sets.len(), "ablation sets written; no hook, so nothing evaluated");
        store.put_release_file(&format!("{}/ablation.json", opts.release.version), &serde_json::to_vec_pretty(&report)?).await?;
        return Ok(report);
    };
    let baseline = run_hook(hook, &opts.out_dir, "baseline", None)?;
    let full_result = run_hook(hook, &opts.out_dir, "full", Some(&full))?;
    report.sets[0].result = Some(full_result.clone());
    let base_s = safety(&baseline).unwrap();
    let full_s = safety(&full_result).unwrap();
    report.baseline = Some(baseline);
    let by_id: HashMap<&str, &Receipt> = plan.eligible.iter().map(|(r, _)| (r.receipt_id.as_str(), r)).collect();
    report.state = "evaluated".into();
    if full_s < base_s - opts.delta {
        let reason = format!("safety {full_s:.3} with the batch, {base_s:.3} without it");
        report.decisions.push(Decision { set: "full".into(), action: "hold_batch".into(), reason: reason.clone(), receipts: all.len() });
        if opts.apply {
            let receipts: Vec<&Receipt> = all.iter().map(|(r, _)| r).collect();
            hold_group(store, &receipts, "ablation_batch", &format!("{reason} ({})", opts.release.version), "ablation").await?;
            report.state = "applied".into();
        }
    } else {
        for (i, (name, group, path)) in minus.iter().enumerate() {
            let result = run_hook(hook, &opts.out_dir, name, Some(path))?;
            let s = safety(&result).unwrap();
            report.sets[i + 1].result = Some(result);
            if s > full_s + opts.delta {
                let reason = format!("safety {s:.3} without cluster {}, {full_s:.3} with it", i + 1);
                report.decisions.push(Decision { set: name.clone(), action: "hold_cluster".into(), reason: reason.clone(), receipts: group.len() });
                if opts.apply {
                    let receipts: Vec<&Receipt> = group.iter().map(|id| by_id[id.as_str()]).collect();
                    hold_group(store, &receipts, "ablation_cluster", &format!("{reason} ({})", opts.release.version), "ablation").await?;
                    report.state = "applied".into();
                }
            } else {
                report.decisions.push(Decision { set: name.clone(), action: "keep".into(), reason: format!("safety {s:.3} without cluster {}, {full_s:.3} with it", i + 1), receipts: group.len() });
            }
        }
    }
    let bytes = serde_json::to_vec_pretty(&report)?;
    std::fs::write(opts.out_dir.join("ablation.json"), &bytes)?;
    store.put_release_file(&format!("{}/ablation.json", opts.release.version), &bytes).await?;
    tracing::info!(version = %opts.release.version, state = %report.state, decisions = report.decisions.len(), "ablation done");
    Ok(report)
}
