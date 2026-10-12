//! `run_python`: runs a Python script in the sandbox and reports its output and the files it wrote.
//!
//! The code is staged in the container's `/tmp` rather than passed through `sh -c`, so quotes,
//! heredocs, and backslashes in it need no escaping. A marker file touched just before the script
//! starts lets a second command list the workspace files that are newer, i.e. created or modified
//! by the script.

use super::{exec_report, shell_quote, Tool, ToolContext};
use crate::sandbox::UNLISTED_DIRS;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Largest script `run_python` accepts, matching `write_file`.
const MAX_CODE_BYTES: usize = 4 * 1024 * 1024;
/// How many changed files are listed.
const MAX_LISTED: usize = 50;
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

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

        let listing = ctx
            .sandbox
            .exec(&changed_files_command(&marker), LIST_TIMEOUT, 64 * 1024)
            .await?;
        if listing.exit_code == Some(0) {
            let files = String::from_utf8_lossy(&listing.stdout);
            report.push_str(&format_changed(&files));
        }
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

/// List workspace files newer than the marker (skipping caches and dependency trees, like the
/// workspace listing does), one more than the cap so truncation shows, then remove the marker.
fn changed_files_command(marker: &str) -> String {
    let prune = UNLISTED_DIRS
        .iter()
        .map(|d| format!("-name {}", shell_quote(d)))
        .collect::<Vec<_>>()
        .join(" -o ");
    let marker = shell_quote(marker);
    format!(
        "[ -e {marker} ] || exit 1; find . -mindepth 1 \\( {prune} \\) -prune -o -type f -newer {marker} -print \
         | sed 's|^\\./||' | sort | head -n {}; rm -f -- {marker}",
        MAX_LISTED + 1
    )
}

/// The "files created or modified" section of the result, or nothing if there were none.
fn format_changed(listing: &str) -> String {
    let files: Vec<&str> = listing.lines().filter(|l| !l.is_empty()).collect();
    if files.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n[files created or modified]");
    for f in files.iter().take(MAX_LISTED) {
        s.push_str("\n- ");
        s.push_str(f);
    }
    if files.len() > MAX_LISTED {
        s.push_str(&format!(
            "\n- … and more (only the first {MAX_LISTED} are listed)"
        ));
    }
    s
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

    #[test]
    fn the_listing_prunes_noise_and_cleans_up() {
        let cmd = changed_files_command("/tmp/p.mark");
        assert!(
            cmd.starts_with(
                "[ -e '/tmp/p.mark' ] || exit 1; find . -mindepth 1 \\( -name '.git' -o "
            ),
            "{cmd}"
        );
        assert!(cmd.contains("-name 'node_modules'"), "{cmd}");
        assert!(cmd.contains("-type f -newer '/tmp/p.mark' -print"), "{cmd}");
        assert!(cmd.ends_with("head -n 51; rm -f -- '/tmp/p.mark'"), "{cmd}");
    }

    #[test]
    fn changed_files_are_listed_and_capped() {
        assert_eq!(format_changed(""), "");
        assert_eq!(
            format_changed("out/a.png\nb.csv\n"),
            "\n[files created or modified]\n- out/a.png\n- b.csv"
        );
        let many: String = (0..=MAX_LISTED).map(|i| format!("f{i}\n")).collect();
        let s = format_changed(&many);
        assert!(s.contains("- f49\n- … and more"), "{s}");
        assert!(!s.contains("- f50"), "{s}");
    }
}
