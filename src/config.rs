//! Service configuration, read from the environment.

use crate::approvals::Rules;
use crate::models::PlanMode;
use anyhow::{anyhow, bail, Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

/// What a model server address saved before the registry existed pointed at when it named no
/// model, and the built-in address of that time: used only to migrate such a saved setting.
pub const LEGACY_DEFAULT_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const LEGACY_DEFAULT_LLM_BASE_URL: &str = "http://localhost:8000/v1";
pub const DEFAULT_BRAVE_BASE_URL: &str = "https://api.search.brave.com/res/v1/web/search";
pub const DEFAULT_WEATHER_BASE_URL: &str = "https://api.open-meteo.com/v1/forecast";
pub const DEFAULT_WEATHER_GEOCODING_BASE_URL: &str =
    "https://geocoding-api.open-meteo.com/v1/search";
pub const DEFAULT_RECIPE_WIKIPEDIA_URL: &str = "https://en.wikipedia.org/w/api.php";
pub const DEFAULT_RECIPE_COMMONS_URL: &str = "https://commons.wikimedia.org/w/api.php";
/// The public Transitous trip planner (a MOTIS server), for transit routes and geocoding.
pub const DEFAULT_TRANSIT_BASE_URL: &str = "https://api.transitous.org";
/// The OSRM project's public demo router, for driving routes.
pub const DEFAULT_DRIVING_BASE_URL: &str = "https://router.project-osrm.org";
/// Yahoo Finance's chart API (daily prices; the symbol goes under it) and search API (news).
pub const DEFAULT_STOCKS_CHART_URL: &str = "https://query1.finance.yahoo.com/v8/finance/chart";
pub const DEFAULT_STOCKS_NEWS_URL: &str = "https://query1.finance.yahoo.com/v1/finance/search";
/// Best Buy's Products API (the search filter goes after it in parentheses), which
/// `search_products` reads products, prices, photos, and ratings from.
pub const DEFAULT_PRODUCTS_API_URL: &str = "https://api.bestbuy.com/v1/products";
pub const DEFAULT_GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const DEFAULT_GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// What "Connect Google" asks for: the account's address, and what the default tools need.
pub const DEFAULT_GOOGLE_SCOPES: &str = "openid email \
    https://www.googleapis.com/auth/gmail.readonly \
    https://www.googleapis.com/auth/gmail.compose \
    https://www.googleapis.com/auth/calendar.readonly";
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
pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are Cedar, a personal AI assistant. You work for one person and help with whatever they bring \
you: questions, research, writing, email and calendar, shopping, travel, money, code and files. \
Your tools run in an isolated sandbox whose working directory is /workspace.

How you work:
- Give the answer first. Keep replies short unless the user asks for more detail.
- Use a tool when it helps and answer directly when it doesn't. If a dedicated tool exists for \
something, use it instead of doing the same thing by hand in the sandbox.
- Ask when it matters. If a request is ambiguous and the answer would change what you do, ask \
the user, offering the likely choices, instead of guessing, and put all your questions in one go. \
Don't ask about things you can look up or reasonably assume: pick a sensible default and say \
which one you picked.
- Plan complex work. When a task has several steps, needs several tools, or would be costly to \
get wrong, write a short numbered plan and then carry it out; don't stop to ask whether the plan \
is fine. Ask first only when a step depends on a choice only the user can make, and then ask \
about that choice, not about the plan. If a plan was already agreed with the user, follow it and \
don't plan again.
- Act carefully for the user. Before anything that sends, buys, deletes or can't be undone, check \
with them unless they already told you to go ahead.
- Keep their information private. Use what you know about them to help them, and never share it \
anywhere they didn't ask for.";

#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// The `docker` command: a name looked up on `PATH` or an absolute path (`DOCKER_BIN`).
    pub docker_bin: String,
    pub image: String,
    /// Docker network mode for the container. `none` by default.
    pub network: String,
    pub memory: String,
    pub cpus: String,
    /// A directory of personal tools written in Python (`TOOLS_DIR`). Each call of one runs in a
    /// container of its own that sees only that tool's folder. See `tools::custom`.
    pub tools_dir: Option<PathBuf>,
    /// Docker network mode for the calls of personal tools whose manifest declares `network`
    /// hosts (`TOOLS_NETWORK`), `bridge` by default; tools that declare none always get `none`.
    pub tools_network: String,
}

