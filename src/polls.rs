//! Calling one tool outside a run (`POST /v1/tools/{name}/call`), for a client that watches
//! something on a timer and only starts a run when it changed (cedar-chat's condition, email and
//! calendar triggers). No model is involved and nothing is added to any conversation.
//!
//! Only tools that say they are [`Tool::pollable`] can be called this way: ones that read and
//! need nothing of a run (the weather, stock prices, a Gmail search, a calendar's events), and
//! personal tools, which get a sandbox and workspace of their own for the one call, removed right
//! after. The pseudo-tool [`URL_TOOL`] reads a web address and returns the body as it came,
//! where `fetch_url` returns a page prepared for a model to read.
//!
//! Calls wait for one of [`SLOTS`] slots of their own, so watching never takes a run's place, and
//! each is recorded (see [`crate::store::Store::record_tool_call`]): who called which tool, how
//! long it took, and whether it failed.

use crate::auth::Owner;
use crate::config::Config;
use crate::sandbox::{ExecOutput, FileContent, Sandbox, SandboxFactory};
use crate::store::Store;
use crate::tools::fetch::{html_to_text, WebClient};
use crate::tools::{Tool, ToolContext, ToolSet};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// The name the web-address source is called by. No tool has it: tool names never clash with it
/// because it is checked first.
pub const URL_TOOL: &str = "url";
/// How many calls run at once; the rest wait.
pub const SLOTS: usize = 2;
/// The largest body [`URL_TOOL`] reads. A longer one is an error, not a cut-off value.
pub const MAX_URL_BYTES: usize = 1 << 20;
/// The most a tool's output may be here: more than in a run, where the cap protects the model's
/// context, so a long JSON reply still parses.
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
/// How long records of calls are kept.
pub const KEEP_CALLS: Duration = Duration::from_secs(30 * 24 * 3600);

/// What a call came to. A tool that failed is a result too (`is_error`), as it is in a run.
#[derive(Debug, serde::Serialize)]
pub struct Called {
    pub output: String,
    pub is_error: bool,
    /// The output as JSON, when it is JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
    /// The output was longer than [`MAX_OUTPUT_BYTES`] and cut.
    pub truncated: bool,
}

/// Why a call wasn't made at all.
#[derive(Debug)]
pub enum Refused {
    /// No tool of that name is offered right now.
    Unknown,
    /// The tool is there but can't be called outside a run.
    NotPollable,
}

pub struct Polls {
    slots: Semaphore,
    /// For [`URL_TOOL`]; `None` with `WEB_FETCH=off`.
    web: Option<WebClient>,
}

impl Polls {
    pub fn new(cfg: &Config) -> Result<Self> {
        let web = match cfg.web_fetch {
            true => Some(WebClient::new(cfg.web_fetch_allow_private)?),
            false => None,
        };
        Ok(Self {
            slots: Semaphore::new(SLOTS),
            web,
        })
    }

    /// Call `name` with `input` for `owner`, and record the call.
    #[allow(clippy::too_many_arguments)]
    pub async fn call(
        &self,
        cfg: &Config,
        store: &Store,
        sandboxes: &Arc<dyn SandboxFactory>,
        tools: &ToolSet,
        owner: &Owner,
        name: &str,
        input: &Value,
    ) -> Result<Called, Refused> {
        if name != URL_TOOL {
            match tools.get(name) {
                None => return Err(Refused::Unknown),
                Some(tool) if !tool.pollable() => return Err(Refused::NotPollable),
                Some(_) => {}
            }
        } else if self.web.is_none() {
            return Err(Refused::Unknown);
        }
        let _slot = self.slots.acquire().await.expect("the semaphore is never closed");
        let started = chrono::Utc::now();
        let clock = Instant::now();
        let result = if name == URL_TOOL {
            let web = self.web.as_ref().expect("checked above");
            read_url(web, input, cfg.tool_timeout).await
        } else {
            let tool = tools.get(name).expect("checked above");
            // A tool has its own limit; this one is for a call that hangs outside it (a
            // container that won't start).
            let limit = cfg.tool_timeout * 2 + Duration::from_secs(60);
            match tokio::time::timeout(limit, call_tool(cfg, sandboxes, tool, input)).await {
                Ok(result) => result,
                Err(_) => Err(anyhow!("{name} did not finish in {}s", limit.as_secs())),
            }
        };
        let duration = clock.elapsed();
        let error = result.as_ref().err().map(|e| format!("{e:#}"));
        tracing::info!(owner = %owner.0, tool = name, duration_ms = duration.as_millis() as u64,
            failed = error.is_some(), "tool called outside a run");
        if let Err(e) = store
            .record_tool_call(&owner.0, name, started, duration, error.as_deref(), KEEP_CALLS)
            .await
        {
            tracing::warn!(error = %format!("{e:#}"), "could not record a tool call");
        }
        Ok(match result {
            Ok(output) => {
                let json = serde_json::from_str(&output).ok();
                let truncated = json.is_none() && was_cut(&output);
                Called {
                    output,
                    is_error: false,
                    json,
                    truncated,
                }
            }
            Err(_) => Called {
                output: error.unwrap_or_default(),
                is_error: true,
                json: None,
                truncated: false,
            },
        })
    }
}

