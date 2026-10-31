//! Where bundles and receipts live. One trait, two homes: the quarantine bucket on Cloud Storage
//! (`GcsStore`), and memory for development and tests (`MemoryStore`).
//!
//! Layout in the bucket:
//!
//! - `bundles/<yyyy-mm>/<receipt>.json`: the bundle as received, with its status, hash,
//!   pseudonym and day in the object's metadata (the screener and the release job read these).
//! - `receipts/<receipt>.json`: what `GET /v1/contributions/{id}` answers from.
//! - `hashes/<sha256>`: one empty object per stored bundle, created only if new, which is how a
//!   duplicate is caught without a database.
//! - `withdrawn/<sha256>`: the exclusion list a release carries.
//! - `pseudonyms/<pseudonym>.json`: the per-pseudonym ledger of days and counts, for quotas.
//! - `clusters/<fingerprint prefix>/<day>/<receipt>`: unlinked bundles that look alike, for
//!   their quota.

use crate::gcs::{Gcs, Upload};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

/// What is known about a contribution, kept beside the bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub receipt_id: String,
    pub sha256: String,
    /// The bundle's object name.
    pub object: String,
    #[serde(default)]
    pub pseudonym: Option<String>,
    /// `YYYY-MM-DD`.
    pub day: String,
    /// `quarantined`, `accepted`, `held`, `rejected`, `released`, `withdrawn`.
    pub status: String,
    #[serde(default)]
    pub released_in: Vec<String>,
    pub fingerprint: String,
    /// A MinHash signature of the conversation's text (see `screen::signature`), for
    /// near-duplicate checks against other bundles without reading them.
    #[serde(default)]
    pub signature: Vec<u64>,
    /// The screener's last verdict, if it has run.
    #[serde(default)]
    pub verdict: Option<Verdict>,
    /// What a release tags the bundle with (`sandbox_only`, `injection_resisted`, ...).
    #[serde(default)]
    pub tags: Vec<String>,
    /// How far the contributor's self-reported labels can be trusted, 0 to 1 (see `release`).
    #[serde(default)]
    pub label_weight: Option<f32>,
}

/// What the screener, a reviewer or the release job decided about a bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// `accepted`, `held` or `rejected`.
    pub status: String,
    /// What was found, as short codes (`near_duplicate`, `obfuscation`, `injection_complied`,
    /// `destructive_command`, `label_disagreement`, `flood`, `pii`, `language`, `thin`,
    /// `judge_harm`, ...).
    #[serde(default)]
    pub flags: Vec<String>,
    /// Scores the judge or the checks produced, by name (`judge_outcome` is 1 for succeeded,
    /// 0.5 for partly, 0 for failed).
    #[serde(default)]
    pub scores: BTreeMap<String, f32>,
    /// One line per finding, for the reviewer.
    #[serde(default)]
    pub notes: Vec<String>,
    /// The checks' version that produced this verdict; a release re-screens when it moved on.
    pub checks_version: u32,
    /// `screener`, `judge`, `reviewer:<name>` or `release`.
    pub by: String,
    pub decided_day: String,
}

/// A pseudonym's history, for its quotas and its label trust.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    pub created_day: String,
    /// Day → bundles that day.
    #[serde(default)]
    pub days: BTreeMap<String, u32>,
    /// How many bundles the judge or the checks could compare with the self-reported outcome,
    /// and how many disagreed (see `screen`).
    #[serde(default)]
    pub judged: u32,
    #[serde(default)]
    pub disagreements: u32,
}

impl Ledger {
    /// 1.0 for a contributor whose labels always agreed, down to 0.0; 1.0 while unknown.
    pub fn label_weight(&self) -> f32 {
        if self.judged < 3 {
            return 1.0;
        }
        1.0 - self.disagreements as f32 / self.judged as f32
    }
}

#[async_trait]
pub trait Store: Send + Sync {
    /// Claim a content hash; false if a bundle with it is already stored.
    async fn claim_hash(&self, sha256: &str) -> Result<bool>;
    async fn release_hash(&self, sha256: &str) -> Result<()>;
    async fn put_bundle(&self, receipt: &Receipt, bytes: &[u8]) -> Result<()>;
    async fn delete_bundle(&self, receipt: &Receipt) -> Result<()>;
    async fn put_receipt(&self, receipt: &Receipt) -> Result<()>;
    async fn get_receipt(&self, id: &str) -> Result<Option<Receipt>>;
    async fn ledger(&self, pseudonym: &str) -> Result<Option<Ledger>>;
    async fn put_ledger(&self, pseudonym: &str, ledger: &Ledger) -> Result<()>;
    async fn count_cluster(&self, cluster: &str, day: &str) -> Result<u32>;
    async fn note_cluster(&self, cluster: &str, day: &str, receipt_id: &str) -> Result<()>;
    async fn note_withdrawn(&self, sha256: &str) -> Result<()>;
    /// Every receipt (the jobs work over all of them; a few thousand objects at most for now).
    async fn list_receipts(&self) -> Result<Vec<Receipt>>;
    async fn get_bundle(&self, receipt: &Receipt) -> Result<Option<Vec<u8>>>;
    /// Save a receipt whose status changed, and mirror the status onto the bundle object.
    async fn update_receipt(&self, receipt: &Receipt) -> Result<()>;
    /// The hashes of withdrawn bundles: the exclusion list a release carries.
    async fn list_withdrawn(&self) -> Result<Vec<String>>;
    /// One append-only line of the decision log (`audit/<day>/<receipt>-<n>.json`).
    async fn put_audit(&self, entry: &serde_json::Value) -> Result<()>;
    /// A file of a release (`releases/<version>/...`).
    async fn put_release_file(&self, path: &str, bytes: &[u8]) -> Result<()>;
}

