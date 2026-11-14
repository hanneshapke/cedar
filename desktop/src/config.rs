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
    /// Serve the chat on the tailnet over HTTPS (`tailscale serve`), for the iOS and Android apps.
    pub tailscale_share: bool,
    /// The tailnet port it is served on; 443 gives an address without a port.
    pub tailscale_https_port: u16,
    /// Look for a newer release every few hours (see update.rs).
    pub check_updates: bool,
    /// The release whose banner was dismissed; a later one shows again.
    pub dismissed_update: String,
    /// The language picked on the setup page (`de-CH`, `fr-FR`, ...; see i18n.rs), for the page,
    /// the tray menu, and cedar's `DEFAULT_LOCALE`. Empty until one is picked: the page then
    /// asks first.
    pub locale: String,
    /// A chat server on another computer to use instead of starting the two servers here (see
    /// [`Remote`]). `None`: everything runs on this computer.
    pub remote: Option<Remote>,
    /// The user chose to run the servers on this computer, on the setup page's question (see
    /// [`Self::asks_choice`]).
    pub run_locally: bool,
    /// How the model server above was named on the setup page: `own` (a server the user runs) or
    /// `hosted` (a provider). Empty in a config from before the page recorded it; see
    /// `models::source`.
    pub model_source: String,
    /// A model the app runs itself with llama.cpp (see llamasetup.rs), beside or instead of the
    /// model server above: cedar gets it as an entry of its own.
    pub llama: Option<Llama>,
    /// The port the app's llama-server listens on, on this computer only.
    pub llama_port: u16,
    /// Extra environment for cedar, applied last (`SANDBOX_NETWORK`, `BRAVE_API_KEY`, ...).
    pub cedar_env: BTreeMap<String, String>,
    /// Extra environment for cedar-chat, applied last.
    pub chat_env: BTreeMap<String, String>,
    pub secrets: Secrets,
    /// Keys this version doesn't know, kept as they are.
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

/// A cedar-chat on another computer (an always-on machine, so scheduled agents run while this one
/// sleeps). The app starts neither server, needs no Docker, and its window shows that chat, as the
/// phone apps do. Set from the same `cedar://connect` link they scan.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Remote {
    /// Its origin, `https://host[:port]` (see `cedar_pairing::connect::server_origin`).
    pub server: String,
    /// Its access token (`CHAT_ACCESS_TOKEN`).
    pub token: String,
}

