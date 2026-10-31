//! Service configuration, from the environment.

use anyhow::{anyhow, bail, Result};
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    /// `CONTRIB_BIND_ADDR`; Cloud Run's `PORT` is honoured when set.
    pub bind_addr: SocketAddr,
    /// `CONTRIB_HMAC_KEY`: derives withdraw tokens from receipts, so none is stored. At least 32
    /// characters.
    pub hmac_key: Vec<u8>,
    /// `CONTRIB_BUCKET`: the quarantine bucket. Unset, everything is kept in memory (development).
    pub bucket: Option<String>,
    /// `CONTRIB_MODELS`: the models whose outputs may be published, comma-separated, each an
    /// exact name or a prefix ending in `*`. Empty means any.
    pub models: Vec<String>,
    /// `CONTRIB_QUOTA_DAY`, `CONTRIB_QUOTA_MONTH`: bundles per pseudonym.
    pub quota_day: u32,
    pub quota_month: u32,
    /// `CONTRIB_QUOTA_NEW`: bundles a pseudonym may send on its first day.
    pub quota_new: u32,
    /// `CONTRIB_QUOTA_CLUSTER`: unlinked bundles per day that look alike (same fingerprint prefix).
    pub quota_cluster: u32,
    /// `CONTRIB_IP_PER_MINUTE`: requests per client address per minute, counted in memory only.
    pub ip_per_minute: u32,
    /// `CONTRIB_MAX_BUNDLE_BYTES`.
    pub max_bundle_bytes: usize,
    /// `GOOGLE_OAUTH_ACCESS_TOKEN`: a token for the bucket when not on Google Cloud (development).
    pub access_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        fn num(name: &str, raw: Option<String>, default: u32) -> Result<u32> {
            match raw {
                None => Ok(default),
                Some(s) => s.parse().map_err(|e| anyhow!("invalid {name}={s:?}: {e}")),
            }
        }
        let port = get("PORT");
        let bind_addr: SocketAddr = match (get("CONTRIB_BIND_ADDR"), port) {
            (Some(a), _) => a.parse().map_err(|e| anyhow!("invalid CONTRIB_BIND_ADDR: {e}"))?,
            (None, Some(p)) => format!("0.0.0.0:{p}").parse().map_err(|e| anyhow!("invalid PORT: {e}"))?,
            (None, None) => "127.0.0.1:8090".parse().unwrap(),
        };
        let hmac_key = get("CONTRIB_HMAC_KEY").ok_or_else(|| anyhow!("CONTRIB_HMAC_KEY must be set"))?;
        if hmac_key.len() < 32 {
            bail!("CONTRIB_HMAC_KEY must be at least 32 characters");
        }
        let models: Vec<String> = get("CONTRIB_MODELS")
            .map(|m| m.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        Ok(Config {
            bind_addr,
            hmac_key: hmac_key.into_bytes(),
            bucket: get("CONTRIB_BUCKET"),
            models,
            quota_day: num("CONTRIB_QUOTA_DAY", get("CONTRIB_QUOTA_DAY"), 20)?,
            quota_month: num("CONTRIB_QUOTA_MONTH", get("CONTRIB_QUOTA_MONTH"), 200)?,
            quota_new: num("CONTRIB_QUOTA_NEW", get("CONTRIB_QUOTA_NEW"), 3)?,
            quota_cluster: num("CONTRIB_QUOTA_CLUSTER", get("CONTRIB_QUOTA_CLUSTER"), 5)?,
            ip_per_minute: num("CONTRIB_IP_PER_MINUTE", get("CONTRIB_IP_PER_MINUTE"), 30)?,
            max_bundle_bytes: num("CONTRIB_MAX_BUNDLE_BYTES", get("CONTRIB_MAX_BUNDLE_BYTES"), 8 << 20)? as usize,
            access_token: get("GOOGLE_OAUTH_ACCESS_TOKEN"),
        })
    }

    /// Whether a model's outputs may be published here (see `models`).
    pub fn model_allowed(&self, name: &str) -> bool {
        self.models.is_empty()
            || self.models.iter().any(|m| match m.strip_suffix('*') {
                Some(prefix) => name.starts_with(prefix),
                None => m == name,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> Result<Config> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Config::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn the_key_is_required_and_long_and_port_is_honoured() {
        assert!(cfg(&[]).is_err());
        assert!(cfg(&[("CONTRIB_HMAC_KEY", "short")]).is_err());
        let c = cfg(&[("CONTRIB_HMAC_KEY", "0123456789abcdef0123456789abcdef"), ("PORT", "8080"), ("CONTRIB_MODELS", "deepseek-ai/*, Qwen/Qwen3-8B")]).unwrap();
        assert_eq!(c.bind_addr.port(), 8080);
        assert!(c.model_allowed("deepseek-ai/DeepSeek-V4-Flash"));
        assert!(c.model_allowed("Qwen/Qwen3-8B"));
        assert!(!c.model_allowed("openai/gpt-5"));
        assert!(cfg(&[("CONTRIB_HMAC_KEY", "0123456789abcdef0123456789abcdef")]).unwrap().model_allowed("anything"));
    }
}
