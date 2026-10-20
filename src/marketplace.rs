//! The community tool marketplace: personal tools other people wrote, listed in a registry and
//! installed into `TOOLS_DIR` by an admin.
//!
//! The registry is a JSON index (`TOOLS_REGISTRY_URL`; `registry/` in this repository builds
//! one). Each listing names a release: its version, what it asks for ([`Permissions`]: hosts,
//! secrets, packages, health data), and the SHA-256 of every file in it. Installing a tool
//! downloads those files, refuses any whose hash differs, checks that the manifest asks for
//! exactly what the listing shows and what the admin accepted, installs its packages into the
//! tool's own environment, and only then puts the folder in place, with an
//! [`INSTALL_RECEIPT`](custom::INSTALL_RECEIPT) that marks it as a community tool. Such tools
//! run like any personal tool, in a container of their own, and in interactive runs every call
//! waits for the user's OK (see `approvals::check_personal`).
//!
//! Tools written locally are never touched: a listing whose name is taken by one can't be
//! installed, and only community tools can be uninstalled here.

use crate::error::AppError;
use crate::integrations::Integrations;
use crate::sandbox::SandboxFactory;
use crate::tools::custom::{self, Installed, Permissions, PythonTool};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a fetched index is reused for browsing (installing always fetches it again).
const INDEX_TTL: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_INDEX_BYTES: usize = 4 << 20;
const MAX_FILES: usize = 50;
const MAX_FILE_BYTES: usize = 2 << 20;
const MAX_TOTAL_BYTES: usize = 8 << 20;
/// The index format this server reads.
const INDEX_VERSION: u32 = 1;

/// The registry's index.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Index {
    pub version: u32,
    pub tools: Vec<Listing>,
}

/// One tool's current release in the registry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Listing {
    pub name: String,
    pub version: String,
    /// One line for the list.
    pub summary: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub permissions: Permissions,
    /// Where the files are, absolute or relative to the index's URL.
    pub base_url: String,
    /// File name → SHA-256 (hex).
    pub files: BTreeMap<String, String>,
    /// A maintainer reviewed this release (the registry's CI checks the rest).
    #[serde(default)]
    pub reviewed: bool,
}

/// What an admin sends to install a tool: the permissions they agreed to, which must be exactly
/// what the release asks for.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    pub accept: Permissions,
}

pub struct Marketplace {
    url: Option<String>,
    tools_dir: Option<PathBuf>,
    http: reqwest::Client,
    cache: Mutex<Option<(Instant, Arc<Index>)>>,
}

/// SHA-256 over the files' names and hashes: one value naming a release's exact contents.
pub fn release_digest(files: &BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    for (name, sha) in files {
        h.update(name.as_bytes());
        h.update(b" ");
        h.update(sha.as_bytes());
        h.update(b"\n");
    }
    hex::encode(h.finalize())
}

/// A file a release may contain: a plain name in the tool's folder. Hidden files are refused
/// (so a release can't bring its own `.env` or install receipt), except `.env.example`.
pub fn check_file_name(name: &str) -> Result<()> {
    let ok = name == ".env.example"
        || (!name.is_empty()
            && name.len() <= 100
            && name.starts_with(|c: char| c.is_ascii_alphanumeric())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)));
    if !ok {
        bail!("{name:?} is not a file name a release may contain");
    }
    Ok(())
}

fn check_sha(sha: &str) -> Result<()> {
    if sha.len() != 64
        || !sha
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        bail!("{sha:?} is not a lowercase hex SHA-256");
    }
    Ok(())
}

/// Check a listing before anything is downloaded for it.
fn check_listing(l: &Listing) -> Result<()> {
    custom::check_name(&l.name)?;
    if l.files.is_empty() || l.files.len() > MAX_FILES {
        bail!("{}: a release has 1 to {MAX_FILES} files", l.name);
    }
    if !l.files.contains_key(custom::MANIFEST) {
        bail!("{}: the release has no {}", l.name, custom::MANIFEST);
    }
    for (name, sha) in &l.files {
        check_file_name(name)?;
        check_sha(sha)?;
        if name == custom::INSTALL_RECEIPT {
            bail!("{}: a release can't bring its own install receipt", l.name);
        }
    }
    Ok(())
}