#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// Offer the `browser` tool (off by default).
    pub enabled: bool,
    /// The `docker` command, as in [`SandboxConfig`].
    pub docker_bin: String,
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

/// The deployment's Google OAuth client. The refresh token is optional here: without one in the
/// environment, an admin connects the account from the chat UI and cedar keeps the token
/// encrypted in its database (see `CEDAR_SECRET_KEY`).
#[derive(Clone)]
pub struct GoogleClientConfig {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: Option<String>,
    pub token_url: String,
    /// Where people are sent to grant access ("Connect Google").
    pub auth_url: String,
    /// The scopes asked for when connecting.
    pub scopes: Vec<String>,
}

impl std::fmt::Debug for GoogleClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleClientConfig")
            .field("client_id", &self.client_id)
            .field("token_url", &self.token_url)
            .field("auth_url", &self.auth_url)
            .field("scopes", &self.scopes)
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
    /// Offer the `show_recipe` tool (on by default).
    pub recipes: bool,
    /// MediaWiki API endpoints `show_recipe` searches for a photo of the dish: Wikipedia first,
    /// then Wikimedia Commons.
    pub recipe_wikipedia_url: String,
    pub recipe_commons_url: String,
    /// Offer the `plan_trip` tool (on by default).
    pub transit: bool,
    /// Root of the MOTIS trip planner `plan_trip` asks for transit routes and looks places up with.
    pub transit_base_url: String,
    /// Root of the OSRM router `plan_trip` asks for driving routes.
    pub driving_base_url: String,
    /// Offer the `compare_stocks` tool (on by default).
    pub stocks: bool,
    /// The chart API `compare_stocks` reads prices from, and the search API it reads news from.
    pub stocks_chart_url: String,
    pub stocks_news_url: String,
    /// Offer the `search_products` tool (on by default), which still needs `bestbuy_api_key`.
    pub products: bool,
    /// Best Buy developer API key. `search_products` is only offered to the model when this is set.
    pub bestbuy_api_key: Option<String>,
    /// The Products API `search_products` searches; overridable so tests can point at a mock.
    pub products_api_url: String,
    /// Offer the `ask_user` tool (on by default), which ends a run with clarifying questions.
    pub ask_user: bool,
    /// The Google account the Gmail and Calendar tools act for. Set when either is on.
    pub google_oauth: Option<GoogleClientConfig>,
    /// Gmail and Calendar through Google's REST APIs (`GOOGLE_BACKEND=api`, the default).
    pub google_apis: Vec<GoogleApiConfig>,
    /// Remote MCP servers (Gmail and Calendar with `GOOGLE_BACKEND=mcp`), in the order their
    /// tools are offered.
    pub mcp_servers: Vec<McpServerConfig>,
    /// Bearer tokens accepted by the API.
    pub api_keys: Vec<String>,
    /// The subset of `api_keys` that may use the admin routes (integrations).
    pub admin_keys: Vec<String>,
    /// Encrypts integration secrets saved through the admin API. Without it they can't be saved.
    pub secret_key: Option<[u8; 32]>,
    pub database_url: String,
    pub bind_addr: SocketAddr,

    pub default_system_prompt: String,
    pub max_turns_cap: u32,

    pub max_concurrent_runs: usize,
    pub max_active_runs_per_key: i64,
    pub run_timeout: Duration,
    pub tool_timeout: Duration,
    pub tool_output_max_bytes: usize,
    /// How many pages of a run's attached PDFs, in all, a model that takes images sees as page
    /// images. 0 shows none; the PDFs are still saved in the workspace.
    pub pdf_preview_pages: usize,
    /// How long a workspace is kept after its last run ends, so a continuation can pick it up.
    pub workspace_ttl: Duration,
    /// Let a run's `callback_url` point at loopback, private and link-local addresses. Off by
    /// default, since the webhook calls come from the cedar host.
    pub webhook_allow_private: bool,
    /// The wait before the first retry of a failed webhook call; each later one doubles it.
    pub webhook_retry: Duration,
    /// Whether a run that doesn't say plans before it starts (`PLAN`: off, on, auto), or may propose
    /// a plan with `propose_plan` (`tool`).
    pub plan: PlanMode,
    /// Which tool calls wait for approval in interactive runs (`TOOL_APPROVALS`).
    pub tool_approvals: Rules,
    /// The community tool registry's index (`TOOLS_REGISTRY_URL`). Unset: no marketplace. Tools
    /// are installed from it into `TOOLS_DIR` (see `marketplace`).
    pub tools_registry_url: Option<String>,
    /// How long a run waits for its plan to be agreed or a call to be approved before it goes on
    /// without (the plan as proposed; the call not made). The wait doesn't count toward
    /// `run_timeout`.
    pub approval_timeout: Duration,

    pub sandbox: SandboxConfig,
    pub browser: BrowserConfig,
}

