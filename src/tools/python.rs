//! `run_python`: runs a Python script in the sandbox and reports its output and the files it wrote.
//!
//! The code is staged in the container's `/tmp` rather than passed through `sh -c`, so quotes,
//! heredocs, and backslashes in it need no escaping. A marker file touched just before the script
//! starts lets a second command list the workspace files that are newer, i.e. created or modified
//! by the script.

use super::{changes, exec_report, shell_quote, Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;

/// Largest script `run_python` accepts, matching `write_file`.
const MAX_CODE_BYTES: usize = 4 * 1024 * 1024;

pub struct RunPython;

#[async_trait]
impl Tool for RunPython {
    fn name(&self) -> &str {
        "run_python"
    }

    fn description(&self) -> &str {
        "Run a Python 3 script in the sandbox, with /workspace as the working directory, and \
         return its stdout, stderr, exit status, and the workspace files it created or modified. \
         Prefer it over bash for anything longer than a one-liner: no shell quoting is needed. \
         Modules in /workspace can be imported. Each call is a fresh interpreter, so keep state \
         in files."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "The Python source to run." },
                    "args": { "type": "array", "items": { "type": "string" },
                              "description": "Command-line arguments, available as sys.argv[1:]." }
                },
                "required": ["code"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let code = input["code"]
            .as_str()
            .ok_or_else(|| anyhow!("missing code"))?;
        if code.trim().is_empty() {
            bail!("code is empty");
        }
        if code.len() > MAX_CODE_BYTES {
            bail!(
                "code is {} bytes; the limit is {MAX_CODE_BYTES}",
                code.len()
            );
        }
        let args = parse_args(&input["args"])?;

        let stem = format!("/tmp/cedar-python-{}", Uuid::new_v4());
        let script = format!("{stem}.py");
        let marker = format!("{stem}.mark");
        ctx.sandbox
            .write_file(&script, code.as_bytes())
            .await
            .context("staging the script in the sandbox")?;
        let out = ctx
            .sandbox
            .exec(
                &run_command(&script, &marker, &args),
                ctx.timeout,
                ctx.max_output,
            )
            .await?;
        let mut report = exec_report(&out, ctx);

        report.push_str(&changes::section(ctx, &marker).await?);
        Ok(report)
    }
}

fn parse_args(v: &Value) -> Result<Vec<&str>> {
    match v {
        Value::Null => Ok(vec![]),
        Value::Array(items) => items
            .iter()
            .map(|a| {
                a.as_str()
                    .ok_or_else(|| anyhow!("args must be an array of strings"))
            })
            .collect(),
        _ => bail!("args must be an array of strings"),
    }
}

/// Touch the marker, run the script, remove it, and keep the interpreter's exit status.
fn run_command(script: &str, marker: &str, args: &[&str]) -> String {
    let script = shell_quote(script);
    // The script lives in /tmp, so put the working directory on the import path explicitly.
    let mut cmd = format!(
        "touch {} && PYTHONPATH=\"$PWD${{PYTHONPATH:+:$PYTHONPATH}}\" python3 -- {script}",
        shell_quote(marker)
    );
    for arg in args {
        cmd.push(' ');
        cmd.push_str(&shell_quote(arg));
    }
    cmd.push_str(&format!("; s=$?; rm -f -- {script}; exit $s"));
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_runs_with_quoted_args_and_is_removed() {
        assert_eq!(
            run_command("/tmp/p.py", "/tmp/p.mark", &["a b", "it's"]),
            "touch '/tmp/p.mark' && PYTHONPATH=\"$PWD${PYTHONPATH:+:$PYTHONPATH}\" python3 -- \
             '/tmp/p.py' 'a b' 'it'\\''s'; s=$?; rm -f -- '/tmp/p.py'; exit $s"
        );
        assert_eq!(
            run_command("/tmp/p.py", "/tmp/p.mark", &[]),
            "touch '/tmp/p.mark' && PYTHONPATH=\"$PWD${PYTHONPATH:+:$PYTHONPATH}\" python3 -- \
             '/tmp/p.py'; s=$?; rm -f -- '/tmp/p.py'; exit $s"
        );
    }

    #[test]
    fn args_must_be_strings() {
        assert!(parse_args(&Value::Null).unwrap().is_empty());
        assert_eq!(parse_args(&json!(["x", "y"])).unwrap(), ["x", "y"]);
        assert!(parse_args(&json!([1])).is_err());
        assert!(parse_args(&json!("x")).is_err());
    }
}