impl Marketplace {
    pub fn new(url: Option<String>, tools_dir: Option<PathBuf>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("cedar/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            url,
            tools_dir,
            http,
            cache: Mutex::default(),
        })
    }

    fn url(&self) -> Result<&str, AppError> {
        self.url.as_deref().ok_or_else(|| {
            AppError::Conflict(
                "no tool registry is set up on the server (TOOLS_REGISTRY_URL)".into(),
            )
        })
    }

    fn tools_dir(&self) -> Result<&Path, AppError> {
        self.tools_dir.as_deref().ok_or_else(|| {
            AppError::Conflict("installing tools needs TOOLS_DIR on the server".into())
        })
    }

    async fn download(&self, url: &reqwest::Url, max: usize) -> Result<Vec<u8>> {
        let mut resp = self
            .http
            .get(url.clone())
            .send()
            .await
            .with_context(|| format!("fetching {url}"))?;
        if !resp.status().is_success() {
            bail!("fetching {url}: {}", resp.status());
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() > max {
                bail!("{url} is larger than {max} bytes");
            }
        }
        Ok(body)
    }

    /// The index, from the cache unless `fresh` or it is older than [`INDEX_TTL`].
    async fn index(&self, fresh: bool) -> Result<Arc<Index>, AppError> {
        let url = self.url()?;
        if !fresh {
            if let Some((at, index)) = &*self.cache.lock().unwrap() {
                if at.elapsed() < INDEX_TTL {
                    return Ok(index.clone());
                }
            }
        }
        let parsed = reqwest::Url::parse(url).map_err(|e| anyhow!("TOOLS_REGISTRY_URL: {e}"))?;
        let body = self
            .download(&parsed, MAX_INDEX_BYTES)
            .await
            .map_err(|e| AppError::BadGateway(format!("the tool registry: {e:#}")))?;
        let index: Index = serde_json::from_slice(&body)
            .map_err(|e| AppError::BadGateway(format!("the tool registry's index: {e}")))?;
        if index.version != INDEX_VERSION {
            return Err(AppError::BadGateway(format!(
                "the tool registry's index is version {}; this server reads version {INDEX_VERSION}",
                index.version
            )));
        }
        let index = Arc::new(index);
        *self.cache.lock().unwrap() = Some((Instant::now(), index.clone()));
        Ok(index)
    }

    /// The registry's tools, each with whether it is installed (and in which version), whether
    /// an update is out, and whether a local tool or one of cedar's own takes its name.
    pub async fn browse(
        &self,
        integrations: &Integrations,
        refresh: bool,
    ) -> Result<Value, AppError> {
        let index = self.index(refresh).await?;
        let set = integrations.tools().load_full();
        let tools: Vec<Value> = index
            .tools
            .iter()
            .filter(|l| check_listing(l).is_ok())
            .map(|l| {
                let mut v = serde_json::to_value(l).unwrap_or_default();
                let existing = set.get(&l.name);
                let installed = existing
                    .and_then(|t| t.personal())
                    .and_then(PythonTool::installed);
                v["installed"] = match installed {
                    Some(i) => json!({
                        "version": i.version,
                        "update_available": i.digest != release_digest(&l.files),
                    }),
                    None => Value::Null,
                };
                v["conflict"] = match existing {
                    Some(t) if t.personal().is_some_and(|p| p.installed().is_none()) => {
                        json!("a local tool has this name")
                    }
                    Some(t) if t.personal().is_none() => json!("a built-in tool has this name"),
                    _ => Value::Null,
                };
                v
            })
            .collect();
        Ok(json!({
            "registry": self.url,
            "installable": self.tools_dir.is_some(),
            "tools": tools,
        }))
    }

    /// Install (or update to) the registry's current release of `name`. See the module docs.
    pub async fn install(
        &self,
        integrations: &Integrations,
        sandboxes: &dyn SandboxFactory,
        name: &str,
        req: InstallRequest,
    ) -> Result<Value, AppError> {
        let dir = self.tools_dir()?.to_path_buf();
        let index = self.index(true).await?;
        let listing = index
            .tools
            .iter()
            .find(|l| l.name == name)
            .ok_or(AppError::NotFound("the registry has no such tool"))?
            .clone();
        check_listing(&listing).map_err(|e| AppError::BadGateway(format!("{e:#}")))?;
        if req.accept != listing.permissions {
            return Err(AppError::Conflict(format!(
                "the release asks for different permissions than were accepted: {}",
                serde_json::to_string(&listing.permissions).unwrap_or_default()
            )));
        }
        if let Some(t) = integrations.tools().load().get(name) {
            match t.personal() {
                None => return Err(AppError::Conflict(format!("{name} is a built-in tool"))),
                Some(p) if p.installed().is_none() => {
                    return Err(AppError::Conflict(format!(
                        "a local tool named {name} exists; remove or rename it first"
                    )))
                }
                Some(_) => {}
            }
        }
        let target = dir.join(name);
        if target.exists() && !target.join(custom::INSTALL_RECEIPT).is_file() {
            return Err(AppError::Conflict(format!(
                "{} exists and wasn't installed from the marketplace",
                target.display()
            )));
        }

        let files = self
            .fetch_release(&listing)
            .await
            .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;
        let manifest = std::str::from_utf8(&files[custom::MANIFEST])
            .map_err(|_| AppError::BadGateway("tool.json is not UTF-8".into()))?;
        let tool = PythonTool::from_manifest(name, manifest)
            .map_err(|e| AppError::BadGateway(format!("the release's {e:#}")))?;
        if tool.permissions() != listing.permissions {
            return Err(AppError::BadGateway(
                "the release's tool.json asks for other permissions than its listing".into(),
            ));
        }
        if tool.version() != Some(listing.version.as_str()) {
            return Err(AppError::BadGateway(
                "the release's tool.json has another version than its listing".into(),
            ));
        }

        // Packages first: if they can't be installed, nothing else changes.
        sandboxes
            .setup_tool(name, tool.pip())
            .await
            .map_err(|e| AppError::BadGateway(format!("installing {name}'s packages: {e:#}")))?;

        let receipt = Installed {
            version: listing.version.clone(),
            registry: self.url()?.to_string(),
            digest: release_digest(&listing.files),
            files: listing.files.clone(),
            installed_at: chrono::Utc::now().timestamp_millis(),
        };
        let _guard = integrations.lock_changes().await;
        place(&dir, name, &files, &receipt)?;
        integrations.reload().await?;
        tracing::info!(tool = name, version = %listing.version, "community tool installed");
        self.installed_info(integrations, name).await
    }

    async fn installed_info(
        &self,
        integrations: &Integrations,
        name: &str,
    ) -> Result<Value, AppError> {
        let all = integrations.personal_tools().await?;
        all["tools"]
            .as_array()
            .and_then(|t| t.iter().find(|t| t["name"] == name))
            .cloned()
            .ok_or_else(|| {
                AppError::Internal(anyhow!(
                    "{name} was installed but could not be loaded; see the server log"
                ))
            })
    }

    /// Download every file of a release and check its hash.
    async fn fetch_release(&self, listing: &Listing) -> Result<BTreeMap<String, Vec<u8>>> {
        let index_url = reqwest::Url::parse(self.url.as_deref().unwrap_or_default())?;
        let mut base = index_url
            .join(&listing.base_url)
            .context("the listing's base_url")?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let mut out = BTreeMap::new();
        let mut total = 0;
        for (file, sha) in &listing.files {
            let url = base.join(file)?;
            let body = self.download(&url, MAX_FILE_BYTES).await?;
            total += body.len();
            if total > MAX_TOTAL_BYTES {
                bail!("the release is larger than {MAX_TOTAL_BYTES} bytes");
            }
            let got = hex::encode(Sha256::digest(&body));
            if &got != sha {
                bail!("{file} doesn't match the registry's hash (expected {sha}, got {got})");
            }
            out.insert(file.clone(), body);
        }
        Ok(out)
    }

    /// Remove a community tool: its folder, its environment, and its saved secrets.
    pub async fn uninstall(
        &self,
        integrations: &Integrations,
        sandboxes: &dyn SandboxFactory,
        name: &str,
    ) -> Result<(), AppError> {
        let dir = self.tools_dir()?;
        custom::check_name(name).map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
        let target = dir.join(name);
        if !target.join(custom::INSTALL_RECEIPT).is_file() {
            return Err(AppError::NotFound(
                "no community tool by that name is installed",
            ));
        }
        {
            let _guard = integrations.lock_changes().await;
            std::fs::remove_dir_all(&target)
                .with_context(|| format!("removing {}", target.display()))?;
            integrations.forget_tool_secrets(name).await?;
            integrations.reload().await?;
        }
        if let Err(e) = sandboxes.remove_tool_env(name).await {
            tracing::warn!(tool = name, error = %format!("{e:#}"), "removing the tool's environment failed");
        }
        tracing::info!(tool = name, "community tool uninstalled");
        Ok(())
    }

    /// Install a tool's packages again (after a failed setup, or for a local tool).
    pub async fn setup(
        integrations: &Integrations,
        sandboxes: &dyn SandboxFactory,
        name: &str,
    ) -> Result<Value, AppError> {
        let pip = integrations
            .tools()
            .load()
            .get(name)
            .and_then(|t| t.personal())
            .map(|t| t.pip().to_vec())
            .ok_or(AppError::NotFound("no such personal tool"))?;
        let output = sandboxes
            .setup_tool(name, &pip)
            .await
            .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;
        Ok(json!({ "name": name, "pip": pip, "output": output }))
    }
}

