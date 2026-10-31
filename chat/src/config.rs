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
    /// Where Android pushes through Firebase go (`{fcm_url}/v1/projects/{project}/messages:send`),
    /// and where the Firebase service account's signed assertion is traded for an access token;
    /// overridable so tests can point at a mock.
    pub fcm_url: String,
    pub google_token_url: String,
    /// The tailscale CLI Settings -> Phone asks about Tailscale, when it isn't in the usual places
    /// (or, in tests, a fake).
    pub tailscale_cli: Option<String>,
    /// Whether to look for a newer release every few hours (`CHAT_UPDATE_CHECK`, on by default;
    /// the desktop app turns it off, it updates itself).
    pub update_check: bool,
    /// Where the latest release is looked up; overridable so tests can point at a mock.
    pub update_url: String,
    /// The directory the root `cedar-update` unit watches (see `update`); without it the page
    /// only shows how to update by hand.
    pub update_dir: Option<std::path::PathBuf>,
    /// The corpus a conversation can be contributed to (`CHAT_CONTRIBUTIONS_URL`, the base URL
    /// of a cedar-contrib ingest; see `contrib`). Unset, nothing can be sent; the review dialog
    /// still exports a bundle to a file.
    pub contributions_url: Option<String>,
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
            fcm_url: get("CHAT_FCM_URL")
                .map(|u| u.trim().trim_end_matches('/').to_string())
                .unwrap_or_else(|| crate::channels::FCM_URL.into()),
            google_token_url: get("CHAT_GOOGLE_TOKEN_URL")
                .map(|u| u.trim().to_string())
                .unwrap_or_else(|| crate::channels::GOOGLE_TOKEN_URL.into()),
            tailscale_cli: get("CHAT_TAILSCALE_CLI").map(|p| p.trim().to_string()),
            update_check: match get("CHAT_UPDATE_CHECK").as_deref().map(|v| v.trim().to_ascii_lowercase()).as_deref() {
                None | Some("on" | "true" | "1" | "yes") => true,
                Some("off" | "false" | "0" | "no") => false,
                Some(other) => bail!("CHAT_UPDATE_CHECK must be on or off, not {other:?}"),
            },
            update_url: get("CHAT_UPDATE_URL").map(|u| u.trim().to_string()).unwrap_or_else(|| crate::update::DEFAULT_URL.into()),
            update_dir: get("CHAT_UPDATE_DIR").map(|d| std::path::PathBuf::from(d.trim())),
            contributions_url: get("CHAT_CONTRIBUTIONS_URL").map(|u| u.trim().trim_end_matches('/').to_string()),
        };
        if let Some(u) = &cfg.contributions_url {
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                bail!("CHAT_CONTRIBUTIONS_URL must start with http:// or https://");
            }
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
        for (name, u) in [("CHAT_FCM_URL", &cfg.fcm_url), ("CHAT_GOOGLE_TOKEN_URL", &cfg.google_token_url), ("CHAT_UPDATE_URL", &cfg.update_url)] {
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                bail!("{name} must start with http:// or https://");
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
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_PUBLIC_URL", "https://chat.example/")])).unwrap();
        assert_eq!(cfg.public_url, "https://chat.example");
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_PUBLIC_URL", "chat.example")])).is_err());
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_ACCESS_TOKEN", "t"), ("CHAT_ADMIN_TOKEN", "t")])).is_err());
        assert!(Config::from_lookup(lookup(&[])).is_err());
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_HISTORY_MAX_BYTES", "999999")])).is_err());
        assert!(cfg.apns_url.is_none());
        assert!(cfg.tailscale_cli.is_none());
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_APNS_URL", "http://127.0.0.1:1/apns/")])).unwrap();
        assert_eq!(cfg.apns_url.as_deref(), Some("http://127.0.0.1:1/apns"));
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_APNS_URL", "apns.example")])).is_err());
        assert_eq!(cfg.fcm_url, "https://fcm.googleapis.com");
        assert_eq!(cfg.google_token_url, "https://oauth2.googleapis.com/token");
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_FCM_URL", "http://127.0.0.1:1/fcm/"), ("CHAT_GOOGLE_TOKEN_URL", "http://127.0.0.1:1/token")])).unwrap();
        assert_eq!((cfg.fcm_url.as_str(), cfg.google_token_url.as_str()), ("http://127.0.0.1:1/fcm", "http://127.0.0.1:1/token"));
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_FCM_URL", "fcm.example")])).is_err());
    }

    #[test]
    fn update_settings() {
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k")])).unwrap();
        assert!(cfg.update_check);
        assert_eq!(cfg.update_url, crate::update::DEFAULT_URL);
        assert!(cfg.update_dir.is_none());
        let cfg = Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_UPDATE_CHECK", "OFF"), ("CHAT_UPDATE_DIR", "/var/lib/cedar/update")])).unwrap();
        assert!(!cfg.update_check);
        assert_eq!(cfg.update_dir.as_deref(), Some(std::path::Path::new("/var/lib/cedar/update")));
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_UPDATE_CHECK", "sometimes")])).is_err());
        assert!(Config::from_lookup(lookup(&[("CEDAR_API_KEY", "k"), ("CHAT_UPDATE_URL", "api.github.com")])).is_err());
    }
}
