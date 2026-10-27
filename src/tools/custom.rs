//! Personal tools written in Python, loaded from `TOOLS_DIR` without recompiling cedar.
//!
//! Each tool is a folder in that directory with a `tool.json` manifest and a script (`tool.py`
//! unless the manifest says otherwise). A tool is either written by the server's owner (a local
//! tool) or installed from the community registry (see `marketplace`), which leaves an
//! [`INSTALL_RECEIPT`] in its folder.
//!
//! Every call runs in a container of its own (see [`crate::sandbox::ToolRun`]): it sees the run's
//! workspace, its own folder read-only at [`MOUNT`], and its own environment, where its packages
//! live, but not the other tools' folders, their secrets, or their packages. It gets a network
//! only if its manifest declares the hosts it reaches, and only the secrets its manifest declares
//! (saved through the admin API, encrypted). The call's input arrives as JSON on stdin, stdout is
//! the result, and a non-zero exit turns stderr into an error the model sees.
//!
//! The manifests are read on the host, every time the tool set is built, so a tool added or
//! changed while cedar runs is picked up by the next rebuild (a credential saved through the
//! admin API, an install, or a restart). A folder whose manifest can't be used is logged and left
//! out.

use super::{changes, shell_quote, stderr_text, truncate_with_total, Tool, ToolContext, GROUPS};
use crate::sandbox::ToolRun;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

/// Where a tool's own folder is mounted in its container.
pub const MOUNT: &str = crate::sandbox::TOOL_MOUNT;
/// The group every personal tool belongs to (see [`GROUPS`]).
pub const GROUP: &str = "custom";
pub const MANIFEST: &str = "tool.json";
/// Written by the marketplace into the folder of a tool it installed.
pub const INSTALL_RECEIPT: &str = ".cedar-install.json";
const DEFAULT_ENTRY: &str = "tool.py";
const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 2000;
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// Longest a manifest may ask a call to run; the run's own timeout still applies.
const MAX_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_LIST: usize = 20;
const MAX_FIELD: usize = 200;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// Must match the folder name when given; the folder names the tool.
    name: Option<String>,
    description: String,
    /// JSON Schema of the input object; `{"type": "object", "properties": {}}` when left out.
    input_schema: Option<Value>,
    /// The script to run, a file name in the tool's folder. `tool.py` by default.
    entry: Option<String>,
    /// Per-call limit in seconds, instead of `TOOL_TIMEOUT_SECS`.
    timeout_secs: Option<u64>,
    version: Option<String>,
    author: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    /// The hosts the tool reaches. Declaring any gives its calls `TOOLS_NETWORK`.
    #[serde(default)]
    network: Vec<String>,
    /// The environment variables it reads its credentials from.
    #[serde(default)]
    secrets: Vec<SecretEntry>,
    /// Packages to `pip install` into the tool's own environment when it is set up.
    #[serde(default)]
    pip: Vec<String>,
    /// It reads or returns personal health data, which brings in the health rules of the system
    /// prompt and a consent step when it is installed.
    #[serde(default)]
    health_data: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SecretEntry {
    Name(String),
    Full(SecretSpec),
}

/// A credential a tool reads from its environment.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The tool works without it.
    #[serde(default)]
    pub optional: bool,
}

/// What a tool asks for, as it is shown before installing it and checked against the registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default)]
    pub network: Vec<String>,
    /// The names of the secrets.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub pip: Vec<String>,
    #[serde(default)]
    pub health_data: bool,
}

/// The marketplace's record of installing a tool (the [`INSTALL_RECEIPT`]).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Installed {
    pub version: String,
    /// The registry index it came from.
    pub registry: String,
    /// SHA-256 over the installed files' names and hashes, which names this exact release.
    pub digest: String,
    /// Each installed file's SHA-256.
    pub files: std::collections::BTreeMap<String, String>,
    /// Unix milliseconds.
    pub installed_at: i64,
    /// The listing's text in other languages, for Settings (see [`ListingText`]).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub translations: std::collections::BTreeMap<String, ListingText>,
}