// -- memory -------------------------------------------------------------------------------------

#[derive(Default)]
pub struct MemoryStore {
    pub hashes: Mutex<HashSet<String>>,
    pub bundles: Mutex<HashMap<String, Vec<u8>>>,
    pub receipts: Mutex<HashMap<String, Receipt>>,
    pub ledgers: Mutex<HashMap<String, Ledger>>,
    pub clusters: Mutex<HashMap<(String, String), Vec<String>>>,
    pub withdrawn: Mutex<Vec<String>>,
    pub audit: Mutex<Vec<serde_json::Value>>,
    pub releases: Mutex<BTreeMap<String, Vec<u8>>>,
}

#[async_trait]
impl Store for MemoryStore {
    async fn claim_hash(&self, sha256: &str) -> Result<bool> {
        Ok(self.hashes.lock().unwrap().insert(sha256.to_string()))
    }
    async fn release_hash(&self, sha256: &str) -> Result<()> {
        self.hashes.lock().unwrap().remove(sha256);
        Ok(())
    }
    async fn put_bundle(&self, receipt: &Receipt, bytes: &[u8]) -> Result<()> {
        self.bundles.lock().unwrap().insert(receipt.object.clone(), bytes.to_vec());
        Ok(())
    }
    async fn delete_bundle(&self, receipt: &Receipt) -> Result<()> {
        self.bundles.lock().unwrap().remove(&receipt.object);
        Ok(())
    }
    async fn put_receipt(&self, receipt: &Receipt) -> Result<()> {
        self.receipts.lock().unwrap().insert(receipt.receipt_id.clone(), receipt.clone());
        Ok(())
    }
    async fn get_receipt(&self, id: &str) -> Result<Option<Receipt>> {
        Ok(self.receipts.lock().unwrap().get(id).cloned())
    }
    async fn ledger(&self, pseudonym: &str) -> Result<Option<Ledger>> {
        Ok(self.ledgers.lock().unwrap().get(pseudonym).cloned())
    }
    async fn put_ledger(&self, pseudonym: &str, ledger: &Ledger) -> Result<()> {
        self.ledgers.lock().unwrap().insert(pseudonym.to_string(), ledger.clone());
        Ok(())
    }
    async fn count_cluster(&self, cluster: &str, day: &str) -> Result<u32> {
        Ok(self.clusters.lock().unwrap().get(&(cluster.to_string(), day.to_string())).map_or(0, |v| v.len() as u32))
    }
    async fn note_cluster(&self, cluster: &str, day: &str, receipt_id: &str) -> Result<()> {
        self.clusters.lock().unwrap().entry((cluster.to_string(), day.to_string())).or_default().push(receipt_id.to_string());
        Ok(())
    }
    async fn note_withdrawn(&self, sha256: &str) -> Result<()> {
        self.withdrawn.lock().unwrap().push(sha256.to_string());
        Ok(())
    }
    async fn list_receipts(&self) -> Result<Vec<Receipt>> {
        let mut out: Vec<Receipt> = self.receipts.lock().unwrap().values().cloned().collect();
        out.sort_by(|a, b| (&a.day, &a.receipt_id).cmp(&(&b.day, &b.receipt_id)));
        Ok(out)
    }
    async fn get_bundle(&self, receipt: &Receipt) -> Result<Option<Vec<u8>>> {
        Ok(self.bundles.lock().unwrap().get(&receipt.object).cloned())
    }
    async fn update_receipt(&self, receipt: &Receipt) -> Result<()> {
        self.put_receipt(receipt).await
    }
    async fn list_withdrawn(&self) -> Result<Vec<String>> {
        Ok(self.withdrawn.lock().unwrap().clone())
    }
    async fn put_audit(&self, entry: &serde_json::Value) -> Result<()> {
        self.audit.lock().unwrap().push(entry.clone());
        Ok(())
    }
    async fn put_release_file(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.releases.lock().unwrap().insert(path.to_string(), bytes.to_vec());
        Ok(())
    }
}

// -- Cloud Storage ------------------------------------------------------------------------------

pub struct GcsStore {
    gcs: Gcs,
}

impl GcsStore {
    pub fn new(gcs: Gcs) -> Self {
        GcsStore { gcs }
    }
}

const JSON: &str = "application/json";

