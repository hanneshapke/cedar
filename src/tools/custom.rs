//! Personal tools written in Python, loaded from `TOOLS_DIR` without recompiling cedar.
//!
//! Each tool is a folder in that directory with a `tool.json` manifest and a script (`tool.py`
//! unless the manifest says otherwise). The directory is mounted read-only into every sandbox at
//! [`MOUNT`], so a tool runs where `run_python` runs: as the agent's user, offline, under the
//! run's limits, with `/workspace` as the working directory. The call's input arrives as JSON on
//! stdin, stdout is the result, and a non-zero exit turns stderr into an error the model sees.
//!
//! The manifests are read on the host, every time the tool set is built, so a tool added or
//! changed while cedar runs is picked up by the next rebuild (a credential saved through the
//! admin API, or a restart). A folder whose manifest can't be used is logged and left out.

use super::{changes, shell_quote, stderr_text, truncate_with_total, Tool, ToolContext, GROUPS};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

/// Where `TOOLS_DIR` is mounted in the sandbox.
pub const MOUNT: &str = "/opt/cedar/tools";
/// The group every personal tool belongs to (see [`GROUPS`]).
pub const GROUP: &str = "custom";
const MANIFEST: &str = "tool.json";
const DEFAULT_ENTRY: &str = "tool.py";
const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 2000;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// Longest a manifest may ask a call to run; the run's own timeout still applies.
const MAX_TIMEOUT: Duration = Duration::from_secs(600);

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
}

/// One tool loaded from `TOOLS_DIR`.
pub struct PythonTool {
    name: String,
    description: String,
    schema: Value,
    entry: String,
    timeout: Option<Duration>,
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
        Ok(Self {
            name: folder.to_string(),
            description,
            schema,
            entry,
            timeout,
        })
    }

    /// The script's path in the sandbox.
    fn script(&self) -> String {
        format!("{MOUNT}/{}/{}", self.name, self.entry)
    }
}

/// Tool names are function names for the model: lowercase ASCII letters, digits, and `_`. The
/// names cedar's own groups and prefixes use are reserved.
fn check_name(name: &str) -> Result<()> {
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

/// Load every tool folder in `dir`, skipping (with a warning) those that can't be used and those
/// whose name is in `taken`, cedar's own tools. Fails only if `dir` can't be read at all.
pub fn load_dir(dir: &Path, taken: &HashSet<&str>) -> Result<Vec<PythonTool>> {
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
        if name.starts_with('.') {
            continue;
        }
        match load_folder(&folder, &name) {
            Ok(None) => {}
            Ok(Some(tool)) if taken.contains(tool.name()) => {
                tracing::warn!(tool = %name, "personal tool left out: a built-in tool has that name");
            }
            Ok(Some(tool)) => tools.push(tool),
            Err(e) => {
                tracing::warn!(tool = %name, error = %format!("{e:#}"), "personal tool left out");
            }
        }
    }
    Ok(tools)
}

/// `None` for a folder without a manifest (a stray directory, say).
fn load_folder(folder: &Path, name: &str) -> Result<Option<PythonTool>> {
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
    let tool = PythonTool::from_manifest(name, &text)?;
    let script = folder.join(&tool.entry);
    if !script.is_file() {
        bail!("{} is missing", script.display());
    }
    Ok(Some(tool))
}

/// Stage the input, touch the marker, run the script on it, remove the input, and keep the
/// script's exit status. A failed call never lists changed files, so it removes the marker too.
fn run_command(script: &str, input: &str, marker: &str) -> String {
    format!(
        "touch {marker} && PYTHONPATH=\"$PWD${{PYTHONPATH:+:$PYTHONPATH}}\" python3 -- {script} < {input}; \
         s=$?; rm -f -- {input}; [ $s -eq 0 ] || rm -f -- {marker}; exit $s",
        marker = shell_quote(marker),
        script = shell_quote(script),
        input = shell_quote(input),
    )
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

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        if !input.is_object() && !input.is_null() {
            bail!("input must be an object");
        }
        let input = if input.is_null() {
            json!({})
        } else {
            input.clone()
        };
        let stem = format!("/tmp/cedar-tool-{}", Uuid::new_v4());
        let staged = format!("{stem}.json");
        let marker = format!("{stem}.mark");
        ctx.sandbox
            .write_file(&staged, input.to_string().as_bytes())
            .await
            .context("staging the tool input in the sandbox")?;
        let timeout = self.timeout.unwrap_or(ctx.timeout);
        let out = ctx
            .sandbox
            .exec(
                &run_command(&self.script(), &staged, &marker),
                timeout,
                ctx.max_output,
            )
            .await?;
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
                return Err(anyhow!("{} failed (exit status {code}): {why}", self.name));
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
        assert_eq!(t.script(), "/opt/cedar/tools/word_count/tool.py");
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
        assert_eq!(t.script(), "/opt/cedar/tools/x/main.py");
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
    fn the_command_stages_input_on_stdin_and_removes_it() {
        assert_eq!(
            run_command("/opt/cedar/tools/x/tool.py", "/tmp/i.json", "/tmp/i.mark"),
            "touch '/tmp/i.mark' && PYTHONPATH=\"$PWD${PYTHONPATH:+:$PYTHONPATH}\" python3 -- \
             '/opt/cedar/tools/x/tool.py' < '/tmp/i.json'; s=$?; rm -f -- '/tmp/i.json'; \
             [ $s -eq 0 ] || rm -f -- '/tmp/i.mark'; exit $s"
        );
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

        let tools = load_dir(&dir, &HashSet::from(["bash"])).unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["good"]);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(
            load_dir(&dir, &HashSet::new()).is_err(),
            "a missing directory is an error"
        );
    }
}
