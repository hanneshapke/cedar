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
    /// `quarantined`, `accepted`, `held`, `rejected`, `withdrawn`.
    pub status: String,
    #[serde(default)]
    pub released_in: Vec<String>,
    pub fingerprint: String,
}

/// A pseudonym's history, for its quotas.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    pub created_day: String,
    /// Day → bundles that day.
    #[serde(default)]
    pub days: BTreeMap<String, u32>,
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
}
