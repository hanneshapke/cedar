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
pub const DEFAULT_GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const DEFAULT_GMAIL_MCP_URL: &str = "https://gmailmcp.googleapis.com/mcp/v1";
pub const DEFAULT_CALENDAR_MCP_URL: &str = "https://calendarmcp.googleapis.com/mcp/v1";
pub const DEFAULT_GMAIL_API_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
pub const DEFAULT_CALENDAR_API_URL: &str = "https://www.googleapis.com/calendar/v3";
/// The Gmail tools offered by default: reading mail and writing drafts, nothing that sends,
/// deletes, or relabels.
pub const DEFAULT_GMAIL_TOOLS: &str =
    "search_threads,get_thread,list_labels,list_drafts,get_draft,create_draft";
/// The Calendar tools offered by default: reading only.
pub const DEFAULT_CALENDAR_TOOLS: &str =
    "list_calendars,list_events,get_event,search_events,suggest_time";
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
pub struct BrowserConfig {
    /// Offer the `browser` tool (off by default).
    pub enabled: bool,
    pub image: String,
    /// Docker network for the browser containers. It needs internet access, so `bridge` by default.
    pub network: String,
    pub memory: String,
    pub cpus: String,
}

/// An OAuth client and the refresh token one Google account granted it.
#[derive(Clone)]
pub struct GoogleOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    /// The token endpoint; overridable so tests can point at a mock.
    pub token_url: String,
}

impl std::fmt::Debug for GoogleOAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleOAuthConfig")
            .field("client_id", &self.client_id)
            .field("token_url", &self.token_url)
            .finish_non_exhaustive()
    }
}

/// A remote MCP server whose tools are offered to the model.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerConfig {
    /// Prepended to the server's tool names (`gmail` makes `search_threads` `gmail_search_threads`).
    pub prefix: String,
    /// The streamable-HTTP endpoint.
    pub url: String,
    /// The server's tool names to offer; `*` offers every tool it lists.
    pub tools: Vec<String>,
}

/// Gmail or Calendar through Google's REST API, with tools named like the MCP server's.
#[derive(Debug, Clone, PartialEq)]
pub struct GoogleApiConfig {
    /// `gmail` or `calendar`; also the tool name prefix.
    pub prefix: String,
    /// The API's base URL (see `DEFAULT_GMAIL_API_URL` and `DEFAULT_CALENDAR_API_URL`).
    pub url: String,
    /// The tool names to offer; `*` offers every tool cedar implements for the API.
    pub tools: Vec<String>,
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
    /// Open-Meteo geocoding endpoint, used to turn a place name into coordinates (for the weather
    /// forecast and for map pins).
    pub weather_geocoding_base_url: String,
    /// Offer the `show_map` tool (on by default).
    pub maps: bool,
    /// The Google account the Gmail and Calendar tools act for. Set when either is on.
    pub google_oauth: Option<GoogleOAuthConfig>,
    /// Gmail and Calendar through Google's REST APIs (`GOOGLE_BACKEND=api`, the default).
    pub google_apis: Vec<GoogleApiConfig>,
    /// Remote MCP servers (Gmail and Calendar with `GOOGLE_BACKEND=mcp`), in the order their
    /// tools are offered.
    pub mcp_servers: Vec<McpServerConfig>,
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
    pub browser: BrowserConfig,
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

