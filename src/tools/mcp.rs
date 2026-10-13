//! Tools from remote MCP servers (Gmail, Calendar). Like `web_search` they run in the cedar
//! process: the OAuth tokens stay here, and the sandbox has no network.
//!
//! A server's tools are discovered the first time a run starts after cedar does, not at startup,
//! so an unreachable server delays nothing and is retried by the next run.

use super::{truncate_output, Tool, ToolContext};
use crate::config::McpServerConfig;
use crate::mcp::{GoogleOAuth, McpClient, RemoteTool};
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

/// Tool descriptions longer than this are cut, to keep the prompt small.
const MAX_DESCRIPTION: usize = 2000;

/// One MCP server and, once discovered, the tools it offers that the configuration allows.
pub struct McpServer {
    prefix: String,
    allowed: Vec<String>,
    client: Arc<McpClient>,
    tools: OnceCell<Vec<McpTool>>,
}

impl McpServer {
    pub fn new(cfg: &McpServerConfig, auth: Arc<GoogleOAuth>) -> Result<Self> {
        Ok(Self {
            prefix: cfg.prefix.clone(),
            allowed: cfg.tools.clone(),
            client: Arc::new(McpClient::new(cfg.url.clone(), auth)?),
            tools: OnceCell::new(),
        })
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Discover the server's tools unless that already succeeded. A failure is returned and
    /// leaves the server undiscovered, so the next call tries again.
    pub async fn connect(&self, timeout: Duration) -> Result<()> {
        self.tools
            .get_or_try_init(|| async {
                let listed = self.client.list_tools(timeout).await?;
                let tools = self.select(listed);
                tracing::info!(
                    server = %self.prefix,
                    tools = ?tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
                    "MCP tools discovered"
                );
                Ok::<_, anyhow::Error>(tools)
            })
            .await
            .map(|_| ())
    }

    /// The tools discovered so far (none before a successful [`Self::connect`]).
    pub fn tools(&self) -> &[McpTool] {
        self.tools.get().map(Vec::as_slice).unwrap_or_default()
    }

    /// The listed tools the configuration allows, in the server's order, as cedar tools.
    fn select(&self, listed: Vec<RemoteTool>) -> Vec<McpTool> {
        let all = self.allowed.iter().any(|t| t == "*");
        for name in &self.allowed {
            if name != "*" && !listed.iter().any(|t| &t.name == name) {
                tracing::warn!(server = %self.prefix, tool = %name, "allowed MCP tool is not offered by the server");
            }
        }
        listed
            .into_iter()
            .filter(|t| all || self.allowed.contains(&t.name))
            .map(|t| McpTool::new(&self.prefix, t, self.client.clone()))
            .collect()
    }
}

/// One remote tool, offered to the model as `<prefix>_<name>`.
pub struct McpTool {
    name: String,
    remote_name: String,
    description: String,
    schema: Value,
    client: Arc<McpClient>,
}

impl McpTool {
    fn new(prefix: &str, tool: RemoteTool, client: Arc<McpClient>) -> Self {
        let mut description = tool.description.trim().to_string();
        if description.len() > MAX_DESCRIPTION {
            let mut end = MAX_DESCRIPTION;
            while !description.is_char_boundary(end) {
                end -= 1;
            }
            description.truncate(end);
            description.push('…');
        }
        // The chat-completions API wants an object schema; servers sometimes leave it empty.
        let schema = match tool.input_schema {
            Value::Object(mut obj) => {
                obj.entry("type").or_insert_with(|| json!("object"));
                obj.entry("properties").or_insert_with(|| json!({}));
                Value::Object(obj)
            }
            _ => json!({ "type": "object", "properties": {} }),
        };
        Self {
            name: tool_name(prefix, &tool.name),
            remote_name: tool.name,
            description,
            schema,
            client,
        }
    }
}

/// `<prefix>_<name>`, with anything a function name may not contain replaced by `_`, cut to 64.
fn tool_name(prefix: &str, name: &str) -> String {
    format!("{prefix}_{name}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

#[async_trait]
impl Tool for McpTool {
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
        let result = self
            .client
            .call_tool(&self.remote_name, input, ctx.timeout)
            .await?;
        let text = truncate_output(&result.text, ctx.max_output);
        if result.is_error {
            bail!(
                "{}",
                if text.is_empty() {
                    "the tool failed"
                } else {
                    &text
                }
            );
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_prefixed_and_made_safe() {
        assert_eq!(tool_name("gmail", "search_threads"), "gmail_search_threads");
        assert_eq!(
            tool_name("calendar", "list.events v2"),
            "calendar_list_events_v2"
        );
        assert_eq!(tool_name("x", &"a".repeat(100)).len(), 64);
    }
}