/// A registry listing's text for people in one language (`translations` in its `listing.json`):
/// the one-line summary and the descriptions of its secrets. It is only shown, never sent to the
/// model, and it is not part of what an admin accepts, so fixing a translation asks for nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ListingText {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Secret name → description.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub secrets: std::collections::BTreeMap<String, String>,
}

/// One tool loaded from `TOOLS_DIR`.
pub struct PythonTool {
    name: String,
    description: String,
    schema: Value,
    entry: String,
    timeout: Option<Duration>,
    /// The tool's folder on the host; set by [`load_dir`].
    dir: PathBuf,
    version: Option<String>,
    author: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    network: Vec<String>,
    secrets: Vec<SecretSpec>,
    pip: Vec<String>,
    health_data: bool,
    /// From the marketplace, or written locally (`None`).
    installed: Option<Installed>,
    /// The saved values of the declared secrets.
    env: Vec<(String, String)>,
}

fn check_text(what: &str, v: Option<String>) -> Result<Option<String>> {
    let Some(v) = v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.len() > MAX_FIELD || v.chars().any(char::is_control) {
        bail!("{what} must be one line of at most {MAX_FIELD} characters");
    }
    Ok(Some(v))
}

/// A host name, or `*.` and a domain for its subdomains.
fn check_host(host: &str) -> Result<()> {
    let bare = host.strip_prefix("*.").unwrap_or(host);
    let ok = !bare.is_empty()
        && bare.len() <= 253
        && bare.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        });
    if !ok {
        bail!("network entries are lowercase host names (or *.domain), not {host:?}");
    }
    Ok(())
}

/// Variables cedar or Python rely on can't be declared as secrets, so a tool can't learn or
/// change them through one.
fn check_secret_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_');
    if !ok {
        bail!("secret names are UPPER_CASE letters, digits, and underscores: {name:?}");
    }
    const RESERVED: &[&str] = &["PATH", "HOME", "USER", "SHELL", "PWD", "TMPDIR", "LANG"];
    const PREFIXES: &[&str] = &["PYTHON", "PIP_", "LD_", "CEDAR_", "DOCKER_", "SSL_", "LC_"];
    if RESERVED.contains(&name) || PREFIXES.iter().any(|p| name.starts_with(p)) {
        bail!("{name:?} is reserved and can't be a secret");
    }
    Ok(())
}

/// A plain requirement (`name`, `name[extra]>=1.2,<2`): no options, URLs, or paths, so a
/// manifest can't make pip read from anywhere but the package index.
fn check_requirement(req: &str) -> Result<()> {
    let ok = !req.is_empty()
        && req.len() <= MAX_FIELD
        && req.starts_with(|c: char| c.is_ascii_alphanumeric())
        && req
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-[],<>=!~* ".contains(c));
    if !ok {
        bail!("pip entries are package requirements like \"name>=1.0\", not {req:?}");
    }
    Ok(())
}

fn capped<T>(what: &str, list: Vec<T>) -> Result<Vec<T>> {
    if list.len() > MAX_LIST {
        bail!("{what} may have at most {MAX_LIST} entries");
    }
    Ok(list)
}