/// Docker wants an absolute host path for a bind mount, so a relative `TOOLS_DIR` is resolved
/// against the working directory cedar started in.
fn absolute(p: PathBuf) -> PathBuf {
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p)
    }
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
        let secret_key = match get("CEDAR_SECRET_KEY").map(|v| v.trim().to_string()) {
            None => None,
            Some(v) => {
                let bytes = hex::decode(&v)
                    .ok()
                    .filter(|b| b.len() == 32)
                    .ok_or_else(|| {
                        anyhow!("CEDAR_SECRET_KEY must be 64 hex characters (openssl rand -hex 32)")
                    })?;
                Some(<[u8; 32]>::try_from(bytes.as_slice()).expect("checked length"))
            }
        };
        let google_oauth = if mcp_servers.is_empty() && google_apis.is_empty() {
            None
        } else {
            let need = |k: &str| {
                get(k).map(|v| v.trim().to_string()).ok_or_else(|| {
                    anyhow!("GMAIL and CALENDAR need GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET; {k} is not set")
                })
            };
            let refresh_token = get("GOOGLE_REFRESH_TOKEN").map(|v| v.trim().to_string());
            if refresh_token.is_none() && secret_key.is_none() {
                bail!("GMAIL and CALENDAR need GOOGLE_REFRESH_TOKEN, or CEDAR_SECRET_KEY so the account can be connected from the chat UI");
            }
            let scopes = match get("GOOGLE_SCOPES") {
                Some(v) => v
                    .split([' ', ','])
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect(),
                None => DEFAULT_GOOGLE_SCOPES.split(' ').map(String::from).collect(),
            };
            Some(GoogleClientConfig {
                client_id: need("GOOGLE_CLIENT_ID")?,
                client_secret: need("GOOGLE_CLIENT_SECRET")?,
                refresh_token,
                token_url: get("GOOGLE_TOKEN_URL")
                    .unwrap_or_else(|| DEFAULT_GOOGLE_TOKEN_URL.into()),
                auth_url: get("GOOGLE_AUTH_URL").unwrap_or_else(|| DEFAULT_GOOGLE_AUTH_URL.into()),
                scopes,
            })
        };
        let admin_keys = list("CEDAR_ADMIN_KEYS");
        if admin_keys.iter().any(|k| !api_keys.contains(k)) {
            bail!("every key in CEDAR_ADMIN_KEYS must also be in CEDAR_API_KEYS");
        }

        // A GUI launcher (the macOS app) runs without the user's PATH, so it names the CLI itself.
        let docker_bin = get("DOCKER_BIN").unwrap_or_else(|| "docker".into());
        for name in ["LLM_BASE_URL", "LLM_API_KEY", "LLM_MODEL", "ALLOWED_MODELS"] {
            if get(name).is_some() {
                tracing::warn!(
                    setting = name,
                    "ignored: models are registered through the admin API (PUT /v1/admin/models/{{id}}) or the chat UI's Settings"
                );
            }
        }
        if get("MAX_TOKENS").is_some() {
            tracing::warn!(
                setting = "MAX_TOKENS",
                "ignored: each model has its own reply limit, max_tokens on PUT /v1/admin/models/{{id}} or Max tokens per reply in the chat UI's Settings → Server (16384 unless set)"
            );
        }

        let cfg = Config {
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
            recipes: flag("RECIPES", true)?,
            recipe_wikipedia_url: get("RECIPE_WIKIPEDIA_URL")
                .unwrap_or_else(|| DEFAULT_RECIPE_WIKIPEDIA_URL.into()),
            recipe_commons_url: get("RECIPE_COMMONS_URL")
                .unwrap_or_else(|| DEFAULT_RECIPE_COMMONS_URL.into()),
            transit: flag("TRANSIT", true)?,
            transit_base_url: get("TRANSIT_BASE_URL")
                .unwrap_or_else(|| DEFAULT_TRANSIT_BASE_URL.into()),
            driving_base_url: get("DRIVING_BASE_URL")
                .unwrap_or_else(|| DEFAULT_DRIVING_BASE_URL.into()),
            stocks: flag("STOCKS", true)?,
            stocks_chart_url: get("STOCKS_CHART_URL")
                .unwrap_or_else(|| DEFAULT_STOCKS_CHART_URL.into()),
            stocks_news_url: get("STOCKS_NEWS_URL")
                .unwrap_or_else(|| DEFAULT_STOCKS_NEWS_URL.into()),
            products: flag("PRODUCTS", true)?,
            bestbuy_api_key: get("BESTBUY_API_KEY")
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty()),
            products_api_url: get("PRODUCTS_API_URL")
                .unwrap_or_else(|| DEFAULT_PRODUCTS_API_URL.into()),
            ask_user: flag("ASK_USER", true)?,
            google_oauth,
            google_apis,
            mcp_servers,
            api_keys,
            admin_keys,
            secret_key,
            database_url: get("DATABASE_URL").unwrap_or_else(|| "sqlite://cedar.db".into()),
            bind_addr: parse(
                "BIND_ADDR",
                get("BIND_ADDR"),
                "127.0.0.1:8080".parse().unwrap(),
            )
            .context("BIND_ADDR")?,

            default_system_prompt: get("SYSTEM_PROMPT")
                .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.into()),
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
            pdf_preview_pages: parse("PDF_PREVIEW_PAGES", get("PDF_PREVIEW_PAGES"), 10)?,
            workspace_ttl: Duration::from_secs(parse(
                "WORKSPACE_TTL_SECS",
                get("WORKSPACE_TTL_SECS"),
                24 * 3600,
            )?),
            webhook_allow_private: flag("WEBHOOK_ALLOW_PRIVATE", false)?,
            webhook_retry: Duration::from_secs(parse(
                "WEBHOOK_RETRY_SECS",
                get("WEBHOOK_RETRY_SECS"),
                5,
            )?),
            plan: match get("PLAN") {
                None => PlanMode::Off,
                Some(v) => PlanMode::parse(&v)
                    .ok_or_else(|| anyhow!("invalid PLAN={v:?}: use off, on, auto or tool"))?,
            },
            tool_approvals: Rules::parse(&get("TOOL_APPROVALS").unwrap_or_default())?,
            tools_registry_url: get("TOOLS_REGISTRY_URL")
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            approval_timeout: Duration::from_secs(parse(
                "APPROVAL_TIMEOUT_SECS",
                get("APPROVAL_TIMEOUT_SECS"),
                300,
            )?),

            sandbox: SandboxConfig {
                docker_bin: docker_bin.clone(),
                image: get("SANDBOX_IMAGE").unwrap_or_else(|| "cedar-sandbox:latest".into()),
                network: get("SANDBOX_NETWORK").unwrap_or_else(|| "none".into()),
                memory: get("SANDBOX_MEMORY").unwrap_or_else(|| "512m".into()),
                cpus: get("SANDBOX_CPUS").unwrap_or_else(|| "1".into()),
                tools_dir: get("TOOLS_DIR").map(|d| absolute(PathBuf::from(d))),
                tools_network: get("TOOLS_NETWORK").unwrap_or_else(|| "bridge".into()),
            },
            browser: BrowserConfig {
                enabled: flag("BROWSER", false)?,
                docker_bin,
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
        if let Some(url) = &self.tools_registry_url {
            let parsed = reqwest::Url::parse(url)
                .map_err(|e| anyhow!("TOOLS_REGISTRY_URL is not a URL: {e}"))?;
            if !matches!(parsed.scheme(), "https" | "http") {
                bail!("TOOLS_REGISTRY_URL must be an http(s) URL");
            }
        }
        if self.tool_timeout.is_zero() {
            bail!("TOOL_TIMEOUT_SECS must be at least 1");
        }
        if self.approval_timeout.is_zero() {
            bail!("APPROVAL_TIMEOUT_SECS must be at least 1");
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
        assert_eq!(cfg.sandbox.network, "none");
        assert_eq!(cfg.sandbox.docker_bin, "docker");
        assert_eq!(cfg.browser.docker_bin, "docker");
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
    fn recipes_are_on_by_default_and_can_be_turned_off() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(cfg.recipes);
        assert_eq!(cfg.recipe_wikipedia_url, DEFAULT_RECIPE_WIKIPEDIA_URL);
        assert_eq!(cfg.recipe_commons_url, DEFAULT_RECIPE_COMMONS_URL);
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("RECIPES", "off"),
            ("RECIPE_WIKIPEDIA_URL", "http://localhost:9999/wiki"),
            ("RECIPE_COMMONS_URL", "http://localhost:9999/commons"),
        ]))
        .unwrap();
        assert!(!cfg.recipes);
        assert_eq!(cfg.recipe_wikipedia_url, "http://localhost:9999/wiki");
        assert_eq!(cfg.recipe_commons_url, "http://localhost:9999/commons");
    }

    #[test]
    fn trip_planning_is_on_by_default_and_its_servers_can_be_moved() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(cfg.transit);
        assert_eq!(cfg.transit_base_url, DEFAULT_TRANSIT_BASE_URL);
        assert_eq!(cfg.driving_base_url, DEFAULT_DRIVING_BASE_URL);
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("TRANSIT", "off"),
            ("TRANSIT_BASE_URL", "http://localhost:9999/motis"),
            ("DRIVING_BASE_URL", "http://localhost:9999/osrm"),
        ]))
        .unwrap();
        assert!(!cfg.transit);
        assert_eq!(cfg.transit_base_url, "http://localhost:9999/motis");
        assert_eq!(cfg.driving_base_url, "http://localhost:9999/osrm");
        let err = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("TRANSIT", "maybe")]))
            .unwrap_err();
        assert!(err.to_string().contains("TRANSIT"), "{err}");
    }

    #[test]
    fn stocks_are_on_by_default_and_their_apis_can_be_moved() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(cfg.stocks);
        assert_eq!(cfg.stocks_chart_url, DEFAULT_STOCKS_CHART_URL);
        assert_eq!(cfg.stocks_news_url, DEFAULT_STOCKS_NEWS_URL);
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("STOCKS", "off"),
            ("STOCKS_CHART_URL", "http://localhost:9999/chart"),
            ("STOCKS_NEWS_URL", "http://localhost:9999/search"),
        ]))
        .unwrap();
        assert!(!cfg.stocks);
        assert_eq!(cfg.stocks_chart_url, "http://localhost:9999/chart");
        assert_eq!(cfg.stocks_news_url, "http://localhost:9999/search");
    }

    #[test]
    fn product_search_needs_a_key_and_its_api_can_be_moved() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(cfg.products);
        assert_eq!(cfg.bestbuy_api_key, None);
        assert_eq!(cfg.products_api_url, DEFAULT_PRODUCTS_API_URL);
        let blank =
            Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("BESTBUY_API_KEY", " ")]))
                .unwrap();
        assert_eq!(blank.bestbuy_api_key, None, "a blank key is no key");
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("PRODUCTS", "off"),
            ("BESTBUY_API_KEY", " bb-secret "),
            ("PRODUCTS_API_URL", "http://localhost:9999/products"),
        ]))
        .unwrap();
        assert!(!cfg.products);
        assert_eq!(cfg.bestbuy_api_key.as_deref(), Some("bb-secret"));
        assert_eq!(cfg.products_api_url, "http://localhost:9999/products");
        let err = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("PRODUCTS", "maybe")]))
            .unwrap_err();
        assert!(err.to_string().contains("PRODUCTS"), "{err}");
    }

    #[test]
    fn clarifying_questions_are_on_by_default_and_can_be_turned_off() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(cfg.ask_user);
        let cfg =
            Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("ASK_USER", "off")])).unwrap();
        assert!(!cfg.ask_user);
    }

    #[test]
    fn webhook_settings_have_safe_defaults() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(!cfg.webhook_allow_private);
        assert_eq!(cfg.webhook_retry, Duration::from_secs(5));
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("WEBHOOK_ALLOW_PRIVATE", "on"),
            ("WEBHOOK_RETRY_SECS", "0"),
        ]))
        .unwrap();
        assert!(cfg.webhook_allow_private);
        assert_eq!(cfg.webhook_retry, Duration::ZERO);
        let err = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("WEBHOOK_RETRY_SECS", "soon")]))
            .unwrap_err();
        assert!(err.to_string().contains("WEBHOOK_RETRY_SECS"), "{err}");
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

        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("DOCKER_BIN", "/opt/homebrew/bin/docker"),
        ]))
        .unwrap();
        assert_eq!(cfg.sandbox.docker_bin, "/opt/homebrew/bin/docker");
        assert_eq!(cfg.browser.docker_bin, "/opt/homebrew/bin/docker");

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

        // With a secret key the account can be connected later, from the chat UI.
        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("GMAIL", "on"),
            ("GOOGLE_CLIENT_ID", "id"),
            ("GOOGLE_CLIENT_SECRET", "secret"),
            ("CEDAR_SECRET_KEY", &"ab".repeat(32)),
        ]))
        .unwrap();
        assert!(cfg.google_oauth.unwrap().refresh_token.is_none());
    }

    #[test]
    fn admin_keys_and_the_secret_key() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a")])).unwrap();
        assert!(cfg.admin_keys.is_empty());
        assert!(cfg.secret_key.is_none());

        let cfg = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a,b"),
            ("CEDAR_ADMIN_KEYS", "b"),
            ("CEDAR_SECRET_KEY", &"0f".repeat(32)),
        ]))
        .unwrap();
        assert_eq!(cfg.admin_keys, ["b"]);
        assert_eq!(cfg.secret_key, Some([0x0f; 32]));

        let err = Config::from_lookup(lookup(&[
            ("CEDAR_API_KEYS", "a"),
            ("CEDAR_ADMIN_KEYS", "z"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("CEDAR_ADMIN_KEYS"), "{err}");
        for bad in ["abc", &"zz".repeat(32), &"ab".repeat(31)] {
            let err = Config::from_lookup(lookup(&[
                ("CEDAR_API_KEYS", "a"),
                ("CEDAR_SECRET_KEY", bad),
            ]))
            .unwrap_err();
            assert!(err.to_string().contains("CEDAR_SECRET_KEY"), "{err}");
        }
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
        assert_eq!(oauth.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(oauth.auth_url, DEFAULT_GOOGLE_AUTH_URL);
        assert!(oauth.scopes.iter().any(|s| s == "email"));
        assert!(oauth
            .scopes
            .contains(&"https://www.googleapis.com/auth/gmail.readonly".to_string()));
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
        let err = Config::from_lookup(lookup(&[("CEDAR_API_KEYS", "a"), ("MAX_TURNS_CAP", "lots")]))
            .unwrap_err();
        assert!(err.to_string().contains("MAX_TURNS_CAP"));
    }
}
