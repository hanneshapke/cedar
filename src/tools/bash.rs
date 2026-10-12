use super::{truncate_with_total, Tool, ToolContext};
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
        let cmd = input["command"].as_str().ok_or_else(|| anyhow!("missing command"))?;
        let out = ctx.sandbox.exec(cmd, ctx.timeout, ctx.max_output).await?;

        let stream = |bytes: &[u8], total: usize| {
            truncate_with_total(&String::from_utf8_lossy(bytes), total, ctx.max_output)
        };
        let mut s = stream(&out.stdout, out.stdout_total);
        let err = stream(&out.stderr, out.stderr_total);
        if !err.is_empty() {
            s.push_str("\n[stderr]\n");
            s.push_str(&err);
        }
        match out.exit_code {
            Some(0) => {}
            Some(124) => s.push_str(&format!("\n[timed out after {}s]", ctx.timeout.as_secs())),
            Some(code) => s.push_str(&format!("\n[exit status: {code}]")),
            None => s.push_str("\n[terminated by signal]"),
        }
        Ok(s)
    }
}
