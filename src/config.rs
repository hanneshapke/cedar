//! Service configuration, read from the environment.

use anyhow::{anyhow, bail, Context, Result};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

pub const DEFAULT_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const DEFAULT_LLM_BASE_URL: &str = "http://localhost:8000/v1";
pub const DEFAULT_BRAVE_BASE_URL: &str = "https://api.search.brave.com/res/v1/web/search";
pub const DEFAULT_WEATHER_BASE_URL: &str = "https://api.open-meteo.com/v1/forecast";
pub const DEFAULT_WEATHER_GEOCODING_BASE_URL: &str =
    "https://geocoding-api.open-meteo.com/v1/search";
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
    /// Base URL of the OpenAI-compatible server (vLLM), including the `/v1` prefix.
    pub llm_base_url: String,
    /// Bearer token for the LLM server; only needed if it was started with `--api-key`.
    pub llm_api_key: Option<String>,
    /// Brave Search API key. The `web_search` tool is only offered to the model when this is set.
    pub brave_api_key: Option<String>,
    /// Brave web search endpoint; overridable so tests can point at a mock.
    pub brave_base_url: String,
    /// Offer the `fetch_url` and `download_file` tools (on by default).
    pub web_fetch: bool,
    /// Let those tools reach loopback, private, and link-local addresses. Off by default, since
    /// the requests come from the cedar host.
    pub web_fetch_allow_private: bool,
    /// Offer the `weather_forecast` tool (on by default).
    pub weather: bool,
    /// Open-Meteo forecast endpoint; overridable for a self-hosted instance or so tests can mock it.
    pub weather_base_url: String,
    /// Open-Meteo geocoding endpoint, used to turn a place name into coordinates.
    pub weather_geocoding_base_url: String,
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

    pub max_concurrent_runs: usize,
    pub max_active_runs_per_key: i64,
    pub run_timeout: Duration,
    pub tool_timeout: Duration,
    pub tool_output_max_bytes: usize,
    /// How long a workspace is kept after its last run ends, so a continuation can pick it up.
    pub workspace_ttl: Duration,

    pub sandbox: SandboxConfig,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Build a config from any key lookup, so tests don't have to touch the process environment.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        fn parse<T: FromStr>(name: &str, raw: Option<String>, default: T) -> Result<T>
        where
            T::Err: std::fmt::Display,
        {
            match raw {
                None => Ok(default),
                Some(s) => s
                    .trim()
                    .parse()
                    .map_err(|e| anyhow!("invalid {name}={s:?}: {e}")),
            }
        }
        let list = |k: &str| -> Vec<String> {
            get(k)
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };

        let flag = |k: &str, default: bool| -> Result<bool> {
            match get(k).map(|v| v.trim().to_ascii_lowercase()) {
                None => Ok(default),
                Some(v) => match v.as_str() {
                    "1" | "true" | "on" | "yes" => Ok(true),
                    "0" | "false" | "off" | "no" => Ok(false),
                    _ => bail!("invalid {k}={v:?}: use on or off"),
                },
            }
        };

        let api_keys = list("CEDAR_API_KEYS");
        if api_keys.is_empty() {
            bail!("CEDAR_API_KEYS must contain at least one comma-separated API key");
        }

        let cfg = Config {
            llm_base_url: get("LLM_BASE_URL").unwrap_or_else(|| DEFAULT_LLM_BASE_URL.into()),
            llm_api_key: get("LLM_API_KEY").map(|k| k.trim().to_string()),
            brave_api_key: get("BRAVE_API_KEY").map(|k| k.trim().to_string()),
            brave_base_url: get("BRAVE_API_BASE_URL")
                .unwrap_or_else(|| DEFAULT_BRAVE_BASE_URL.into()),
            web_fetch: flag("WEB_FETCH", true)?,
            web_fetch_allow_private: flag("WEB_FETCH_ALLOW_PRIVATE", false)?,
            weather: flag("WEATHER", true)?,
            weather_base_url: get("WEATHER_BASE_URL")
                .unwrap_or_else(|| DEFAULT_WEATHER_BASE_URL.into()),
            weather_geocoding_base_url: get("WEATHER_GEOCODING_BASE_URL")
                .unwrap_or_else(|| DEFAULT_WEATHER_GEOCODING_BASE_URL.into()),
            api_keys,
            database_url: get("DATABASE_URL").unwrap_or_else(|| "sqlite://cedar.db".into()),
            bind_addr: parse(
                "BIND_ADDR",
                get("BIND_ADDR"),
                "127.0.0.1:8080".parse().unwrap(),
            )
            .context("BIND_ADDR")?,

            default_model: get("LLM_MODEL").unwrap_or_else(|| DEFAULT_MODEL.into()),
            allowed_models: list("ALLOWED_MODELS"),
            default_system_prompt: get("SYSTEM_PROMPT")
                .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.into()),
            max_tokens: parse("MAX_TOKENS", get("MAX_TOKENS"), 4096)?,
            max_turns_cap: parse("MAX_TURNS_CAP", get("MAX_TURNS_CAP"), 25)?,

            max_concurrent_runs: parse("MAX_CONCURRENT_RUNS", get("MAX_CONCURRENT_RUNS"), 4)?,
            max_active_runs_per_key: parse(
                "MAX_ACTIVE_RUNS_PER_KEY",
                get("MAX_ACTIVE_RUNS_PER_KEY"),
                2,
            )?,
            run_timeout: Duration::from_secs(parse(
                "RUN_TIMEOUT_SECS",
                get("RUN_TIMEOUT_SECS"),
                600,
            )?),
            tool_timeout: Duration::from_secs(parse(
                "TOOL_TIMEOUT_SECS",
                get("TOOL_TIMEOUT_SECS"),
                60,
            )?),
            tool_output_max_bytes: parse(
                "TOOL_OUTPUT_MAX_BYTES",
                get("TOOL_OUTPUT_MAX_BYTES"),
                32 * 1024,
            )?,
            workspace_ttl: Duration::from_secs(parse(
                "WORKSPACE_TTL_SECS",
                get("WORKSPACE_TTL_SECS"),
                24 * 3600,
            )?),

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
        if self.workspace_ttl.is_zero() {
            bail!("WORKSPACE_TTL_SECS must be at least 1");
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
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults_and_required() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a, b ,,")])).unwrap();
        assert_eq!(cfg.api_keys, ["a", "b"]);
        assert_eq!(cfg.default_model, DEFAULT_MODEL);
        assert_eq!(cfg.llm_base_url, DEFAULT_LLM_BASE_URL);
        assert_eq!(cfg.llm_api_key, None);
        assert_eq!(cfg.sandbox.network, "none");
        assert_eq!(cfg.max_turns_cap, 25);
        assert_eq!(cfg.workspace_ttl, Duration::from_secs(86_400));
        assert!(cfg.web_fetch);
        assert!(!cfg.web_fetch_allow_private);
    }

    #[test]
    fn web_fetch_flags_parse_on_and_off() {
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("WEB_FETCH", "Off"),
            ("WEB_FETCH_ALLOW_PRIVATE", "true"),
        ]))
        .unwrap();
        assert!(!cfg.web_fetch);
        assert!(cfg.web_fetch_allow_private);
        let err = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("WEB_FETCH", "maybe")]))
            .unwrap_err();
        assert!(err.to_string().contains("WEB_FETCH"), "{err}");
    }

    #[test]
    fn weather_is_on_by_default_and_points_at_open_meteo() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(cfg.weather);
        assert_eq!(cfg.weather_base_url, DEFAULT_WEATHER_BASE_URL);
        assert_eq!(
            cfg.weather_geocoding_base_url,
            DEFAULT_WEATHER_GEOCODING_BASE_URL
        );
    }

    #[test]
    fn weather_settings_come_from_the_environment() {
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("WEATHER", "off"),
            ("WEATHER_BASE_URL", "http://localhost:9999/forecast"),
            ("WEATHER_GEOCODING_BASE_URL", "http://localhost:9999/geo"),
        ]))
        .unwrap();
        assert!(!cfg.weather);
        assert_eq!(cfg.weather_base_url, "http://localhost:9999/forecast");
        assert_eq!(cfg.weather_geocoding_base_url, "http://localhost:9999/geo");
        let err = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("WEATHER", "maybe")]))
            .unwrap_err();
        assert!(err.to_string().contains("WEATHER"), "{err}");
    }

    #[test]
    fn llm_settings_come_from_the_environment() {
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("LLM_BASE_URL", "http://gpu-box:9000/v1"),
            ("LLM_API_KEY", " secret "),
            ("LLM_MODEL", "my-model"),
        ]))
        .unwrap();
        assert_eq!(cfg.llm_base_url, "http://gpu-box:9000/v1");
        assert_eq!(cfg.llm_api_key.as_deref(), Some("secret"));
        assert_eq!(cfg.default_model, "my-model");
    }

    #[test]
    fn brave_search_is_off_unless_a_key_is_set() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert_eq!(cfg.brave_api_key, None);
        assert_eq!(cfg.brave_base_url, DEFAULT_BRAVE_BASE_URL);

        let blank =
            Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("BRAVE_API_KEY", "  ")]))
                .unwrap();
        assert_eq!(blank.brave_api_key, None);
    }

    #[test]
    fn brave_settings_come_from_the_environment() {
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("BRAVE_API_KEY", " BSA-secret "),
            ("BRAVE_API_BASE_URL", "http://localhost:9999/search"),
        ]))
        .unwrap();
        assert_eq!(cfg.brave_api_key.as_deref(), Some("BSA-secret"));
        assert_eq!(cfg.brave_base_url, "http://localhost:9999/search");
    }

    #[test]
    fn missing_keys_fail_fast() {
        assert!(Config::from_lookup(lookup(&[])).is_err());
    }

    #[test]
    fn invalid_number_is_reported() {
        let err = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("MAX_TOKENS", "lots")]))
            .unwrap_err();
        assert!(err.to_string().contains("MAX_TOKENS"));
    }
}