        // Google's MCP servers only serve projects enrolled in the Workspace Developer Preview,
        // which personal accounts can't join; the REST APIs serve everyone.
        let use_mcp = match get("GOOGLE_BACKEND").map(|v| v.trim().to_ascii_lowercase()) {
            None => false,
            Some(v) if v == "api" => false,
            Some(v) if v == "mcp" => true,
            Some(v) => bail!("invalid GOOGLE_BACKEND={v:?}: use api or mcp"),
        };
        let mut google_apis = Vec::new();
        let mut mcp_servers = Vec::new();
        for (prefix, flag_name, default_mcp_url, default_api_url, default_tools) in [
            (
                "gmail",
                "GMAIL",
                DEFAULT_GMAIL_MCP_URL,
                DEFAULT_GMAIL_API_URL,
                DEFAULT_GMAIL_TOOLS,
            ),
            (
                "calendar",
                "CALENDAR",
                DEFAULT_CALENDAR_MCP_URL,
                DEFAULT_CALENDAR_API_URL,
                DEFAULT_CALENDAR_TOOLS,
            ),
        ] {
            if !flag(flag_name, false)? {
                continue;
            }
            let tools_key = format!("{flag_name}_TOOLS");
            let mut tools = list(&tools_key);
            if tools.is_empty() {
                tools = default_tools.split(',').map(String::from).collect();
            }
            if use_mcp {
                mcp_servers.push(McpServerConfig {
                    prefix: prefix.into(),
                    url: get(&format!("{flag_name}_MCP_URL"))
                        .unwrap_or_else(|| default_mcp_url.into()),
                    tools,
                });
            } else {
                google_apis.push(GoogleApiConfig {
                    prefix: prefix.into(),
                    url: get(&format!("{flag_name}_API_URL"))
                        .map(|u| u.trim().trim_end_matches('/').to_string())
                        .unwrap_or_else(|| default_api_url.into()),
                    tools,
                });
            }
        }
        let google_oauth = if mcp_servers.is_empty() && google_apis.is_empty() {
            None
        } else {
            let need = |k: &str| {
                get(k).map(|v| v.trim().to_string()).ok_or_else(|| {
                    anyhow!("GMAIL and CALENDAR need GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET, and GOOGLE_REFRESH_TOKEN; {k} is not set")
                })
            };
            Some(GoogleOAuthConfig {
                client_id: need("GOOGLE_CLIENT_ID")?,
                client_secret: need("GOOGLE_CLIENT_SECRET")?,
                refresh_token: need("GOOGLE_REFRESH_TOKEN")?,
                token_url: get("GOOGLE_TOKEN_URL")
                    .unwrap_or_else(|| DEFAULT_GOOGLE_TOKEN_URL.into()),
            })
        };

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
            maps: flag("MAPS", true)?,
            google_oauth,
            google_apis,
            mcp_servers,
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
            browser: BrowserConfig {
                enabled: flag("BROWSER", false)?,
                image: get("BROWSER_IMAGE").unwrap_or_else(|| "cedar-browser:latest".into()),
                network: get("BROWSER_NETWORK").unwrap_or_else(|| "bridge".into()),
                memory: get("BROWSER_MEMORY").unwrap_or_else(|| "1g".into()),
                cpus: get("BROWSER_CPUS").unwrap_or_else(|| "1".into()),
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
        if self.browser.enabled && self.browser.network == "none" {
            bail!("BROWSER_NETWORK=none leaves the browser without internet access; use a network that has it");
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
        assert!(cfg.weather && cfg.maps);
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
    fn maps_can_be_turned_off() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("MAPS", "off")])).unwrap();
        assert!(!cfg.maps);
        let err =
            Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("MAPS", "maybe")])).unwrap_err();
        assert!(err.to_string().contains("MAPS"), "{err}");
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
    fn the_browser_is_off_unless_enabled() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(!cfg.browser.enabled);
        assert_eq!(cfg.browser.image, "cedar-browser:latest");
        assert_eq!(cfg.browser.network, "bridge");

        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("BROWSER", "on"),
            ("BROWSER_IMAGE", "my-browser:2"),
            ("BROWSER_NETWORK", "cedar-web"),
            ("BROWSER_MEMORY", "2g"),
        ]))
        .unwrap();
        assert!(cfg.browser.enabled);
        assert_eq!(cfg.browser.image, "my-browser:2");
        assert_eq!(cfg.browser.network, "cedar-web");
        assert_eq!(cfg.browser.memory, "2g");

        let err = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("BROWSER", "on"),
            ("BROWSER_NETWORK", "none"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("BROWSER_NETWORK"), "{err}");
    }

    #[test]
    fn gmail_and_calendar_are_off_unless_enabled() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(cfg.mcp_servers.is_empty());
        assert!(cfg.google_apis.is_empty());
        assert!(cfg.google_oauth.is_none());
    }

    #[test]
    fn gmail_and_calendar_need_google_credentials() {
        let err = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("GMAIL", "on"),
            ("GOOGLE_CLIENT_ID", "id"),
            ("GOOGLE_CLIENT_SECRET", "secret"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("GOOGLE_REFRESH_TOKEN"), "{err}");
    }

    #[test]
    fn gmail_and_calendar_settings_come_from_the_environment() {
        let google = [
            ("CEDAR_API_KEYS", "a"),
            ("GOOGLE_CLIENT_ID", "id"),
            ("GOOGLE_CLIENT_SECRET", "secret"),
            ("GOOGLE_REFRESH_TOKEN", " refresh "),
        ];
        let cfg = Config::from_lookup(lookup(
            &[&google[..], &[("GMAIL", "on"), ("CALENDAR", "on")]].concat(),
        ))
        .unwrap();
        let oauth = cfg.google_oauth.as_ref().unwrap();
        assert_eq!(oauth.refresh_token, "refresh");
        assert_eq!(oauth.token_url, DEFAULT_GOOGLE_TOKEN_URL);
        assert!(!format!("{oauth:?}").contains("secret"));
        // The REST APIs by default.
        assert!(cfg.mcp_servers.is_empty());
        assert_eq!(
            cfg.google_apis,
            [
                GoogleApiConfig {
                    prefix: "gmail".into(),
                    url: DEFAULT_GMAIL_API_URL.into(),
                    tools: DEFAULT_GMAIL_TOOLS.split(',').map(String::from).collect(),
                },
                GoogleApiConfig {
                    prefix: "calendar".into(),
                    url: DEFAULT_CALENDAR_API_URL.into(),
                    tools: DEFAULT_CALENDAR_TOOLS
                        .split(',')
                        .map(String::from)
                        .collect(),
                },
            ]
        );

        let cfg = Config::from_lookup(lookup(
            &[
                &google[..],
                &[
                    ("GMAIL", "on"),
                    ("CALENDAR", "on"),
                    ("GOOGLE_BACKEND", "mcp"),
                ],
            ]
            .concat(),
        ))
        .unwrap();
        assert!(cfg.google_apis.is_empty());
        assert_eq!(
            cfg.mcp_servers,
            [
                McpServerConfig {
                    prefix: "gmail".into(),
                    url: DEFAULT_GMAIL_MCP_URL.into(),
                    tools: DEFAULT_GMAIL_TOOLS.split(',').map(String::from).collect(),
                },
                McpServerConfig {
                    prefix: "calendar".into(),
                    url: DEFAULT_CALENDAR_MCP_URL.into(),
                    tools: DEFAULT_CALENDAR_TOOLS
                        .split(',')
                        .map(String::from)
                        .collect(),
                },
            ]
        );

        let cfg = Config::from_lookup(lookup(
            &[
                &google[..],
                &[
                    ("CALENDAR", "on"),
                    ("GOOGLE_BACKEND", "MCP"),
                    ("CALENDAR_MCP_URL", "http://localhost:9999/mcp"),
                    ("CALENDAR_TOOLS", "list_events, create_event"),
                    ("GOOGLE_TOKEN_URL", "http://localhost:9999/token"),
                ],
            ]
            .concat(),
        ))
        .unwrap();
        assert_eq!(cfg.mcp_servers.len(), 1);
        assert_eq!(cfg.mcp_servers[0].url, "http://localhost:9999/mcp");
        assert_eq!(cfg.mcp_servers[0].tools, ["list_events", "create_event"]);
        assert_eq!(
            cfg.google_oauth.unwrap().token_url,
            "http://localhost:9999/token"
        );

        let cfg = Config::from_lookup(lookup(
            &[
                &google[..],
                &[
                    ("GMAIL", "on"),
                    ("GMAIL_API_URL", "http://localhost:9999/gmail/"),
                ],
            ]
            .concat(),
        ))
        .unwrap();
        assert_eq!(cfg.google_apis[0].url, "http://localhost:9999/gmail");

        let err = Config::from_lookup(lookup(
            &[&google[..], &[("GMAIL", "on"), ("GOOGLE_BACKEND", "rest")]].concat(),
        ))
        .unwrap_err();
        assert!(err.to_string().contains("GOOGLE_BACKEND"), "{err}");
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
