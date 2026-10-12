use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Largest file `write_file` accepts.
const MAX_WRITE_BYTES: usize = 4 * 1024 * 1024;

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a text file in the sandbox and return its contents."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing path"))?;
        let file = ctx.sandbox.read_file(path, ctx.max_output).await?;
        let text = match std::str::from_utf8(&file.bytes) {
            Ok(s) => s,
            // A truncated read may end in the middle of a multi-byte character; that's fine.
            Err(e) if file.truncated && e.error_len().is_none() => {
                std::str::from_utf8(&file.bytes[..e.valid_up_to()]).expect("valid prefix")
            }
            Err(_) => bail!("{path} is not valid UTF-8 text"),
        };
        let mut out = text.to_string();
        if file.truncated {
            out.push_str(&format!(
                "\n[truncated: file is longer than {} bytes]",
                ctx.max_output
            ));
        }
        Ok(out)
    }
}

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file in the sandbox, creating or overwriting it."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": { "path": { "type": "string" }, "content": { "type": "string" } },
                "required": ["path", "content"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing path"))?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| anyhow!("missing content"))?;
        if content.len() > MAX_WRITE_BYTES {
            bail!(
                "content is {} bytes; the limit is {MAX_WRITE_BYTES}",
                content.len()
            );
        }
        ctx.sandbox.write_file(path, content.as_bytes()).await?;
        Ok(format!("wrote {} bytes to {path}", content.len()))
    }
}
