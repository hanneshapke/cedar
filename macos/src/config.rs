//! `config.json`: what the user set up, plus the secrets the app generated for the two servers.
//! Every field has a default, so an older file keeps working after an upgrade, and unknown keys
//! are left in place.

use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Where the release workflow publishes the sandbox and browser images.
pub const IMAGE_REPOSITORY: &str = "ghcr.io/hanneshapke";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// The OpenAI-compatible model server, with its `/v1` path. Empty until set up.
    pub llm_base_url: String,
    pub llm_api_key: String,
    /// The model name the server registered. Empty until set up.
    pub llm_model: String,
    pub cedar_port: u16,
    pub chat_port: u16,
    /// Empty means the published image for this app version.
    pub sandbox_image: String,
    pub browser_image: String,
    /// Pull the browser image and offer the `browser` tool.
    pub browser: bool,
    /// Extra environment for cedar, applied last (`SANDBOX_NETWORK`, `BRAVE_API_KEY`, ...).
    pub cedar_env: BTreeMap<String, String>,
    /// Extra environment for cedar-chat, applied last.
    pub chat_env: BTreeMap<String, String>,
    pub secrets: Secrets,
    /// Keys this version doesn't know, kept as they are.
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Secrets {
    /// cedar's API key, which is also cedar-chat's key and the admin key.
    pub cedar_api_key: String,
    /// Encrypts integration credentials saved from the chat's Settings page.
    pub cedar_secret_key: String,
    /// What the chat page needs; the app seeds it into the page.
    pub chat_access_token: String,
    /// Lets the chat's Settings page manage cedar's integrations; seeded into the page too.
    pub chat_admin_token: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llm_base_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            cedar_port: 18080,
            chat_port: 13000,
            sandbox_image: String::new(),
            browser_image: String::new(),
            browser: false,
            cedar_env: BTreeMap::new(),
            chat_env: BTreeMap::new(),
            secrets: Secrets::default(),
            other: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Reads the file, or starts from the defaults. Missing secrets are generated and the file
    /// written back, so the servers always get the same keys.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        let mut cfg = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("{} is not valid; fix or delete it", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if cfg.fill_secrets() {
            cfg.save(path)?;
        }
        Ok(cfg)
    }

    fn fill_secrets(&mut self) -> bool {
        let mut changed = false;
        for slot in [
            &mut self.secrets.cedar_api_key,
            &mut self.secrets.chat_access_token,
            &mut self.secrets.chat_admin_token,
        ] {
            if slot.trim().is_empty() {
                *slot = random_hex(24);
                changed = true;
            }
        }
        // cedar wants exactly 64 hex characters here.
        if self.secrets.cedar_secret_key.trim().len() != 64 {
            self.secrets.cedar_secret_key = random_hex(32);
            changed = true;
        }
        changed
    }

    /// Writes the file with mode 0600 (it holds the keys), replacing it atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self)?;
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts
                .open(&tmp)
                .with_context(|| format!("writing {}", tmp.display()))?;
            f.write_all(&json)?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    /// Both the model server and the model name are known.
    pub fn configured(&self) -> bool {
        !self.llm_base_url.trim().is_empty() && !self.llm_model.trim().is_empty()
    }

    pub fn sandbox_image(&self) -> String {
        if self.sandbox_image.trim().is_empty() {
            format!("{IMAGE_REPOSITORY}/cedar-sandbox:{VERSION}")
        } else {
            self.sandbox_image.trim().to_string()
        }
    }

    pub fn browser_image(&self) -> String {
        if self.browser_image.trim().is_empty() {
            format!("{IMAGE_REPOSITORY}/cedar-browser:{VERSION}")
        } else {
            self.browser_image.trim().to_string()
        }
    }

    pub fn cedar_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.cedar_port)
    }

    pub fn chat_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.chat_port)
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_generated_once() {
        let dir = std::env::temp_dir().join(format!("cedar-app-test-{}", random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let a = Config::load_or_create(&path).unwrap();
        assert_eq!(a.secrets.cedar_api_key.len(), 48);
        assert_eq!(a.secrets.cedar_secret_key.len(), 64);
        assert!(!a.configured());
        let b = Config::load_or_create(&path).unwrap();
        assert_eq!(a.secrets.cedar_api_key, b.secrets.cedar_api_key);
        assert_eq!(a.secrets.chat_access_token, b.secrets.chat_access_token);
        assert_eq!(a.secrets.chat_admin_token, b.secrets.chat_admin_token);
        assert_ne!(a.secrets.chat_admin_token, a.secrets.chat_access_token);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_keys_and_defaults_survive() {
        let cfg: Config =
            serde_json::from_str(r#"{"llm_model": "x", "future_setting": 1}"#).unwrap();
        assert_eq!(cfg.chat_port, 13000);
        assert_eq!(cfg.other["future_setting"], 1);
        let back = serde_json::to_value(&cfg).unwrap();
        assert_eq!(back["future_setting"], 1);
        assert_eq!(cfg.sandbox_image(), format!("{IMAGE_REPOSITORY}/cedar-sandbox:{VERSION}"));
    }
}