impl PythonTool {
    /// Build a tool from a folder's name and manifest text, checking both.
    pub fn from_manifest(folder: &str, manifest: &str) -> Result<Self> {
        check_name(folder)?;
        let m: Manifest = serde_json::from_str(manifest).context("tool.json is not valid")?;
        if let Some(name) = &m.name {
            if name != folder {
                bail!("tool.json names the tool {name:?}, but the folder is {folder:?}");
            }
        }
        let mut description = m.description.trim().to_string();
        if description.is_empty() {
            bail!("tool.json needs a description");
        }
        if description.len() > MAX_DESCRIPTION {
            let mut end = MAX_DESCRIPTION;
            while !description.is_char_boundary(end) {
                end -= 1;
            }
            description.truncate(end);
            description.push('…');
        }
        let schema = match m.input_schema {
            None => json!({ "type": "object", "properties": {} }),
            Some(Value::Object(mut obj)) => {
                if obj.get("type").is_some_and(|t| t != "object") {
                    bail!("input_schema must describe an object");
                }
                obj.entry("type").or_insert_with(|| json!("object"));
                obj.entry("properties").or_insert_with(|| json!({}));
                Value::Object(obj)
            }
            Some(_) => bail!("input_schema must be a JSON object"),
        };
        let entry = m.entry.unwrap_or_else(|| DEFAULT_ENTRY.into());
        if entry.is_empty()
            || entry == "."
            || entry == ".."
            || entry.contains('/')
            || entry.contains('\\')
            || entry.contains('\0')
            || entry.starts_with('-')
        {
            bail!("entry must be a file name in the tool's folder, not {entry:?}");
        }
        let timeout = match m.timeout_secs {
            None => None,
            Some(0) => bail!("timeout_secs must be at least 1"),
            Some(s) => Some(Duration::from_secs(s).min(MAX_TIMEOUT)),
        };
        let network = capped("network", m.network)?;
        for host in &network {
            check_host(host)?;
        }
        let secrets: Vec<SecretSpec> = capped("secrets", m.secrets)?
            .into_iter()
            .map(|s| match s {
                SecretEntry::Name(name) => SecretSpec {
                    name,
                    description: None,
                    optional: false,
                },
                SecretEntry::Full(spec) => spec,
            })
            .collect();
        let mut seen = HashSet::new();
        for s in &secrets {
            check_secret_name(&s.name)?;
            check_text("a secret's description", s.description.clone())?;
            if !seen.insert(s.name.as_str()) {
                bail!("the secret {:?} is declared twice", s.name);
            }
        }
        let pip = capped("pip", m.pip)?;
        for req in &pip {
            check_requirement(req)?;
        }
        Ok(Self {
            name: folder.to_string(),
            description,
            schema,
            entry,
            timeout,
            dir: PathBuf::new(),
            version: check_text("version", m.version)?,
            author: check_text("author", m.author)?,
            homepage: check_text("homepage", m.homepage)?,
            license: check_text("license", m.license)?,
            network,
            secrets,
            pip,
            health_data: m.health_data,
            installed: None,
            env: Vec::new(),
        })
    }

    /// The script's path in the tool's container.
    #[cfg(test)]
    fn script(&self) -> String {
        format!("{MOUNT}/{}", self.entry)
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub fn secrets(&self) -> &[SecretSpec] {
        &self.secrets
    }

    pub fn pip(&self) -> &[String] {
        &self.pip
    }

    pub fn health_data(&self) -> bool {
        self.health_data
    }

    pub fn network(&self) -> &[String] {
        &self.network
    }

    /// Installed from the marketplace, rather than written on this server.
    pub fn installed(&self) -> Option<&Installed> {
        self.installed.as_ref()
    }

    /// What the tool asks for, in the form the registry lists it.
    pub fn permissions(&self) -> Permissions {
        Permissions {
            network: self.network.clone(),
            secrets: self.secrets.iter().map(|s| s.name.clone()).collect(),
            pip: self.pip.clone(),
            health_data: self.health_data,
        }
    }

    /// Hand the tool the saved values of the secrets it declares, ignoring any others.
    pub fn with_secrets(mut self, saved: Option<&HashMap<String, String>>) -> Self {
        self.env = self
            .secrets
            .iter()
            .filter_map(|s| Some((s.name.clone(), saved?.get(&s.name)?.clone())))
            .collect();
        self
    }

    /// For admins: what the tool is, where it comes from, what it asks for, and which of its
    /// secrets are saved. Never includes a secret's value.
    pub fn info(&self, saved: &HashSet<String>) -> Value {
        let secrets: Vec<Value> = self
            .secrets
            .iter()
            .map(|s| {
                json!({ "name": s.name, "description": s.description, "optional": s.optional,
                        "saved": saved.contains(&s.name) })
            })
            .collect();
        json!({
            "name": self.name,
            "description": self.description,
            "version": self.version,
            "author": self.author,
            "homepage": self.homepage,
            "license": self.license,
            "source": if self.installed.is_some() { "marketplace" } else { "local" },
            "installed": self.installed,
            "network": self.network,
            "secrets": secrets,
            "pip": self.pip,
            "health_data": self.health_data,
        })
    }
}

/// Tool names are function names for the model: lowercase ASCII letters, digits, and `_`. The
/// names cedar's own groups and prefixes use are reserved.
pub fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME {
        bail!("tool names are 1 to {MAX_NAME} characters");
    }
    if !name
        .bytes()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        || name.starts_with('_')
    {
        bail!("tool names use lowercase letters, digits, and underscores, and start with a letter or digit: {name:?}");
    }
    if GROUPS.iter().any(|(id, _)| *id == name) {
        bail!("{name:?} is the name of a tool group");
    }
    if name.starts_with("gmail_") || name.starts_with("calendar_") {
        bail!("{name:?} would be mistaken for a Gmail or Calendar tool");
    }
    Ok(())
}