/// Whether `output` ends in the note [`crate::tools::truncate_output`] leaves.
fn was_cut(output: &str) -> bool {
    output
        .rsplit_once("\n[truncated ")
        .is_some_and(|(_, rest)| rest.ends_with(" bytes]") && !rest.contains('\n'))
}

/// `{"url", "as"?: "json" | "text"}`: the body of a web page or API reply. As `json` (the
/// default for a JSON content type) it must parse; as `text` an HTML page is reduced to its
/// text, so markup that changes with every load (nonces, tracking) doesn't look like a change.
async fn read_url(web: &WebClient, input: &Value, timeout: Duration) -> Result<String> {
    let url = input["url"]
        .as_str()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow!("url is required"))?;
    let want = match input.get("as").and_then(Value::as_str) {
        None => None,
        Some("json") => Some(true),
        Some("text") => Some(false),
        Some(other) => bail!("as must be \"json\" or \"text\", not {other:?}"),
    };
    let page = web.get(url, MAX_URL_BYTES, false, timeout).await?;
    if page.truncated() {
        bail!("{url} is larger than {} KiB", MAX_URL_BYTES / 1024);
    }
    let as_json = want.unwrap_or_else(|| page.content_type.contains("json"));
    if as_json {
        let value: Value = serde_json::from_slice(&page.body)
            .map_err(|e| anyhow!("{url} did not return JSON: {e}"))?;
        return Ok(value.to_string());
    }
    if page.content_type.contains("html") {
        return Ok(html_to_text(&page.body)?.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&page.body).trim().to_string())
}

async fn call_tool(
    cfg: &Config,
    sandboxes: &Arc<dyn SandboxFactory>,
    tool: &dyn Tool,
    input: &Value,
) -> Result<String> {
    let context = |sandbox: Arc<dyn Sandbox>| ToolContext {
        sandbox,
        browser: None,
        timeout: cfg.tool_timeout,
        max_output: MAX_OUTPUT_BYTES,
        model: None,
        locale: cfg.default_locale.clone(),
        memory: None,
        records: None,
    };
    if tool.personal().is_none() {
        return tool.call(&context(Arc::new(NoSandbox)), input).await;
    }
    // A personal tool runs in a container that mounts a workspace, so it gets one of its own
    // for this call. Should cedar stop in between, both are swept at the next start like any
    // other leftover (see `RunManager::recover`).
    let (id, workspace) = (Uuid::new_v4(), Uuid::new_v4());
    let sandbox = sandboxes.create(id, workspace).await?;
    let result = tool.call(&context(sandbox.clone()), input).await;
    sandbox.destroy().await;
    if let Err(e) = sandboxes.remove_workspace(workspace).await {
        tracing::warn!(%workspace, error = %format!("{e:#}"), "could not remove a poll's workspace");
    }
    result
}

/// The sandbox of a call that has none. Tools that are pollable without being personal never
/// reach for it; one that did would get an error, not a container.
struct NoSandbox;

const NO_SANDBOX: &str = "there is no sandbox outside a run";

#[async_trait]
impl Sandbox for NoSandbox {
    async fn exec(&self, _: &str, _: Duration, _: usize) -> Result<ExecOutput> {
        bail!(NO_SANDBOX)
    }
    async fn read_file(&self, _: &str, _: usize) -> Result<FileContent> {
        bail!(NO_SANDBOX)
    }
    async fn write_file(&self, _: &str, _: &[u8]) -> Result<()> {
        bail!(NO_SANDBOX)
    }
    async fn destroy(&self) {}
}

/// The entry for [`URL_TOOL`] in what `GET /v1/tools` lists as pollable.
pub fn url_source() -> Value {
    json!({
        "name": URL_TOOL,
        "description": "Read a web address: a JSON API's reply, or a page's text.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_truncation_note_counts_as_cut() {
        assert!(was_cut("abc\n[truncated 12 bytes]"));
        assert!(!was_cut("abc"));
        assert!(!was_cut("abc\n[truncated 12 bytes]\nmore"));
        assert!(!was_cut("[truncated]"));
    }
}
