//! Service configuration, read from the environment.

use anyhow::{anyhow, bail, Result};
use std::net::SocketAddr;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct Config {
    /// Base URL of the cedar server.
    pub cedar_url: String,
    /// The bearer key this service uses for cedar. Never sent to the browser.
    pub cedar_api_key: String,
    pub bind_addr: SocketAddr,
    pub database_url: String,
    /// If set, the browser must present this token. Without it, anyone who can reach
    /// `bind_addr` can use the agent, so only leave it unset on a loopback address.
    pub access_token: Option<String>,
    /// If set, the Integrations settings (API keys, the Google account) are offered to whoever
    /// presents this token as well. Without it they are hidden.
    pub admin_token: Option<String>,
    /// This server's address as the browser reaches it, for the "Connect Google" redirect URI.
    pub public_url: String,
    /// How much earlier conversation is inlined into the prompt when the previous cedar run is gone
    /// and can't be continued.
    pub history_max_bytes: usize,
    /// Where Pushover messages are POSTed; overridable so tests can point at a mock.
    pub pushover_url: String,
    /// Where iPhone pushes are POSTed, when set: overrides the production and sandbox APNs
    /// addresses (for a mock in tests, or a relay).
    pub apns_url: Option<String>,
    /// The front end `/` opens until the browser has picked one in Settings → Interface: `v1`–`v5`.
    pub default_ui: String,
}

/// The front ends `default_ui` can name (see `static/ui-pref.js`).
pub const UIS: [&str; 5] = ["v1", "v2", "v3", "v4", "v5"];

/// The default front end baked in at build time (`CEDAR_DEFAULT_UI=v4 cargo build …`, as
/// `scripts/build-dmg.sh` does for a white-label app); `CHAT_DEFAULT_UI` overrides it at run time.
pub const BUILD_DEFAULT_UI: &str = match option_env!("CEDAR_DEFAULT_UI") {
    Some(ui) => ui,
    None => "v3",
};
const _: () = assert!(is_ui(BUILD_DEFAULT_UI), "CEDAR_DEFAULT_UI must be one of v1, v2, v3, v4, v5");

/// Whether `ui` is one of `UIS`, usable in a constant.
const fn is_ui(ui: &str) -> bool {
    matches!(ui.as_bytes(), [b'v', b'1'..=b'5'])
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        fn parse<T: FromStr>(name: &str, raw: Option<String>, default: T) -> Result<T>
        where
            T::Err: std::fmt::Display,
        {
            match raw {
                None => Ok(default),
                Some(s) => s.trim().parse().map_err(|e| anyhow!("invalid {name}={s:?}: {e}")),
            }
        }

        let bind_addr: SocketAddr = parse("CHAT_BIND_ADDR", get("CHAT_BIND_ADDR"), "127.0.0.1:3000".parse().unwrap())?;
        let cfg = Config {
            cedar_url: get("CEDAR_URL").unwrap_or_else(|| "http://127.0.0.1:8080".into()),
            cedar_api_key: get("CEDAR_API_KEY").ok_or_else(|| anyhow!("CEDAR_API_KEY must be set"))?,
            bind_addr,
            database_url: get("CHAT_DATABASE_URL").unwrap_or_else(|| "sqlite://cedar-chat.db".into()),
            access_token: get("CHAT_ACCESS_TOKEN").map(|t| t.trim().to_string()),
            admin_token: get("CHAT_ADMIN_TOKEN").map(|t| t.trim().to_string()),
            public_url: get("CHAT_PUBLIC_URL")
                .map(|u| u.trim().trim_end_matches('/').to_string())
                .unwrap_or_else(|| format!("http://{bind_addr}")),
            history_max_bytes: parse("CHAT_HISTORY_MAX_BYTES", get("CHAT_HISTORY_MAX_BYTES"), 40_000)?,
            pushover_url: get("CHAT_PUSHOVER_URL").unwrap_or_else(|| crate::channels::PUSHOVER_URL.into()),
            apns_url: get("CHAT_APNS_URL").map(|u| u.trim().trim_end_matches('/').to_string()),
            default_ui: get("CHAT_DEFAULT_UI").map(|u| u.trim().to_string()).unwrap_or_else(|| BUILD_DEFAULT_UI.into()),
        };
        if !UIS.contains(&cfg.default_ui.as_str()) {
            bail!("CHAT_DEFAULT_UI (or CEDAR_DEFAULT_UI at build time) must be one of {}", UIS.join(", "));
        }
        if cfg.history_max_bytes > 60_000 {
            // cedar rejects prompts over 100 KB; leave room for the new message itself.
            bail!("CHAT_HISTORY_MAX_BYTES must be at most 60000");
        }
        if !(cfg.public_url.starts_with("http://") || cfg.public_url.starts_with("https://")) {
            bail!("CHAT_PUBLIC_URL must start with http:// or https://");
        }
        if let Some(u) = &cfg.apns_url {
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                bail!("CHAT_APNS_URL must start with http:// or https://");
            }
        }
        if cfg.admin_token.is_some() && cfg.admin_token == cfg.access_token {
            bail!("CHAT_ADMIN_TOKEN must differ from CHAT_ACCESS_TOKEN");
        }
        Ok(cfg)
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
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k")])).unwrap();
        assert_eq!(cfg.cedar_url, "http://127.0.0.1:8080");
        assert!(cfg.access_token.is_none());
        assert!(cfg.admin_token.is_none());
        assert_eq!(cfg.public_url, "http://127.0.0.1:3000");
        assert_eq!(cfg.default_ui, BUILD_DEFAULT_UI);
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_PUBLIC_URL", "https://chat.example/")])).unwrap();
        assert_eq!(cfg.public_url, "https://chat.example");
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_PUBLIC_URL", "chat.example")])).is_err());
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_ACCESS_TOKEN", "t"), ("CHAT_ADMIN_TOKEN", "t")])).is_err());
        assert!(Config::from_lookup(lookup(&[])).is_err());
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_HISTORY_MAX_BYTES", "999999")])).is_err());
        assert!(cfg.apns_url.is_none());
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_APNS_URL", "http://127.0.0.1:1/apns/")])).unwrap();
        assert_eq!(cfg.apns_url.as_deref(), Some("http://127.0.0.1:1/apns"));
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_APNS_URL", "apns.example")])).is_err());
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_DEFAULT_UI", " v4 ")])).unwrap();
        assert_eq!(cfg.default_ui, "v4");
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_DEFAULT_UI", "v9")])).is_err());
        assert!(UIS.iter().all(|ui| is_ui(ui)) && !is_ui("v6") && !is_ui("v"));
    }
}