/// The saved secrets of every tool: tool name → variable → value.
pub type SavedSecrets = HashMap<String, HashMap<String, String>>;

/// Load every tool folder in `dir`, skipping (with a warning) those that can't be used and those
/// whose name is in `taken`, cedar's own tools. Each gets the saved values of the secrets it
/// declares. Fails only if `dir` can't be read at all.
pub fn load_dir(
    dir: &Path,
    taken: &HashSet<&str>,
    secrets: &SavedSecrets,
) -> Result<Vec<PythonTool>> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("reading TOOLS_DIR {}", dir.display()))?;
    let mut folders: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    folders.sort();
    let mut tools = Vec::new();
    for folder in folders {
        let name = folder
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        // Hidden folders include the marketplace's half-finished installs.
        if name.starts_with('.') {
            continue;
        }
        match load_folder(&folder, &name) {
            Ok(None) => {}
            Ok(Some(tool)) if taken.contains(tool.name()) => {
                tracing::warn!(tool = %name, "personal tool left out: a built-in tool has that name");
            }
            Ok(Some(tool)) => {
                let saved = secrets.get(tool.name());
                tools.push(tool.with_secrets(saved));
            }
            Err(e) => {
                tracing::warn!(tool = %name, error = %format!("{e:#}"), "personal tool left out");
            }
        }
    }
    Ok(tools)
}

/// `None` for a folder without a manifest (a stray directory, say).
pub fn load_folder(folder: &Path, name: &str) -> Result<Option<PythonTool>> {
    let manifest = folder.join(MANIFEST);
    let meta = match std::fs::metadata(&manifest) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("reading tool.json"),
    };
    if meta.len() > MAX_MANIFEST_BYTES {
        bail!("tool.json is larger than {MAX_MANIFEST_BYTES} bytes");
    }
    let text = std::fs::read_to_string(&manifest).context("reading tool.json")?;
    let mut tool = PythonTool::from_manifest(name, &text)?;
    let script = folder.join(&tool.entry);
    if !script.is_file() {
        bail!("{} is missing", script.display());
    }
    tool.dir = folder.to_path_buf();
    let receipt = folder.join(INSTALL_RECEIPT);
    if receipt.is_file() {
        let text = std::fs::read_to_string(&receipt).context("reading the install receipt")?;
        tool.installed =
            Some(serde_json::from_str(&text).context("the install receipt is not valid")?);
    }
    Ok(Some(tool))
}

/// Remove the marker that [`changes::touch`] left, when a failed call should list no files.
fn drop_marker(marker: &str) -> String {
    format!("rm -f -- {}", shell_quote(marker))
}

