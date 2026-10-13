use super::{changes, exec_report, Tool, ToolContext};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;

pub struct Bash;

#[async_trait]
impl Tool for Bash {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run a shell command in the sandbox and return its stdout and stderr, and the workspace \
         files it created or modified."
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
        // The command runs as given; the marker is touched, and the changes listed, around it.
        let marker = format!("/tmp/cedar-bash-{}.mark", Uuid::new_v4());
        changes::touch(ctx, &marker).await?;
        let out = ctx.sandbox.exec(cmd, ctx.timeout, ctx.max_output).await?;
        let mut report = exec_report(&out, ctx);
        report.push_str(&changes::section(ctx, &marker).await?);
        Ok(report)
    }
}
