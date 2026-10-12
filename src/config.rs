//! Service configuration, read from the environment.

use anyhow::{anyhow, bail, Context, Result};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful coding agent. Your tools run inside an \
    isolated sandbox whose working directory is /workspace. Use tools when they help; keep answers concise.";

#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub image: String,
    /// Docker network mode for the container. `none` by default.
    pub network: String,
    pub memory: String,
    pub cpus: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub anthropic_api_key: String,
    pub anthropic_base_url: String,
    /// Bearer tokens accepted by the API.
    pub api_keys: Vec<String>,
    pub database_url: String,
    pub bind_addr: SocketAddr,

    pub default_model: String,
    /// If non-empty, requests may only use these models (the default model is always allowed).
    pub allowed_models: Vec<String>,
    pub default_system_prompt: String,
    pub max_tokens: u32,
    pub max_turns_cap: u32,
    pub web_search_max_uses: u32,

    pub max_concurrent_runs: usize,
    pub max_active_runs_per_key: i64,
    pub run_timeout: Duration,
    pub tool_timeout: Duration,
    pub tool_output_max_bytes: usize,

    pub sandbox: SandboxConfig,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Build a config from any key lookup, so tests don't have to touch the process environment.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        let required = |k: &str| get(k).ok_or_else(|| anyhow!("{k} must be set"));
        fn parse<T: FromStr>(name: &str, raw: Option<String>, default: T) -> Result<T>
        where
            T::Err: std::fmt::Display,
        {
            match raw {
                None => Ok(default),
                Some(s) => s.trim().parse().map_err(|e| anyhow!("invalid {name}={s:?}: {e}")),
            }
        }
        let list = |k: &str| -> Vec<String> {
            get(k)
                .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default()
        };

        let api_keys = list("CEDAR_API_KEYS");
        if api_keys.is_empty() {
            bail!("CEDAR_API_KEYS must contain at least one comma-separated API key");
        }

        let cfg = Config {
            anthropic_api_key: required("ANTHROPIC_API_KEY")?,
            anthropic_base_url: get("ANTHROPIC_BASE_URL").unwrap_or_else(|| "https://api.anthropic.com".into()),
            api_keys,
            database_url: get("DATABASE_URL").unwrap_or_else(|| "sqlite://cedar.db".into()),
            bind_addr: parse("BIND_ADDR", get("BIND_ADDR"), "127.0.0.1:8080".parse().unwrap())
                .context("BIND_ADDR")?,

            default_model: get("ANTHROPIC_MODEL").unwrap_or_else(|| DEFAULT_MODEL.into()),
            allowed_models: list("ALLOWED_MODELS"),
            default_system_prompt: get("SYSTEM_PROMPT").unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.into()),
            max_tokens: parse("MAX_TOKENS", get("MAX_TOKENS"), 4096)?,
            max_turns_cap: parse("MAX_TURNS_CAP", get("MAX_TURNS_CAP"), 25)?,
            web_search_max_uses: parse("WEB_SEARCH_MAX_USES", get("WEB_SEARCH_MAX_USES"), 5)?,

            max_concurrent_runs: parse("MAX_CONCURRENT_RUNS", get("MAX_CONCURRENT_RUNS"), 4)?,
            max_active_runs_per_key: parse("MAX_ACTIVE_RUNS_PER_KEY", get("MAX_ACTIVE_RUNS_PER_KEY"), 2)?,
            run_timeout: Duration::from_secs(parse("RUN_TIMEOUT_SECS", get("RUN_TIMEOUT_SECS"), 600)?),
            tool_timeout: Duration::from_secs(parse("TOOL_TIMEOUT_SECS", get("TOOL_TIMEOUT_SECS"), 60)?),
            tool_output_max_bytes: parse("TOOL_OUTPUT_MAX_BYTES", get("TOOL_OUTPUT_MAX_BYTES"), 32 * 1024)?,

            sandbox: SandboxConfig {
                image: get("SANDBOX_IMAGE").unwrap_or_else(|| "cedar-sandbox:latest".into()),
                network: get("SANDBOX_NETWORK").unwrap_or_else(|| "none".into()),
                memory: get("SANDBOX_MEMORY").unwrap_or_else(|| "512m".into()),
                cpus: get("SANDBOX_CPUS").unwrap_or_else(|| "1".into()),
            },
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.max_turns_cap == 0 {
            bail!("MAX_TURNS_CAP must be at least 1");
        }
        if self.max_concurrent_runs == 0 {
            bail!("MAX_CONCURRENT_RUNS must be at least 1");
        }
        if self.max_active_runs_per_key < 1 {
            bail!("MAX_ACTIVE_RUNS_PER_KEY must be at least 1");
        }
        if self.tool_timeout.is_zero() {
            bail!("TOOL_TIMEOUT_SECS must be at least 1");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults_and_required() {
        let cfg = Config::from_lookup(lookup(&[("ANTHROPIC_API_KEY", "sk"), ("CEDAR_API_KEYS", "a, b ,,")])).unwrap();
        assert_eq!(cfg.api_keys, ["a", "b"]);
        assert_eq!(cfg.default_model, DEFAULT_MODEL);
        assert_eq!(cfg.sandbox.network, "none");
        assert_eq!(cfg.max_turns_cap, 25);
    }

    #[test]
    fn missing_keys_fail_fast() {
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).is_err());
        assert!(Config::from_lookup(lookup(&[("ANTHROPIC_API_KEY", "sk")])).is_err());
    }

    #[test]
    fn invalid_number_is_reported() {
        let err = Config::from_lookup(lookup(&[
            ("ANTHROPIC_API_KEY", "sk"),
            ("CEDAR_API_KEYS", "a"),
            ("MAX_TOKENS", "lots"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("MAX_TOKENS"));
    }
}