#[async_trait]
impl Tool for PythonTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn personal(&self) -> Option<&PythonTool> {
        Some(self)
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        if !input.is_object() && !input.is_null() {
            bail!("input must be an object");
        }
        let input = if input.is_null() {
            json!({})
        } else {
            input.clone()
        };
        let missing: Vec<&str> = self
            .secrets
            .iter()
            .filter(|s| !s.optional && !self.env.iter().any(|(k, _)| *k == s.name))
            .map(|s| s.name.as_str())
            .collect();
        // A tool may still read them from a .env file in its own folder, so this is only noted.
        if !missing.is_empty() {
            tracing::debug!(tool = %self.name, ?missing, "personal tool has unsaved secrets");
        }
        let marker = format!("/tmp/cedar-tool-{}.mark", Uuid::new_v4());
        changes::touch(ctx, &marker).await?;
        let timeout = self.timeout.unwrap_or(ctx.timeout);
        let staged = input.to_string();
        // The locale is cedar's to set: `CEDAR_*` names are reserved, so no secret can take them.
        let mut env = self.env.clone();
        env.push(("CEDAR_LOCALE".into(), ctx.locale.clone()));
        env.push(("CEDAR_LANGUAGE".into(), crate::locale::language(&ctx.locale).into()));
        let call = ToolRun {
            name: &self.name,
            dir: &self.dir,
            entry: &self.entry,
            input: staged.as_bytes(),
            env: &env,
            network: !self.network.is_empty(),
        };
        let out = ctx.sandbox.run_tool(&call, timeout, ctx.max_output).await;
        let out = match out {
            Ok(out) if out.exit_code == Some(0) => out,
            other => {
                let _ = ctx
                    .sandbox
                    .exec(&drop_marker(&marker), Duration::from_secs(15), 1024)
                    .await;
                other?
            }
        };
        let stdout = truncate_with_total(
            &String::from_utf8_lossy(&out.stdout),
            out.stdout_total,
            ctx.max_output,
        );
        let stderr = stderr_text(&out, ctx.max_output);
        match out.exit_code {
            Some(0) => {}
            Some(124) => bail!("{} timed out after {}s", self.name, timeout.as_secs()),
            Some(code) => {
                let why = if stderr.is_empty() {
                    stdout.trim()
                } else {
                    &stderr
                };
                let mut msg = format!("{} failed (exit status {code}): {why}", self.name);
                if !missing.is_empty() {
                    msg.push_str(&format!(
                        " (its secrets {} are not saved; an admin can add them in the tool settings)",
                        missing.join(", ")
                    ));
                }
                return Err(anyhow!(msg));
            }
            None => bail!("{} was terminated by a signal", self.name),
        }
        let mut report = stdout;
        if !stderr.is_empty() {
            report.push_str("\n[stderr]\n");
            report.push_str(&stderr);
        }
        report.push_str(&changes::section(ctx, &marker).await?);
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_manifest_gets_an_object_schema_and_the_default_entry() {
        let t = PythonTool::from_manifest("word_count", r#"{"description": " Counts words. "}"#)
            .unwrap();
        assert_eq!(t.name(), "word_count");
        assert_eq!(t.description(), "Counts words.");
        assert_eq!(t.schema(), json!({ "type": "object", "properties": {} }));
        assert_eq!(t.script(), "/opt/cedar/tool/tool.py");
        assert!(t.timeout.is_none());
        assert_eq!(
            t.definition(),
            json!({ "name": "word_count", "description": "Counts words.",
                    "input_schema": { "type": "object", "properties": {} } })
        );
    }

    #[test]
    fn the_manifest_may_set_the_schema_entry_and_timeout() {
        let t = PythonTool::from_manifest(
            "x",
            r#"{"name": "x", "description": "d", "entry": "main.py", "timeout_secs": 5,
                "input_schema": {"properties": {"path": {"type": "string"}}, "required": ["path"]}}"#,
        )
        .unwrap();
        assert_eq!(t.script(), "/opt/cedar/tool/main.py");
        assert_eq!(t.timeout, Some(Duration::from_secs(5)));
        assert_eq!(t.schema()["type"], "object");
        assert_eq!(t.schema()["required"], json!(["path"]));
        let long = PythonTool::from_manifest("x", r#"{"description": "d", "timeout_secs": 99999}"#)
            .unwrap();
        assert_eq!(long.timeout, Some(MAX_TIMEOUT));
    }

    #[test]
    fn bad_manifests_are_refused_with_a_reason() {
        let err = |folder: &str, m: &str| {
            PythonTool::from_manifest(folder, m)
                .err()
                .map(|e| format!("{e:#}"))
                .unwrap_or_else(|| panic!("{folder} {m} was accepted"))
        };
        assert!(err("x", "{").contains("not valid"));
        assert!(err("x", r#"{"description": ""}"#).contains("description"));
        assert!(err("x", r#"{"description": "d", "name": "y"}"#).contains("folder"));
        assert!(err("x", r#"{"description": "d", "input_schema": []}"#).contains("JSON object"));
        assert!(err(
            "x",
            r#"{"description": "d", "input_schema": {"type": "string"}}"#
        )
        .contains("an object"));
        assert!(err("x", r#"{"description": "d", "entry": "../tool.py"}"#).contains("entry"));
        assert!(err("x", r#"{"description": "d", "entry": "-x.py"}"#).contains("entry"));
        assert!(err("x", r#"{"description": "d", "timeout_secs": 0}"#).contains("timeout"));
        assert!(err("x", r#"{"description": "d", "extra": 1}"#).contains("extra"));
        // The folder names the tool, so it is checked too.
        assert!(err("Word-Count", r#"{"description": "d"}"#).contains("lowercase"));
        assert!(err("_x", r#"{"description": "d"}"#).contains("start"));
        assert!(err("weather", r#"{"description": "d"}"#).contains("group"));
        assert!(err("gmail_x", r#"{"description": "d"}"#).contains("Gmail"));
        assert!(err(&"a".repeat(65), r#"{"description": "d"}"#).contains("64"));
    }

    #[test]
    fn a_v2_manifest_declares_what_the_tool_needs() {
        let t = PythonTool::from_manifest(
            "glucose",
            r#"{"description": "d", "version": "1.2.0", "author": " Ada ",
                "network": ["source.tandemdiabetes.com", "*.tandemdiabetes.eu"],
                "secrets": ["TANDEM_EMAIL",
                            {"name": "TANDEM_REGION", "description": "us or eu", "optional": true}],
                "pip": ["tconnectsync>=3.0", "setuptools>=70,<81"],
                "health_data": true}"#,
        )
        .unwrap();
        assert_eq!(t.version(), Some("1.2.0"));
        assert_eq!(t.author.as_deref(), Some("Ada"));
        assert!(t.health_data());
        assert_eq!(
            t.permissions(),
            Permissions {
                network: vec![
                    "source.tandemdiabetes.com".into(),
                    "*.tandemdiabetes.eu".into()
                ],
                secrets: vec!["TANDEM_EMAIL".into(), "TANDEM_REGION".into()],
                pip: vec!["tconnectsync>=3.0".into(), "setuptools>=70,<81".into()],
                health_data: true,
            }
        );
        assert!(t.secrets()[1].optional);

        // Only the declared secrets are handed over, and only those that are saved.
        let saved = HashMap::from([
            ("TANDEM_EMAIL".to_string(), "a@b.c".to_string()),
            ("OTHER".to_string(), "x".to_string()),
        ]);
        let t = t.with_secrets(Some(&saved));
        assert_eq!(
            t.env,
            vec![("TANDEM_EMAIL".to_string(), "a@b.c".to_string())]
        );
        let info = t.info(&HashSet::from(["TANDEM_EMAIL".to_string()]));
        assert_eq!(info["source"], "local");
        assert_eq!(info["secrets"][0]["saved"], true);
        assert_eq!(info["secrets"][1]["saved"], false);
        assert!(!info.to_string().contains("a@b.c"), "values never leave");
    }

    #[test]
    fn v2_fields_are_checked() {
        let err = |m: &str| {
            PythonTool::from_manifest("x", m)
                .err()
                .map(|e| format!("{e:#}"))
                .unwrap_or_else(|| panic!("{m} was accepted"))
        };
        assert!(err(r#"{"description": "d", "network": ["https://x.com"]}"#).contains("host"));
        assert!(err(r#"{"description": "d", "network": ["X.com"]}"#).contains("host"));
        assert!(err(r#"{"description": "d", "secrets": ["lower"]}"#).contains("UPPER"));
        assert!(err(r#"{"description": "d", "secrets": ["PATH"]}"#).contains("reserved"));
        assert!(
            err(r#"{"description": "d", "secrets": ["CEDAR_SECRET_KEY"]}"#).contains("reserved")
        );
        assert!(err(r#"{"description": "d", "secrets": ["LD_PRELOAD"]}"#).contains("reserved"));
        assert!(err(r#"{"description": "d", "secrets": ["A", "A"]}"#).contains("twice"));
        assert!(err(r#"{"description": "d", "pip": ["--index-url=http://evil"]}"#).contains("pip"));
        assert!(err(r#"{"description": "d", "pip": ["pkg @ https://x/y.whl"]}"#).contains("pip"));
        assert!(err(r#"{"description": "d", "pip": ["./local"]}"#).contains("pip"));
        assert!(err(r#"{"description": "d", "version": "1\n2"}"#).contains("version"));
        let many = format!(
            r#"{{"description": "d", "network": [{}]}}"#,
            vec!["\"a.com\""; 21].join(",")
        );
        assert!(err(&many).contains("at most"));
    }

    #[test]
    fn load_dir_keeps_the_usable_folders_and_skips_the_rest() {
        let dir = std::env::temp_dir().join(format!("cedar-tools-{}", Uuid::new_v4()));
        let write = |rel: &str, text: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write("good/tool.json", r#"{"description": "ok"}"#);
        write("good/tool.py", "print('hi')");
        write("no_script/tool.json", r#"{"description": "ok"}"#);
        write("bad_json/tool.json", "{");
        write("bad_json/tool.py", "");
        write("bash/tool.json", r#"{"description": "shadows a built-in"}"#);
        write("bash/tool.py", "");
        write("stray/notes.txt", "no manifest here");
        write(".hidden/tool.json", r#"{"description": "ok"}"#);
        write(".hidden/tool.py", "");
        write("loose.json", "not a folder");

        write(
            "installed/tool.json",
            r#"{"description": "from the marketplace", "secrets": ["KEY"]}"#,
        );
        write("installed/tool.py", "");
        write(
            "installed/.cedar-install.json",
            r#"{"version": "1.0.0", "registry": "https://r/index.json", "digest": "abc",
                "files": {"tool.py": "00"}, "installed_at": 1}"#,
        );
        write(
            ".staging-1234/tool.json",
            r#"{"description": "half installed"}"#,
        );
        write(".staging-1234/tool.py", "");

        let secrets = SavedSecrets::from([(
            "installed".to_string(),
            HashMap::from([("KEY".to_string(), "v".to_string())]),
        )]);
        let tools = load_dir(&dir, &HashSet::from(["bash"]), &secrets).unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["good", "installed"]);
        assert!(tools[0].installed().is_none());
        assert_eq!(tools[0].dir, dir.join("good"));
        let installed = tools[1].installed().expect("the receipt is read");
        assert_eq!(installed.version, "1.0.0");
        assert_eq!(tools[1].env, vec![("KEY".to_string(), "v".to_string())]);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(
            load_dir(&dir, &HashSet::new(), &SavedSecrets::new()).is_err(),
            "a missing directory is an error"
        );
    }
}