/// Write the release into a hidden staging folder, then swap it in for the tool's folder, so a
/// rebuild never sees half of it. The previous release, if any, is removed afterwards.
fn place(
    dir: &Path,
    name: &str,
    files: &BTreeMap<String, Vec<u8>>,
    receipt: &Installed,
) -> Result<()> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let staging = dir.join(format!(".staging-{name}-{id}"));
    let write = || -> Result<()> {
        std::fs::create_dir(&staging)?;
        for (file, body) in files {
            std::fs::write(staging.join(file), body)?;
        }
        std::fs::write(
            staging.join(custom::INSTALL_RECEIPT),
            serde_json::to_vec_pretty(receipt)?,
        )?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.context("writing the tool's files"));
    }
    let target = dir.join(name);
    let old = dir.join(format!(".old-{name}-{id}"));
    let had_old = target.exists();
    if had_old {
        std::fs::rename(&target, &old).context("moving the previous release aside")?;
    }
    if let Err(e) = std::fs::rename(&staging, &target) {
        if had_old {
            let _ = std::fs::rename(&old, &target);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(anyhow::Error::from(e).context("putting the tool in place"));
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(files: &[(&str, &str)]) -> Listing {
        Listing {
            name: "t".into(),
            version: "1.0.0".into(),
            summary: "s".into(),
            category: None,
            author: None,
            homepage: None,
            license: None,
            permissions: Permissions::default(),
            base_url: "tools/t/".into(),
            files: files
                .iter()
                .map(|(n, s)| (n.to_string(), s.to_string()))
                .collect(),
            reviewed: false,
        }
    }

    #[test]
    fn release_files_are_plain_names_with_hashes() {
        let sha = "a".repeat(64);
        assert!(check_listing(&listing(&[("tool.json", &sha), ("tool.py", &sha)])).is_ok());
        assert!(check_listing(&listing(&[("tool.json", &sha), (".env.example", &sha)])).is_ok());
        for bad in ["../x", "a/b", ".env", ".cedar-install.json", "-x", ""] {
            assert!(
                check_listing(&listing(&[("tool.json", &sha), (bad, &sha)])).is_err(),
                "{bad}"
            );
        }
        assert!(
            check_listing(&listing(&[("tool.py", &sha)])).is_err(),
            "no manifest"
        );
        assert!(
            check_listing(&listing(&[("tool.json", "ABC")])).is_err(),
            "not a hash"
        );
        let mut l = listing(&[("tool.json", &sha)]);
        l.name = "Bad-Name".into();
        assert!(check_listing(&l).is_err());
    }

    #[test]
    fn the_digest_names_the_exact_files() {
        let a = listing(&[("tool.json", "1"), ("tool.py", "2")]).files;
        let b = listing(&[("tool.json", "1"), ("tool.py", "3")]).files;
        assert_eq!(release_digest(&a), release_digest(&a.clone()));
        assert_ne!(release_digest(&a), release_digest(&b));
    }

    #[test]
    fn placing_a_release_replaces_the_previous_one_whole() {
        let dir = std::env::temp_dir().join(format!("cedar-place-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("t")).unwrap();
        std::fs::write(dir.join("t/old.py"), "old").unwrap();
        let receipt = Installed {
            version: "2".into(),
            registry: "r".into(),
            digest: "d".into(),
            files: BTreeMap::new(),
            installed_at: 0,
        };
        let files = BTreeMap::from([("tool.py".to_string(), b"new".to_vec())]);
        place(&dir, "t", &files, &receipt).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("t/tool.py")).unwrap(),
            "new"
        );
        assert!(!dir.join("t/old.py").exists());
        assert!(dir.join("t").join(custom::INSTALL_RECEIPT).is_file());
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(left.len(), 1, "no staging or old folders are left");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