#[async_trait]
impl Store for GcsStore {
    async fn claim_hash(&self, sha256: &str) -> Result<bool> {
        Ok(self.gcs.upload(&format!("hashes/{sha256}"), b"", "text/plain", &BTreeMap::new(), true).await? == Upload::Created)
    }
    async fn release_hash(&self, sha256: &str) -> Result<()> {
        self.gcs.delete(&format!("hashes/{sha256}")).await?;
        Ok(())
    }
    async fn put_bundle(&self, receipt: &Receipt, bytes: &[u8]) -> Result<()> {
        let mut meta = BTreeMap::new();
        meta.insert("status".into(), receipt.status.clone());
        meta.insert("sha256".into(), receipt.sha256.clone());
        meta.insert("day".into(), receipt.day.clone());
        meta.insert("fingerprint".into(), receipt.fingerprint.clone());
        if let Some(p) = &receipt.pseudonym {
            meta.insert("pseudonym".into(), p.clone());
        }
        self.gcs.upload(&receipt.object, bytes, JSON, &meta, true).await?;
        Ok(())
    }
    async fn delete_bundle(&self, receipt: &Receipt) -> Result<()> {
        self.gcs.delete(&receipt.object).await?;
        Ok(())
    }
    async fn put_receipt(&self, receipt: &Receipt) -> Result<()> {
        self.gcs.upload(&format!("receipts/{}.json", receipt.receipt_id), &serde_json::to_vec(receipt)?, JSON, &BTreeMap::new(), false).await?;
        Ok(())
    }
    async fn get_receipt(&self, id: &str) -> Result<Option<Receipt>> {
        match self.gcs.get(&format!("receipts/{id}.json")).await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }
    async fn ledger(&self, pseudonym: &str) -> Result<Option<Ledger>> {
        match self.gcs.get(&format!("pseudonyms/{pseudonym}.json")).await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }
    async fn put_ledger(&self, pseudonym: &str, ledger: &Ledger) -> Result<()> {
        self.gcs.upload(&format!("pseudonyms/{pseudonym}.json"), &serde_json::to_vec(ledger)?, JSON, &BTreeMap::new(), false).await?;
        Ok(())
    }
    async fn count_cluster(&self, cluster: &str, day: &str) -> Result<u32> {
        Ok(self.gcs.list(&format!("clusters/{cluster}/{day}/")).await?.len() as u32)
    }
    async fn note_cluster(&self, cluster: &str, day: &str, receipt_id: &str) -> Result<()> {
        self.gcs.upload(&format!("clusters/{cluster}/{day}/{receipt_id}"), b"", "text/plain", &BTreeMap::new(), false).await?;
        Ok(())
    }
    async fn note_withdrawn(&self, sha256: &str) -> Result<()> {
        self.gcs.upload(&format!("withdrawn/{sha256}"), b"", "text/plain", &BTreeMap::new(), false).await?;
        Ok(())
    }
    async fn list_receipts(&self) -> Result<Vec<Receipt>> {
        let mut out = Vec::new();
        for name in self.gcs.list("receipts/").await? {
            if let Some(bytes) = self.gcs.get(&name).await? {
                match serde_json::from_slice::<Receipt>(&bytes) {
                    Ok(r) => out.push(r),
                    Err(e) => tracing::warn!(object = %name, error = %e, "unreadable receipt, skipped"),
                }
            }
        }
        out.sort_by(|a, b| (&a.day, &a.receipt_id).cmp(&(&b.day, &b.receipt_id)));
        Ok(out)
    }
    async fn get_bundle(&self, receipt: &Receipt) -> Result<Option<Vec<u8>>> {
        self.gcs.get(&receipt.object).await
    }
    async fn update_receipt(&self, receipt: &Receipt) -> Result<()> {
        self.put_receipt(receipt).await?;
        let mut meta = BTreeMap::new();
        meta.insert("status".into(), receipt.status.clone());
        if let Err(e) = self.gcs.patch_metadata(&receipt.object, &meta).await {
            // A withdrawn bundle has no object any more; that is fine.
            tracing::debug!(object = %receipt.object, error = %e, "could not mirror the status onto the object");
        }
        Ok(())
    }
    async fn list_withdrawn(&self) -> Result<Vec<String>> {
        Ok(self.gcs.list("withdrawn/").await?.into_iter().filter_map(|n| n.strip_prefix("withdrawn/").map(String::from)).collect())
    }
    async fn put_audit(&self, entry: &serde_json::Value) -> Result<()> {
        let day = entry["day"].as_str().unwrap_or("undated");
        let receipt = entry["receipt_id"].as_str().unwrap_or("none");
        let n = chrono::Utc::now().timestamp_millis();
        self.gcs.upload(&format!("audit/{day}/{receipt}-{n}.json"), &serde_json::to_vec(entry)?, JSON, &BTreeMap::new(), true).await?;
        Ok(())
    }
    async fn put_release_file(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let content_type = if path.ends_with(".json") { JSON } else { "application/octet-stream" };
        self.gcs.upload(&format!("releases/{path}"), bytes, content_type, &BTreeMap::new(), false).await?;
        Ok(())
    }
}