/// What the app installed for llama.cpp: names from cedar-llama's lists, so that a later version
/// of the app finds the same build and model again.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Llama {
    /// `cedar_llama::release::Build::id`, such as `macos-arm64`.
    pub build: String,
    /// `cedar_llama::catalog::Model::id`, such as `gemma-4-26b-a4b`.
    pub model: String,
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
    /// The key the app's llama-server asks for, so that only cedar uses it.
    pub llama_api_key: String,
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
            tailscale_share: false,
            tailscale_https_port: 443,
            check_updates: true,
            dismissed_update: String::new(),
            locale: String::new(),
            remote: None,
            run_locally: false,
            model_source: String::new(),
            llama: None,
            llama_port: 18081,
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
        let moved = cfg.split_llama();
        if cfg.fill_secrets() || moved {
            cfg.save(path)?;
        }
        Ok(cfg)
    }

    /// The address the app's llama-server answers at.
    pub fn llama_url(&self) -> String {
        cedar_llama::server::base_url(self.llama_port)
    }

    /// An early build of the llama.cpp setup wrote llama-server's address into the model server
    /// fields too. It is an entry of its own now, so those fields are emptied again.
    fn split_llama(&mut self) -> bool {
        let doubled = self.llama.is_some() && self.llm_base_url.trim() == self.llama_url();
        if doubled {
            self.llm_base_url.clear();
            self.llm_model.clear();
            self.llm_api_key.clear();
            self.model_source.clear();
        }
        doubled
    }

    fn fill_secrets(&mut self) -> bool {
        let mut changed = false;
        for slot in [
            &mut self.secrets.cedar_api_key,
            &mut self.secrets.chat_access_token,
            &mut self.secrets.chat_admin_token,
            &mut self.secrets.llama_api_key,
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

    /// A model server is named: both its address and the model name are known.
    pub fn has_server(&self) -> bool {
        !self.llm_base_url.trim().is_empty() && !self.llm_model.trim().is_empty()
    }

    /// There is a model to register with cedar: a model server, or one the app runs itself.
    pub fn configured(&self) -> bool {
        self.has_server() || self.llama.is_some()
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

    /// The chat the window shows: the remote one when set, else the one this app starts.
    pub fn chat_url(&self) -> String {
        match self.remote_server() {
            Some(r) => r.server.clone(),
            None => self.local_chat_url(),
        }
    }

    /// The chat this app starts, whichever chat the window shows.
    pub fn local_chat_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.chat_port)
    }

    /// The setup page asks whether Cedar runs here or uses another computer's, once: until it is
    /// answered (`run_locally`) or a remote server is set. Someone who set the app up with an
    /// earlier version is asked too, after updating, since the choice is new.
    pub fn asks_choice(&self) -> bool {
        self.remote_server().is_none() && !self.run_locally
    }

    /// While the question is open on a first launch (no model server chosen either), nothing is
    /// started, checked or downloaded. An earlier setup keeps starting as before meanwhile, so its
    /// scheduled agents don't stop until the question is answered.
    pub fn holds_start(&self) -> bool {
        self.asks_choice() && !self.configured()
    }

    /// The chat on another computer, when one is set.
    pub fn remote_server(&self) -> Option<&Remote> {
        self.remote.as_ref().filter(|r| !r.server.trim().is_empty())
    }

    /// The access token of the chat [`Self::chat_url`] points at.
    pub fn chat_token(&self) -> &str {
        match self.remote_server() {
            Some(r) => &r.token,
            None => &self.secrets.chat_access_token,
        }
    }

    /// The admin token handed to the chat page. Only this app's own chat's is known; a remote
    /// chat's Settings page asks for its admin token itself.
    pub fn chat_admin_token(&self) -> &str {
        match self.remote_server() {
            Some(_) => "",
            None => &self.secrets.chat_admin_token,
        }
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
        assert!(!cfg.tailscale_share);
        assert_eq!(cfg.tailscale_https_port, 443);
        assert!(cfg.check_updates);
        assert_eq!(cfg.locale, "");
        assert_eq!(cfg.other["future_setting"], 1);
        let back = serde_json::to_value(&cfg).unwrap();
        assert_eq!(back["future_setting"], 1);
        assert_eq!(cfg.sandbox_image(), format!("{IMAGE_REPOSITORY}/cedar-sandbox:{VERSION}"));
        assert!(cfg.remote_server().is_none());
        assert!(cfg.asks_choice() && cfg.holds_start());
    }

    #[test]
    fn a_model_server_or_llama_is_enough() {
        let mut cfg = Config::default();
        assert!(!cfg.has_server() && !cfg.configured());
        cfg.llama = Some(Llama { build: "macos-arm64".into(), model: "gemma-4-e4b".into() });
        assert!(!cfg.has_server() && cfg.configured());
        cfg.llm_base_url = "https://api.groq.com/openai/v1".into();
        assert!(!cfg.has_server(), "an address without a model is not a server yet");
        cfg.llm_model = "openai/gpt-oss-120b".into();
        assert!(cfg.has_server() && cfg.configured());
        cfg.llama = None;
        assert!(cfg.has_server() && cfg.configured());
    }

    /// A config from the first build of the llama.cpp setup: the model server fields held
    /// llama-server's address. They are emptied; a real model server beside llama.cpp is kept.
    #[test]
    fn llama_gets_out_of_the_model_server_fields() {
        let mut cfg = Config {
            llama: Some(Llama { build: "macos-arm64".into(), model: "gemma-4-e4b".into() }),
            llm_base_url: "http://127.0.0.1:18081/v1".into(),
            llm_model: "gemma-4-e4b".into(),
            llm_api_key: "k".into(),
            model_source: "llama".into(),
            ..Config::default()
        };
        assert!(cfg.split_llama());
        assert!(cfg.llm_base_url.is_empty() && cfg.llm_model.is_empty() && cfg.llm_api_key.is_empty());
        assert!(cfg.model_source.is_empty() && cfg.llama.is_some() && cfg.configured());
        assert!(!cfg.split_llama(), "once");

        cfg.llm_base_url = "https://api.groq.com/openai/v1".into();
        cfg.llm_model = "m".into();
        assert!(!cfg.split_llama());
        assert!(cfg.has_server());
        // Without llama.cpp, a server on that port is simply the user's.
        let mut own = Config { llm_base_url: "http://127.0.0.1:18081/v1".into(), llm_model: "m".into(), ..Config::default() };
        assert!(!own.split_llama() && own.has_server());
    }

    #[test]
    fn the_question_is_asked_once() {
        // A first launch: asked, and nothing starts meanwhile.
        let mut cfg = Config::default();
        assert!(cfg.asks_choice() && cfg.holds_start());
        cfg.run_locally = true;
        assert!(!cfg.asks_choice() && !cfg.holds_start());
        // Set up with an earlier version (a model server chosen): asked once too, but its servers
        // keep starting meanwhile.
        let mut cfg: Config = serde_json::from_str(r#"{"llm_base_url": "http://x/v1", "llm_model": "m"}"#).unwrap();
        assert!(cfg.asks_choice() && !cfg.holds_start());
        cfg.run_locally = true;
        assert!(!cfg.asks_choice());
        // Connected to another computer: that was the answer.
        let cfg = Config {
            remote: Some(Remote { server: "https://s.t.ts.net".into(), token: "t".into() }),
            ..Config::default()
        };
        assert!(!cfg.asks_choice() && !cfg.holds_start());
    }

    #[test]
    fn a_remote_server_replaces_the_local_chat() {
        let mut cfg = Config::default();
        cfg.secrets.chat_access_token = "local".into();
        cfg.secrets.chat_admin_token = "admin".into();
        assert_eq!(cfg.chat_url(), "http://127.0.0.1:13000");
        assert_eq!(cfg.chat_token(), "local");
        assert_eq!(cfg.chat_admin_token(), "admin");
        cfg.remote = Some(Remote { server: "https://spark.t.ts.net".into(), token: "remote".into() });
        assert_eq!(cfg.chat_url(), "https://spark.t.ts.net");
        assert_eq!(cfg.chat_token(), "remote");
        assert_eq!(cfg.chat_admin_token(), "");
        let back: Config = serde_json::from_value(serde_json::to_value(&cfg).unwrap()).unwrap();
        assert_eq!(back.remote, cfg.remote);
        // An empty address left in the file by hand means none.
        cfg.remote = Some(Remote::default());
        assert!(cfg.remote_server().is_none());
        assert_eq!(cfg.chat_url(), "http://127.0.0.1:13000");
    }
}
