use super::{exec_report, Tool, ToolContext};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct Bash;

#[async_trait]
impl Tool for Bash {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run a shell command in the sandbox and return its stdout and stderr."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let cmd = input["command"]
            .as_str()
            .ok_or_else(|| anyhow!("missing command"))?;
        let out = ctx.sandbox.exec(cmd, ctx.timeout, ctx.max_output).await?;
        Ok(exec_report(&out, ctx))
    }
}
